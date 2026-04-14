use std::thread;

use tokio::sync::mpsc;

use crate::{
    error::Error,
    traits::{VideoEncoderInput, VideoEncoderOutput},
    types::{EncodedVideoPacket, VideoDecoderConfig, VideoEncoderConfig, VideoFrame},
};

pub struct CrosVideoEncoderInput {
    tx: mpsc::UnboundedSender<(VideoFrame, Option<bool>)>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
    config: VideoEncoderConfig,
}

pub struct CrosVideoEncoderOutput {
    rx: mpsc::UnboundedReceiver<Result<EncodedVideoPacket, Error>>,
}

impl VideoEncoderInput for CrosVideoEncoderInput {
    fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error> {
        self.queue
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.tx.send((frame, keyframe)).map_err(|_| Error::Dropped)
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

    fn config(&self) -> &VideoEncoderConfig {
        &self.config
    }
}

impl VideoEncoderOutput for CrosVideoEncoderOutput {
    async fn packet(&mut self) -> Result<Option<EncodedVideoPacket>, Error> {
        match self.rx.recv().await {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }

    fn decoder_config(&self) -> Option<&VideoDecoderConfig> {
        None
    }
}

pub fn create(
    config: VideoEncoderConfig,
) -> Result<(CrosVideoEncoderInput, CrosVideoEncoderOutput), Error> {
    let supported = ["video/avc", "video/vp8", "video/vp9", "video/av01"];
    if !supported.contains(&config.codec.0.as_str()) {
        return Err(Error::Unsupported);
    }

    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<(VideoFrame, Option<bool>)>();
    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel::<Result<EncodedVideoPacket, Error>>();
    let queue = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let queue2 = queue.clone();
    let config2 = config.clone();

    thread::spawn(move || encode_loop(config2, frame_rx, pkt_tx, queue2));

    Ok((
        CrosVideoEncoderInput {
            tx: frame_tx,
            queue,
            config,
        },
        CrosVideoEncoderOutput { rx: pkt_rx },
    ))
}

fn encode_loop(
    config: VideoEncoderConfig,
    mut frame_rx: mpsc::UnboundedReceiver<(VideoFrame, Option<bool>)>,
    pkt_tx: mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    let _ = config;
    let _ = queue;
    let _ = pkt_tx.send(Err(Error::Platform(
        "Linux VAAPI encoder not yet implemented. Enable linux feature with VAAPI runtime.".into(),
    )));

    while frame_rx.blocking_recv().is_some() {}
}