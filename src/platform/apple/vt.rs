//! macOS VideoToolbox backend via the runtime-loaded oxideav bridge.

use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::Duration,
};

use oxideav_core::{
    CodecId, CodecParameters, Decoder as OxDecoder, Encoder as OxEncoder, Error as OxError,
    Frame as OxFrame, Packet as OxPacket, PixelFormat as OxPixelFormat, Rational, TimeBase,
};
use oxideav_videotoolbox as oxvt;
use tokio::sync::{mpsc, oneshot};

use crate::{
    error::Error,
    traits::{
        AudioDecoderInput, AudioDecoderOutput, AudioEncoderInput, AudioEncoderOutput,
        VideoDecoderInput, VideoDecoderOutput, VideoEncoderInput, VideoEncoderOutput,
    },
    types::{
        AudioDecoderConfig, AudioEncoderConfig, AudioFrame, Dimensions, EncodedAudioPacket,
        EncodedVideoPacket, PixelFormat, VideoCodecId, VideoDecoderConfig, VideoEncoderConfig,
        VideoFrame, VideoPlanes,
    },
};

/// Microsecond time base: `baa` timestamps are `Duration`s, oxideav packets
/// in this backend always carry micros.
const MICROS_BASE: TimeBase = TimeBase::new(1, 1_000_000);

fn ox_err(e: OxError) -> Error {
    match e {
        OxError::Unsupported(msg) => {
            let _ = msg;
            Error::Unsupported
        }
        OxError::InvalidData(msg) => Error::InvalidConfig(msg),
        OxError::NeedMore | OxError::Eof => Error::Platform("unexpected oxideav flow error".into()),
        other => Error::Platform(other.to_string()),
    }
}

fn is_ox_need_more_or_eof(e: &OxError) -> bool {
    matches!(e, OxError::NeedMore | OxError::Eof)
}

pub struct VideoToolboxHost;

impl VideoToolboxHost {
    pub fn new() -> Self {
        Self
    }

    pub fn create_video_encoder(
        &self,
        config: VideoEncoderConfig,
    ) -> Result<(AppleVideoEncoderInput, AppleVideoEncoderOutput), Error> {
        create_encoder(config)
    }

    pub fn create_video_decoder(
        &self,
        config: VideoDecoderConfig,
    ) -> Result<(AppleVideoDecoderInput, AppleVideoDecoderOutput), Error> {
        create_decoder(config)
    }

    pub fn create_audio_encoder(
        &self,
        config: AudioEncoderConfig,
    ) -> Result<(AppleAudioEncoderInput, AppleAudioEncoderOutput), Error> {
        let _ = config;
        Err(Error::Unsupported)
    }

    pub fn create_audio_decoder(
        &self,
        _config: AudioDecoderConfig,
    ) -> Result<(AppleAudioDecoderInput, AppleAudioDecoderOutput), Error> {
        Err(Error::Unsupported)
    }

    pub async fn is_video_encoder_supported(
        &self,
        config: &VideoEncoderConfig,
    ) -> Result<bool, Error> {
        Ok(framework_available()
            && matches!(config.codec, VideoCodecId::H264 { .. } | VideoCodecId::Hevc))
    }

    pub async fn is_video_decoder_supported(
        &self,
        config: &VideoDecoderConfig,
    ) -> Result<bool, Error> {
        Ok(framework_available()
            && matches!(
                config.codec,
                VideoCodecId::H264 { .. }
                    | VideoCodecId::Hevc
                    | VideoCodecId::Vp9
                    | VideoCodecId::Av1
            ))
    }

    pub async fn is_audio_encoder_supported(
        &self,
        _config: &AudioEncoderConfig,
    ) -> Result<bool, Error> {
        Ok(false)
    }

    pub async fn is_audio_decoder_supported(
        &self,
        _config: &AudioDecoderConfig,
    ) -> Result<bool, Error> {
        Ok(false)
    }
}

/// Probe that the VideoToolbox framework can actually be `dlopen`ed.
/// Cheap and side-effect free; the bridge caches the handle.
fn framework_available() -> bool {
    oxvt::sys::vtable().is_ok()
}

type DecoderFactory = fn(&CodecParameters) -> oxideav_core::Result<Box<dyn OxDecoder>>;

fn decoder_factory(codec: &VideoCodecId) -> Result<(DecoderFactory, &'static str), Error> {
    match codec {
        VideoCodecId::H264 { .. } => Ok((oxvt::decoder::H264VtDecoder::make, "h264")),
        VideoCodecId::Hevc => Ok((oxvt::decoder::HevcVtDecoder::make, "hevc")),
        VideoCodecId::Vp9 => Ok((oxvt::blob::make_vp9_decoder, "vp9")),
        VideoCodecId::Av1 => Ok((oxvt::blob::make_av1_decoder, "av1")),
        VideoCodecId::Vp8 | VideoCodecId::Other(_) => Err(Error::Unsupported),
    }
}

enum DecCmd {
    Packet(EncodedVideoPacket),
    Flush(oneshot::Sender<Result<(), Error>>),
    Close,
}

pub struct AppleVideoDecoderInput {
    tx: mpsc::UnboundedSender<DecCmd>,
    queue: Arc<AtomicU32>,
}

impl Drop for AppleVideoDecoderInput {
    fn drop(&mut self) {
        let _ = self.tx.send(DecCmd::Close);
    }
}

pub struct AppleVideoDecoderOutput {
    rx: mpsc::UnboundedReceiver<Result<VideoFrame, Error>>,
}

impl VideoDecoderInput for AppleVideoDecoderInput {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error> {
        self.queue.fetch_add(1, Ordering::Relaxed);
        self.tx.send(DecCmd::Packet(packet)).map_err(|_| {
            self.queue.fetch_sub(1, Ordering::Relaxed);
            Error::Dropped
        })
    }

    async fn flush(&mut self) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(DecCmd::Flush(tx))
            .map_err(|_| Error::Dropped)?;
        rx.await.map_err(|_| Error::Dropped)?
    }

    fn queue_size(&self) -> u32 {
        self.queue.load(Ordering::Relaxed)
    }
}

impl VideoDecoderOutput for AppleVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.rx.recv().await {
            Some(r) => r.map(Some),
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

pub fn create_decoder(
    config: VideoDecoderConfig,
) -> Result<(AppleVideoDecoderInput, AppleVideoDecoderOutput), Error> {
    let (factory, codec_id) = decoder_factory(&config.codec)?;

    let mut params = CodecParameters::video(CodecId::new(codec_id));
    if let Some(res) = config.resolution {
        params.width = Some(res.width);
        params.height = Some(res.height);
    }
    params.pixel_format = Some(OxPixelFormat::Yuv420P);
    if let Some(desc) = &config.description {
        params.extradata = desc.to_vec();
    }

    let mut decoder = factory(&params).map_err(ox_err)?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<DecCmd>();
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Result<VideoFrame, Error>>();
    let queue = Arc::new(AtomicU32::new(0));
    let queue2 = queue.clone();
    let expected = config.resolution;

    thread::spawn(move || {
        decode_loop(&mut *decoder, cmd_rx, frame_tx, queue2, expected);
    });

    Ok((
        AppleVideoDecoderInput { tx: cmd_tx, queue },
        AppleVideoDecoderOutput { rx: frame_rx },
    ))
}

/// Convert one oxideav I420 video frame into a `baa` frame.
///
/// Planes may carry row padding (`stride > visible width`) — rows are
/// copied tightly. `expected` (the configured resolution) selects the
/// visible window when present and sane; otherwise the full stride is
/// reported. `fallback_pts_us` covers the H.264/HEVC path whose VT
/// callback reports `pts: None`: callers pass the head of the input-PTS
/// FIFO.
fn convert_video_frame(
    vf: oxideav_core::VideoFrame,
    expected: Option<Dimensions>,
    fallback_pts_us: i64,
) -> Result<VideoFrame, Error> {
    if vf.planes.len() != 3 {
        return Err(Error::Platform(format!(
            "videotoolbox: expected 3 I420 planes, got {}",
            vf.planes.len()
        )));
    }
    let (y, u, v) = (&vf.planes[0], &vf.planes[1], &vf.planes[2]);
    if y.stride == 0 || u.stride == 0 || v.stride == 0 {
        return Err(Error::Platform(
            "videotoolbox: zero plane stride (side-channel record?)".into(),
        ));
    }
    let rows_y = y.data.len() / y.stride;
    if rows_y == 0 {
        return Err(Error::Platform("videotoolbox: empty Y plane".into()));
    }
    let w = expected
        .map(|d| d.width as usize)
        .filter(|&ew| ew > 0 && ew <= y.stride)
        .unwrap_or(y.stride);
    let h = expected
        .map(|d| d.height as usize)
        .filter(|&eh| eh > 0 && eh <= rows_y)
        .unwrap_or(rows_y);
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);

    let mut data = vec![0u8; w * h + 2 * cw * ch];
    let (dst_y, dst_c) = data.split_at_mut(w * h);
    let (dst_u, dst_v) = dst_c.split_at_mut(cw * ch);

    for row in 0..h {
        let src = &y.data[row * y.stride..row * y.stride + w];
        dst_y[row * w..row * w + w].copy_from_slice(src);
    }
    for row in 0..ch {
        if (row + 1) * u.stride <= u.data.len() {
            let n = cw.min(u.stride);
            let src = &u.data[row * u.stride..row * u.stride + n];
            dst_u[row * cw..row * cw + n].copy_from_slice(src);
        }
        if (row + 1) * v.stride <= v.data.len() {
            let n = cw.min(v.stride);
            let src = &v.data[row * v.stride..row * v.stride + n];
            dst_v[row * cw..row * cw + n].copy_from_slice(src);
        }
    }

    let pts_us = vf.pts.unwrap_or(fallback_pts_us).max(0) as u64;
    Ok(VideoFrame {
        dimensions: Dimensions::new(w as u32, h as u32),
        format: PixelFormat::Yuv420p,
        timestamp: Duration::from_micros(pts_us),
        planes: VideoPlanes::Cpu(data),
    })
}

fn drain_decoded(
    decoder: &mut dyn OxDecoder,
    frame_tx: &mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    pending_pts: &mut VecDeque<i64>,
    last_pts_us: &mut i64,
    expected: Option<Dimensions>,
) {
    loop {
        match decoder.receive_frame() {
            Ok(OxFrame::Video(vf)) => {
                let fallback = pending_pts.pop_front().unwrap_or(*last_pts_us);
                *last_pts_us = fallback;
                match convert_video_frame(vf, expected, fallback) {
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
            Ok(_) => {} // Audio/subtitle/vector — impossible from a video decoder.
            Err(e) if is_ox_need_more_or_eof(&e) => break,
            Err(e) => {
                let _ = frame_tx.send(Err(ox_err(e)));
                break;
            }
        }
    }
}

fn decode_loop(
    decoder: &mut dyn OxDecoder,
    mut cmd_rx: mpsc::UnboundedReceiver<DecCmd>,
    frame_tx: mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    queue: Arc<AtomicU32>,
    expected: Option<Dimensions>,
) {
    let mut pending_pts: VecDeque<i64> = VecDeque::new();
    let mut last_pts_us: i64 = 0;

    loop {
        match cmd_rx.blocking_recv() {
            Some(DecCmd::Packet(pkt)) => {
                queue.fetch_sub(1, Ordering::Relaxed);
                let pts_us = pkt.timestamp.as_micros().min(i64::MAX as u128) as i64;
                pending_pts.push_back(pts_us);

                let ox_pkt = OxPacket::new(0, MICROS_BASE, pkt.payload.to_vec())
                    .with_pts(pts_us)
                    .with_keyframe(pkt.keyframe);
                if let Err(e) = decoder.send_packet(&ox_pkt) {
                    if !is_ox_need_more_or_eof(&e) {
                        let _ = frame_tx.send(Err(ox_err(e)));
                    }
                }
                drain_decoded(
                    decoder,
                    &frame_tx,
                    &mut pending_pts,
                    &mut last_pts_us,
                    expected,
                );

                if cmd_rx.is_closed() {
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            }
            Some(DecCmd::Flush(done)) => {
                let res = (|| -> Result<(), Error> {
                    decoder.flush().map_err(ox_err)?;
                    drain_decoded(
                        decoder,
                        &frame_tx,
                        &mut pending_pts,
                        &mut last_pts_us,
                        expected,
                    );
                    pending_pts.clear();
                    Ok(())
                })();
                let _ = done.send(res);
            }
            Some(DecCmd::Close) | None => {
                queue.store(0, Ordering::Relaxed);
                return;
            }
        }
    }
}

type EncoderFactory = fn(&CodecParameters) -> oxideav_core::Result<Box<dyn OxEncoder>>;

fn encoder_factory(codec: &VideoCodecId) -> Result<(EncoderFactory, &'static str), Error> {
    match codec {
        VideoCodecId::H264 { .. } => Ok((oxvt::encoder::make_h264_encoder, "h264")),
        VideoCodecId::Hevc => Ok((oxvt::encoder::make_hevc_encoder, "hevc")),
        _ => Err(Error::Unsupported),
    }
}

enum EncCmd {
    Item(VideoFrame, Option<bool>),
    Flush(oneshot::Sender<Result<(), Error>>),
    Close,
}

pub struct AppleVideoEncoderInput {
    tx: mpsc::UnboundedSender<EncCmd>,
    queue: Arc<AtomicU32>,
    config: VideoEncoderConfig,
}

impl Drop for AppleVideoEncoderInput {
    fn drop(&mut self) {
        let _ = self.tx.send(EncCmd::Close);
    }
}

pub struct AppleVideoEncoderOutput {
    rx: mpsc::UnboundedReceiver<Result<EncodedVideoPacket, Error>>,
    decoder_cfg: Option<VideoDecoderConfig>,
}

impl VideoEncoderInput for AppleVideoEncoderInput {
    fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error> {
        self.queue.fetch_add(1, Ordering::Relaxed);
        self.tx.send(EncCmd::Item(frame, keyframe)).map_err(|_| {
            self.queue.fetch_sub(1, Ordering::Relaxed);
            Error::Dropped
        })
    }

    async fn flush(&mut self) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(EncCmd::Flush(tx))
            .map_err(|_| Error::Dropped)?;
        rx.await.map_err(|_| Error::Dropped)?
    }

    fn queue_size(&self) -> u32 {
        self.queue.load(Ordering::Relaxed)
    }

    fn config(&self) -> &VideoEncoderConfig {
        &self.config
    }
}

impl VideoEncoderOutput for AppleVideoEncoderOutput {
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

pub fn create_encoder(
    config: VideoEncoderConfig,
) -> Result<(AppleVideoEncoderInput, AppleVideoEncoderOutput), Error> {
    let (factory, codec_id) = encoder_factory(&config.codec)?;

    if config.dimensions.width == 0 || config.dimensions.height == 0 {
        return Err(Error::InvalidConfig("dimensions must be non-zero".into()));
    }

    let mut params = CodecParameters::video(CodecId::new(codec_id));
    params.width = Some(config.dimensions.width);
    params.height = Some(config.dimensions.height);
    params.pixel_format = Some(OxPixelFormat::Yuv420P);
    if let Some(br) = config.bitrate {
        params.bit_rate = Some(br as u64);
    }
    if let Some(fr) = config.framerate {
        if fr.is_finite() && fr > 0.0 {
            params.frame_rate = Some(Rational::new((fr * 1000.0).round() as i64, 1000));
        }
    }
    match config.hardware_acceleration {
        Some(true) => params.options.insert("hardware", "enable"),
        Some(false) => params.options.insert("hardware", "disable"),
        None => {}
    }

    let mut encoder = factory(&params).map_err(ox_err)?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<EncCmd>();
    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel::<Result<EncodedVideoPacket, Error>>();
    let queue = Arc::new(AtomicU32::new(0));
    let queue2 = queue.clone();

    let decoder_cfg = VideoDecoderConfig {
        codec: config.codec.clone(),
        resolution: Some(config.dimensions),
        description: None,
        hardware_acceleration: None,
        output_mode: crate::types::VideoOutputMode::Cpu,
    };

    let width = config.dimensions.width;
    let height = config.dimensions.height;
    thread::spawn(move || {
        encode_loop(&mut *encoder, cmd_rx, pkt_tx, queue2, width, height);
    });

    Ok((
        AppleVideoEncoderInput {
            tx: cmd_tx,
            queue,
            config,
        },
        AppleVideoEncoderOutput {
            rx: pkt_rx,
            decoder_cfg: Some(decoder_cfg),
        },
    ))
}

/// Convert a `baa` CPU frame into an oxideav I420 frame for VT.
fn to_oxideav_frame(frame: &VideoFrame) -> Result<oxideav_core::VideoFrame, Error> {
    let w = frame.dimensions.width as usize;
    let h = frame.dimensions.height as usize;
    if w == 0 || h == 0 {
        return Err(Error::InvalidConfig(
            "encode frame dimensions must be non-zero".into(),
        ));
    }
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);

    let VideoPlanes::Cpu(buf) = &frame.planes else {
        return Err(Error::InvalidConfig(
            "Apple VideoToolbox encoder requires CPU frames".into(),
        ));
    };

    let (y, u, v) = match frame.format {
        PixelFormat::Yuv420p => {
            let ys = w * h;
            let cs = cw * ch;
            if buf.len() < ys + 2 * cs {
                return Err(Error::InvalidConfig(format!(
                    "I420 buffer wrong size: got {}, expected {}",
                    buf.len(),
                    ys + 2 * cs
                )));
            }
            (
                buf[..ys].to_vec(),
                buf[ys..ys + cs].to_vec(),
                buf[ys + cs..ys + 2 * cs].to_vec(),
            )
        }
        PixelFormat::Nv12 => {
            let ys = w * h;
            if buf.len() < ys + 2 * cw * ch {
                return Err(Error::InvalidConfig(format!(
                    "NV12 buffer wrong size: got {}, expected {}",
                    buf.len(),
                    ys + 2 * cw * ch
                )));
            }
            let uv = &buf[ys..ys + 2 * cw * ch];
            let mut u = vec![0u8; cw * ch];
            let mut v = vec![0u8; cw * ch];
            for i in 0..cw * ch {
                u[i] = uv[2 * i];
                v[i] = uv[2 * i + 1];
            }
            (buf[..ys].to_vec(), u, v)
        }
        _ => return Err(Error::Unsupported),
    };

    Ok(oxideav_core::VideoFrame {
        pts: Some(frame.timestamp.as_micros().min(i64::MAX as u128) as i64),
        planes: vec![
            oxideav_core::VideoPlane { stride: w, data: y },
            oxideav_core::VideoPlane {
                stride: cw,
                data: u,
            },
            oxideav_core::VideoPlane {
                stride: cw,
                data: v,
            },
        ],
    })
}

fn drain_encoded(
    encoder: &mut dyn OxEncoder,
    pkt_tx: &mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
) {
    loop {
        match encoder.receive_packet() {
            Ok(pkt) => {
                let ts_us = pkt.pts.unwrap_or(0).max(0) as u64;
                let out = EncodedVideoPacket {
                    payload: bytes::Bytes::from(pkt.data),
                    timestamp: Duration::from_micros(ts_us),
                    keyframe: pkt.flags.keyframe,
                };
                if pkt_tx.send(Ok(out)).is_err() {
                    return;
                }
            }
            Err(e) if is_ox_need_more_or_eof(&e) => break,
            Err(e) => {
                let _ = pkt_tx.send(Err(ox_err(e)));
                break;
            }
        }
    }
}

fn encode_loop(
    encoder: &mut dyn OxEncoder,
    mut cmd_rx: mpsc::UnboundedReceiver<EncCmd>,
    pkt_tx: mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
    queue: Arc<AtomicU32>,
    _width: u32,
    _height: u32,
) {
    loop {
        match cmd_rx.blocking_recv() {
            Some(EncCmd::Item(frame, _keyframe)) => {
                queue.fetch_sub(1, Ordering::Relaxed);
                let res = (|| -> Result<(), Error> {
                    let vf = to_oxideav_frame(&frame)?;
                    encoder.send_frame(&OxFrame::Video(vf)).map_err(ox_err)?;
                    drain_encoded(encoder, &pkt_tx);
                    Ok(())
                })();
                if let Err(e) = res {
                    let _ = pkt_tx.send(Err(e));
                }
                if cmd_rx.is_closed() {
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            }
            Some(EncCmd::Flush(done)) => {
                let res = (|| -> Result<(), Error> {
                    encoder.flush().map_err(ox_err)?;
                    drain_encoded(encoder, &pkt_tx);
                    Ok(())
                })();
                let _ = done.send(res);
            }
            Some(EncCmd::Close) | None => {
                queue.store(0, Ordering::Relaxed);
                return;
            }
        }
    }
}

pub struct AppleAudioEncoderInput {
    pub config: AudioEncoderConfig,
}
pub struct AppleAudioEncoderOutput;
pub struct AppleAudioDecoderInput;
pub struct AppleAudioDecoderOutput;

impl AudioEncoderInput for AppleAudioEncoderInput {
    fn encode(&mut self, _frame: AudioFrame) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn queue_size(&self) -> u32 {
        0
    }
    fn config(&self) -> &AudioEncoderConfig {
        &self.config
    }
}

impl AudioEncoderOutput for AppleAudioEncoderOutput {
    async fn packet(&mut self) -> Result<Option<EncodedAudioPacket>, Error> {
        Err(Error::Unsupported)
    }
    fn decoder_config(&self) -> Option<&AudioDecoderConfig> {
        None
    }
}

impl AudioDecoderInput for AppleAudioDecoderInput {
    fn decode(&mut self, _packet: EncodedAudioPacket) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn queue_size(&self) -> u32 {
        0
    }
}

impl AudioDecoderOutput for AppleAudioDecoderOutput {
    async fn frame(&mut self) -> Result<Option<AudioFrame>, Error> {
        Err(Error::Unsupported)
    }
}
