use bytes::Bytes;

use crate::types::common::{Dimensions, PixelFormat, Timestamp};

#[derive(Debug, Clone)]
pub struct VideoCodecId(pub String);

#[derive(Debug, Clone)]
pub struct VideoEncoderConfig {
    pub codec: VideoCodecId,
    pub dimensions: Dimensions,
    pub bitrate: Option<u32>,
    pub framerate: Option<f64>,
    pub hardware_acceleration: Option<bool>,
    pub latency_optimized: Option<bool>,
}

impl Default for VideoEncoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodecId("video/avc".into()),
            dimensions: Dimensions::new(1920, 1080),
            bitrate: None,
            framerate: None,
            hardware_acceleration: None,
            latency_optimized: None,
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
            codec: VideoCodecId("video/avc".into()),
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
