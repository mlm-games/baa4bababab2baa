pub mod error;
pub mod host;
pub mod traits;
pub mod types;

pub use error::Error;
pub use host::{Host, HostId, default_host, host_from_id};
pub use traits::{
    AudioDecoderInput, AudioDecoderOutput, AudioEncoderInput, AudioEncoderOutput,
    VideoDecoderInput, VideoDecoderOutput, VideoEncoderInput, VideoEncoderOutput,
};
pub use types::{
    AudioCodecId, AudioDecoderConfig, AudioEncoderConfig, AudioFrame, Dimensions,
    EncodedAudioPacket, EncodedVideoPacket, PixelFormat, SampleFormat, Timestamp, VideoCodecId,
    VideoDecoderConfig, VideoEncoderConfig, VideoFrame, VideoPlanes,
};

#[cfg(target_arch = "wasm32")]
pub mod platform {
    pub mod wasm;
    pub use wasm::video;
}

#[cfg(target_os = "android")]
pub mod platform {
    pub mod android;
    pub use android::video;
}

#[cfg(all(target_os = "linux", feature = "linux"))]
pub mod platform {
    pub mod linux;
    pub use linux::video;
}