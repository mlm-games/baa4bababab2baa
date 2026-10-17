//! Windows Media Foundation backend: hardware H.264 / HEVC decode through decoder MFTs driven
//! with the process's Direct3D 11 video device (DXVA), with NV12 / P010 surfaces read back to
//! CPU frames.
//!
//! Ported from filmcraft's `media_foundation` backend (MIT OR Apache-2.0). H.264 and HEVC are
//! taken; every other codec, and every stream Media Foundation does not accept, returns
//! [`Error::Unsupported`], so callers fall back to software. Encoders are not provided (the app
//! encodes in software on Windows).

#[allow(unsafe_code)]
mod biplanar;
#[allow(unsafe_code)]
mod gpu;
#[allow(unsafe_code)]
mod mft;

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use std::thread;

use tokio::sync::{mpsc, oneshot};

use crate::{
    error::Error,
    traits::{VideoDecoderInput, VideoDecoderOutput},
    types::{
        AudioDecoderConfig, AudioEncoderConfig, Dimensions, EncodedVideoPacket, PixelFormat,
        Timestamp, VideoCodecId, VideoDecoderConfig, VideoEncoderConfig, VideoFrame, VideoPlanes,
    },
};

use self::gpu::{Gpu, Readback, SurfaceFormat};
use self::mft::{Attr, MfApi, Mft, Poll, Spec};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_DECODER_PROFILE_H264_VLD_NOFGT, D3D11_DECODER_PROFILE_HEVC_VLD_MAIN,
    D3D11_DECODER_PROFILE_HEVC_VLD_MAIN10,
};
use windows::Win32::Media::MediaFoundation::{
    MF_MT_MPEG2_PROFILE, MFVideoFormat_H264, MFVideoFormat_HEVC,
};

/// Pictures the decoder may hold back (a DPB is at most 16 frames; more means it lost track).
const MAX_IN_FLIGHT: usize = 64;
/// Upper bound on the outputs taken per call: a misbehaving MFT cannot make a decode call loop.
const MAX_OUTPUTS_PER_CALL: usize = 4096;

enum Cmd {
    Packet(EncodedVideoPacket),
    Flush(oneshot::Sender<Result<(), Error>>),
    Close,
}

pub struct WindowsVideoDecoderInput {
    tx: mpsc::UnboundedSender<Cmd>,
    queue: Arc<AtomicU32>,
}

pub struct WindowsVideoDecoderOutput {
    rx: mpsc::UnboundedReceiver<Result<VideoFrame, Error>>,
    /// Echoed onto every frame handed back; see [`VideoDecoderConfig::color`].
    color: Option<crate::types::VideoColorInfo>,
    /// Set when the worker thread exits via a clean teardown path (`Close` / sender dropped). A
    /// `Disconnected` channel *without* this flag means the thread died unexpectedly (panic),
    /// which must surface as an error so callers can fall back to software instead of stalling
    /// forever on `Ok(None)`.
    clean_exit: Arc<AtomicBool>,
}

impl Drop for WindowsVideoDecoderInput {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

impl VideoDecoderInput for WindowsVideoDecoderInput {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error> {
        self.queue.fetch_add(1, Ordering::Relaxed);
        if let Err(e) = self.tx.send(Cmd::Packet(packet)) {
            self.queue.fetch_sub(1, Ordering::Relaxed);
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
        self.queue.load(Ordering::Relaxed)
    }
}

impl WindowsVideoDecoderOutput {
    fn stamp(&self, frame: &mut VideoFrame) {
        frame.color = self.color;
    }
}

impl VideoDecoderOutput for WindowsVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.rx.recv().await {
            Some(Ok(mut frame)) => {
                self.stamp(&mut frame);
                Ok(Some(frame))
            }
            Some(Err(e)) => Err(e),
            // Clean shutdown reads as EOS; an unclean one is decoder death.
            None if self.clean_exit.load(Ordering::Acquire) => Ok(None),
            None => Err(Error::Dropped),
        }
    }

    fn try_frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.rx.try_recv() {
            Ok(Ok(mut frame)) => {
                self.stamp(&mut frame);
                Ok(Some(frame))
            }
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

/// Open a hardware H.264 / HEVC decoder. May fail: Media Foundation is unavailable, there is no
/// Direct3D 11 video device, or no decoder MFT takes the stream (for HEVC, the HEVC Video
/// Extensions). Callers then decode in software.
pub fn create(
    config: VideoDecoderConfig,
) -> Result<(WindowsVideoDecoderInput, WindowsVideoDecoderOutput), Error> {
    // Validate codec support synchronously (for easier fallback).
    if !matches!(config.codec, VideoCodecId::H264 { .. } | VideoCodecId::Hevc) {
        return Err(Error::Unsupported);
    }
    if config
        .resolution
        .is_some_and(|r| r.width == 0 || r.height == 0)
    {
        return Err(Error::InvalidConfig("dimensions must be non-zero".into()));
    }

    let color = config.color;
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Result<VideoFrame, Error>>();
    let queue = Arc::new(AtomicU32::new(0));

    let queue2 = queue.clone();
    let clean_exit = Arc::new(AtomicBool::new(false));
    let clean_exit2 = clean_exit.clone();
    thread::spawn(move || {
        worker_loop(config, cmd_rx, frame_tx, queue2, clean_exit2);
    });

    Ok((
        WindowsVideoDecoderInput { tx: cmd_tx, queue },
        WindowsVideoDecoderOutput {
            rx: frame_rx,
            color,
            clean_exit,
        },
    ))
}

/// The MFT request for the stream, for one output format. H.264 takes NV12 only; HEVC is offered
/// NV12 (Main) first and P010 (Main 10) second — the ladder [`Decoder::open`] walks.
fn spec(config: &VideoDecoderConfig, format: SurfaceFormat) -> Result<Spec, Error> {
    let dimensions = config
        .resolution
        .ok_or_else(|| Error::InvalidConfig("decoder needs a resolution".into()))?;
    let size = (dimensions.width, dimensions.height);
    match config.codec {
        VideoCodecId::H264 { .. } => {
            if format != SurfaceFormat::Nv12 {
                return Err(Error::Unsupported);
            }
            Ok(Spec {
                subtype: MFVideoFormat_H264,
                label: "H.264",
                missing: "",
                size,
                format,
                attrs: Vec::new(),
            })
        }
        VideoCodecId::Hevc => {
            // the HEVC decoder lists P010 output only when told the stream is Main 10
            // (eAVEncH265VProfile_Main_420_8 = 1, eAVEncH265VProfile_Main_420_10 = 2)
            let profile = match format {
                SurfaceFormat::Nv12 => 1,
                SurfaceFormat::P010 => 2,
            };
            Ok(Spec {
                subtype: MFVideoFormat_HEVC,
                label: "HEVC",
                missing: " (the HEVC Video Extensions are not installed)",
                size,
                format,
                attrs: vec![(MF_MT_MPEG2_PROFILE, Attr::U32(profile))],
            })
        }
        _ => Err(Error::Unsupported),
    }
}

struct Decoder {
    api: &'static MfApi,
    gpu: Arc<Gpu>,
    mft: Mft,
    format: SurfaceFormat,
    readback: Readback,
    /// Presentation times (µs) of submitted access units whose picture has not come out yet.
    in_flight: Vec<i64>,
}

impl Decoder {
    fn open(config: &VideoDecoderConfig) -> Result<Decoder, Error> {
        let api = mft::api().map_err(Error::Platform)?;
        let gpu = Gpu::shared(api).map_err(Error::Platform)?;
        // HEVC is offered NV12 first, P010 second (a GPU may list only one of them).
        let ladder: &[SurfaceFormat] = match config.codec {
            VideoCodecId::Hevc => &[SurfaceFormat::Nv12, SurfaceFormat::P010],
            _ => &[SurfaceFormat::Nv12],
        };
        let mut reasons = Vec::new();
        for format in ladder {
            let request = match spec(config, *format) {
                Ok(s) => s,
                Err(e) => {
                    reasons.push(e.to_string());
                    continue;
                }
            };
            let profile = match (&config.codec, *format) {
                (VideoCodecId::Hevc, SurfaceFormat::P010) => D3D11_DECODER_PROFILE_HEVC_VLD_MAIN10,
                (VideoCodecId::Hevc, _) => D3D11_DECODER_PROFILE_HEVC_VLD_MAIN,
                _ => D3D11_DECODER_PROFILE_H264_VLD_NOFGT,
            };
            if let Err(e) = gpu.supports(profile, *format, request.size) {
                reasons.push(e);
                continue;
            }
            match Mft::new(api, &gpu, &request) {
                Ok(mft) => {
                    log::info!("hardware decoding: {} on {}", mft.name(), gpu.name());
                    return Ok(Decoder {
                        api,
                        gpu,
                        mft,
                        format: *format,
                        readback: Readback::default(),
                        in_flight: Vec::new(),
                    });
                }
                Err(e) => reasons.push(e),
            }
        }
        Err(Error::Platform(reasons.join("; ")))
    }

    /// The submitted presentation time an output stamped `t` belongs to.
    fn take_pts(&mut self, t: i64) -> i64 {
        if let Some(i) = self.in_flight.iter().position(|&p| p == t) {
            return self.in_flight.swap_remove(i);
        }
        // the MFT restamped it: pictures come out in presentation order, so the earliest
        match self
            .in_flight
            .iter()
            .enumerate()
            .min_by_key(|(_, p)| **p)
            .map(|(i, _)| i)
        {
            Some(i) => self.in_flight.swap_remove(i),
            None => t,
        }
    }

    /// One picture from the decoder's output sample, as a tight I420 frame.
    fn picture(
        &mut self,
        config: &VideoDecoderConfig,
        sample: &windows::Win32::Media::MediaFoundation::IMFSample,
        time: i64,
    ) -> Result<(i64, Vec<u8>), Error> {
        let dimensions = config
            .resolution
            .ok_or_else(|| Error::InvalidConfig("decoder needs a resolution".into()))?;
        let crop = (0, 0, dimensions.width, dimensions.height);
        let bits = self.format.bits();
        let data = self
            .readback
            .read(&self.gpu, sample, self.format, |b| {
                biplanar::to_i420(b, crop, bits)
            })
            .map_err(Error::Platform)?;
        let pts_us = self.take_pts(time / 10);
        Ok((pts_us, data))
    }

    /// Take every picture the decoder has ready. `until_input`: the decoder is draining, so
    /// `NeedInput` ends the stream instead of waiting for more input.
    fn collect(
        &mut self,
        config: &VideoDecoderConfig,
        out: &mut Vec<(i64, Vec<u8>)>,
    ) -> Result<(), Error> {
        for _ in 0..MAX_OUTPUTS_PER_CALL {
            match self.mft.poll() {
                Ok(Poll::NeedInput) => return Ok(()),
                Ok(Poll::FormatChanged) => continue,
                Ok(Poll::Frame(sample, time)) => {
                    out.push(self.picture(config, &sample, time)?);
                }
                Err(e) => return Err(Error::Platform(e)),
            }
        }
        Err(Error::Platform("the decoder keeps producing output".into()))
    }

    fn feed(
        &mut self,
        annex_b: &[u8],
        pts_us: i64,
        config: &VideoDecoderConfig,
        out: &mut Vec<(i64, Vec<u8>)>,
    ) -> Result<(), Error> {
        for attempt in 0..2 {
            let accepted = self
                .mft
                .input(self.api, annex_b, pts_us * 10)
                .map_err(Error::Platform)?;
            if accepted {
                self.in_flight.push(pts_us);
                self.collect(config, out)?;
                if self.in_flight.len() > MAX_IN_FLIGHT {
                    return Err(Error::Platform(format!(
                        "the decoder holds {} pictures back",
                        self.in_flight.len()
                    )));
                }
                return Ok(());
            }
            // full: take what it has ready, then offer the sample again
            self.collect(config, out)?;
            if attempt == 1 {
                break;
            }
        }
        Err(Error::Platform("the decoder does not accept input".into()))
    }

    fn flush(
        &mut self,
        config: &VideoDecoderConfig,
        out: &mut Vec<(i64, Vec<u8>)>,
    ) -> Result<(), Error> {
        self.mft.drain().map_err(Error::Platform)?;
        self.collect(config, out)?;
        self.in_flight.clear();
        Ok(())
    }
}

fn worker_loop(
    config: VideoDecoderConfig,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    frame_tx: mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    queue: Arc<AtomicU32>,
    clean_exit: Arc<AtomicBool>,
) {
    let dimensions = config.resolution.unwrap_or(Dimensions {
        width: 0,
        height: 0,
    });
    let mut decoder: Option<Decoder> = None;

    loop {
        let Some(cmd) = cmd_rx.blocking_recv() else {
            queue.store(0, Ordering::Relaxed);
            clean_exit.store(true, Ordering::Release);
            return;
        };

        match cmd {
            Cmd::Close => {
                queue.store(0, Ordering::Relaxed);
                clean_exit.store(true, Ordering::Release);
                return;
            }
            Cmd::Flush(done) => {
                let res = (|| -> Result<(), Error> {
                    if let Some(dec) = decoder.as_mut() {
                        let mut frames = Vec::new();
                        dec.flush(&config, &mut frames)?;
                        send_frames(&frame_tx, dimensions, frames)?;
                    }
                    Ok(())
                })();
                let _ = done.send(res);
            }
            Cmd::Packet(pkt) => {
                queue.fetch_sub(1, Ordering::Relaxed);
                let res = (|| -> Result<(), Error> {
                    if decoder.is_none() {
                        decoder = Some(Decoder::open(&config)?);
                    }
                    let dec = decoder.as_mut().ok_or(Error::Dropped)?;
                    let pts_us = pkt.timestamp.as_micros() as i64;
                    let mut frames = Vec::new();
                    dec.feed(&pkt.payload, pts_us, &config, &mut frames)?;
                    send_frames(&frame_tx, dimensions, frames)
                })();
                if let Err(e) = res {
                    let _ = frame_tx.send(Err(e));
                    queue.store(0, Ordering::Relaxed);
                    clean_exit.store(true, Ordering::Release);
                    return;
                }
            }
        }
    }
}

fn send_frames(
    frame_tx: &mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    dimensions: Dimensions,
    frames: Vec<(i64, Vec<u8>)>,
) -> Result<(), Error> {
    for (pts_us, data) in frames {
        let frame = VideoFrame {
            dimensions,
            format: PixelFormat::Yuv420p,
            timestamp: Timestamp::from_micros(pts_us.max(0) as u64),
            color: None,
            planes: VideoPlanes::Cpu(data),
        };
        frame_tx.send(Ok(frame)).map_err(|_| Error::Dropped)?;
    }
    Ok(())
}

/// The Media Foundation host: what the crate's platform dispatch calls on Windows.
pub struct MediaFoundationHost;

impl MediaFoundationHost {
    pub fn new() -> Self {
        Self
    }

    pub fn create_video_decoder(
        &self,
        config: VideoDecoderConfig,
    ) -> Result<(WindowsVideoDecoderInput, WindowsVideoDecoderOutput), Error> {
        create(config)
    }

    /// Media Foundation encoding is not provided: the app encodes in software on Windows.
    pub fn create_video_encoder(
        &self,
        _config: VideoEncoderConfig,
    ) -> Result<(WindowsVideoEncoderInput, WindowsVideoEncoderOutput), Error> {
        Err(Error::Unsupported)
    }

    pub fn create_audio_encoder(
        &self,
        _config: AudioEncoderConfig,
    ) -> Result<(WindowsAudioEncoderInput, WindowsAudioEncoderOutput), Error> {
        Ok((WindowsAudioEncoderInput, WindowsAudioEncoderOutput))
    }

    pub fn create_audio_decoder(
        &self,
        _config: AudioDecoderConfig,
    ) -> Result<(WindowsAudioDecoderInput, WindowsAudioDecoderOutput), Error> {
        Ok((WindowsAudioDecoderInput, WindowsAudioDecoderOutput))
    }

    pub async fn is_video_decoder_supported(
        &self,
        config: &VideoDecoderConfig,
    ) -> Result<bool, Error> {
        Ok(matches!(
            config.codec,
            VideoCodecId::H264 { .. } | VideoCodecId::Hevc
        ))
    }

    pub async fn is_video_encoder_supported(
        &self,
        _config: &VideoEncoderConfig,
    ) -> Result<bool, Error> {
        Ok(false)
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

/// Placeholder audio endpoints: this backend carries no audio codec support.
pub struct WindowsVideoEncoderInput;
pub struct WindowsVideoEncoderOutput;
pub struct WindowsAudioEncoderInput;
pub struct WindowsAudioEncoderOutput;
pub struct WindowsAudioDecoderInput;
pub struct WindowsAudioDecoderOutput;
