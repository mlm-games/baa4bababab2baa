use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::thread;
use std::time::Duration;
use std::collections::VecDeque;

use mediacodec::{BufferFlag, MediaCodec, MediaFormat};
use tokio::sync::{mpsc, oneshot};

use crate::{
    error::Error,
    traits::{VideoEncoderInput, VideoEncoderOutput},
    types::{EncodedVideoPacket, VideoDecoderConfig, VideoEncoderConfig, VideoFrame, VideoPlanes},
};

enum Cmd {
    Frame(VideoFrame, Option<bool>),
    Flush(oneshot::Sender<Result<(), Error>>),
    Close,
}

pub struct AndroidVideoEncoderInput {
    tx: mpsc::UnboundedSender<Cmd>,
    queue: Arc<AtomicU32>,
    config: VideoEncoderConfig,
}

impl Drop for AndroidVideoEncoderInput {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

pub struct AndroidVideoEncoderOutput {
    rx: mpsc::UnboundedReceiver<Result<EncodedVideoPacket, Error>>,
}

impl VideoEncoderInput for AndroidVideoEncoderInput {
    fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error> {
        self.queue.fetch_add(1, Ordering::Relaxed);
        self.tx.send(Cmd::Frame(frame, keyframe)).map_err(|_| Error::Dropped)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Flush(tx)).map_err(|_| Error::Dropped)?;
        rx.await.map_err(|_| Error::Dropped)?
    }

    fn queue_size(&self) -> u32 {
        self.queue.load(Ordering::Relaxed)
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
    format.set_i32("color-format", 21); // COLOR_FormatYUV420SemiPlanar (NV12)

    let mime = config.codec.0.clone();
    let mut codec = MediaCodec::create_encoder(&mime)
        .ok_or_else(|| Error::Platform(format!("No encoder for {mime}")))?;

    codec
        .init(&format, None, 1)
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    codec
        .start()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel::<Result<EncodedVideoPacket, Error>>();
    let queue = Arc::new(AtomicU32::new(0));
    let queue2 = queue.clone();

    thread::spawn(move || {
        encode_loop(codec, cmd_rx, pkt_tx, queue2);
    });

    Ok((
        AndroidVideoEncoderInput {
            tx: cmd_tx,
            queue,
            config,
        },
        AndroidVideoEncoderOutput { rx: pkt_rx },
    ))
}

fn drain_pending_frames(
    codec: &mut MediaCodec,
    pending: &mut VecDeque<(VideoFrame, Option<bool>)>,
    queue: &Arc<AtomicU32>,
    _pkt_tx: &mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
) -> Result<(), Error> {
    while let Some((frame, keyframe)) = pending.pop_front() {
        if let Ok(buf) = codec.dequeue_input() {
            let mut buf: mediacodec::CodecInputBuffer = buf;
            let (ptr, cap): (*mut u8, usize) = buf.buffer();
            if let VideoPlanes::Cpu(data) = &frame.planes {
                let copy_len = data.len().min(cap);
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, copy_len);
                }
                buf.set_write_size(copy_len);
            }
            buf.set_time(frame.timestamp.as_micros() as u64);
            if keyframe.unwrap_or(false) {
                buf.set_flags(BufferFlag::Encode as u32);
            }
            queue.fetch_sub(1, Ordering::Relaxed);
        } else {
            pending.push_front((frame, keyframe));
            break;
        }
    }
    Ok(())
}

fn drain_encoded_output(
    codec: &mut MediaCodec,
    pkt_tx: &mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
) -> Result<(), Error> {
    while let Ok(out) = codec.dequeue_output() {
        let out_buf: mediacodec::CodecOutputBuffer = out;
        let info = out_buf.info();
        let is_key = BufferFlag::Encode.is_contained_in(info.flags as i32);
        let ts = Duration::from_micros(info.presentation_time_us as u64);

        if BufferFlag::EndOfStream.is_contained_in(info.flags as i32) {
            continue;
        }

        let payload_bytes = if let Some(slice) = out_buf.buffer_slice() {
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
            return Err(Error::Dropped);
        }
    }
    Ok(())
}

fn send_encoder_eos(codec: &mut MediaCodec) -> Result<(), Error> {
    loop {
        if let Ok(buf) = codec.dequeue_input() {
            let mut buf: mediacodec::CodecInputBuffer = buf;
            buf.set_flags(BufferFlag::EndOfStream as u32);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn drain_encoder_until_eos(
    codec: &mut MediaCodec,
    pkt_tx: &mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
) -> Result<(), Error> {
    loop {
        match codec.dequeue_output() {
            Ok(out) => {
                let out: mediacodec::CodecOutputBuffer = out;
                if BufferFlag::EndOfStream.is_contained_in(out.info().flags as i32) {
                    return Ok(());
                }
                let info = out.info();
                let is_key = BufferFlag::Encode.is_contained_in(info.flags as i32);
                let ts = Duration::from_micros(info.presentation_time_us as u64);
                let payload_bytes = if let Some(slice) = out.buffer_slice() {
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
                    return Err(Error::Dropped);
                }
            }
            Err(_) => thread::sleep(Duration::from_millis(1)),
        }
    }
}

fn encode_loop(
    mut codec: MediaCodec,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    pkt_tx: mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
    queue: Arc<AtomicU32>,
) {
    let mut pending: VecDeque<(VideoFrame, Option<bool>)> = VecDeque::new();

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Frame(frame, keyframe) => pending.push_back((frame, keyframe)),
                Cmd::Flush(done) => {
                    let res = (|| -> Result<(), Error> {
                        drain_pending_frames(&mut codec, &mut pending, &queue, &pkt_tx)?;
                        send_encoder_eos(&mut codec)?;
                        drain_encoder_until_eos(&mut codec, &pkt_tx)?;
                        Ok(())
                    })();
                    let _ = done.send(res);
                }
                Cmd::Close => {
                    let _ = drain_encoded_output(&mut codec, &pkt_tx);
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            }
        }

        let _ = drain_pending_frames(&mut codec, &mut pending, &queue, &pkt_tx);
        let _ = drain_encoded_output(&mut codec, &pkt_tx);

        thread::sleep(Duration::from_millis(1));

        if cmd_rx.is_closed() && pending.is_empty() {
            let _ = drain_encoded_output(&mut codec, &pkt_tx);
            queue.store(0, Ordering::Relaxed);
            return;
        }
    }
}
