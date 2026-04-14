use crate::error::Error;
use crate::types::{AudioDecoderConfig, AudioEncoderConfig, VideoDecoderConfig, VideoEncoderConfig};

#[cfg(target_arch = "wasm32")]
use crate::platform::wasm::{
    WebCodecsHost, WasmAudioDecoderInput, WasmAudioDecoderOutput,
    WasmAudioEncoderInput, WasmAudioEncoderOutput,
    WasmVideoDecoderInput, WasmVideoDecoderOutput,
    WasmVideoEncoderInput, WasmVideoEncoderOutput,
};

#[cfg(target_os = "android")]
use crate::platform::android::{
    MediaCodecHost, AndroidAudioDecoderInput, AndroidAudioDecoderOutput,
    AndroidAudioEncoderInput, AndroidAudioEncoderOutput,
    AndroidVideoDecoderInput, AndroidVideoDecoderOutput,
    AndroidVideoEncoderInput, AndroidVideoEncoderOutput,
};

#[cfg(all(target_os = "linux", not(target_os = "android")))]
use crate::platform::linux::{
    CrosCodecsHost, CrosAudioDecoderInput, CrosAudioDecoderOutput,
    CrosAudioEncoderInput, CrosAudioEncoderOutput,
    CrosVideoDecoderInput, CrosVideoDecoderOutput,
    CrosVideoEncoderInput, CrosVideoEncoderOutput,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostId {
    #[cfg(target_arch = "wasm32")]
    WebCodecs,
    #[cfg(target_os = "android")]
    MediaCodec,
    #[cfg(all(target_os = "linux", not(target_os = "android")))]
    CrosCodecs,
}

pub enum Host {
    #[cfg(target_arch = "wasm32")]
    WebCodecs(WebCodecsHost),
    #[cfg(target_os = "android")]
    MediaCodec(MediaCodecHost),
    #[cfg(all(target_os = "linux", not(target_os = "android")))]
    CrosCodecs(CrosCodecsHost),
}

pub fn default_host() -> Host {
    #[cfg(target_arch = "wasm32")]
    return Host::WebCodecs(WebCodecsHost::new());

    #[cfg(target_os = "android")]
    return Host::MediaCodec(MediaCodecHost::new());

    #[cfg(all(target_os = "linux", not(target_os = "android")))]
    return Host::CrosCodecs(CrosCodecsHost::new());
}

pub fn host_from_id(id: HostId) -> Result<Host, Error> {
    match id {
        #[cfg(target_arch = "wasm32")]
        HostId::WebCodecs => Ok(Host::WebCodecs(WebCodecsHost::new())),
        #[cfg(target_os = "android")]
        HostId::MediaCodec => Ok(Host::MediaCodec(MediaCodecHost::new())),
        #[cfg(all(target_os = "linux", not(target_os = "android")))]
        HostId::CrosCodecs => Ok(Host::CrosCodecs(CrosCodecsHost::new())),
    }
}

impl Host {
    pub fn create_video_encoder(
        &self,
        config: VideoEncoderConfig,
    ) -> Result<(impl crate::traits::VideoEncoderInput, impl crate::traits::VideoEncoderOutput), Error>
    {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.create_video_encoder(config),
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.create_video_encoder(config),
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            Host::CrosCodecs(h) => h.create_video_encoder(config),
        }
    }

    pub fn create_video_decoder(
        &self,
        config: VideoDecoderConfig,
    ) -> Result<(impl crate::traits::VideoDecoderInput, impl crate::traits::VideoDecoderOutput), Error>
    {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.create_video_decoder(config),
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.create_video_decoder(config),
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            Host::CrosCodecs(h) => h.create_video_decoder(config),
        }
    }

    pub fn create_audio_encoder(
        &self,
        config: AudioEncoderConfig,
    ) -> Result<(impl crate::traits::AudioEncoderInput, impl crate::traits::AudioEncoderOutput), Error>
    {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.create_audio_encoder(config),
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.create_audio_encoder(config),
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            Host::CrosCodecs(h) => h.create_audio_encoder(config),
        }
    }

    pub fn create_audio_decoder(
        &self,
        config: AudioDecoderConfig,
    ) -> Result<(impl crate::traits::AudioDecoderInput, impl crate::traits::AudioDecoderOutput), Error>
    {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.create_audio_decoder(config),
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.create_audio_decoder(config),
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            Host::CrosCodecs(h) => h.create_audio_decoder(config),
        }
    }

    pub async fn is_video_encoder_supported(
        &self,
        config: &VideoEncoderConfig,
    ) -> Result<bool, Error> {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.is_video_encoder_supported(config).await,
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.is_video_encoder_supported(config).await,
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            Host::CrosCodecs(h) => h.is_video_encoder_supported(config).await,
        }
    }

    pub async fn is_video_decoder_supported(
        &self,
        config: &VideoDecoderConfig,
    ) -> Result<bool, Error> {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.is_video_decoder_supported(config).await,
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.is_video_decoder_supported(config).await,
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            Host::CrosCodecs(h) => h.is_video_decoder_supported(config).await,
        }
    }

    pub async fn is_audio_encoder_supported(
        &self,
        config: &AudioEncoderConfig,
    ) -> Result<bool, Error> {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.is_audio_encoder_supported(config).await,
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.is_audio_encoder_supported(config).await,
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            Host::CrosCodecs(h) => h.is_audio_encoder_supported(config).await,
        }
    }

    pub async fn is_audio_decoder_supported(
        &self,
        config: &AudioDecoderConfig,
    ) -> Result<bool, Error> {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.is_audio_decoder_supported(config).await,
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.is_audio_decoder_supported(config).await,
            #[cfg(all(target_os = "linux", not(target_os = "android")))]
            Host::CrosCodecs(h) => h.is_audio_decoder_supported(config).await,
        }
    }
}