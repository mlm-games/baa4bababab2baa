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
