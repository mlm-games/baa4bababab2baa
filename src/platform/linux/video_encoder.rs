use std::{
    borrow::Borrow,
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
        mpsc as std_mpsc,
    },
    thread,
    time::Duration,
};

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};

use crate::{
    error::Error,
    traits::{VideoEncoderInput, VideoEncoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, PixelFormat, VideoDecoderConfig, VideoEncoderConfig,
        VideoFrame, VideoPlanes,
    },
};

use cros_codecs::{
    BlockingMode, Fourcc, FrameLayout, PlaneLayout, Resolution,
    backend::vaapi::surface_pool::VaSurfacePool,
    decoder::FramePool,
    encoder::{self, VideoEncoder as CcVideoEncoder},
    libva as va,
    utils::align_up,
};

use va::{Display, Image, Surface, UsageHint, VA_RT_FORMAT_YUV420, VAImageFormat};

type PooledSurface = cros_codecs::backend::vaapi::surface_pool::PooledVaSurface<()>;

struct EncoderInit {
    display: Arc<Display>,
    nv12_fmt: VAImageFormat,
    encoder: Box<dyn CcVideoEncoder<PooledSurface>>,
    pool: VaSurfacePool<()>,
    frame_layout: FrameLayout,
}

enum Cmd {
    Encode(VideoFrame, Option<bool>),
    Flush(oneshot::Sender<Result<(), Error>>),
    Close,
}

pub struct CrosVideoEncoderInput {
    pub config: VideoEncoderConfig,
    tx: mpsc::UnboundedSender<Cmd>,
    queue: Arc<AtomicU32>,
}

pub struct CrosVideoEncoderOutput {
    rx: mpsc::UnboundedReceiver<Result<EncodedVideoPacket, Error>>,
    decoder_cfg: Option<VideoDecoderConfig>,
}

impl Drop for CrosVideoEncoderInput {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

impl VideoEncoderInput for CrosVideoEncoderInput {
    fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error> {
        self.queue.fetch_add(1, Ordering::Relaxed);
        self.tx
            .send(Cmd::Encode(frame, keyframe))
            .map_err(|_| Error::Dropped)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Flush(tx)).map_err(|_| Error::Dropped)?;
        rx.await.map_err(|_| Error::Dropped)?
    }

    fn queue_size(&self) -> u32 {
        self.queue.load(Ordering::Relaxed)
    }

    fn config(&self) -> &VideoEncoderConfig {
        &self.config
    }
}

impl VideoEncoderOutput for CrosVideoEncoderOutput {
    async fn packet(&mut self) -> Result<Option<EncodedVideoPacket>, Error> {
        match self.rx.recv().await {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }

    fn decoder_config(&self) -> Option<&VideoDecoderConfig> {
        self.decoder_cfg.as_ref()
    }
}

fn init_encoder_inner(config: &VideoEncoderConfig) -> Result<EncoderInit, Error> {
    let display =
        Display::open().ok_or_else(|| Error::Platform("VAAPI Display::open failed".into()))?;

    let nv12_fmt = display
        .query_image_formats()
        .map_err(|e| Error::Platform(format!("query_image_formats failed: {e:?}")))?
        .into_iter()
        .find(|f| f.fourcc == u32::from(Fourcc::from(b"NV12")))
        .ok_or_else(|| {
            Error::Platform("VAAPI driver does not expose NV12 mapping format".into())
        })?;

    let width = config.dimensions.width;
    let height = config.dimensions.height;

    if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
        return Err(Error::InvalidConfig(
            "dimensions must be non-zero and even (for NV12 4:2:0)".into(),
        ));
    }

    let coded_w = align_up(width, 16); // for av1 when supported
    let coded_h = align_up(height, 16);
    let coded_size = Resolution {
        width: coded_w,
        height: coded_h,
    };
    let input_fourcc = Fourcc::from(b"NV12");

    let encoder = create_vaapi_encoder(&display, config, input_fourcc, coded_size)?;

    let mut pool = VaSurfacePool::new(
        Arc::clone(&display),
        VA_RT_FORMAT_YUV420,
        Some(UsageHint::USAGE_HINT_ENCODER),
        coded_size,
    );

    pool.add_frames(vec![(); 16])
        .map_err(|e| Error::Platform(format!("create VA surfaces failed: {e:?}")))?;

    let frame_layout = FrameLayout {
        format: (input_fourcc, 0),
        size: coded_size,
        planes: vec![
            PlaneLayout {
                buffer_index: 0,
                offset: 0,
                stride: coded_w as usize,
            },
            PlaneLayout {
                buffer_index: 0,
                offset: (coded_w * coded_h) as usize,
                stride: coded_w as usize,
            },
        ],
    };

    Ok(EncoderInit {
        display,
        nv12_fmt,
        encoder,
        pool,
        frame_layout,
    })
}

pub fn create(
    config: VideoEncoderConfig,
) -> Result<(CrosVideoEncoderInput, CrosVideoEncoderOutput), Error> {
    let codec = config.codec.0.as_str();
    if !matches!(
        codec,
        "video/avc" | "video/h264" | "video/vp9" | "video/av01" | "video/av1"
    ) {
        return Err(Error::Unsupported);
    }

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel();
    let (init_tx, init_rx) = std_mpsc::channel();

    let queue = Arc::new(AtomicU32::new(0));
    let queue2 = queue.clone();

    let config_clone = config.clone();
    thread::spawn(move || {
        let result = init_encoder_inner(&config_clone);
        match result {
            Ok(init) => {
                let _ = init_tx.send(Ok(()));
                worker_loop(config_clone, cmd_rx, pkt_tx, queue2, init);
            }
            Err(e) => {
                let _ = init_tx.send(Err(e));
            }
        }
    });

    init_rx.recv().map_err(|_| Error::Dropped)??;

    Ok((
        CrosVideoEncoderInput {
            config,
            tx: cmd_tx,
            queue,
        },
        CrosVideoEncoderOutput {
            rx: pkt_rx,
            decoder_cfg: None,
        },
    ))
}

fn worker_loop(
    config: VideoEncoderConfig,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    pkt_tx: mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
    queue: Arc<AtomicU32>,
    init: EncoderInit,
) {
    let EncoderInit {
        display: _display,
        nv12_fmt,
        mut encoder,
        mut pool,
        frame_layout,
    } = init;

    let width = config.dimensions.width;
    let height = config.dimensions.height;

    let mut pending: VecDeque<(VideoFrame, bool)> = VecDeque::new();
    let mut flushing: Vec<oneshot::Sender<Result<(), Error>>> = Vec::new();
    let mut idle = true;

    loop {
        // Block when truly idle
        if idle && pending.is_empty() && flushing.is_empty() {
            match cmd_rx.blocking_recv() {
                Some(Cmd::Encode(frame, keyopt)) => {
                    queue.fetch_sub(1, Ordering::Relaxed);
                    pending.push_back((frame, keyopt.unwrap_or(false)));
                }
                Some(Cmd::Flush(done)) => flushing.push(done),
                Some(Cmd::Close) | None => {
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            }
        }

        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Encode(frame, keyopt) => {
                    queue.fetch_sub(1, Ordering::Relaxed);
                    pending.push_back((frame, keyopt.unwrap_or(false)));
                }
                Cmd::Flush(done) => flushing.push(done),
                Cmd::Close => {
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            }
        }

        // Submit pending frames
        while let Some((frame, force_keyframe)) = pending.pop_front() {
            let Some(handle) = pool.get_surface() else {
                pending.push_front((frame, force_keyframe));
                break;
            };

            let surface: &Surface<()> = Borrow::borrow(&handle);

            let nv12 = match to_nv12_bytes(&frame) {
                Ok(v) => v,
                Err(e) => {
                    let _ = pkt_tx.send(Err(e));
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            };

            if let Err(e) = upload_nv12(&nv12_fmt, surface, width, height, &nv12) {
                let _ = pkt_tx.send(Err(e));
                queue.store(0, Ordering::Relaxed);
                return;
            }

            let meta = encoder::FrameMetadata {
                timestamp: frame.timestamp.as_micros() as u64,
                layout: frame_layout.clone(),
                force_keyframe,
                force_idr: false,
            };

            if let Err(e) = encoder.encode(meta, handle) {
                let _ = pkt_tx.send(Err(Error::Platform(format!("encode failed: {e}"))));
                queue.store(0, Ordering::Relaxed);
                return;
            }
        }

        // Poll output
        let mut polled = false;
        loop {
            let coded = match encoder.poll() {
                Ok(c) => c,
                Err(e) => {
                    if !flushing.is_empty() {
                        break;
                    }
                    let _ = pkt_tx.send(Err(Error::Platform(format!("poll failed: {e}"))));
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            };

            let Some(coded) = coded else {
                break;
            };
            polled = true;

            let ts = Duration::from_micros(coded.metadata.timestamp);
            let keyframe = keyframe_from_bitstream(&coded.bitstream, &config.codec.0)
                .unwrap_or(coded.metadata.force_keyframe);

            let pkt = EncodedVideoPacket {
                payload: Bytes::from(coded.bitstream),
                timestamp: ts,
                keyframe,
            };

            if pkt_tx.send(Ok(pkt)).is_err() {
                queue.store(0, Ordering::Relaxed);
                return;
            }
        }

        // If a flush was requested and all pending frames are submitted, drain the encoder
        if !flushing.is_empty() && pending.is_empty() {
            if let Err(e) = encoder.drain() {
                for done in flushing.drain(..) {
                    let _ = done.send(Err(Error::Platform(format!("drain failed: {e}"))));
                }
                queue.store(0, Ordering::Relaxed);
                return;
            }
            // Poll all remaining after drain
            loop {
                match encoder.poll() {
                    Ok(Some(coded)) => {
                        let ts = Duration::from_micros(coded.metadata.timestamp);
                        let keyframe = keyframe_from_bitstream(&coded.bitstream, &config.codec.0)
                            .unwrap_or(coded.metadata.force_keyframe);
                        let pkt = EncodedVideoPacket {
                            payload: Bytes::from(coded.bitstream),
                            timestamp: ts,
                            keyframe,
                        };
                        if pkt_tx.send(Ok(pkt)).is_err() {
                            for done in flushing.drain(..) {
                                let _ = done.send(Err(Error::Dropped));
                            }
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        for done in flushing.drain(..) {
                            let _ = done.send(Err(Error::Platform(format!("poll failed: {e}"))));
                        }
                        queue.store(0, Ordering::Relaxed);
                        return;
                    }
                }
            }
            for done in flushing.drain(..) {
                let _ = done.send(Ok(()));
            }
        }

        if cmd_rx.is_closed() && pending.is_empty() && flushing.is_empty() {
            queue.store(0, Ordering::Relaxed);
            return;
        }

        // If we didn't poll anything (no output ready) and nothing is pending, go idle.
        idle = !polled && pending.is_empty() && flushing.is_empty();
        if !idle {
            // Avoids tight loop when surfaces aren't available
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn create_vaapi_encoder(
    display: &Arc<Display>,
    config: &VideoEncoderConfig,
    fourcc: Fourcc,
    coded_size: Resolution,
) -> Result<
    Box<dyn CcVideoEncoder<cros_codecs::backend::vaapi::surface_pool::PooledVaSurface<()>>>,
    Error,
> {
    let bitrate = config.bitrate.unwrap_or(1_200_000) as u64;
    let framerate = config.framerate.unwrap_or(30.0) as u32;

    let low_power = config.latency_optimized.unwrap_or(false);

    match config.codec.0.as_str() {
        "video/avc" | "video/h264" => {
            use cros_codecs::codec::h264::parser::{Level, Profile};
            use cros_codecs::encoder::{RateControl, Tunings};

            let level = config
                .level
                .map(|l| match l {
                    30 => Level::L3,
                    31 => Level::L3_1,
                    40 => Level::L4,
                    41 => Level::L4_1,
                    50 => Level::L5,
                    51 => Level::L5_1,
                    52 => Level::L5_2,
                    _ => Level::L4,
                })
                .unwrap_or(Level::L4);

            let cfg = cros_codecs::encoder::h264::EncoderConfig {
                resolution: coded_size,
                profile: Profile::Main,
                level,
                initial_tunings: Tunings {
                    rate_control: RateControl::ConstantBitrate(bitrate),
                    framerate,
                    ..Default::default()
                },
                ..Default::default()
            };

            let enc = cros_codecs::encoder::stateless::h264::StatelessEncoder::new_native_vaapi(
                Arc::clone(display),
                cfg,
                fourcc,
                coded_size,
                low_power,
                BlockingMode::NonBlocking,
            )
            .map_err(|e| Error::Platform(format!("create h264 encoder failed: {e}")))?;

            Ok(Box::new(enc))
        }

        "video/vp9" | "video/av01" | "video/av1" => {
            // TODO: VP9 and AV1 encoders need new_native_vaapi upstream
            return Err(Error::Unsupported);
        }

        _ => Err(Error::Unsupported),
    }
}

fn to_nv12_bytes(frame: &VideoFrame) -> Result<Vec<u8>, Error> {
    let Dimensions { width, height } = frame.dimensions;
    let w = width as usize;
    let h = height as usize;

    let expect = w * h * 3 / 2;

    let VideoPlanes::Cpu(buf) = &frame.planes else {
        return Err(Error::InvalidConfig(
            "Linux VAAPI encoder requires CPU frames".into(),
        ));
    };

    match frame.format {
        PixelFormat::Nv12 => {
            if buf.len() != expect {
                return Err(Error::InvalidConfig(format!(
                    "NV12 buffer wrong size: got {}, expected {}",
                    buf.len(),
                    expect
                )));
            }
            Ok(buf.clone())
        }

        PixelFormat::Yuv420p => {
            let y_sz = w * h;
            let uv_sz = y_sz / 4;

            if buf.len() != expect {
                return Err(Error::InvalidConfig(format!(
                    "I420 buffer wrong size: got {}, expected {}",
                    buf.len(),
                    expect
                )));
            }

            let y = &buf[..y_sz];
            let u = &buf[y_sz..(y_sz + uv_sz)];
            let v = &buf[(y_sz + uv_sz)..];

            let mut out = vec![0u8; expect];
            out[..y_sz].copy_from_slice(y);

            let uv = &mut out[y_sz..];
            for i in 0..uv_sz {
                uv[2 * i] = u[i];
                uv[2 * i + 1] = v[i];
            }

            Ok(out)
        }

        _ => Err(Error::Unsupported),
    }
}

fn upload_nv12(
    nv12_fmt: &VAImageFormat,
    surface: &Surface<()>,
    width: u32,
    height: u32,
    data: &[u8],
) -> Result<(), Error> {
    let mut image = Image::create_from(surface, *nv12_fmt, (width, height), (width, height))
        .map_err(|e| Error::Platform(format!("Image::create_from failed: {e:?}")))?;

    let va_image = *image.image();
    let dest = image.as_mut();

    let w = width as usize;
    let h = height as usize;

    let y_sz = w * h;

    if data.len() != y_sz + y_sz / 2 {
        return Err(Error::InvalidConfig("upload_nv12: wrong input size".into()));
    }

    {
        let y_stride = va_image.pitches[0] as usize;
        let dst_base = &mut dest[va_image.offsets[0] as usize..];
        for (row, src_row) in data[..y_sz].chunks(w).enumerate() {
            let dst_start = row * y_stride;
            if dst_start + w > dst_base.len() {
                return Err(Error::Platform("upload_nv12: y plane dst overflow".into()));
            }
            dst_base[dst_start..dst_start + w].copy_from_slice(src_row);
        }
    }

    {
        let uv_stride = va_image.pitches[1] as usize;
        let uv_off = va_image.offsets[1] as usize;
        let dst_base = &mut dest[uv_off..];
        for (row, src_row) in data[y_sz..].chunks(w).enumerate() {
            let dst_start = row * uv_stride;
            if dst_start + w > dst_base.len() {
                return Err(Error::Platform("upload_nv12: uv plane dst overflow".into()));
            }
            dst_base[dst_start..dst_start + w].copy_from_slice(src_row);
        }
    }

    drop(image);

    surface
        .sync()
        .map_err(|e| Error::Platform(format!("surface.sync failed: {e:?}")))?;

    Ok(())
}

fn keyframe_from_bitstream(data: &[u8], codec: &str) -> Option<bool> {
    if data.is_empty() {
        return None;
    }
    match codec {
        "video/avc" | "video/h264" => h264_has_idr(data),

        "video/hevc" | "video/h265" => h265_is_idr(data),

        "video/vp9" => vp9_is_keyframe(data),

        "video/av1" | "video/av01" => av1_is_keyframe(data),

        _ => None,
    }
}

/// Scan all NAL units in an H.264 access unit for an IDR slice (type 5).
fn h264_has_idr(data: &[u8]) -> Option<bool> {
    let mut pos = 0;
    while pos < data.len() {
        let offset = skip_annexb(data.get(pos..)?)?;
        let nal_start = pos + offset;
        if nal_start >= data.len() {
            return None;
        }
        let nal_type = data[nal_start] & 0x1F;
        if nal_type == 5 {
            return Some(true);
        }
        let remaining = &data[pos + 1..];
        let next_start = remaining
            .windows(3)
            .position(|w| w == [0, 0, 1])
            .map(|i| pos + 1 + i)
            .or_else(|| {
                remaining
                    .windows(4)
                    .position(|w| w == [0, 0, 0, 1])
                    .map(|i| pos + 1 + i)
            })
            .unwrap_or(data.len());
        pos = next_start;
    }
    Some(false)
}

fn skip_annexb(data: &[u8]) -> Option<usize> {
    if data.len() > 4 && data[..4] == [0, 0, 0, 1] {
        Some(4)
    } else if data.len() > 3 && data[..3] == [0, 0, 1] {
        Some(3)
    } else {
        None
    }
}

fn h265_is_idr(data: &[u8]) -> Option<bool> {
    let offset = skip_annexb(data)?;
    if offset < data.len() {
        let nal_type = (data[offset] >> 1) & 0x3F;
        Some(nal_type == 19 || nal_type == 20 || nal_type == 21)
    } else {
        None
    }
}

fn vp9_is_keyframe(data: &[u8]) -> Option<bool> {
    use cros_codecs::codec::vp9::parser::{FrameType, Parser};
    let mut parser = Parser::default();
    match parser.parse_frame(data, 0, data.len()) {
        Ok(frame) => Some(frame.header.frame_type == FrameType::KeyFrame),
        Err(_) => None,
    }
}

fn av1_is_keyframe(data: &[u8]) -> Option<bool> {
    use cros_codecs::codec::av1::parser::{FrameType, ObuType, Parser};
    let mut parser = Parser::default();
    loop {
        let action = match parser.read_obu(data) {
            Ok(a) => a,
            Err(_) => return None,
        };
        match action {
            cros_codecs::codec::av1::parser::ObuAction::Process(obu) => {
                match obu.header.obu_type {
                    ObuType::FrameHeader | ObuType::Frame | ObuType::RedundantFrameHeader => {
                        match parser.parse_frame_header_obu(&obu) {
                            Ok(fh) => return Some(fh.frame_type == FrameType::KeyFrame),
                            Err(_) => return None,
                        }
                    }
                    ObuType::SequenceHeader => {
                        // Parser internally stores sequence header; continue
                    }
                    ObuType::TemporalDelimiter | ObuType::TileGroup | ObuType::Metadata => {
                        // Skip these OBU types
                    }
                    _ => {} // Reserved, Padding etc
                }
            }
            cros_codecs::codec::av1::parser::ObuAction::Drop(_consumed) => {
                // OBU was dropped by parser (e.g. not in operating point) — continue
            }
        }
        // If we've exhausted the data, parser.read_obu returns Err on next call
    }
}
