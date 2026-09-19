use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use anodecs::{BufferFlag, DequeueInputError, DequeueOutputError, MediaCodec, MediaFormat};
use log::info;
use tokio::sync::{mpsc, oneshot};

use super::cmd::{self, Cmd};
use crate::{
    error::Error,
    traits::{VideoDecoderInput, VideoDecoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, PixelFormat, VideoDecoderConfig, VideoFrame,
        VideoOutputMode, VideoPlanes,
    },
};

pub struct AndroidVideoDecoderInput {
    tx: mpsc::UnboundedSender<Cmd<EncodedVideoPacket>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl Drop for AndroidVideoDecoderInput {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

pub struct AndroidVideoDecoderOutput {
    rx: mpsc::UnboundedReceiver<Result<VideoFrame, Error>>,
    /// Set when the decode thread exits via a clean teardown path (`Close` /
    /// sender dropped). A `Disconnected` channel *without* this flag means the
    /// thread died unexpectedly (panic), which must surface as an error so
    /// callers can fall back to software instead of stalling forever on
    /// `Ok(None)`.
    clean_exit: Arc<AtomicBool>,
}

impl VideoDecoderInput for AndroidVideoDecoderInput {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error> {
        self.queue
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Err(e) = self.tx.send(Cmd::Item(packet)) {
            self.queue
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            let _ = e;
            return Err(Error::Dropped);
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Flush(tx)).map_err(|_| Error::Dropped)?;
        rx.await.map_err(|_| Error::Dropped)?
    }

    fn queue_size(&self) -> u32 {
        self.queue.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl VideoDecoderOutput for AndroidVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.rx.recv().await {
            Some(result) => result.map(Some),
            // Clean shutdown reads as EOS; an unclean one is decoder death.
            None if self.clean_exit.load(Ordering::Acquire) => Ok(None),
            None => Err(Error::Dropped),
        }
    }

    fn try_frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.rx.try_recv() {
            Ok(Ok(frame)) => Ok(Some(frame)),
            Ok(Err(e)) => Err(e),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                if self.clean_exit.load(Ordering::Acquire) {
                    Ok(None)
                } else {
                    Err(Error::Dropped)
                }
            }
        }
    }
}

pub fn create(
    config: VideoDecoderConfig,
) -> Result<(AndroidVideoDecoderInput, AndroidVideoDecoderOutput), Error> {
    // Surface path needs ImageReader/NativeWindow — not wired yet.
    if matches!(config.output_mode, VideoOutputMode::HardwareOnly) {
        return Err(Error::InvalidConfig(
            "Android HardwareOnly needs a NativeWindow/ImageReader path (not wired yet)".into(),
        ));
    }

    let mut format =
        MediaFormat::new().map_err(|_| Error::Platform("Failed to create MediaFormat".into()))?;
    let _ = format.set_string("mime", config.codec.to_mime());

    if let Some(res) = config.resolution {
        let _ = format.set_i32("width", res.width as i32);
        let _ = format.set_i32("height", res.height as i32);
    }

    if let Some(desc) = &config.description {
        match config.description_format {
            Some(
                crate::types::VideoDescriptionFormat::AvcC
                | crate::types::VideoDescriptionFormat::HvcC
                | crate::types::VideoDescriptionFormat::Av1C
                | crate::types::VideoDescriptionFormat::CodecPrivate,
            ) => {
                info!(
                    "decoder csd-0: {} bytes, format={:?}",
                    desc.len(),
                    config.description_format
                );
                let _ = format.set_buffer("csd-0", desc);
            }
            // Declared Annex-B or sequence-header data is not valid csd-0.
            // MediaCodec expects avcC/hvcC here, so refuse loudly instead of
            // feeding SPS/PPS bytes the decoder will misparse. Miniter-style
            // callers prepend Annex-B config to the first keyframe in-band,
            // so dropping csd-0 keeps the stream decodable.
            Some(
                crate::types::VideoDescriptionFormat::AnnexB
                | crate::types::VideoDescriptionFormat::Av1SequenceHeaderObu,
            ) => {
                info!(
                    "decoder csd-0: skipping {:?} config ({} bytes); relying on in-band parameter sets",
                    config.description_format,
                    desc.len()
                );
            }
            // keep the previous lenient behavior so existing
            // callers that pass avcC/hvcC without a format keep working.
            None => {
                let csd_first: Vec<u8> = desc.iter().take(8).copied().collect();
                info!(
                    "decoder csd-0: {} bytes (format undeclared), first={:02x?}, starts_with_annexb={}",
                    desc.len(),
                    csd_first,
                    desc.len() >= 4 && (desc[..4] == [0x00, 0x00, 0x00, 0x01])
                );
                let _ = format.set_buffer("csd-0", desc);
            }
        }
    }

    info!(
        "decoder format: mime={}, {}x{}, csd-0 present={}",
        config.codec.to_mime(),
        config.resolution.map(|r| r.width).unwrap_or(0),
        config.resolution.map(|r| r.height).unwrap_or(0),
        config.description.is_some()
    );

    let mime = config.codec.to_mime().to_string();

    let mut codec = MediaCodec::create_decoder(&mime)
        .map_err(|e| Error::Platform(format!("No decoder for {mime}: {e:?}")))?;

    codec
        .init(&format, None, 0)
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    codec
        .start()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd<EncodedVideoPacket>>();
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Result<VideoFrame, Error>>();
    let queue = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let queue2 = queue.clone();
    let clean_exit = Arc::new(AtomicBool::new(false));
    let clean_exit2 = clean_exit.clone();

    thread::spawn(move || {
        decode_loop(codec, cmd_rx, frame_tx, queue2, clean_exit2);
    });

    Ok((
        AndroidVideoDecoderInput { tx: cmd_tx, queue },
        AndroidVideoDecoderOutput {
            rx: frame_rx,
            clean_exit,
        },
    ))
}

/// Extract a [`VideoFrame`] from a MediaCodec output buffer, accounting for
/// stride, slice-height, crop rectangle, and color format.
fn output_to_frame(out_buf: &anodecs::CodecOutputBuffer) -> Result<VideoFrame, Error> {
    let fmt = out_buf.format();
    let raw = out_buf.buffer_slice().unwrap_or_default();
    let ts_us = out_buf.info().presentation_time_us;

    // Read the actual display size (crop rect overrides width/height for visible region)
    let crop_left = fmt.get_i32("crop-left").unwrap_or(0) as u32;
    let crop_top = fmt.get_i32("crop-top").unwrap_or(0) as u32;
    let crop_right = fmt.get_i32("crop-right").unwrap_or(0) as u32;
    let crop_bottom = fmt.get_i32("crop-bottom").unwrap_or(0) as u32;
    let fmt_w = fmt.get_i32("width").unwrap_or(0) as u32;
    let fmt_h = fmt.get_i32("height").unwrap_or(0) as u32;

    // Crop rect is preferred; fall back to width/height
    let (vis_w, vis_h) = if crop_right > 0 && crop_bottom > 0 {
        (crop_right - crop_left + 1, crop_bottom - crop_top + 1)
    } else {
        (fmt_w, fmt_h)
    };

    // Stride and slice-height may be provided; fall back to width/height
    let stride = fmt.get_i32("stride").unwrap_or(fmt_w as i32) as usize;
    let slice_h = fmt.get_i32("slice-height").unwrap_or(fmt_h as i32) as usize;

    // A missing color-format key historically defaulted to NV12. Report it
    // explicitly instead so callers can distinguish "known NV12" from
    // "decoder did not report a layout".
    let color_format = match fmt.color_format() {
        Some(cf) => cf,
        None => {
            return Err(crate::error::MediaFailure::new(
                crate::error::MediaFailureCode::UnsupportedOutputFormat,
                "MediaCodec output omitted color-format; refusing to assume NV12",
            )
            .backend("android")
            .into());
        }
    };

    let w = vis_w as usize;
    let h = vis_h as usize;

    match color_format {
        anodecs::ColorFormat::Yuv420SemiPlanar
        | anodecs::ColorFormat::Yuv420Flexible
        | anodecs::ColorFormat::Yuv420PackedSemiPlanar
        | anodecs::ColorFormat::QcomYuv420SemiPlanar => {
            // COLOR_FormatYUV420SemiPlanar (NV12) or compatible vendor formats
            let y_size = stride * slice_h;
            let uv_h = h / 2;
            let uv_crop_top = crop_top as usize / 2;
            // Buffer has Y plane padded to slice_h then UV plane.
            // Some decoders only fill visible chroma rows (uv_h), not all slice_h/2 rows,
            // so check against visible chroma extent rather than full padded uv_size.
            let expected = y_size + uv_h * stride;
            if raw.len() < expected {
                return Err(Error::Platform(format!(
                    "NV12 buffer too small: {} < {}",
                    raw.len(),
                    expected
                )));
            }
            // Repack: copy visible rows from stride-pitched buffers into tight rows
            let mut out = vec![0u8; w * h * 3 / 2];
            let (out_y, out_uv) = out.split_at_mut(w * h);
            let crop_l = crop_left as usize;
            // Y plane
            for row in 0..h {
                let src_start = (crop_top as usize + row) * stride + crop_l;
                let dst_start = row * w;
                out_y[dst_start..dst_start + w].copy_from_slice(&raw[src_start..src_start + w]);
            }
            // UV plane (interleaved)
            for row in 0..uv_h {
                let src_start = y_size + (uv_crop_top + row) * stride + crop_l;
                let dst_start = row * w;
                out_uv[dst_start..dst_start + w].copy_from_slice(&raw[src_start..src_start + w]);
            }
            Ok(VideoFrame {
                dimensions: Dimensions::new(vis_w, vis_h),
                format: PixelFormat::Nv12,
                timestamp: std::time::Duration::from_micros(ts_us as u64),
                planes: VideoPlanes::Cpu(out),
            })
        }
        anodecs::ColorFormat::Yuv420Planar | anodecs::ColorFormat::Yuv420PackedPlanar => {
            // COLOR_FormatYUV420Planar (I420) or compatible vendor formats
            let y_size = stride * slice_h;
            let uv_stride = stride / 2;
            let uv_h = h / 2;
            // U and V planes each have slice_h/2 rows in the standard layout,
            // but some decoders only fill visible chroma rows (uv_h).
            let chroma_rows_per_plane = uv_h;
            let u_size = uv_stride * chroma_rows_per_plane;
            let expected = y_size + 2 * u_size;
            if raw.len() < expected {
                return Err(Error::Platform(format!(
                    "I420 buffer too small: {} < {}",
                    raw.len(),
                    expected
                )));
            }
            let mut out = vec![0u8; w * h * 3 / 2];
            let (out_y, out_uv) = out.split_at_mut(w * h);
            let (out_u, out_v) = out_uv.split_at_mut(w * h / 4);
            let crop_l = crop_left as usize;
            let uv_crop_l = crop_l / 2;
            let uv_crop_top = (crop_top / 2) as usize;
            // Y plane
            for row in 0..h {
                let src_start = (crop_top as usize + row) * stride + crop_l;
                let dst_start = row * w;
                out_y[dst_start..dst_start + w].copy_from_slice(&raw[src_start..src_start + w]);
            }
            // U plane
            for row in 0..uv_h {
                let src_start = y_size + (uv_crop_top + row) * uv_stride + uv_crop_l;
                let dst_start = row * (w / 2);
                out_u[dst_start..dst_start + w / 2]
                    .copy_from_slice(&raw[src_start..src_start + w / 2]);
            }
            // V plane
            for row in 0..uv_h {
                let src_start = y_size + u_size + (uv_crop_top + row) * uv_stride + uv_crop_l;
                let dst_start = row * (w / 2);
                out_v[dst_start..dst_start + w / 2]
                    .copy_from_slice(&raw[src_start..src_start + w / 2]);
            }
            Ok(VideoFrame {
                dimensions: Dimensions::new(vis_w, vis_h),
                format: PixelFormat::Yuv420p,
                timestamp: std::time::Duration::from_micros(ts_us as u64),
                planes: VideoPlanes::Cpu(out),
            })
        }
        anodecs::ColorFormat::YuvP010 => {
            // 10-bit semi-planar: Y and interleaved UV are 16-bit
            // little-endian words with the 10-bit sample in the high bits.
            // Downconvert to 8-bit NV12 by keeping the high byte of each word
            let y_size = stride * slice_h * 2;
            let uv_h = h / 2;
            let uv_crop_top = crop_top as usize / 2;
            let expected = y_size + uv_h * stride * 2;
            if raw.len() < expected {
                return Err(Error::Platform(format!(
                    "P010 buffer too small: {} < {}",
                    raw.len(),
                    expected
                )));
            }
            let mut out = vec![0u8; w * h * 3 / 2];
            let (out_y, out_uv) = out.split_at_mut(w * h);
            let crop_l = crop_left as usize;
            for row in 0..h {
                let src_row = (crop_top as usize + row) * stride * 2 + crop_l * 2;
                let dst_start = row * w;
                for col in 0..w {
                    out_y[dst_start + col] = raw[src_row + col * 2 + 1];
                }
            }
            for row in 0..uv_h {
                let src_row = y_size + (uv_crop_top + row) * stride * 2 + crop_l * 2;
                let dst_start = row * w;
                for col in 0..w {
                    out_uv[dst_start + col] = raw[src_row + col * 2 + 1];
                }
            }
            Ok(VideoFrame {
                dimensions: Dimensions::new(vis_w, vis_h),
                format: PixelFormat::Nv12,
                timestamp: std::time::Duration::from_micros(ts_us as u64),
                planes: VideoPlanes::Cpu(out),
            })
        }
        anodecs::ColorFormat::TiYuv420PackedSemiPlanar
        | anodecs::ColorFormat::Surface
        | anodecs::ColorFormat::Unknown(other) => {
            Err(crate::error::MediaFailure::new(
                crate::error::MediaFailureCode::UnsupportedOutputFormat,
                format!("unsupported MediaCodec color-format: {other}"),
            )
            .backend("android")
            .into())
        }
    }
}

fn drain_output(
    codec: &mut MediaCodec,
    frame_tx: &mpsc::UnboundedSender<Result<VideoFrame, Error>>,
) -> usize {
    let mut count = 0;
    loop {
        match codec.dequeue_output(0) {
            Ok(out) => {
                let out_buf: anodecs::CodecOutputBuffer = out;
                let info = out_buf.info();
                let flags = info.flags;
                if BufferFlag::EndOfStream.is_contained_in(flags) {
                    continue;
                }
                if BufferFlag::CodecConfig.is_contained_in(flags) {
                    continue;
                }
                match output_to_frame(&out_buf) {
                    Ok(frame) => {
                        if frame_tx.send(Ok(frame)).is_err() {
                            return count;
                        }
                        count += 1;
                    }
                    Err(e) => {
                        info!("decoder output_to_frame error: {e:?}");
                        let _ = frame_tx.send(Err(e));
                        return count;
                    }
                }
            }
            Err(DequeueOutputError::TryAgainLater) => break,
            Err(DequeueOutputError::OutputFormatChanged)
            | Err(DequeueOutputError::OutputBuffersChanged) => {
                // format/buffers already refreshed by wrapper; continue polling
            }
            Err(DequeueOutputError::CodecError(e)) => {
                info!("decoder drain: CodecError {e:?}");
                let _ = frame_tx.send(Err(Error::Platform(format!("codec error: {e:?}"))));
                return count;
            }
        }
    }
    count
}

fn decode_loop(
    mut codec: MediaCodec,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd<EncodedVideoPacket>>,
    frame_tx: mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
    clean_exit: Arc<AtomicBool>,
) {
    let mut pending: std::collections::VecDeque<EncodedVideoPacket> =
        std::collections::VecDeque::new();
    let mut in_flight: u32 = 0;

    info!("decode_loop started");

    loop {
        // When no work in the pipeline, block for the next command.
        // Otherwise, non-blocking poll so we keep draining output.
        if pending.is_empty() && in_flight == 0 {
            match cmd_rx.blocking_recv() {
                Some(Cmd::Item(pkt)) => {
                    pending.push_back(pkt);
                }
                Some(Cmd::Flush(done)) => {
                    let res =
                        handle_flush(&mut codec, &mut pending, &frame_tx, &queue, &mut in_flight);
                    let _ = done.send(res);
                }
                Some(Cmd::Close) | None => {
                    info!("decode_loop: close");
                    drain_output(&mut codec, &frame_tx);
                    queue.store(0, Ordering::Relaxed);
                    clean_exit.store(true, Ordering::Release);
                    return;
                }
            }
        } else {
            // Drain any pending commands without blocking
            loop {
                match cmd_rx.try_recv() {
                    Ok(Cmd::Item(pkt)) => {
                        pending.push_back(pkt);
                    }
                    Ok(Cmd::Flush(done)) => {
                        let res = handle_flush(
                            &mut codec,
                            &mut pending,
                            &frame_tx,
                            &queue,
                            &mut in_flight,
                        );
                        let _ = done.send(res);
                    }
                    Ok(Cmd::Close) | Err(mpsc::error::TryRecvError::Disconnected) => {
                        info!("decode_loop: close (non-blocking)");
                        drain_output(&mut codec, &frame_tx);
                        queue.store(0, Ordering::Relaxed);
                        clean_exit.store(true, Ordering::Release);
                        return;
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                }
            }
        }

        // Service the codec: submit pending packets, drain finished frames.
        // submit_pending() removes packets from `pending` as it submits them,
        // so a submission error must surface to the output instead of being ignored.
        match submit_pending(&mut codec, &mut pending, &queue) {
            Ok(submitted) => {
                in_flight = in_flight.saturating_add(submitted as u32);
            }
            Err(error) => {
                pending.clear();
                queue.store(0, Ordering::Relaxed);
                let _ = frame_tx.send(Err(error));
                return;
            }
        }
        let produced = drain_output(&mut codec, &frame_tx);
        in_flight = in_flight.saturating_sub(produced as u32);

        // Brief sleep when work is in-flight but nothing progressed
        if pending.is_empty() && in_flight > 0 {
            thread::sleep(std::time::Duration::from_millis(1));
        }

        // Exit cleanly when sender is dropped and all work is done
        if cmd_rx.is_closed() && pending.is_empty() && in_flight == 0 {
            info!("decode_loop: closed+empty");
            drain_output(&mut codec, &frame_tx);
            queue.store(0, Ordering::Relaxed);
            clean_exit.store(true, Ordering::Release);
            return;
        }
    }
}

fn handle_flush(
    codec: &mut MediaCodec,
    pending: &mut std::collections::VecDeque<EncodedVideoPacket>,
    frame_tx: &mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    queue: &std::sync::Arc<std::sync::atomic::AtomicU32>,
    in_flight: &mut u32,
) -> Result<(), Error> {
    info!("decode_loop: flush start, pending={}", pending.len());

    let produced = drain_output(codec, frame_tx);
    *in_flight = in_flight.saturating_sub(produced as u32);

    for _ in 0..5000 {
        if pending.is_empty() {
            break;
        }
        let submitted = match submit_pending(codec, pending, queue) {
            Ok(submitted) => submitted,
            Err(error) => {
                pending.clear();
                queue.store(0, Ordering::Relaxed);
                *in_flight = 0;
                return Err(error);
            }
        };
        *in_flight = in_flight.saturating_add(submitted as u32);
        let produced = drain_output(codec, frame_tx);
        *in_flight = in_flight.saturating_sub(produced as u32);
        if !pending.is_empty() {
            thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    if !pending.is_empty() {
        return Err(Error::Platform("flush timed out submitting pending".into()));
    }

    info!("decode_loop: flush sending EOS");
    cmd::send_eos(codec)?;
    cmd::drain_until_eos(codec, |out| {
        let frame = output_to_frame(&out)?;
        frame_tx.send(Ok(frame)).map_err(|_| Error::Dropped)
    })?;

    codec
        .flush()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;

    *in_flight = 0;
    info!("decode_loop: flush done");
    Ok(())
}

fn submit_pending(
    codec: &mut MediaCodec,
    pending: &mut std::collections::VecDeque<EncodedVideoPacket>,
    queue: &std::sync::Arc<std::sync::atomic::AtomicU32>,
) -> Result<usize, Error> {
    let mut count = 0usize;
    while let Some(pkt) = pending.pop_front() {
        match codec.dequeue_input(0) {
            Ok(buf) => {
                let mut buf: anodecs::CodecInputBuffer = buf;
                let (ptr, cap): (*mut u8, usize) = buf.buffer();
                if pkt.payload.len() > cap {
                    // Hand the slot back to the codec instead of leaking it,
                    // then surface the failure: the packet is intentionally
                    // NOT requeued so the decode thread cannot spin forever
                    // on a packet that will never fit.
                    buf.cancel();
                    return Err(Error::Platform(format!(
                        "video packet too large: {} > {}",
                        pkt.payload.len(),
                        cap
                    )));
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(pkt.payload.as_ptr(), ptr, pkt.payload.len());
                }
                buf.set_write_size(pkt.payload.len());
                buf.set_time(pkt.timestamp.as_micros() as u64);
                count += 1;
                queue.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
            Err(DequeueInputError::TryAgainLater) => {
                pending.push_front(pkt);
                break;
            }
            Err(DequeueInputError::CodecError(status)) => {
                return Err(Error::Platform(format!(
                    "video input unavailable (terminal): {status:?}"
                )));
            }
        }
    }
    Ok(count)
}
