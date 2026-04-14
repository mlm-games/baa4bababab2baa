use std::thread;

use mediacodec::{BufferFlag, MediaCodec, MediaFormat};
use tokio::sync::mpsc;

use crate::{
    error::Error,
    traits::{VideoEncoderInput, VideoEncoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, VideoDecoderConfig, VideoEncoderConfig, VideoFrame,
        VideoPlanes,
    },
};

pub struct AndroidVideoEncoderInput {
    tx: mpsc::UnboundedSender<(VideoFrame, Option<bool>)>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
    config: VideoEncoderConfig,
}

pub struct AndroidVideoEncoderOutput {
    rx: mpsc::UnboundedReceiver<Result<EncodedVideoPacket, Error>>,
}

impl VideoEncoderInput for AndroidVideoEncoderInput {
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

impl VideoEncoderOutput for AndroidVideoEncoderOutput {
    async fn packet(&mut self) -> Result<Option<EncodedVideoPacket>, Error> {
        match self.rx.recv().await {
            Some(result) => result.map(Some),
            None => Ok(None),
        }
    }

    fn decoder_config(&self) -> Option<&VideoDecoderConfig> {
        None
    }
}

pub fn create(
    config: VideoEncoderConfig,
) -> Result<(AndroidVideoEncoderInput, AndroidVideoEncoderOutput), Error> {
    let mut format =
        MediaFormat::new().ok_or_else(|| Error::Platform("Failed to create MediaFormat".into()))?;

    format.set_string("mime", &config.codec.0);
    format.set_i32("width", config.dimensions.width as i32);
    format.set_i32("height", config.dimensions.height as i32);
    if let Some(br) = config.bitrate {
        format.set_i32("bitrate", br as i32);
    }
    if let Some(fr) = config.framerate {
        format.set_i32("frame-rate", fr as i32);
    }
    format.set_i32("i-frame-interval", 1);

    let mime = config.codec.0.clone();
    let mut codec = MediaCodec::create_encoder(&mime)
        .ok_or_else(|| Error::Platform(format!("No encoder for {mime}")))?;

    codec
        .init(&format, None, 1)
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    codec
        .start()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;

    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<(VideoFrame, Option<bool>)>();
    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel::<Result<EncodedVideoPacket, Error>>();
    let queue = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let queue2 = queue.clone();

    thread::spawn(move || {
        encode_loop(codec, frame_rx, pkt_tx, queue2);
    });

    Ok((
        AndroidVideoEncoderInput {
            tx: frame_tx,
            queue,
            config,
        },
        AndroidVideoEncoderOutput { rx: pkt_rx },
    ))
}

fn encode_loop(
    mut codec: MediaCodec,
    mut frame_rx: mpsc::UnboundedReceiver<(VideoFrame, Option<bool>)>,
    pkt_tx: mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    loop {
        if let Ok((frame, _keyframe)) = frame_rx.try_recv() {
            if let Ok(mut buf) = codec.dequeue_input() {
                let (ptr, cap) = buf.buffer();
                if let VideoPlanes::Cpu(data) = &frame.planes {
                    let copy_len = data.len().min(cap);
                    unsafe {
                        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, copy_len);
                    }
                    buf.set_write_size(copy_len);
                }
                buf.set_time(frame.timestamp.as_micros() as u64);
                queue.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        while let Ok(out) = codec.dequeue_output() {
            let info = out.info();
            let is_key = BufferFlag::CodecConfig.is_contained_in(info.flags as i32)
                || BufferFlag::Encode.is_contained_in(info.flags as i32);
            let ts = std::time::Duration::from_micros(info.presentation_time_us as u64);

            let payload_bytes = if let Some(slice) = out.buffer_slice_pub() {
                bytes::Bytes::copy_from_slice(slice)
            } else {
                bytes::Bytes::new()
            };

            let pkt = EncodedVideoPacket {
                payload: payload_bytes,
                timestamp: ts,
                keyframe: is_key,
            };

            if pkt_tx.send(Ok(pkt)).is_err() {
                return;
            }
        }

        thread::sleep(std::time::Duration::from_micros(100));

        if frame_rx.is_closed() {
            return;
        }
    }
}
