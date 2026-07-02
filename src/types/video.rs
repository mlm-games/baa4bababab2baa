use std::fmt;

use bytes::Bytes;

use crate::types::common::{Dimensions, PixelFormat, Timestamp};

/// A video codec identifier with type-safe variants and a raw escape hatch.
///
/// Use [`to_mime()`](VideoCodecId::to_mime) to get an Android MediaCodec MIME string,
/// and [`to_webcodecs_strings()`](VideoCodecId::to_webcodecs_strings) for WASM
/// WebCodecs full codec strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoCodecId {
    H264 { profile: Option<u32>, level: Option<u32> },
    Hevc,
    Vp8,
    Vp9,
    Av1,
    /// Raw MIME string for codecs not covered by variants above.
    Other(String),
}

impl VideoCodecId {
    /// Create from a MIME-type string (e.g. `"video/avc"`).
    pub fn from_mime(mime: &str) -> Self {
        match mime {
            "video/avc" | "video/h264" => VideoCodecId::H264 { profile: None, level: None },
            "video/hevc" | "video/h265" => VideoCodecId::Hevc,
            "video/vp8" => VideoCodecId::Vp8,
            "video/vp9" => VideoCodecId::Vp9,
            "video/av1" | "video/av01" => VideoCodecId::Av1,
            other => VideoCodecId::Other(other.to_string()),
        }
    }

    /// Return the canonical MIME string for this codec (Android MediaCodec format).
    pub fn to_mime(&self) -> &str {
        match self {
            VideoCodecId::H264 { .. } => "video/avc",
            VideoCodecId::Hevc => "video/hevc",
            VideoCodecId::Vp8 => "video/vp8",
            VideoCodecId::Vp9 => "video/vp9",
            VideoCodecId::Av1 => "video/av1",
            VideoCodecId::Other(s) => s.as_str(),
        }
    }

    /// Return WebCodecs full codec strings for this codec (WASM target).
    /// Multiple strings are returned for H.264/HEVC to try different profile/level combinations.
    pub fn to_webcodecs_strings(&self) -> Vec<&str> {
        match self {
            VideoCodecId::H264 { .. } => vec!["avc1.42001E", "avc1.4D001E", "avc1.64001E"],
            VideoCodecId::Hevc => vec!["hvc1.1.6.L93.B0", "hev1.1.6.L93.B0"],
            VideoCodecId::Av1 => vec!["av01.0.04M.08"],
            VideoCodecId::Vp9 => vec!["vp09.00.10.08"],
            VideoCodecId::Vp8 => vec!["vp8"],
            VideoCodecId::Other(s) => vec![s.as_str()],
        }
    }
}

impl fmt::Display for VideoCodecId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_mime())
    }
}

impl From<&str> for VideoCodecId {
    fn from(s: &str) -> Self {
        VideoCodecId::from_mime(s)
    }
}

#[derive(Debug, Clone)]
pub struct VideoEncoderConfig {
    pub codec: VideoCodecId,
    pub dimensions: Dimensions,
    pub bitrate: Option<u32>,
    pub framerate: Option<f64>,
    pub hardware_acceleration: Option<bool>,
    pub latency_optimized: Option<bool>,
    pub level: Option<u32>,
}

impl Default for VideoEncoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodecId::H264 { profile: None, level: None },
            dimensions: Dimensions::new(1920, 1080),
            bitrate: None,
            framerate: None,
            hardware_acceleration: None,
            latency_optimized: None,
            level: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VideoDecoderConfig {
    pub codec: VideoCodecId,
    pub resolution: Option<Dimensions>,
    pub description: Option<Bytes>,
    pub hardware_acceleration: Option<bool>,
}

impl Default for VideoDecoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodecId::H264 { profile: None, level: None },
            resolution: None,
            description: None,
            hardware_acceleration: None,
        }
    }
}

#[derive(Debug)]
pub struct EncodedVideoPacket {
    pub payload: Bytes,
    pub timestamp: Timestamp,
    pub keyframe: bool,
}

#[derive(Debug)]
pub struct VideoFrame {
    pub dimensions: Dimensions,
    pub format: PixelFormat,
    pub timestamp: Timestamp,
    pub planes: VideoPlanes,
}

#[derive(Debug)]
pub enum VideoPlanes {
    Cpu(Vec<u8>),
    Hardware,
}
