use crate::{
    error::Error,
    types::{AudioDecoderConfig, AudioEncoderConfig, VideoDecoderConfig, VideoEncoderConfig},
};

mod audio;
mod video_decoder;
mod video_encoder;

pub use audio::{
    CrosAudioDecoderInput, CrosAudioDecoderOutput, CrosAudioEncoderInput, CrosAudioEncoderOutput,
};
pub use video_decoder::{CrosVideoDecoderInput, CrosVideoDecoderOutput};
pub use video_encoder::{CrosVideoEncoderInput, CrosVideoEncoderOutput};

pub struct CrosCodecsHost;

impl CrosCodecsHost {
    pub fn new() -> Self {
        Self
    }

    pub fn create_video_decoder(
        &self,
        config: VideoDecoderConfig,
    ) -> Result<(CrosVideoDecoderInput, CrosVideoDecoderOutput), Error> {
        video_decoder::create(config)
    }

    pub fn create_video_encoder(
        &self,
        config: VideoEncoderConfig,
    ) -> Result<(CrosVideoEncoderInput, CrosVideoEncoderOutput), Error> {
        video_encoder::create(config)
    }

    pub fn create_audio_encoder(
        &self,
        config: AudioEncoderConfig,
    ) -> Result<(CrosAudioEncoderInput, CrosAudioEncoderOutput), Error> {
        Ok((CrosAudioEncoderInput { config }, CrosAudioEncoderOutput))
    }

    pub fn create_audio_decoder(
        &self,
        _config: AudioDecoderConfig,
    ) -> Result<(CrosAudioDecoderInput, CrosAudioDecoderOutput), Error> {
        Ok((CrosAudioDecoderInput, CrosAudioDecoderOutput))
    }

    pub async fn is_video_decoder_supported(
        &self,
        config: &VideoDecoderConfig,
    ) -> Result<bool, Error> {
        Ok(matches!(
            config.codec.0.as_str(),
            "video/avc" | "video/h264" | "video/hevc" | "video/h265"
                | "video/vp8" | "video/vp9" | "video/av01" | "video/av1"
        ))
    }

    pub async fn is_video_encoder_supported(
        &self,
        config: &VideoEncoderConfig,
    ) -> Result<bool, Error> {
        Ok(matches!(
            config.codec.0.as_str(),
            "video/avc" | "video/h264"
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
