use web_codecs::{
    EncodedFrame, VideoDecoded, VideoDecoder, VideoDecoderConfig as WcVideoDecoderConfig,
};

use crate::{
    error::Error,
    traits::{VideoDecoderInput, VideoDecoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, PixelFormat, VideoDecoderConfig, VideoFrame, VideoPlanes,
    },
};

pub(super) fn to_wc_config(cfg: &VideoDecoderConfig) -> WcVideoDecoderConfig {
    let mut wc = WcVideoDecoderConfig::new(&cfg.codec.0);

    if let Some(res) = cfg.resolution {
        wc.resolution = Some(web_codecs::Dimensions::new(res.width, res.height));
        wc.display = Some(web_codecs::Dimensions::new(res.width, res.height));
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

fn to_our_frame(f: web_codecs::VideoFrame) -> VideoFrame {
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
        planes: VideoPlanes::Hardware,
    }
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
}

/// Async version of `to_our_frame` that copies GPU pixel data to CPU memory
async fn to_our_frame_copied(f: web_codecs::VideoFrame) -> Result<VideoFrame, Error> {
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

impl VideoDecoderOutput for WasmVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        let frame = self.inner.next().await.map_err(|e| match e {
            web_codecs::Error::Dropped => Error::Dropped,
            other => Error::Platform(format!("{other:?}")),
        })?;
        match frame {
            Some(f) => Ok(Some(to_our_frame_copied(f).await?)),
            None => Ok(None),
        }
    }

    fn try_frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        // Pixel data from web_codecs::VideoFrame can only be obtained
        // asynchronously (via copy_to_cpu). Use try_frame_raw() to get
        // the raw web_codecs::VideoFrame, then call copy_to_cpu() on it.
        Err(Error::InvalidConfig(
            "try_frame() is not supported on wasm; use try_frame_raw() instead".into(),
        ))
    }
}

impl WasmVideoDecoderOutput {
    /// Returns the raw `web_codecs::VideoFrame` without converting to [`VideoPlanes::Hardware`]. The caller can copy the
    /// pixel data to CPU memory later via [`web_codecs::VideoFrame::copy_to_cpu`].
    pub fn try_frame_raw(&mut self) -> Result<Option<web_codecs::VideoFrame>, Error> {
        self.inner.try_recv().map_err(|e| match e {
            web_codecs::Error::Dropped => Error::Dropped,
            other => Error::Platform(format!("{other:?}")),
        })
    }
}

pub fn create(
    config: VideoDecoderConfig,
) -> Result<(WasmVideoDecoderInput, WasmVideoDecoderOutput), Error> {
    let candidates = super::mime_to_codec_strings(&config.codec.0);
    let mut last_err = None;

    for codec_str in candidates {
        let mut wc_cfg = to_wc_config(&config);
        wc_cfg.codec = codec_str.to_string();

        match wc_cfg.build() {
            Ok((dec, decoded)) => {
                return Ok((
                    WasmVideoDecoderInput { inner: dec },
                    WasmVideoDecoderOutput { inner: decoded },
                ));
            }
            Err(e) => last_err = Some(e),
        }
    }

    Err(Error::Platform(format!(
        "No supported codec variant for {:?}: {:?}",
        config.codec.0, last_err
    )))
}
