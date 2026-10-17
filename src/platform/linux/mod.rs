use std::sync::OnceLock;

use nuxodecs::libva;

use crate::{
    error::Error,
    types::{AudioDecoderConfig, AudioEncoderConfig, VideoDecoderConfig, VideoEncoderConfig},
};

/// The VA-API driver and the video codecs it decodes: the process's capability answer for the
/// UI, probed once. Opening a display is the only reliable check — presence of a DRM node does
/// not mean a working VA-API driver.
pub fn probe() -> Result<(String, Vec<String>), String> {
    static PROBE: OnceLock<Result<(String, Vec<String>), String>> = OnceLock::new();
    PROBE
        .get_or_init(|| {
            let display = libva::Display::open().ok_or("no VA-API display")?;
            let vendor = display
                .query_vendor_string()
                .map_err(|e| format!("VA-API: {e}"))?;
            let profiles = display
                .query_config_profiles()
                .map_err(|e| format!("VA-API: {e}"))?;
            let has =
                |accepted: &[libva::VAProfile::Type]| profiles.iter().any(|p| accepted.contains(p));
            let mut codecs = Vec::new();
            if has(&[
                libva::VAProfile::VAProfileH264Baseline,
                libva::VAProfile::VAProfileH264ConstrainedBaseline,
                libva::VAProfile::VAProfileH264Main,
                libva::VAProfile::VAProfileH264High,
            ]) {
                codecs.push("H.264".to_string());
            }
            if has(&[
                libva::VAProfile::VAProfileHEVCMain,
                libva::VAProfile::VAProfileHEVCMain10,
                libva::VAProfile::VAProfileHEVCMain12,
                libva::VAProfile::VAProfileHEVCMain422_10,
                libva::VAProfile::VAProfileHEVCMain422_12,
                libva::VAProfile::VAProfileHEVCMain444,
                libva::VAProfile::VAProfileHEVCMain444_10,
                libva::VAProfile::VAProfileHEVCMain444_12,
            ]) {
                codecs.push("HEVC".to_string());
            }
            if has(&[libva::VAProfile::VAProfileVP8Version0_3]) {
                codecs.push("VP8".to_string());
            }
            if has(&[
                libva::VAProfile::VAProfileVP9Profile0,
                libva::VAProfile::VAProfileVP9Profile1,
                libva::VAProfile::VAProfileVP9Profile2,
                libva::VAProfile::VAProfileVP9Profile3,
            ]) {
                codecs.push("VP9".to_string());
            }
            if has(&[
                libva::VAProfile::VAProfileAV1Profile0,
                libva::VAProfile::VAProfileAV1Profile1,
            ]) {
                codecs.push("AV1".to_string());
            }
            Ok((format!("VA-API: {vendor}"), codecs))
        })
        .clone()
}

mod audio;
pub(crate) mod video_decoder;
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
            config.codec,
            crate::types::VideoCodecId::H264 { .. }
                | crate::types::VideoCodecId::Hevc
                | crate::types::VideoCodecId::Vp8
                | crate::types::VideoCodecId::Vp9
                | crate::types::VideoCodecId::Av1
        ))
    }

    pub async fn is_video_encoder_supported(
        &self,
        config: &VideoEncoderConfig,
    ) -> Result<bool, Error> {
        Ok(matches!(
            config.codec,
            crate::types::VideoCodecId::H264 { .. }
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
