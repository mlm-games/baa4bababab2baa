mod audio_decoder;
mod audio_encoder;
mod video_decoder;
mod video_encoder;

use crate::{
    error::Error,
    types::{AudioDecoderConfig, AudioEncoderConfig, VideoDecoderConfig, VideoEncoderConfig},
};

pub use audio_decoder::{WasmAudioDecoderInput, WasmAudioDecoderOutput};
pub use audio_encoder::{WasmAudioEncoderInput, WasmAudioEncoderOutput};
pub use video_decoder::{WasmVideoDecoderInput, WasmVideoDecoderOutput};
pub use video_encoder::{WasmVideoEncoderInput, WasmVideoEncoderOutput};

pub struct WebCodecsHost;

impl WebCodecsHost {
    pub fn new() -> Self {
        Self
    }

    pub fn create_video_encoder(
        &self,
        config: VideoEncoderConfig,
    ) -> Result<(WasmVideoEncoderInput, WasmVideoEncoderOutput), Error> {
        video_encoder::create(config)
    }

    pub fn create_video_decoder(
        &self,
        config: VideoDecoderConfig,
    ) -> Result<(WasmVideoDecoderInput, WasmVideoDecoderOutput), Error> {
        video_decoder::create(config)
    }

    pub fn create_audio_encoder(
        &self,
        config: AudioEncoderConfig,
    ) -> Result<(WasmAudioEncoderInput, WasmAudioEncoderOutput), Error> {
        audio_encoder::create(config)
    }

    pub fn create_audio_decoder(
        &self,
        config: AudioDecoderConfig,
    ) -> Result<(WasmAudioDecoderInput, WasmAudioDecoderOutput), Error> {
        audio_decoder::create(config)
    }

    pub async fn is_video_encoder_supported(
        &self,
        config: &VideoEncoderConfig,
    ) -> Result<bool, Error> {
        use web_codecs::VideoEncoderConfig as WcCfg;
        let wc = video_encoder::to_wc_config_pub(config);
        wc.is_supported()
            .await
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }

    pub async fn is_video_decoder_supported(
        &self,
        config: &VideoDecoderConfig,
    ) -> Result<bool, Error> {
        let wc = video_decoder::to_wc_config_pub(config);
        wc.is_supported()
            .await
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }

    pub async fn is_audio_encoder_supported(
        &self,
        config: &AudioEncoderConfig,
    ) -> Result<bool, Error> {
        let wc = audio_encoder::to_wc_config_pub(config);
        wc.is_supported()
            .await
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }

    pub async fn is_audio_decoder_supported(
        &self,
        config: &AudioDecoderConfig,
    ) -> Result<bool, Error> {
        let wc = audio_decoder::to_wc_config_pub(config);
        wc.is_supported()
            .await
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }
}