use crate::error::Error;
use crate::types::{
    AudioDecoderConfig, AudioEncoderConfig, VideoDecoderConfig, VideoEncoderConfig,
};

#[cfg(target_arch = "wasm32")]
use crate::platform::wasm::WebCodecsHost;

#[cfg(target_os = "android")]
use crate::platform::android::MediaCodecHost;

#[cfg(all(target_os = "linux", feature = "linux"))]
use crate::platform::linux::CrosCodecsHost;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostId {
    #[cfg(target_arch = "wasm32")]
    WebCodecs,
    #[cfg(target_os = "android")]
    MediaCodec,
    #[cfg(all(target_os = "linux", feature = "linux"))]
    CrosCodecs,
    #[cfg(not(any(
        target_arch = "wasm32",
        target_os = "android",
        all(target_os = "linux", feature = "linux")
    )))]
    NoBackend,
}

pub enum Host {
    #[cfg(target_arch = "wasm32")]
    WebCodecs(WebCodecsHost),
    #[cfg(target_os = "android")]
    MediaCodec(MediaCodecHost),
    #[cfg(all(target_os = "linux", feature = "linux"))]
    CrosCodecs(CrosCodecsHost),

    #[cfg(not(any(
        target_arch = "wasm32",
        target_os = "android",
        all(target_os = "linux", feature = "linux")
    )))]
    NoBackend,
}

pub fn default_host() -> Host {
    #[cfg(target_arch = "wasm32")]
    return Host::WebCodecs(WebCodecsHost::new());

    #[cfg(target_os = "android")]
    return Host::MediaCodec(MediaCodecHost::new());

    #[cfg(all(target_os = "linux", feature = "linux"))]
    return Host::CrosCodecs(CrosCodecsHost::new());

    #[cfg(not(any(
        target_arch = "wasm32",
        target_os = "android",
        all(target_os = "linux", feature = "linux")
    )))]
    return Host::NoBackend;
}

pub fn host_from_id(id: HostId) -> Result<Host, Error> {
    match id {
        #[cfg(target_arch = "wasm32")]
        HostId::WebCodecs => Ok(Host::WebCodecs(WebCodecsHost::new())),
        #[cfg(target_os = "android")]
        HostId::MediaCodec => Ok(Host::MediaCodec(MediaCodecHost::new())),
        #[cfg(all(target_os = "linux", feature = "linux"))]
        HostId::CrosCodecs => Ok(Host::CrosCodecs(CrosCodecsHost::new())),
        #[cfg(not(any(
            target_arch = "wasm32",
            target_os = "android",
            all(target_os = "linux", feature = "linux")
        )))]
        HostId::NoBackend => Err(Error::NoBackend),
    }
}

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
struct NoBackendVideoEncoderInput;
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
struct NoBackendVideoEncoderOutput;
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
struct NoBackendVideoDecoderInput;
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
struct NoBackendVideoDecoderOutput;
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
struct NoBackendAudioEncoderInput;
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
struct NoBackendAudioEncoderOutput;
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
struct NoBackendAudioDecoderInput;
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
struct NoBackendAudioDecoderOutput;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
impl crate::traits::VideoEncoderInput for NoBackendVideoEncoderInput {
    fn encode(
        &mut self,
        _frame: crate::types::VideoFrame,
        _keyframe: Option<bool>,
    ) -> Result<(), Error> {
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

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
impl crate::traits::VideoEncoderOutput for NoBackendVideoEncoderOutput {
    async fn packet(&mut self) -> Result<Option<crate::types::EncodedVideoPacket>, Error> {
        Err(Error::NoBackend)
    }

    fn decoder_config(&self) -> Option<&VideoDecoderConfig> {
        None
    }
}

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
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

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
impl crate::traits::VideoDecoderOutput for NoBackendVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<crate::types::VideoFrame>, Error> {
        Err(Error::NoBackend)
    }

    fn try_frame(&mut self) -> Result<Option<crate::types::VideoFrame>, Error> {
        Err(Error::NoBackend)
    }
}

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
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

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
impl crate::traits::AudioEncoderOutput for NoBackendAudioEncoderOutput {
    async fn packet(&mut self) -> Result<Option<crate::types::EncodedAudioPacket>, Error> {
        Err(Error::NoBackend)
    }

    fn decoder_config(&self) -> Option<&AudioDecoderConfig> {
        None
    }
}

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
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

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    all(target_os = "linux", feature = "linux")
)))]
impl crate::traits::AudioDecoderOutput for NoBackendAudioDecoderOutput {
    async fn frame(&mut self) -> Result<Option<crate::types::AudioFrame>, Error> {
        Err(Error::NoBackend)
    }
}

impl Host {
    pub fn create_video_encoder(
        &self,
        _config: VideoEncoderConfig,
    ) -> Result<
        (
            impl crate::traits::VideoEncoderInput,
            impl crate::traits::VideoEncoderOutput,
        ),
        Error,
    > {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.create_video_encoder(_config),
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.create_video_encoder(_config),
            #[cfg(all(target_os = "linux", feature = "linux"))]
            Host::CrosCodecs(h) => h.create_video_encoder(_config),
            #[cfg(not(any(
                target_arch = "wasm32",
                target_os = "android",
                all(target_os = "linux", feature = "linux")
            )))]
            Host::NoBackend => Err::<
                (NoBackendVideoEncoderInput, NoBackendVideoEncoderOutput),
                Error,
            >(Error::NoBackend),
        }
    }

    pub fn create_video_decoder(
        &self,
        _config: VideoDecoderConfig,
    ) -> Result<
        (
            impl crate::traits::VideoDecoderInput,
            impl crate::traits::VideoDecoderOutput,
        ),
        Error,
    > {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.create_video_decoder(_config),
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.create_video_decoder(_config),
            #[cfg(all(target_os = "linux", feature = "linux"))]
            Host::CrosCodecs(h) => h.create_video_decoder(_config),
            #[cfg(not(any(
                target_arch = "wasm32",
                target_os = "android",
                all(target_os = "linux", feature = "linux")
            )))]
            Host::NoBackend => Err::<
                (NoBackendVideoDecoderInput, NoBackendVideoDecoderOutput),
                Error,
            >(Error::NoBackend),
        }
    }

    pub fn create_audio_encoder(
        &self,
        _config: AudioEncoderConfig,
    ) -> Result<
        (
            impl crate::traits::AudioEncoderInput,
            impl crate::traits::AudioEncoderOutput,
        ),
        Error,
    > {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.create_audio_encoder(_config),
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.create_audio_encoder(_config),
            #[cfg(all(target_os = "linux", feature = "linux"))]
            Host::CrosCodecs(h) => h.create_audio_encoder(_config),
            #[cfg(not(any(
                target_arch = "wasm32",
                target_os = "android",
                all(target_os = "linux", feature = "linux")
            )))]
            Host::NoBackend => Err::<
                (NoBackendAudioEncoderInput, NoBackendAudioEncoderOutput),
                Error,
            >(Error::NoBackend),
        }
    }

    pub fn create_audio_decoder(
        &self,
        _config: AudioDecoderConfig,
    ) -> Result<
        (
            impl crate::traits::AudioDecoderInput,
            impl crate::traits::AudioDecoderOutput,
        ),
        Error,
    > {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.create_audio_decoder(_config),
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.create_audio_decoder(_config),
            #[cfg(all(target_os = "linux", feature = "linux"))]
            Host::CrosCodecs(h) => h.create_audio_decoder(_config),
            #[cfg(not(any(
                target_arch = "wasm32",
                target_os = "android",
                all(target_os = "linux", feature = "linux")
            )))]
            Host::NoBackend => Err::<
                (NoBackendAudioDecoderInput, NoBackendAudioDecoderOutput),
                Error,
            >(Error::NoBackend),
        }
    }

    pub async fn is_video_encoder_supported(
        &self,
        _config: &VideoEncoderConfig,
    ) -> Result<bool, Error> {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.is_video_encoder_supported(_config).await,
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.is_video_encoder_supported(_config).await,
            #[cfg(all(target_os = "linux", feature = "linux"))]
            Host::CrosCodecs(h) => h.is_video_encoder_supported(_config).await,
            #[cfg(not(any(
                target_arch = "wasm32",
                target_os = "android",
                all(target_os = "linux", feature = "linux")
            )))]
            Host::NoBackend => Ok(false),
        }
    }

    pub async fn is_video_decoder_supported(
        &self,
        _config: &VideoDecoderConfig,
    ) -> Result<bool, Error> {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.is_video_decoder_supported(_config).await,
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.is_video_decoder_supported(_config).await,
            #[cfg(all(target_os = "linux", feature = "linux"))]
            Host::CrosCodecs(h) => h.is_video_decoder_supported(_config).await,
            #[cfg(not(any(
                target_arch = "wasm32",
                target_os = "android",
                all(target_os = "linux", feature = "linux")
            )))]
            Host::NoBackend => Ok(false),
        }
    }

    pub async fn is_audio_encoder_supported(
        &self,
        _config: &AudioEncoderConfig,
    ) -> Result<bool, Error> {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.is_audio_encoder_supported(_config).await,
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.is_audio_encoder_supported(_config).await,
            #[cfg(all(target_os = "linux", feature = "linux"))]
            Host::CrosCodecs(h) => h.is_audio_encoder_supported(_config).await,
            #[cfg(not(any(
                target_arch = "wasm32",
                target_os = "android",
                all(target_os = "linux", feature = "linux")
            )))]
            Host::NoBackend => Ok(false),
        }
    }

    pub async fn is_audio_decoder_supported(
        &self,
        _config: &AudioDecoderConfig,
    ) -> Result<bool, Error> {
        match self {
            #[cfg(target_arch = "wasm32")]
            Host::WebCodecs(h) => h.is_audio_decoder_supported(_config).await,
            #[cfg(target_os = "android")]
            Host::MediaCodec(h) => h.is_audio_decoder_supported(_config).await,
            #[cfg(all(target_os = "linux", feature = "linux"))]
            Host::CrosCodecs(h) => h.is_audio_decoder_supported(_config).await,
            #[cfg(not(any(
                target_arch = "wasm32",
                target_os = "android",
                all(target_os = "linux", feature = "linux")
            )))]
            Host::NoBackend => Ok(false),
        }
    }
}
