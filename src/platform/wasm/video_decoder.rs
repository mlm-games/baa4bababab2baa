use wasodecs::{
    EncodedFrame, VideoDecoded, VideoDecoder, VideoDecoderConfig as WcVideoDecoderConfig,
};

use crate::{
    error::Error,
    traits::{VideoDecoderInput, VideoDecoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, HardwareBuffer, PixelFormat, VideoDecoderConfig,
        VideoFrame, VideoOutputMode, VideoPlanes, video::HardwareBufferInner,
    },
};

pub(super) fn to_wc_config(cfg: &VideoDecoderConfig) -> WcVideoDecoderConfig {
    let mut wc = WcVideoDecoderConfig::new(cfg.codec.to_mime());

    if let Some(res) = cfg.resolution {
        wc.resolution = Some(wasodecs::Dimensions::new(res.width, res.height));
        wc.display = Some(wasodecs::Dimensions::new(res.width, res.height));
    }

    if let Some(desc) = &cfg.description {
        wc.description = Some(desc.clone());
    }

    if let Some(hw) = cfg.hardware_acceleration {
        wc.hardware_acceleration = Some(hw);
    }

    wc
}

fn to_our_pixel_format(fmt: web_sys::VideoPixelFormat) -> PixelFormat {
    use web_sys::VideoPixelFormat;
    match fmt {
        VideoPixelFormat::I420 | VideoPixelFormat::I420a => PixelFormat::Yuv420p,
        VideoPixelFormat::Nv12 => PixelFormat::Nv12,
        VideoPixelFormat::Rgba => PixelFormat::Rgba8,
        VideoPixelFormat::Bgra => PixelFormat::Bgra8,
        _ => PixelFormat::Nv12,
    }
}

fn to_our_frame_hw(f: wasodecs::VideoFrame) -> VideoFrame {
    let dims = f.dimensions();
    let ts = f.timestamp();
    let fmt = f
        .format()
        .map(to_our_pixel_format)
        .unwrap_or(PixelFormat::Nv12);
    VideoFrame {
        dimensions: Dimensions::new(dims.width, dims.height),
        format: fmt,
        timestamp: ts,
        planes: VideoPlanes::Hardware(HardwareBuffer {
            inner: HardwareBufferInner::WebCodecs(f),
        }),
    }
}

async fn to_our_frame_copied(f: wasodecs::VideoFrame) -> Result<VideoFrame, Error> {
    let dims = f.dimensions();
    let ts = f.timestamp();
    let fmt = f
        .format()
        .map(to_our_pixel_format)
        .unwrap_or(PixelFormat::Nv12);
    let data = f
        .copy_to_cpu()
        .await
        .map_err(|e| Error::Platform(format!("copy_to_cpu: {e:?}")))?;
    Ok(VideoFrame {
        dimensions: Dimensions::new(dims.width, dims.height),
        format: fmt,
        timestamp: ts,
        planes: VideoPlanes::Cpu(data),
    })
}

pub struct WasmVideoDecoderInput {
    inner: VideoDecoder,
}

impl VideoDecoderInput for WasmVideoDecoderInput {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error> {
        let frame = EncodedFrame {
            payload: packet.payload,
            timestamp: packet.timestamp,
            keyframe: packet.keyframe,
        };
        self.inner
            .decode(frame)
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }

    async fn flush(&mut self) -> Result<(), Error> {
        self.inner
            .flush()
            .await
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }

    fn queue_size(&self) -> u32 {
        self.inner.queue_size()
    }
}

pub struct WasmVideoDecoderOutput {
    inner: VideoDecoded,
    output_mode: VideoOutputMode,
}

impl VideoDecoderOutput for WasmVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        let frame = self.inner.next().await.map_err(|e| match e {
            wasodecs::Error::Dropped => Error::Dropped,
            other => Error::Platform(format!("{other:?}")),
        })?;
        match frame {
            Some(f) => match self.output_mode {
                VideoOutputMode::Cpu => Ok(Some(to_our_frame_copied(f).await?)),
                _ => Ok(Some(to_our_frame_hw(f))),
            },
            None => Ok(None),
        }
    }

    fn try_frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.output_mode {
            VideoOutputMode::Cpu => Err(Error::InvalidConfig(
                "try_frame() with Cpu output is not supported on wasm; use frame() for async copy"
                    .into(),
            )),
            _ => {
                let frame = self.inner.try_recv().map_err(|e| match e {
                    wasodecs::Error::Dropped => Error::Dropped,
                    other => Error::Platform(format!("{other:?}")),
                })?;
                Ok(frame.map(to_our_frame_hw))
            }
        }
    }
}

impl WasmVideoDecoderOutput {
    /// Returns the raw `wasodecs::VideoFrame` without converting to [`VideoPlanes::Hardware`].
    /// The caller can copy the pixel data to CPU memory later via
    /// [`wasodecs::VideoFrame::copy_to_cpu`].
    pub fn try_frame_raw(&mut self) -> Result<Option<wasodecs::VideoFrame>, Error> {
        self.inner.try_recv().map_err(|e| match e {
            wasodecs::Error::Dropped => Error::Dropped,
            other => Error::Platform(format!("{other:?}")),
        })
    }
}

pub fn create(
    config: VideoDecoderConfig,
) -> Result<(WasmVideoDecoderInput, WasmVideoDecoderOutput), Error> {
    let candidates = config.codec.to_webcodecs_strings();
    let mut last_err = None;

    let try_hw = config.hardware_acceleration;
    let output_mode = config.output_mode;

    let hw_passes: &[Option<bool>] = match try_hw {
        Some(false) => &[Some(false)],
        Some(true) => &[Some(true), Some(false)],
        None => &[None, Some(false)],
    };

    for &prefer_hw in hw_passes {
        for codec_str in candidates.iter().map(String::as_str) {
            let mut wc_cfg = to_wc_config(&config);
            wc_cfg.codec = codec_str.to_string();
            wc_cfg.hardware_acceleration = prefer_hw;

            match wc_cfg.build() {
                Ok((dec, decoded)) => {
                    return Ok((
                        WasmVideoDecoderInput { inner: dec },
                        WasmVideoDecoderOutput {
                            inner: decoded,
                            output_mode,
                        },
                    ));
                }
                Err(e) => last_err = Some(e),
            }
        }
    }

    Err(Error::Platform(format!(
        "No supported codec variant for {:?}: {:?}",
        config.codec, last_err
    )))
}
