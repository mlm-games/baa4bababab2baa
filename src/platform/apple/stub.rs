//! Non-macOS stub for the Apple backend (currently iOS).
//!
//! `oxideav-videotoolbox 0.0.3` is `#![cfg(target_os = "macos")]`, so on iOS
//! there is no bridge to call into yet. Everything returns
//! [`Error::Unsupported`] / `Ok(false)` so downstream crates (`repadio`,
//! `Miniter`) fall back to software.
//!
//! Upgrade path (verified 2026-09): implement the iOS backend against
//! [`objc2-video-toolbox`] 0.3.2 (madsmtm/objc2 — docs built for
//! `aarch64-apple-ios`/`-macabi`, `tvos`, `visionos`; features
//! `VTCompressionSession`, `VTDecompressionSession`, `VTErrors` plus
//! `objc2-core-media`/`objc2-core-video`/`objc2-foundation`/`block2`).
//! VideoToolbox is the same C API on iOS (Apple documents it from iOS 6.0+),
//! so the [`vt`](super::vt) adapter logic ports directly: Annex-B in, I420
//! out, input-PTS FIFO for timestamping. Raw-FFI alternative:
//! `video-toolbox-sys` 0.2.0 (macOS + iOS). Swapping the stub bodies for
//! real constructors needs no public `Host` API change.
//!
//! [`objc2-video-toolbox`]: https://github.com/madsmtm/objc2

use crate::{
    error::Error,
    types::{AudioDecoderConfig, AudioEncoderConfig, VideoDecoderConfig, VideoEncoderConfig},
};

pub struct VideoToolboxHost;

impl VideoToolboxHost {
    pub fn new() -> Self {
        Self
    }

    pub fn create_video_encoder(
        &self,
        _config: VideoEncoderConfig,
    ) -> Result<(AppleVideoEncoderInput, AppleVideoEncoderOutput), Error> {
        Err(Error::Unsupported)
    }

    pub fn create_video_decoder(
        &self,
        _config: VideoDecoderConfig,
    ) -> Result<(AppleVideoDecoderInput, AppleVideoDecoderOutput), Error> {
        Err(Error::Unsupported)
    }

    pub fn create_audio_encoder(
        &self,
        _config: AudioEncoderConfig,
    ) -> Result<(AppleAudioEncoderInput, AppleAudioEncoderOutput), Error> {
        Err(Error::Unsupported)
    }

    pub fn create_audio_decoder(
        &self,
        _config: AudioDecoderConfig,
    ) -> Result<(AppleAudioDecoderInput, AppleAudioDecoderOutput), Error> {
        Err(Error::Unsupported)
    }

    pub async fn is_video_encoder_supported(
        &self,
        _config: &VideoEncoderConfig,
    ) -> Result<bool, Error> {
        Ok(false)
    }

    pub async fn is_video_decoder_supported(
        &self,
        _config: &VideoDecoderConfig,
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

pub struct AppleVideoEncoderInput;
pub struct AppleVideoEncoderOutput;
pub struct AppleVideoDecoderInput;
pub struct AppleVideoDecoderOutput;
pub struct AppleAudioEncoderInput {
    pub config: AudioEncoderConfig,
}
pub struct AppleAudioEncoderOutput;
pub struct AppleAudioDecoderInput;
pub struct AppleAudioDecoderOutput;

impl crate::traits::VideoEncoderInput for AppleVideoEncoderInput {
    fn encode(&mut self, _frame: crate::types::VideoFrame, _k: Option<bool>) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn queue_size(&self) -> u32 {
        0
    }
    fn config(&self) -> &VideoEncoderConfig {
        unreachable!("Apple VideoToolbox encoder not yet implemented on this target")
    }
}

impl crate::traits::VideoEncoderOutput for AppleVideoEncoderOutput {
    async fn packet(&mut self) -> Result<Option<crate::types::EncodedVideoPacket>, Error> {
        Err(Error::Unsupported)
    }
    fn decoder_config(&self) -> Option<&VideoDecoderConfig> {
        None
    }
}

impl crate::traits::VideoDecoderInput for AppleVideoDecoderInput {
    fn decode(&mut self, _p: crate::types::EncodedVideoPacket) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn queue_size(&self) -> u32 {
        0
    }
}

impl crate::traits::VideoDecoderOutput for AppleVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<crate::types::VideoFrame>, Error> {
        Err(Error::Unsupported)
    }
    fn try_frame(&mut self) -> Result<Option<crate::types::VideoFrame>, Error> {
        Err(Error::Unsupported)
    }
}

impl crate::traits::AudioEncoderInput for AppleAudioEncoderInput {
    fn encode(&mut self, _f: crate::types::AudioFrame) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn queue_size(&self) -> u32 {
        0
    }
    fn config(&self) -> &AudioEncoderConfig {
        &self.config
    }
}

impl crate::traits::AudioEncoderOutput for AppleAudioEncoderOutput {
    async fn packet(&mut self) -> Result<Option<crate::types::EncodedAudioPacket>, Error> {
        Err(Error::Unsupported)
    }
    fn decoder_config(&self) -> Option<&AudioDecoderConfig> {
        None
    }
}

impl crate::traits::AudioDecoderInput for AppleAudioDecoderInput {
    fn decode(&mut self, _p: crate::types::EncodedAudioPacket) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn queue_size(&self) -> u32 {
        0
    }
}

impl crate::traits::AudioDecoderOutput for AppleAudioDecoderOutput {
    async fn frame(&mut self) -> Result<Option<crate::types::AudioFrame>, Error> {
        Err(Error::Unsupported)
    }
}
