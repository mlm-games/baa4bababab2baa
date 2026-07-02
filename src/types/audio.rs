use std::fmt;

use bytes::Bytes;

use crate::types::common::{SampleFormat, Timestamp};

/// An audio codec identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioCodecId {
    Aac,
    Opus,
    Mp3,
    Vorbis,
    Flac,
    /// Raw MIME string for codecs not covered above.
    Other(String),
}

impl AudioCodecId {
    /// Create from a MIME-type string.
    pub fn from_mime(mime: &str) -> Self {
        match mime {
            "audio/mp4a-latm" | "audio/aac" => AudioCodecId::Aac,
            "audio/opus" => AudioCodecId::Opus,
            "audio/mpeg" => AudioCodecId::Mp3,
            "audio/vorbis" => AudioCodecId::Vorbis,
            "audio/flac" => AudioCodecId::Flac,
            other => AudioCodecId::Other(other.to_string()),
        }
    }

    /// Return the canonical MIME string for this codec.
    pub fn to_mime(&self) -> &str {
        match self {
            AudioCodecId::Aac => "audio/mp4a-latm",
            AudioCodecId::Opus => "audio/opus",
            AudioCodecId::Mp3 => "audio/mpeg",
            AudioCodecId::Vorbis => "audio/vorbis",
            AudioCodecId::Flac => "audio/flac",
            AudioCodecId::Other(s) => s.as_str(),
        }
    }
}

impl fmt::Display for AudioCodecId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_mime())
    }
}

impl From<&str> for AudioCodecId {
    fn from(s: &str) -> Self {
        AudioCodecId::from_mime(s)
    }
}

#[derive(Debug, Clone)]
pub struct AudioEncoderConfig {
    pub codec: AudioCodecId,
    pub sample_rate: u32,
    pub channels: u32,
    pub bitrate: Option<u32>,
}

impl Default for AudioEncoderConfig {
    fn default() -> Self {
        Self {
            codec: AudioCodecId::Aac,
            sample_rate: 48_000,
            channels: 2,
            bitrate: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioDecoderConfig {
    pub codec: AudioCodecId,
    pub channel_count: u32,
    pub sample_rate: u32,
    pub description: Option<Bytes>,
}

impl Default for AudioDecoderConfig {
    fn default() -> Self {
        Self {
            codec: AudioCodecId::Aac,
            channel_count: 2,
            sample_rate: 48_000,
            description: None,
        }
    }
}

#[derive(Debug)]
pub struct EncodedAudioPacket {
    pub payload: Bytes,
    pub timestamp: Timestamp,
    pub keyframe: bool,
}

#[derive(Debug)]
pub struct AudioFrame {
    pub timestamp: Timestamp,
    pub sample_rate: u32,
    pub channels: u32,
    pub format: SampleFormat,
    pub samples: Vec<u8>,
}
