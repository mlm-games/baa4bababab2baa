use std::thread;

use tokio::sync::mpsc;

use crate::{
    error::Error,
    traits::{VideoDecoderInput, VideoDecoderOutput},
    types::{Dimensions, EncodedVideoPacket, PixelFormat, VideoDecoderConfig, VideoFrame, VideoPlanes},
};

pub struct CrosVideoDecoderInput {
    tx: mpsc::UnboundedSender<EncodedVideoPacket>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

pub struct CrosVideoDecoderOutput {
    rx: mpsc::UnboundedReceiver<Result<VideoFrame, Error>>,
}

impl VideoDecoderInput for CrosVideoDecoderInput {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error> {
        self.queue
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.tx.send(packet).map_err(|_| Error::Dropped)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        while self.queue.load(std::sync::atomic::Ordering::Relaxed) > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        Ok(())
    }

    fn queue_size(&self) -> u32 {
        self.queue.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl VideoDecoderOutput for CrosVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.rx.recv().await {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }
}

pub fn create(
    config: VideoDecoderConfig,
) -> Result<(CrosVideoDecoderInput, CrosVideoDecoderOutput), Error> {
    let supported = ["video/avc", "video/vp8", "video/vp9", "video/av01"];
    if !supported.contains(&config.codec.0.as_str()) {
        return Err(Error::Unsupported);
    }

    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel::<EncodedVideoPacket>();
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Result<VideoFrame, Error>>();
    let queue = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let queue2 = queue.clone();
    let config2 = config.clone();

    thread::spawn(move || decode_loop(config2, pkt_rx, frame_tx, queue2));

    Ok((
        CrosVideoDecoderInput { tx: pkt_tx, queue },
        CrosVideoDecoderOutput { rx: frame_rx },
    ))
}

fn decode_loop(
    config: VideoDecoderConfig,
    mut pkt_rx: mpsc::UnboundedReceiver<EncodedVideoPacket>,
    frame_tx: mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    let _ = config;
    let _ = queue;
    let _ = frame_tx.send(Err(Error::Platform(
        "cros-codecs decoder not yet implemented; waiting for stable API".into(),
    )));

    while pkt_rx.blocking_recv().is_some() {}
}