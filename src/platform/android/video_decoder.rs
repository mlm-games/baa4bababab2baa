use std::sync::atomic::Ordering;
use std::thread;

use mediacodec::{BufferFlag, MediaCodec, MediaFormat};
use tokio::sync::{mpsc, oneshot};

use crate::{
    error::Error,
    traits::{VideoDecoderInput, VideoDecoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, PixelFormat, VideoDecoderConfig, VideoFrame, VideoPlanes,
    },
};

enum Cmd {
    Packet(EncodedVideoPacket),
    Flush(oneshot::Sender<Result<(), Error>>),
    Close,
}

pub struct AndroidVideoDecoderInput {
    tx: mpsc::UnboundedSender<Cmd>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl Drop for AndroidVideoDecoderInput {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

pub struct AndroidVideoDecoderOutput {
    rx: mpsc::UnboundedReceiver<Result<VideoFrame, Error>>,
}

impl VideoDecoderInput for AndroidVideoDecoderInput {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error> {
        self.queue
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.tx
            .send(Cmd::Packet(packet))
            .map_err(|_| Error::Dropped)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Flush(tx)).map_err(|_| Error::Dropped)?;
        rx.await.map_err(|_| Error::Dropped)?
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

    fn try_frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.rx.try_recv() {
            Ok(Ok(frame)) => Ok(Some(frame)),
            Ok(Err(e)) => Err(e),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => Ok(None),
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

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Result<VideoFrame, Error>>();
    let queue = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let queue2 = queue.clone();

    thread::spawn(move || {
        decode_loop(codec, cmd_rx, frame_tx, queue2);
    });

    Ok((
        AndroidVideoDecoderInput { tx: cmd_tx, queue },
        AndroidVideoDecoderOutput { rx: frame_rx },
    ))
}

fn drain_output(
    codec: &mut MediaCodec,
    frame_tx: &mpsc::UnboundedSender<Result<VideoFrame, Error>>,
) {
    while let Ok(out) = codec.dequeue_output() {
        let out_buf: mediacodec::CodecOutputBuffer = out;
        if BufferFlag::EndOfStream.is_contained_in(out_buf.info().flags as i32) {
            continue;
        }
        let w = out_buf.format().get_i32("width").unwrap_or(0) as u32;
        let h = out_buf.format().get_i32("height").unwrap_or(0) as u32;
        let ts_us = out_buf.info().presentation_time_us;
        let data = out_buf
            .buffer_slice()
            .unwrap_or_default()
            .to_vec();
        let frame = VideoFrame {
            dimensions: Dimensions::new(w, h),
            format: PixelFormat::Nv12,
            timestamp: std::time::Duration::from_micros(ts_us as u64),
            planes: VideoPlanes::Cpu(data),
        };
        if frame_tx.send(Ok(frame)).is_err() {
            return;
        }
    }
}

fn send_eos(codec: &mut MediaCodec) -> Result<(), Error> {
    loop {
        if let Ok(buf) = codec.dequeue_input() {
            let mut buf: mediacodec::CodecInputBuffer = buf;
            buf.set_flags(BufferFlag::EndOfStream as u32);
            return Ok(());
        }
        thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn drain_until_eos(
    codec: &mut MediaCodec,
    frame_tx: &mpsc::UnboundedSender<Result<VideoFrame, Error>>,
) -> Result<(), Error> {
    loop {
        match codec.dequeue_output() {
            Ok(out) => {
                let out: mediacodec::CodecOutputBuffer = out;
                if BufferFlag::EndOfStream.is_contained_in(out.info().flags as i32) {
                    return Ok(());
                }
                let w = out.format().get_i32("width").unwrap_or(0) as u32;
                let h = out.format().get_i32("height").unwrap_or(0) as u32;
                let ts_us = out.info().presentation_time_us;
                let data: Vec<u8> = out.buffer_slice().map(|s| s.to_vec()).unwrap_or_default();
                let frame = VideoFrame {
                    dimensions: Dimensions::new(w, h),
                    format: PixelFormat::Nv12,
                    timestamp: std::time::Duration::from_micros(ts_us as u64),
                    planes: VideoPlanes::Cpu(data),
                };
                if frame_tx.send(Ok(frame)).is_err() {
                    return Err(Error::Dropped);
                }
            }
            Err(_) => thread::sleep(std::time::Duration::from_millis(1)),
        }
    }
}

fn decode_loop(
    mut codec: MediaCodec,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    frame_tx: mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    let mut pending: std::collections::VecDeque<EncodedVideoPacket> =
        std::collections::VecDeque::new();

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Packet(pkt) => pending.push_back(pkt),
                Cmd::Flush(done) => {
                    let res = (|| -> Result<(), Error> {
                        submit_pending(&mut codec, &mut pending, &queue)?;
                        send_eos(&mut codec)?;
                        drain_until_eos(&mut codec, &frame_tx)?;
                        Ok(())
                    })();
                    let _ = done.send(res);
                }
                Cmd::Close => {
                    drain_output(&mut codec, &frame_tx);
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            }
        }

        let _ = submit_pending(&mut codec, &mut pending, &queue);

        drain_output(&mut codec, &frame_tx);

        thread::sleep(std::time::Duration::from_millis(1));

        if cmd_rx.is_closed() && pending.is_empty() {
            drain_output(&mut codec, &frame_tx);
            queue.store(0, std::sync::atomic::Ordering::Relaxed);
            return;
        }
    }
}

fn submit_pending(
    codec: &mut MediaCodec,
    pending: &mut std::collections::VecDeque<EncodedVideoPacket>,
    queue: &std::sync::Arc<std::sync::atomic::AtomicU32>,
) -> Result<(), Error> {
    while let Some(pkt) = pending.pop_front() {
        if let Ok(buf) = codec.dequeue_input() {
            let mut buf: mediacodec::CodecInputBuffer = buf;
            let (ptr, cap): (*mut u8, usize) = buf.buffer();
            let copy_len = pkt.payload.len().min(cap);
            unsafe {
                std::ptr::copy_nonoverlapping(pkt.payload.as_ptr(), ptr, copy_len);
            }
            buf.set_write_size(copy_len);
            buf.set_time(pkt.timestamp.as_micros() as u64);
            queue.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            pending.push_front(pkt);
            break;
        }
    }
    Ok(())
}
