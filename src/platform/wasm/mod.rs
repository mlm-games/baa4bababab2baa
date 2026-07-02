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

/// Map MIME-type codec identifiers to WebCodecs full codec strings.
/// WebCodecs requires specific codec strings (e.g. "avc1.42001E") not bare MIMEs.
pub(super) fn mime_to_codec_strings(mime: &str) -> Vec<&str> {
    match mime {
        "video/avc" => vec!["avc1.42001E", "avc1.4D001E", "avc1.64001E"],
        "video/hevc" => vec!["hvc1.1.6.L93.B0", "hev1.1.6.L93.B0"],
        "video/av01" => vec!["av01.0.04M.08"],
        "video/vp9" => vec!["vp09.00.10.08"],
        "video/vp8" => vec!["vp8"],
        _ => vec![mime],
    }
}

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
        video_encoder::to_wc_config(config)
            .is_supported()
            .await
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }

    pub async fn is_video_decoder_supported(
        &self,
        config: &VideoDecoderConfig,
    ) -> Result<bool, Error> {
        video_decoder::to_wc_config(config)
            .is_supported()
            .await
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }

    pub async fn is_audio_encoder_supported(
        &self,
        config: &AudioEncoderConfig,
    ) -> Result<bool, Error> {
        audio_encoder::to_wc_config(config)
            .is_supported()
            .await
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }

    pub async fn is_audio_decoder_supported(
        &self,
        config: &AudioDecoderConfig,
    ) -> Result<bool, Error> {
        audio_decoder::to_wc_config(config)
            .is_supported()
            .await
            .map_err(|e| Error::Platform(format!("{e:?}")))
    }
}
