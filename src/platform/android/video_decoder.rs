use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use anodecs::{BufferFlag, DequeueInputError, DequeueOutputError, MediaCodec, MediaFormat};
use log::{debug, info, trace, warn};
use tokio::sync::{mpsc, oneshot};

use super::cmd::{self, Cmd};
use crate::{
    error::Error,
    traits::{VideoDecoderInput, VideoDecoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, VideoDecoderConfig, VideoFrame, VideoOutputMode,
        VideoPlanes,
    },
    util::repack,
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
        // Report the CODED (macroblock-aligned) size to MediaCodec, not the
        // visible size: the SPS crop rectangle derives visible dims from coded
        // dims, and configuring visible dims makes the codec expect a crop it
        // never gets. Observed: 220x162-in-224x176 streams stall with zero
        // output when configured as 220x162.
        let (cw, ch) = (res.width.div_ceil(16) * 16, res.height.div_ceil(16) * 16);
        if (cw, ch) != (res.width, res.height) {
            info!(
                "decoder resolution: aligning {}x{} to coded {}x{} for MediaCodec",
                res.width, res.height, cw, ch
            );
        }
        let _ = format.set_i32("width", cw as i32);
        let _ = format.set_i32("height", ch as i32);
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

    // Sanity: 220x162 Baseline should emit its first frame from a single
    // IDR within milliseconds. If the codec accepts input but never produces
    // output, the usual cause is a csd-0 / in-band parameter-set mismatch, so
    // log the first submitted packet shape here for correlation.
    info!(
        "decoder started: mime={mime} output_mode={:?}",
        config.output_mode
    );

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
    // Negative keys (unset/malformed) are treated as absent: wrapping to u32
    // would otherwise produce a ~4-billion-pixel visible size.
    let crop_left = fmt.get_i32("crop-left").unwrap_or(0).max(0) as u32;
    let crop_top = fmt.get_i32("crop-top").unwrap_or(0).max(0) as u32;
    let crop_right = fmt.get_i32("crop-right").unwrap_or(0).max(0) as u32;
    let crop_bottom = fmt.get_i32("crop-bottom").unwrap_or(0).max(0) as u32;
    let fmt_w = fmt.get_i32("width").unwrap_or(0).max(0) as u32;
    let fmt_h = fmt.get_i32("height").unwrap_or(0).max(0) as u32;

    let (vis_w, vis_h) = repack::resolve_visible(
        crop_left,
        crop_top,
        crop_right,
        crop_bottom,
        fmt_w,
        fmt_h,
    )?;

    // Per the MediaCodec docs slice-height may be advertised as 0, meaning
    // "same as frame height OR height aligned up (usually pow2)" — there is
    // no way to tell which. Prefer the visible height, then fall back to the
    // coded height, so a 0/negative key never becomes stride 0 or a
    // slice smaller than the visible picture.
    let stride = fmt
        .get_i32("stride")
        .filter(|&v| v > 0)
        .map(|v| v as usize)
        .unwrap_or(fmt_w.max(1) as usize);
    let slice_h = fmt
        .get_i32("slice-height")
        .filter(|&v| v >= vis_h as i32 && v > 0)
        .map(|v| v as usize)
        .unwrap_or((vis_h.max(1) as usize).max(fmt_h.max(1) as usize));

    // A missing color-format key historically defaulted to NV12. Report it
    // explicitly instead so callers can distinguish "known NV12" from
    // "decoder did not report a layout".
    let layout = match fmt.color_format() {
        Some(cf) => repack::layout_from_color_format_raw(cf.raw()),
        None => {
            return Err(crate::error::MediaFailure::new(
                crate::error::MediaFailureCode::UnsupportedOutputFormat,
                "MediaCodec output omitted color-format; refusing to assume NV12",
            )
            .backend("android")
            .into());
        }
    };
    if layout == repack::ColorLayout::Unsupported {
        return Err(crate::error::MediaFailure::new(
            crate::error::MediaFailureCode::UnsupportedOutputFormat,
            format!(
                "unsupported MediaCodec color-format: {:?}",
                fmt.color_format()
            ),
        )
        .backend("android")
        .into());
    }

    // Crop-rect sanity: MediaCodec advertises coded dims + inclusive crop.
    // A crop smaller than coded dims is normal (odd sizes like 220x162 in a
    // 224x176 frame); a crop LARGER than coded dims is driver garbage and
    // would make repack read out of bounds. Clamp defensively and log.
    let (vis_w, vis_h) = if vis_w > fmt_w || vis_h > fmt_h {
        log::warn!(
            "crop rect {vis_w}x{vis_h} exceeds coded {fmt_w}x{fmt_h}; clamping to coded size"
        );
        (fmt_w.min(vis_w).max(1), fmt_h.min(vis_h).max(1))
    } else {
        (vis_w, vis_h)
    };
    let (format, data) = repack::repack(
        raw,
        layout,
        repack::Geometry {
            vis_w,
            vis_h,
            stride,
            slice_h,
            crop_left: crop_left.min(vis_w.saturating_sub(1)) as usize,
            crop_top: crop_top.min(vis_h.saturating_sub(1)) as usize,
        },
    )
    .map_err(|e| {
        // Surface the failing geometry: stride/slice vs visible/coded sizes
        // is the usual suspect when MediaCodec reports odd layouts.
        warn!(
            "output_to_frame repack failed: {e:?} (vis={vis_w}x{vis_h} coded={fmt_w}x{fmt_h} stride={stride} slice_h={slice_h} raw_len={} layout={layout:?})",
            raw.len()
        );
        e
    })?;
    Ok(VideoFrame {
        dimensions: Dimensions::new(vis_w, vis_h),
        format,
        timestamp: std::time::Duration::from_micros(ts_us as u64),
        planes: VideoPlanes::Cpu(data),
    })
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
                        static OUT_OK: std::sync::atomic::AtomicU64 =
                            std::sync::atomic::AtomicU64::new(0);
                        if OUT_OK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 3 {
                            info!(
                                "drain_output: frame {} ts={} fmt={:?}",
                                OUT_OK.load(std::sync::atomic::Ordering::Relaxed),
                                frame.timestamp.as_micros(),
                                frame.format
                            );
                        }
                        if frame_tx.send(Ok(frame)).is_err() {
                            return count;
                        }
                        count += 1;
                    }
                    Err(e) => {
                        log::warn!("decoder output_to_frame error: {e:?}");
                        let _ = frame_tx.send(Err(e));
                        return count;
                    }
                }
            }
            Err(DequeueOutputError::TryAgainLater) => {
                static DRAIN_EMPTY: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                let n = DRAIN_EMPTY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n % 600 == 0 {
                    info!("drain_output: TryAgainLater (empty polls={n})");
                }
                break;
            }
            Err(DequeueOutputError::OutputFormatChanged)
            | Err(DequeueOutputError::OutputBuffersChanged) => {
                // format/buffers already refreshed by wrapper; continue polling
            }
            Err(DequeueOutputError::CodecError(e)) => {
                warn!("decoder drain: CodecError {e:?}");
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

        // Service the codec: drain finished frames BEFORE submitting more
        // input. MediaCodec stalls output when all of its input buffers are
        // queued but none released (observed: 121 in-flight, zero output,
        // zero error on all-IDR streams). Draining first keeps at least one
        // input slot free, which is what lets the codec emit.
        let produced = drain_output(&mut codec, &frame_tx);
        in_flight = in_flight.saturating_sub(produced as u32);

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
        // Rate-limit: log every 120 iterations (~every few seconds when idle)
        // to avoid spamming logcat while still showing liveness.
        // NOTE(log-verify): heartbeat intentionally info! until the Android
        // HW-stall investigation closes; then demote back to debug!.
        static TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 120 == 0 {
            info!(
                "decode_loop: pending={} in_flight={} produced={}",
                pending.len(),
                in_flight,
                produced
            );
        }

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
                // MediaCodec needs BUFFER_FLAG_KEY_FRAME on sync samples;
                // without it the decoder may hold output waiting for a
                // reference it never recognizes (all-IDR streams stall with
                // zero output and zero error, exactly as observed).
                if pkt.keyframe {
                    buf.set_flags(anodecs::BufferFlag::KeyFrame as u32);
                }
                static SUBMITTED: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                let n = SUBMITTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n < 5 {
                    // Peek first NALU type: Annex-B start code + header byte.
                    // type 5 = IDR, 1 = non-IDR slice, 7/8 = SPS/PPS.
                    let peek = if pkt.payload.len() >= 5
                        && pkt.payload[0] == 0x00
                        && pkt.payload[1] == 0x00
                        && (pkt.payload[2] == 0x01
                            || (pkt.payload[2] == 0x00 && pkt.payload[3] == 0x01))
                    {
                        let hb = if pkt.payload[2] == 0x01 {
                            pkt.payload[3]
                        } else {
                            pkt.payload[4]
                        };
                        format!("annexb nal_type={}", hb & 0x1f)
                    } else {
                        format!("head={:02x?}", &pkt.payload[..pkt.payload.len().min(8)])
                    };
                    info!(
                        "submit_pending: pkt#{n} bytes={} ts_us={} keyframe={} {peek}",
                        pkt.payload.len(),
                        pkt.timestamp.as_micros(),
                        pkt.keyframe
                    );
                }
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
