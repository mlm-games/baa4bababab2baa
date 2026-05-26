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

impl VideoDecoderOutput for WasmVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        self.inner
            .next()
            .await
            .map(|opt| opt.map(to_our_frame))
            .map_err(|e| match e {
                web_codecs::Error::Dropped => Error::Dropped,
                other => Error::Platform(format!("{other:?}")),
            })
    }
}

impl WasmVideoDecoderOutput {
    pub fn try_frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        self.inner
            .try_recv()
            .map(|opt| opt.map(to_our_frame))
            .map_err(|e| match e {
                web_codecs::Error::Dropped => Error::Dropped,
                other => Error::Platform(format!("{other:?}")),
            })
    }
}

pub fn create(
    config: VideoDecoderConfig,
) -> Result<(WasmVideoDecoderInput, WasmVideoDecoderOutput), Error> {
    let wc_cfg = to_wc_config(&config);
    let (dec, decoded) = wc_cfg
        .build()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    Ok((
        WasmVideoDecoderInput { inner: dec },
        WasmVideoDecoderOutput { inner: decoded },
    ))
}


