use std::future::Future;

use crate::error::Error;
use crate::types::{EncodedVideoPacket, VideoDecoderConfig, VideoEncoderConfig, VideoFrame};

#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSend for T {}

#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: ?Sized + Send> MaybeSend for T {}

pub trait VideoEncoderInput: MaybeSend {
    fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error>;
    fn flush(&mut self) -> impl Future<Output = Result<(), Error>> + MaybeSend;
    fn queue_size(&self) -> u32;
    fn config(&self) -> &VideoEncoderConfig;
}

pub trait VideoEncoderOutput: MaybeSend {
    fn packet(
        &mut self,
    ) -> impl Future<Output = Result<Option<EncodedVideoPacket>, Error>> + MaybeSend;
    fn decoder_config(&self) -> Option<&VideoDecoderConfig>;
}

pub trait VideoDecoderInput: MaybeSend {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error>;
    fn flush(&mut self) -> impl Future<Output = Result<(), Error>> + MaybeSend;
    fn queue_size(&self) -> u32;
}

pub trait VideoDecoderOutput: MaybeSend {
    fn frame(&mut self) -> impl Future<Output = Result<Option<VideoFrame>, Error>> + MaybeSend;
}
