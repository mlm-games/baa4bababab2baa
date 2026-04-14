use bytes::Bytes;

use crate::types::common::{SampleFormat, Timestamp};

#[derive(Debug, Clone)]
pub struct AudioCodecId(pub String);

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
            codec: AudioCodecId("audio/mp4a-latm".into()),
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
            codec: AudioCodecId("audio/mp4a-latm".into()),
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
