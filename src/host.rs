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

#[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
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
    #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
    CrosCodecs,
}

pub enum Host {
    #[cfg(target_arch = "wasm32")]
    WebCodecs(WebCodecsHost),
    #[cfg(target_os = "android")]
    MediaCodec(MediaCodecHost),
    #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
    CrosCodecs(CrosCodecsHost),

    #[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
    NoBackend,
}

pub fn default_host() -> Host {
    #[cfg(target_arch = "wasm32")]
    return Host::WebCodecs(WebCodecsHost::new());

    #[cfg(target_os = "android")]
    return Host::MediaCodec(MediaCodecHost::new());

    #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
    return Host::CrosCodecs(CrosCodecsHost::new());

    #[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
    return Host::NoBackend;
}

pub fn host_from_id(id: HostId) -> Result<Host, Error> {
    match id {
        #[cfg(target_arch = "wasm32")]
        HostId::WebCodecs => Ok(Host::WebCodecs(WebCodecsHost::new())),
        #[cfg(target_os = "android")]
        HostId::MediaCodec => Ok(Host::MediaCodec(MediaCodecHost::new())),
        #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
        HostId::CrosCodecs => Ok(Host::CrosCodecs(CrosCodecsHost::new())),
    }
}

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
struct NoBackendVideoEncoderInput;

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
struct NoBackendVideoEncoderOutput;

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
struct NoBackendVideoDecoderInput;

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
struct NoBackendVideoDecoderOutput;

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
struct NoBackendAudioEncoderInput;

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
struct NoBackendAudioEncoderOutput;

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
struct NoBackendAudioDecoderInput;

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
struct NoBackendAudioDecoderOutput;

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
impl crate::traits::VideoEncoderInput for NoBackendVideoEncoderInput {
    fn encode(&mut self, _frame: crate::types::VideoFrame, _keyframe: Option<bool>) -> Result<(), Error> {
        Err(Error::NoBackend)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::NoBackend)
    }

    fn queue_size(&self) -> u32 {
        0
    }

    fn config(&self) -> &VideoEncoderConfig {
        unreachable!()
    }
}

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
impl crate::traits::VideoEncoderOutput for NoBackendVideoEncoderOutput {
    async fn packet(&mut self) -> Result<Option<crate::types::EncodedVideoPacket>, Error> {
        Err(Error::NoBackend)
    }

    fn decoder_config(&self) -> Option<&VideoDecoderConfig> {
        None
    }
}

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
impl crate::traits::VideoDecoderInput for NoBackendVideoDecoderInput {
    fn decode(&mut self, _packet: crate::types::EncodedVideoPacket) -> Result<(), Error> {
        Err(Error::NoBackend)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::NoBackend)
    }

    fn queue_size(&self) -> u32 {
        0
    }
}

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
impl crate::traits::VideoDecoderOutput for NoBackendVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<crate::types::VideoFrame>, Error> {
        Err(Error::NoBackend)
    }
}

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
impl crate::traits::AudioEncoderInput for NoBackendAudioEncoderInput {
    fn encode(&mut self, _frame: crate::types::AudioFrame) -> Result<(), Error> {
        Err(Error::NoBackend)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::NoBackend)
    }

    fn queue_size(&self) -> u32 {
        0
    }

    fn config(&self) -> &AudioEncoderConfig {
        unreachable!()
    }
}

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
impl crate::traits::AudioEncoderOutput for NoBackendAudioEncoderOutput {
    async fn packet(&mut self) -> Result<Option<crate::types::EncodedAudioPacket>, Error> {
        Err(Error::NoBackend)
    }

    fn decoder_config(&self) -> Option<&AudioDecoderConfig> {
        None
    }
}

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
impl crate::traits::AudioDecoderInput for NoBackendAudioDecoderInput {
    fn decode(&mut self, _packet: crate::types::EncodedAudioPacket) -> Result<(), Error> {
        Err(Error::NoBackend)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        Err(Error::NoBackend)
    }

    fn queue_size(&self) -> u32 {
        0
    }
}

#[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
impl crate::traits::AudioDecoderOutput for NoBackendAudioDecoderOutput {
    async fn frame(&mut self) -> Result<Option<crate::types::AudioFrame>, Error> {
        Err(Error::NoBackend)
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
            #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
            Host::CrosCodecs(h) => h.create_video_encoder(config),
            #[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
            Host::NoBackend => Err::<(NoBackendVideoEncoderInput, NoBackendVideoEncoderOutput), Error>(Error::NoBackend),
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
            #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
            Host::CrosCodecs(h) => h.create_video_decoder(config),
            #[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
            Host::NoBackend => Err::<(NoBackendVideoDecoderInput, NoBackendVideoDecoderOutput), Error>(Error::NoBackend),
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
            #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
            Host::CrosCodecs(h) => h.create_audio_encoder(config),
            #[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
            Host::NoBackend => Err::<(NoBackendAudioEncoderInput, NoBackendAudioEncoderOutput), Error>(Error::NoBackend),
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
            #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
            Host::CrosCodecs(h) => h.create_audio_decoder(config),
            #[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
            Host::NoBackend => Err::<(NoBackendAudioDecoderInput, NoBackendAudioDecoderOutput), Error>(Error::NoBackend),
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
            #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
            Host::CrosCodecs(h) => h.is_video_encoder_supported(config).await,
            #[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
            Host::NoBackend => Ok(false),
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
            #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
            Host::CrosCodecs(h) => h.is_video_decoder_supported(config).await,
            #[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
            Host::NoBackend => Ok(false),
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
            #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
            Host::CrosCodecs(h) => h.is_audio_encoder_supported(config).await,
            #[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
            Host::NoBackend => Ok(false),
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
            #[cfg(all(target_os = "linux", not(target_os = "android"), feature = "linux"))]
            Host::CrosCodecs(h) => h.is_audio_decoder_supported(config).await,
            #[cfg(all(target_os = "linux", not(target_os = "android"), not(feature = "linux")))]
            Host::NoBackend => Ok(false),
        }
    }
}
