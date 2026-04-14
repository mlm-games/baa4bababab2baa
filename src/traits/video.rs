use std::future::Future;

use crate::error::Error;
use crate::types::{EncodedVideoPacket, VideoDecoderConfig, VideoEncoderConfig, VideoFrame};

pub trait VideoEncoderInput: Send {
    fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error>;
    fn flush(&mut self) -> impl Future<Output = Result<(), Error>> + Send;
    fn queue_size(&self) -> u32;
    fn config(&self) -> &VideoEncoderConfig;
}

pub trait VideoEncoderOutput: Send {
    fn packet(&mut self) -> impl Future<Output = Result<Option<EncodedVideoPacket>, Error>> + Send;
    fn decoder_config(&self) -> Option<&VideoDecoderConfig>;
}

pub trait VideoDecoderInput: Send {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error>;
    fn flush(&mut self) -> impl Future<Output = Result<(), Error>> + Send;
    fn queue_size(&self) -> u32;
}

pub trait VideoDecoderOutput: Send {
    fn frame(&mut self) -> impl Future<Output = Result<Option<VideoFrame>, Error>> + Send;
}
