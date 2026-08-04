pub mod error;
pub mod host;
pub mod platform;
pub mod traits;
pub mod types;

pub use error::Error;
pub use host::{Host, HostId, default_host, host_from_id};
pub use traits::{
    AudioDecoderInput, AudioDecoderOutput, AudioEncoderInput, AudioEncoderOutput,
    VideoDecoderInput, VideoDecoderOutput, VideoEncoderInput, VideoEncoderOutput,
};
pub use types::{
    AudioCodecId, AudioDecoderConfig, AudioEncoderConfig, AudioFrame, AvcBitstreamFormat,
    Dimensions, EncodedAudioPacket, EncodedVideoPacket, HardwareBuffer, PixelFormat, SampleFormat,
    Timestamp, VideoCodecId, VideoColorSpace, VideoDecoderConfig, VideoEncoderConfig, VideoFrame,
    VideoOutputMode, VideoPlanes,
};
#[cfg(all(target_os = "linux", feature = "linux"))]
pub use types::{DmaBufFrame, DmaBufPlane};
