use web_codecs::{
    Dimensions as WcDimensions, EncodedFrame, VideoEncodeOptions, VideoEncoded, VideoEncoder,
    VideoEncoderConfig as WcVideoEncoderConfig,
};

use crate::{
    error::Error,
    traits::{VideoEncoderInput, VideoEncoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, VideoDecoderConfig, VideoEncoderConfig, VideoFrame,
        VideoPlanes,
    },
};

fn to_wc_config(cfg: &VideoEncoderConfig) -> WcVideoEncoderConfig {
    let mut wc = WcVideoEncoderConfig::new(
        &cfg.codec.0,
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

    wc
}

pub struct WasmVideoEncoderInput {
    inner: VideoEncoder,
    config: VideoEncoderConfig,
}

impl VideoEncoderInput for WasmVideoEncoderInput {
    fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error> {
        let wc_frame: web_codecs::VideoFrame = match frame.planes {
            VideoPlanes::Hardware => {
                return Err(Error::InvalidConfig(
                    "Cannot re-encode a hardware VideoFrame on WASM".into(),
                ));
            }
            VideoPlanes::Cpu(data) => build_wasm_frame(&data, &frame.dimensions, frame.timestamp)?,
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

fn build_wasm_frame(
    data: &[u8],
    dims: &Dimensions,
    timestamp: crate::types::Timestamp,
) -> Result<web_codecs::VideoFrame, Error> {
    use js_sys::Uint8Array;
    use wasm_bindgen::JsValue;
    use web_sys::{VideoFrame, VideoFrameBufferInit, VideoPixelFormat};

    let array = Uint8Array::from(data);

    let init = VideoFrameBufferInit::new(
        dims.height,
        VideoPixelFormat::Rgba,
        timestamp.as_micros() as f64,
        dims.width,
    );

    VideoFrame::new_with_u8_array_and_video_frame_buffer_init(&array, &init)
        .map(web_codecs::VideoFrame::from)
        .map_err(|e| Error::Platform(format!("{e:?}")))
}

pub struct WasmVideoEncoderOutput {
    inner: VideoEncoded,
    decoder_cfg: Option<VideoDecoderConfig>,
}

impl VideoEncoderOutput for WasmVideoEncoderOutput {
    async fn packet(&mut self) -> Result<Option<EncodedVideoPacket>, Error> {
        let pkt = self.inner.frame().await.map_err(|e| match e {
            web_codecs::Error::Dropped => Error::Dropped,
            other => Error::Platform(format!("{other:?}")),
        })?;

        if let Some(wc_cfg) = self.inner.config() {
            self.decoder_cfg = Some(VideoDecoderConfig {
                codec: crate::types::VideoCodecId(wc_cfg.codec.clone()),
                resolution: wc_cfg
                    .resolution
                    .map(|d| Dimensions::new(d.width, d.height)),
                description: wc_cfg.description.clone(),
                hardware_acceleration: wc_cfg.hardware_acceleration,
            });
        }

        Ok(pkt.map(|f: EncodedFrame| EncodedVideoPacket {
            payload: f.payload,
            timestamp: f.timestamp,
            keyframe: f.keyframe,
        }))
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

pub(super) fn to_wc_config_pub(config: &VideoEncoderConfig) -> WcVideoEncoderConfig {
    to_wc_config(config)
}
