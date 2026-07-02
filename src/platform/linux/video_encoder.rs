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

use va::{Display, Image, Surface, UsageHint, VAImageFormat, VA_RT_FORMAT_YUV420};

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
                stride: width as usize,
            },
            PlaneLayout {
                buffer_index: 0,
                offset: (width * height) as usize,
                stride: width as usize,
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
    let mut flushing: Option<oneshot::Sender<Result<(), Error>>> = None;

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Encode(frame, keyopt) => {
                    queue.fetch_sub(1, Ordering::Relaxed);
                    pending.push_back((frame, keyopt.unwrap_or(false)));
                }
                Cmd::Flush(done) => {
                    flushing = Some(done);
                }
                Cmd::Close => {
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            }
        }

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

        if let Some(done) = flushing.take() {
            // Don't flush while frames are still pending
            if !pending.is_empty() {
                flushing = Some(done);
            } else {
                let res = (|| -> Result<(), Error> {
                    encoder
                        .drain()
                        .map_err(|e| Error::Platform(format!("drain failed: {e}")))?;
                    // Poll all remaining output after drain
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
                                    return Err(Error::Dropped);
                                }
                            }
                            Ok(None) => break,
                            Err(e) => return Err(Error::Platform(format!("poll failed: {e}"))),
                        }
                    }
                    Ok(())
                })();
                let _ = done.send(res);
            }
        } else {
            // Normal poll output (not flushing)
            loop {
                let coded = match encoder.poll() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = pkt_tx.send(Err(Error::Platform(format!("poll failed: {e}"))));
                        queue.store(0, Ordering::Relaxed);
                        return;
                    }
                };

                let Some(coded) = coded else {
                    break;
                };

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
        }

        if cmd_rx.is_closed() && pending.is_empty() && flushing.is_none() {
            queue.store(0, Ordering::Relaxed);
            return;
        }

        thread::sleep(Duration::from_millis(1));
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
        "video/avc" | "video/h264" => {
            // Skip start code (0x00000001 or 0x000001)
            let offset = if data.len() > 4 && data[..4] == [0, 0, 0, 1] {
                4
            } else if data.len() > 3 && data[..3] == [0, 0, 1] {
                3
            } else {
                return None;
            };
            if offset < data.len() {
                // NAL unit type is in the first byte: type = byte & 0x1F
                let nal_type = data[offset] & 0x1F;
                // IDR slice: NAL type 5
                Some(nal_type == 5)
            } else {
                None
            }
        }
        "video/hevc" | "video/h265" => {
            let offset = if data.len() > 4 && data[..4] == [0, 0, 0, 1] {
                4
            } else if data.len() > 3 && data[..3] == [0, 0, 1] {
                3
            } else {
                return None;
            };
            if offset < data.len() {
                // NAL unit type is in the first byte shifted right by 1: (byte >> 1) & 0x3F
                let nal_type = (data[offset] >> 1) & 0x3F;
                // IDR_W_RADL = 19, IDR_N_LP = 20, CRA_NUT = 21
                Some(nal_type == 19 || nal_type == 20 || nal_type == 21)
            } else {
                None
            }
        }
        "video/vp9" => {
            // Frame marker byte, bit 0 indicates keyframe (0 = keyframe)
            Some((data[0] & 0x80) == 0)
        }
        "video/av1" | "video/av01" => {
            // Temporal delimiter + sequence header. Keyframe = frame_type == 0 (KEY_FRAME)
            // The frame_type is in the uncompressed header after the sequence header
            // AV1 OBU header: first byte has obu_type in bits 3-7
            if data.is_empty() {
                return None;
            }
            let obu_type = (data[0] >> 3) & 0x0F;
            // OBU_SEQUENCE_HEADER = 1, OBU_FRAME = 6, OBU_FRAME_HEADER = 3
            Some(obu_type == 1 || obu_type == 6 || obu_type == 3)
        }
        _ => None,
    }
}
