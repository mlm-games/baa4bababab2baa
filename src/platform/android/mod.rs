mod audio_decoder;
mod audio_encoder;
mod video_decoder;
mod video_encoder;

pub use audio_decoder::{AndroidAudioDecoderInput, AndroidAudioDecoderOutput};
pub use audio_encoder::{AndroidAudioEncoderInput, AndroidAudioEncoderOutput};
pub use video_decoder::{AndroidVideoDecoderInput, AndroidVideoDecoderOutput};
pub use video_encoder::{AndroidVideoEncoderInput, AndroidVideoEncoderOutput};

use crate::{
    error::Error,
    types::{AudioDecoderConfig, AudioEncoderConfig, VideoDecoderConfig, VideoEncoderConfig},
};

pub struct MediaCodecHost;

impl MediaCodecHost {
    pub fn new() -> Self {
        Self
    }

    pub fn create_video_encoder(
        &self,
        config: VideoEncoderConfig,
    ) -> Result<(AndroidVideoEncoderInput, AndroidVideoEncoderOutput), Error> {
        video_encoder::create(config)
    }

    pub fn create_video_decoder(
        &self,
        config: VideoDecoderConfig,
    ) -> Result<(AndroidVideoDecoderInput, AndroidVideoDecoderOutput), Error> {
        video_decoder::create(config)
    }

    pub fn create_audio_encoder(
        &self,
        config: AudioEncoderConfig,
    ) -> Result<(AndroidAudioEncoderInput, AndroidAudioEncoderOutput), Error> {
        audio_encoder::create(config)
    }

    pub fn create_audio_decoder(
        &self,
        config: AudioDecoderConfig,
    ) -> Result<(AndroidAudioDecoderInput, AndroidAudioDecoderOutput), Error> {
        audio_decoder::create(config)
    }

    pub async fn is_video_encoder_supported(
        &self,
        config: &VideoEncoderConfig,
    ) -> Result<bool, Error> {
        Ok(mediacodec::MediaCodec::create_encoder(&config.codec.0).is_some())
    }

    pub async fn is_video_decoder_supported(
        &self,
        config: &VideoDecoderConfig,
    ) -> Result<bool, Error> {
        Ok(mediacodec::MediaCodec::create_decoder(&config.codec.0).is_some())
    }

    pub async fn is_audio_encoder_supported(
        &self,
        config: &AudioEncoderConfig,
    ) -> Result<bool, Error> {
        Ok(mediacodec::MediaCodec::create_encoder(&config.codec.0).is_some())
    }

    pub async fn is_audio_decoder_supported(
        &self,
        config: &AudioDecoderConfig,
    ) -> Result<bool, Error> {
        Ok(mediacodec::MediaCodec::create_decoder(&config.codec.0).is_some())
    }
}
