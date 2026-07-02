use std::sync::atomic::Ordering;
use std::thread;

use mediacodec::{BufferFlag, MediaCodec, MediaFormat};
use tokio::sync::{mpsc, oneshot};

use super::cmd::{self, Cmd};
use crate::{
    error::Error,
    traits::{VideoDecoderInput, VideoDecoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, PixelFormat, VideoDecoderConfig, VideoFrame, VideoPlanes,
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
}

impl VideoDecoderInput for AndroidVideoDecoderInput {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error> {
        self.queue
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.tx
            .send(Cmd::Item(packet))
            .map_err(|_| Error::Dropped)
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
            None => Ok(None),
        }
    }

    fn try_frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.rx.try_recv() {
            Ok(Ok(frame)) => Ok(Some(frame)),
            Ok(Err(e)) => Err(e),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => Ok(None),
        }
    }
}

pub fn create(
    config: VideoDecoderConfig,
) -> Result<(AndroidVideoDecoderInput, AndroidVideoDecoderOutput), Error> {
    let mut format =
        MediaFormat::new().map_err(|_| Error::Platform("Failed to create MediaFormat".into()))?;
    let _ = format.set_string("mime", &config.codec.0);

    if let Some(res) = config.resolution {
        let _ = format.set_i32("width", res.width as i32);
        let _ = format.set_i32("height", res.height as i32);
    }

    if let Some(desc) = &config.description {
        let _ = format.set_buffer("csd-0", desc);
    }

    let mime = config.codec.0.clone();

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

    thread::spawn(move || {
        decode_loop(codec, cmd_rx, frame_tx, queue2);
    });

    Ok((
        AndroidVideoDecoderInput { tx: cmd_tx, queue },
        AndroidVideoDecoderOutput { rx: frame_rx },
    ))
}

/// Extract a [`VideoFrame`] from a MediaCodec output buffer, accounting for
/// stride, slice-height, crop rectangle, and color format.
fn output_to_frame(out_buf: &mediacodec::CodecOutputBuffer) -> Result<VideoFrame, Error> {
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

    let color_format = fmt.get_i32("color-format").unwrap_or(21) as u32;

    let w = vis_w as usize;
    let h = vis_h as usize;

    match color_format {
        21 | 2141391876 | 2141391878 | 2130708361 => {
            // COLOR_FormatYUV420SemiPlanar (NV12) or compatible vendor formats
            let y_size = stride * slice_h;
            let uv_size = stride * (slice_h / 2);
            let expected = y_size + uv_size;
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
            // Y plane
            for row in 0..h {
                let src_start = (crop_top as usize + row) * stride;
                let dst_start = row * w;
                out_y[dst_start..dst_start + w]
                    .copy_from_slice(&raw[src_start..src_start + w]);
            }
            // UV plane (interleaved)
            let uv_h = h / 2;
            let uv_crop_top = crop_top as usize / 2;
            for row in 0..uv_h {
                let src_start = y_size + (uv_crop_top + row) * stride;
                let dst_start = row * w;
                out_uv[dst_start..dst_start + w]
                    .copy_from_slice(&raw[src_start..src_start + w]);
            }
            Ok(VideoFrame {
                dimensions: Dimensions::new(vis_w, vis_h),
                format: PixelFormat::Nv12,
                timestamp: std::time::Duration::from_micros(ts_us as u64),
                planes: VideoPlanes::Cpu(out),
            })
        }
        19 | 2141391872 | 2130706688 => {
            // COLOR_FormatYUV420Planar (I420) or compatible vendor formats
            let y_size = stride * slice_h;
            let u_size = stride / 2 * (slice_h / 2);
            let v_size = u_size;
            let expected = y_size + u_size + v_size;
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
            // Y plane
            for row in 0..h {
                let src_start = (crop_top as usize + row) * stride;
                let dst_start = row * w;
                out_y[dst_start..dst_start + w]
                    .copy_from_slice(&raw[src_start..src_start + w]);
            }
            // U plane
            let uv_stride = stride / 2;
            let uv_h = h / 2;
            for row in 0..uv_h {
                let src_start = y_size + (uv_crop_top(row)) * uv_stride;
                let dst_start = row * (w / 2);
                out_u[dst_start..dst_start + w / 2]
                    .copy_from_slice(&raw[src_start..src_start + w / 2]);
            }
            // V plane
            for row in 0..uv_h {
                let src_start = y_size + u_size + (uv_crop_top(row)) * uv_stride;
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
        other => Err(Error::Platform(format!(
            "unsupported MediaCodec color-format: {other}"
        ))),
    }
}

/// Helper: map crop_top for chroma planes (accounts for odd alignment).
fn uv_crop_top(crop_top: u32) -> usize {
    (crop_top / 2) as usize
}

fn drain_output(
    codec: &mut MediaCodec,
    frame_tx: &mpsc::UnboundedSender<Result<VideoFrame, Error>>,
) {
    while let Ok(out) = codec.dequeue_output(0) {
        let out_buf: mediacodec::CodecOutputBuffer = out;
        if BufferFlag::EndOfStream.is_contained_in(out_buf.info().flags as i32) {
            continue;
        }
        match output_to_frame(&out_buf) {
            Ok(frame) => {
                if frame_tx.send(Ok(frame)).is_err() {
                    return;
                }
            }
            Err(e) => {
                let _ = frame_tx.send(Err(e));
                return;
            }
        }
    }
}

fn decode_loop(
    mut codec: MediaCodec,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd<EncodedVideoPacket>>,
    frame_tx: mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    let mut pending: std::collections::VecDeque<EncodedVideoPacket> =
        std::collections::VecDeque::new();

    loop {
        match cmd_rx.blocking_recv() {
            Some(Cmd::Item(pkt)) => pending.push_back(pkt),
            Some(Cmd::Flush(done)) => {
                let res = (|| -> Result<(), Error> {
                    for _ in 0..5000 {
                        if pending.is_empty() {
                            break;
                        }
                        submit_pending(&mut codec, &mut pending, &queue)?;
                        if !pending.is_empty() {
                            thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                    if !pending.is_empty() {
                        return Err(Error::Platform("flush timed out submitting pending".into()));
                    }
                    cmd::send_eos(&mut codec)?;
                    cmd::drain_until_eos(&mut codec, |out| {
                        let frame = output_to_frame(&out)?;
                        frame_tx.send(Ok(frame)).map_err(|_| Error::Dropped)
                    })?;
                    codec
                        .flush()
                        .map_err(|e| Error::Platform(format!("{e:?}")))?;
                    Ok(())
                })();
                let _ = done.send(res);
            }
            Some(Cmd::Close) | None => {
                drain_output(&mut codec, &frame_tx);
                queue.store(0, Ordering::Relaxed);
                return;
            }
        }

        let _ = submit_pending(&mut codec, &mut pending, &queue);
        drain_output(&mut codec, &frame_tx);

        if cmd_rx.is_closed() && pending.is_empty() {
            drain_output(&mut codec, &frame_tx);
            queue.store(0, std::sync::atomic::Ordering::Relaxed);
            return;
        }
    }
}

fn submit_pending(
    codec: &mut MediaCodec,
    pending: &mut std::collections::VecDeque<EncodedVideoPacket>,
    queue: &std::sync::Arc<std::sync::atomic::AtomicU32>,
) -> Result<(), Error> {
    while let Some(pkt) = pending.pop_front() {
        if let Ok(buf) = codec.dequeue_input(0) {
            let mut buf: mediacodec::CodecInputBuffer = buf;
            let (ptr, cap): (*mut u8, usize) = buf.buffer();
            if pkt.payload.len() > cap {
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
            queue.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            pending.push_front(pkt);
            break;
        }
    }
    Ok(())
}
