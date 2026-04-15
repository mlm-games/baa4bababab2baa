use std::thread;

use mediacodec::{MediaCodec, MediaFormat};
use tokio::sync::mpsc;

use crate::{
    error::Error,
    traits::{VideoDecoderInput, VideoDecoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, PixelFormat, VideoDecoderConfig, VideoFrame, VideoPlanes,
    },
};
use mediacodec::{CodecInputBuffer, CodecOutputBuffer};

pub struct AndroidVideoDecoderInput {
    tx: mpsc::UnboundedSender<EncodedVideoPacket>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

pub struct AndroidVideoDecoderOutput {
    rx: mpsc::UnboundedReceiver<Result<VideoFrame, Error>>,
}

impl VideoDecoderInput for AndroidVideoDecoderInput {
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

impl VideoDecoderOutput for AndroidVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.rx.recv().await {
            Some(result) => result.map(Some),
            None => Ok(None),
        }
    }
}

pub fn create(
    config: VideoDecoderConfig,
) -> Result<(AndroidVideoDecoderInput, AndroidVideoDecoderOutput), Error> {
    let mut format =
        MediaFormat::new().ok_or_else(|| Error::Platform("Failed to create MediaFormat".into()))?;
    format.set_string("mime", &config.codec.0);

    if let Some(res) = config.resolution {
        format.set_i32("width", res.width as i32);
        format.set_i32("height", res.height as i32);
    }

    let mime = config.codec.0.clone();

    let mut codec = MediaCodec::create_decoder(&mime)
        .ok_or_else(|| Error::Platform(format!("No decoder for {mime}")))?;

    codec
        .init(&format, None, 0)
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    codec
        .start()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;

    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel::<EncodedVideoPacket>();
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Result<VideoFrame, Error>>();
    let queue = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let queue2 = queue.clone();

    thread::spawn(move || {
        decode_loop(codec, pkt_rx, frame_tx, queue2);
    });

    Ok((
        AndroidVideoDecoderInput { tx: pkt_tx, queue },
        AndroidVideoDecoderOutput { rx: frame_rx },
    ))
}

fn decode_loop(
    mut codec: MediaCodec,
    mut pkt_rx: mpsc::UnboundedReceiver<EncodedVideoPacket>,
    frame_tx: mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    loop {
        if let Ok(pkt) = pkt_rx.try_recv() {
            if let Ok(buf) = codec.dequeue_input() {
                let mut buf: mediacodec::CodecInputBuffer = buf;
                let (ptr, cap): (*mut u8, usize) = buf.buffer();
                let copy_len = pkt.payload.len().min(cap);
                unsafe {
                    std::ptr::copy_nonoverlapping(pkt.payload.as_ptr(), ptr, copy_len);
                }
                buf.set_write_size(copy_len);
                buf.set_time(pkt.timestamp.as_micros() as u64);
                if pkt.keyframe {
                    buf.set_flags(mediacodec::BufferFlag::CodecConfig as u32);
                }
                queue.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        while let Ok(out) = codec.dequeue_output() {
            let out_buf: mediacodec::CodecOutputBuffer = out;
            let w = out_buf.format().get_i32("width").unwrap_or(0) as u32;
            let h = out_buf.format().get_i32("height").unwrap_or(0) as u32;
            let ts_us = out_buf.info().presentation_time_us;

            let frame = VideoFrame {
                dimensions: Dimensions::new(w, h),
                format: PixelFormat::Nv12,
                timestamp: std::time::Duration::from_micros(ts_us as u64),
                planes: VideoPlanes::Hardware,
            };

            if frame_tx.send(Ok(frame)).is_err() {
                return;
            }
        }

        thread::sleep(std::time::Duration::from_micros(100));

        if pkt_rx.is_closed() {
            while let Ok(out) = codec.dequeue_output() {
                let out_buf: mediacodec::CodecOutputBuffer = out;
                let w = out_buf.format().get_i32("width").unwrap_or(0) as u32;
                let h = out_buf.format().get_i32("height").unwrap_or(0) as u32;
                let ts_us = out_buf.info().presentation_time_us;
                let frame = VideoFrame {
                    dimensions: Dimensions::new(w, h),
                    format: PixelFormat::Nv12,
                    timestamp: std::time::Duration::from_micros(ts_us as u64),
                    planes: VideoPlanes::Hardware,
                };
                let _ = frame_tx.send(Ok(frame));
            }
            return;
        }
    }
}
