use js_sys;
use wasodecs::{
    Dimensions as WcDimensions, EncodedFrame, VideoEncodeOptions, VideoEncoded, VideoEncoder,
    VideoEncoderConfig as WcVideoEncoderConfig,
};

use crate::{
    error::Error,
    traits::{VideoEncoderInput, VideoEncoderOutput},
    types::{
        AvcBitstreamFormat, Dimensions, EncodedVideoPacket, PixelFormat, VideoColorSpace,
        VideoDecoderConfig, VideoEncoderConfig, VideoFrame, VideoOutputMode, VideoPlanes,
    },
};

pub(super) fn to_wc_config(cfg: &VideoEncoderConfig) -> WcVideoEncoderConfig {
    let codec = cfg
        .codec
        .to_webcodecs_strings()
        .into_iter()
        .next()
        .unwrap_or_else(|| cfg.codec.to_mime().to_string());

    let mut wc = WcVideoEncoderConfig::new(
        &codec,
        WcDimensions::new(cfg.dimensions.width, cfg.dimensions.height),
    );

    if let Some(br) = cfg.bitrate {
        wc.bitrate = Some(br);
    }
    if let Some(fr) = cfg.framerate {
        wc.framerate = Some(fr);
    }
    if let Some(hw) = cfg.hardware_acceleration {
        wc.hardware_acceleration = Some(hw);
    }
    if let Some(lat) = cfg.latency_optimized {
        wc.latency_optimized = Some(lat);
    }
    if let Some(fmt) = cfg.avc_bitstream_format {
        wc.avc_bitstream_format = Some(match fmt {
            AvcBitstreamFormat::AnnexB => wasodecs::AvcBitstreamFormat::AnnexB,
            AvcBitstreamFormat::Avc => wasodecs::AvcBitstreamFormat::Avc,
        });
    }

    wc
}

pub struct WasmVideoEncoderInput {
    inner: VideoEncoder,
    config: VideoEncoderConfig,
}

impl VideoEncoderInput for WasmVideoEncoderInput {
    fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error> {
        let wc_frame: wasodecs::VideoFrame = match frame.planes {
            VideoPlanes::Hardware(_) => {
                return Err(Error::InvalidConfig(
                    "Cannot re-encode a hardware VideoFrame on WASM".into(),
                ));
            }
            VideoPlanes::Cpu(data) => {
                build_wasm_frame(&data, &frame.dimensions, frame.format, frame.timestamp)?
            }
        };

        let opts = VideoEncodeOptions {
            key_frame: keyframe,
        };
        self.inner
            .encode(&wc_frame, opts)
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

    fn config(&self) -> &VideoEncoderConfig {
        &self.config
    }
}

impl WasmVideoEncoderInput {
    /// Start an async flush without waiting.
    pub fn start_flush(&mut self) -> js_sys::Promise {
        self.inner.start_flush()
    }
}

fn build_wasm_frame(
    data: &[u8],
    dims: &Dimensions,
    format: PixelFormat,
    timestamp: crate::types::Timestamp,
) -> Result<wasodecs::VideoFrame, Error> {
    use js_sys::Uint8Array;
    use web_sys::{VideoFrame, VideoFrameBufferInit, VideoPixelFormat};

    let array = Uint8Array::from(data);

    let wc_format = match format {
        PixelFormat::Yuv420p => VideoPixelFormat::I420,
        PixelFormat::Nv12 => VideoPixelFormat::Nv12,
        PixelFormat::Rgba8 => VideoPixelFormat::Rgba,
        PixelFormat::Bgra8 => VideoPixelFormat::Bgra,
    };

    // (height, width): web-sys VideoFrameBufferInit dictionary constructor
    // args follow alphabetical order: codedHeight before codedWidth.
    let init = VideoFrameBufferInit::new_with_f64(
        dims.height,
        dims.width,
        wc_format,
        timestamp.as_micros() as f64,
    );

    VideoFrame::new_with_u8_array_and_video_frame_buffer_init(&array, &init)
        .map(wasodecs::VideoFrame::from)
        .map_err(|e| Error::Platform(format!("{e:?}")))
}

pub struct WasmVideoEncoderOutput {
    inner: VideoEncoded,
    decoder_cfg: Option<VideoDecoderConfig>,
}

impl WasmVideoEncoderOutput {
    fn build_packet(&mut self, frame: EncodedFrame) -> EncodedVideoPacket {
        if let Some(wc_cfg) = self.inner.config() {
            self.decoder_cfg = Some(VideoDecoderConfig {
                codec: crate::types::VideoCodecId::from_mime(&wc_cfg.codec),
                resolution: wc_cfg
                    .resolution
                    .map(|d| Dimensions::new(d.width, d.height)),
                description: wc_cfg.description.clone(),
                hardware_acceleration: wc_cfg.hardware_acceleration,
                output_mode: VideoOutputMode::Cpu,
            });
        }
        EncodedVideoPacket {
            payload: frame.payload,
            timestamp: frame.timestamp,
            keyframe: frame.keyframe,
        }
    }

    /// Check if the encoder's error callback has fired.
    pub fn check_error(&self) -> Option<Error> {
        self.inner.check_error().map(|e| match e {
            wasodecs::Error::Dropped => Error::Dropped,
            other => Error::Platform(format!("{other:?}")),
        })
    }

    /// Non-blocking read -> returns `Ok(None)` if no frame is available yet.
    pub fn try_packet(&mut self) -> Result<Option<EncodedVideoPacket>, Error> {
        match self.inner.try_recv() {
            Ok(Some(frame)) => Ok(Some(self.build_packet(frame))),
            Ok(None) => Ok(None),
            Err(e) => Err(match e {
                wasodecs::Error::Dropped => Error::Dropped,
                other => Error::Platform(format!("{other:?}")),
            }),
        }
    }
}

impl VideoEncoderOutput for WasmVideoEncoderOutput {
    async fn packet(&mut self) -> Result<Option<EncodedVideoPacket>, Error> {
        let pkt = self.inner.next().await.map_err(|e| match e {
            wasodecs::Error::Dropped => Error::Dropped,
            other => Error::Platform(format!("{other:?}")),
        })?;

        Ok(pkt.map(|f| self.build_packet(f)))
    }

    fn decoder_config(&self) -> Option<&VideoDecoderConfig> {
        self.decoder_cfg.as_ref()
    }
}

pub fn create(
    config: VideoEncoderConfig,
) -> Result<(WasmVideoEncoderInput, WasmVideoEncoderOutput), Error> {
    let wc_cfg = to_wc_config(&config);
    let (enc, encoded) = wc_cfg
        .init()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;

    Ok((
        WasmVideoEncoderInput { inner: enc, config },
        WasmVideoEncoderOutput {
            inner: encoded,
            decoder_cfg: None,
        },
    ))
}
