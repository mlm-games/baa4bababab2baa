use thiserror::Error;

#[cfg(all(target_os = "linux", feature = "linux"))]
use nuxodecs::video_frame::FrameMapError;

#[derive(Debug, Clone, Error)]
pub enum Error {
    #[error("dropped: sender or receiver was released")]
    Dropped,
    #[error("unsupported codec or config on this platform")]
    Unsupported,
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("platform error: {0}")]
    Platform(String),
    #[error("no backend available for this platform")]
    NoBackend,
    #[error(transparent)]
    Failure(#[from] MediaFailure),
}

#[cfg(all(target_os = "linux", feature = "linux"))]
impl From<FrameMapError> for Error {
    fn from(e: FrameMapError) -> Self {
        match e {
            FrameMapError::UnsupportedModifier(m) | FrameMapError::UnsupportedTiling(m) => {
                MediaFailure::new(
                    MediaFailureCode::UnsupportedOutputFormat,
                    format!("unsupported DRM layout {m:#x} for CPU mapping"),
                )
                .backend("linux")
                .into()
            }
            FrameMapError::MapFailed(msg) => {
                MediaFailure::new(MediaFailureCode::FrameTransfer, msg)
                    .backend("linux")
                    .into()
            }
        }
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Platform(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaFailureCode {
    NoVideoTrack,
    UnsupportedCodec,
    UnsupportedProfile,
    DecoderUnavailable,
    DecoderInitialization,
    CodecConfiguration,
    PacketSubmission,
    PacketDecode,
    UnsupportedOutputFormat,
    FrameTransfer,
    NoDecodableFrame,
    Timeout,
    InvalidContainer,
    BackendDisconnected,
}

impl std::fmt::Display for MediaFailureCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            MediaFailureCode::NoVideoTrack => "no-video-track",
            MediaFailureCode::UnsupportedCodec => "unsupported-codec",
            MediaFailureCode::UnsupportedProfile => "unsupported-profile",
            MediaFailureCode::DecoderUnavailable => "decoder-unavailable",
            MediaFailureCode::DecoderInitialization => "decoder-initialization",
            MediaFailureCode::CodecConfiguration => "codec-configuration",
            MediaFailureCode::PacketSubmission => "packet-submission",
            MediaFailureCode::PacketDecode => "packet-decode",
            MediaFailureCode::UnsupportedOutputFormat => "unsupported-output-format",
            MediaFailureCode::FrameTransfer => "frame-transfer",
            MediaFailureCode::NoDecodableFrame => "no-decodable-frame",
            MediaFailureCode::Timeout => "timeout",
            MediaFailureCode::InvalidContainer => "invalid-container",
            MediaFailureCode::BackendDisconnected => "backend-disconnected",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFailure {
    pub code: MediaFailureCode,
    pub message: String,
    pub codec: Option<String>,
    pub codec_tag: Option<String>,
    pub profile: Option<String>,
    pub bit_depth: Option<u8>,
    pub backend: Option<String>,
    pub track_id: Option<u32>,
    pub timestamp_us: Option<i64>,
}

impl MediaFailure {
    pub fn new(code: MediaFailureCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            codec: None,
            codec_tag: None,
            profile: None,
            bit_depth: None,
            backend: None,
            track_id: None,
            timestamp_us: None,
        }
    }

    pub fn backend(mut self, backend: impl Into<String>) -> Self {
        self.backend = Some(backend.into());
        self
    }

    pub fn codec(mut self, codec: impl Into<String>) -> Self {
        self.codec = Some(codec.into());
        self
    }
}

impl std::fmt::Display for MediaFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for MediaFailure {}
