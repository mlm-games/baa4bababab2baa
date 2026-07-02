use std::future::Future;
use std::pin::Pin;

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

/// A convenience supertrait for `Future + MaybeSend` usable as a trait object bound.
pub trait FutureMaybeSend: Future + MaybeSend {}
impl<T: Future + MaybeSend + ?Sized> FutureMaybeSend for T {}

/// Number of items submitted via `encode()`/`decode()` that have not yet been
/// accepted by the underlying codec backend.
///
/// A value of 0 means the backend has accepted all submitted work and any new
/// item should be processed without delay. Callers may use this for
/// backpressure: if the value exceeds some threshold, wait for output before
/// submitting more work.
///
/// **Backend semantics:**
/// - **WASM**: delegates to the browser's `VideoEncoder.queueSize`.
/// - **Android**: decremented when the packet/frame is copied into a MediaCodec
///   input buffer.
/// - **Linux decoder**: decremented when the packet is received off the channel,
///   before it has been decoded.
/// - **Linux encoder**: decremented when moved into the internal pending queue,
///   measuring channel depth, not VAAPI encoder backlog.
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

    /// Returns the number of packets submitted via `decode()` that the backend
    /// has not yet fully processed. See [`VideoEncoderInput::queue_size`] for
    /// per-backend semantics.
    fn queue_size(&self) -> u32;
}

pub trait VideoDecoderOutput: MaybeSend {
    fn frame(&mut self) -> impl Future<Output = Result<Option<VideoFrame>, Error>> + MaybeSend;
    fn try_frame(&mut self) -> Result<Option<VideoFrame>, Error>;
}

pub trait VideoEncoderInputBoxed: MaybeSend {
    fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error>;
    fn flush(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<(), Error>> + '_>>;
    fn queue_size(&self) -> u32;
    fn config(&self) -> &VideoEncoderConfig;
}

impl<T: VideoEncoderInput + ?Sized> VideoEncoderInputBoxed for T {
    fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error> {
        VideoEncoderInput::encode(self, frame, keyframe)
    }
    fn flush(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<(), Error>> + '_>> {
        Box::pin(VideoEncoderInput::flush(self))
    }
    fn queue_size(&self) -> u32 {
        VideoEncoderInput::queue_size(self)
    }
    fn config(&self) -> &VideoEncoderConfig {
        VideoEncoderInput::config(self)
    }
}

pub trait VideoEncoderOutputBoxed: MaybeSend {
    fn packet(
        &mut self,
    ) -> Pin<Box<dyn FutureMaybeSend<Output = Result<Option<EncodedVideoPacket>, Error>> + '_>>;
    fn decoder_config(&self) -> Option<&VideoDecoderConfig>;
}

impl<T: VideoEncoderOutput + ?Sized> VideoEncoderOutputBoxed for T {
    fn packet(
        &mut self,
    ) -> Pin<Box<dyn FutureMaybeSend<Output = Result<Option<EncodedVideoPacket>, Error>> + '_>>
    {
        Box::pin(VideoEncoderOutput::packet(self))
    }
    fn decoder_config(&self) -> Option<&VideoDecoderConfig> {
        VideoEncoderOutput::decoder_config(self)
    }
}

pub trait VideoDecoderInputBoxed: MaybeSend {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error>;
    fn flush(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<(), Error>> + '_>>;
    fn queue_size(&self) -> u32;
}

impl<T: VideoDecoderInput + ?Sized> VideoDecoderInputBoxed for T {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error> {
        VideoDecoderInput::decode(self, packet)
    }
    fn flush(&mut self) -> Pin<Box<dyn FutureMaybeSend<Output = Result<(), Error>> + '_>> {
        Box::pin(VideoDecoderInput::flush(self))
    }
    fn queue_size(&self) -> u32 {
        VideoDecoderInput::queue_size(self)
    }
}

pub trait VideoDecoderOutputBoxed: MaybeSend {
    fn frame(
        &mut self,
    ) -> Pin<Box<dyn FutureMaybeSend<Output = Result<Option<VideoFrame>, Error>> + '_>>;
    fn try_frame(&mut self) -> Result<Option<VideoFrame>, Error>;
}

impl<T: VideoDecoderOutput + ?Sized> VideoDecoderOutputBoxed for T {
    fn frame(
        &mut self,
    ) -> Pin<Box<dyn FutureMaybeSend<Output = Result<Option<VideoFrame>, Error>> + '_>> {
        Box::pin(VideoDecoderOutput::frame(self))
    }
    fn try_frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        VideoDecoderOutput::try_frame(self)
    }
}
