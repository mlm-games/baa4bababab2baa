use std::thread;

use mediacodec::{BufferFlag, MediaCodec, MediaFormat};
use tokio::sync::{mpsc, oneshot};

use crate::{
    error::Error,
    traits::{AudioEncoderInput, AudioEncoderOutput},
    types::{AudioDecoderConfig, AudioEncoderConfig, AudioFrame, EncodedAudioPacket},
};

enum Cmd {
    Frame(AudioFrame),
    Flush(oneshot::Sender<Result<(), Error>>),
    Close,
}

pub struct AndroidAudioEncoderInput {
    tx: mpsc::UnboundedSender<Cmd>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
    config: AudioEncoderConfig,
}

pub struct AndroidAudioEncoderOutput {
    rx: mpsc::UnboundedReceiver<Result<EncodedAudioPacket, Error>>,
}

impl AudioEncoderInput for AndroidAudioEncoderInput {
    fn encode(&mut self, frame: AudioFrame) -> Result<(), Error> {
        self.queue
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.tx.send(Cmd::Frame(frame)).map_err(|_| Error::Dropped)
    }

    async fn flush(&mut self) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Flush(tx)).map_err(|_| Error::Dropped)?;
        rx.await.map_err(|_| Error::Dropped)?
    }

    fn queue_size(&self) -> u32 {
        self.queue.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn config(&self) -> &AudioEncoderConfig {
        &self.config
    }
}

impl AudioEncoderOutput for AndroidAudioEncoderOutput {
    async fn packet(&mut self) -> Result<Option<EncodedAudioPacket>, Error> {
        match self.rx.recv().await {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }

    fn decoder_config(&self) -> Option<&AudioDecoderConfig> {
        None
    }
}

impl Drop for AndroidAudioEncoderInput {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

pub fn create(
    config: AudioEncoderConfig,
) -> Result<(AndroidAudioEncoderInput, AndroidAudioEncoderOutput), Error> {
    let mut format =
        MediaFormat::new().map_err(|_| Error::Platform("Failed to create MediaFormat".into()))?;
    let _ = format.set_string("mime", &config.codec.0);
    let _ = format.set_i32("channel-count", config.channels as i32);
    let _ = format.set_i32("sample-rate", config.sample_rate as i32);
    if let Some(br) = config.bitrate {
        let _ = format.set_i32("bitrate", br as i32);
    }

    let mime = config.codec.0.clone();
    let mut codec = MediaCodec::create_encoder(&mime)
        .map_err(|e| Error::Platform(format!("No audio encoder for {mime}: {e:?}")))?;

    codec
        .init(&format, None, 1)
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    codec
        .start()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel::<Result<EncodedAudioPacket, Error>>();
    let queue = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let queue2 = queue.clone();

    thread::spawn(move || audio_encode_loop(codec, cmd_rx, pkt_tx, queue2));

    Ok((
        AndroidAudioEncoderInput {
            tx: cmd_tx,
            queue,
            config,
        },
        AndroidAudioEncoderOutput { rx: pkt_rx },
    ))
}

fn submit_one(
    codec: &mut MediaCodec,
    frame: &AudioFrame,
    queue: &std::sync::Arc<std::sync::atomic::AtomicU32>,
) -> Result<(), Error> {
    if let Ok(buf) = codec.dequeue_input(0) {
        let mut buf: mediacodec::CodecInputBuffer = buf;
        let (ptr, cap): (*mut u8, usize) = buf.buffer();
        if frame.samples.len() > cap {
            return Err(Error::Platform(format!(
                "audio frame too large: {} > {}",
                frame.samples.len(),
                cap
            )));
        }
        unsafe {
            std::ptr::copy_nonoverlapping(frame.samples.as_ptr(), ptr, frame.samples.len());
        }
        buf.set_write_size(frame.samples.len());
        buf.set_time(frame.timestamp.as_micros() as u64);
        queue.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    } else {
        Err(Error::Platform("no input buffer available".into()))
    }
}

fn drain_encoded_output(
    codec: &mut MediaCodec,
    pkt_tx: &mpsc::UnboundedSender<Result<EncodedAudioPacket, Error>>,
) {
    while let Ok(out) = codec.dequeue_output(0) {
        let out_buf: mediacodec::CodecOutputBuffer = out;
        let info = out_buf.info();
        let is_key = false;
        let ts = std::time::Duration::from_micros(info.presentation_time_us as u64);

        if BufferFlag::EndOfStream.is_contained_in(info.flags as i32) {
            continue;
        }

        let payload = if let Some(slice) = out_buf.buffer_slice() {
            bytes::Bytes::copy_from_slice(slice)
        } else {
            bytes::Bytes::new()
        };

        let pkt = EncodedAudioPacket {
            payload,
            timestamp: ts,
            keyframe: is_key,
        };

        if pkt_tx.send(Ok(pkt)).is_err() {
            return;
        }
    }
}

fn send_audio_eos(codec: &mut MediaCodec) -> Result<(), Error> {
    for _ in 0..5000 {
        if let Ok(buf) = codec.dequeue_input(0) {
            let mut buf: mediacodec::CodecInputBuffer = buf;
            buf.set_flags(BufferFlag::EndOfStream as u32);
            return Ok(());
        }
        thread::sleep(std::time::Duration::from_millis(1));
    }
    Err(Error::Platform("send_audio_eos timed out".into()))
}

fn drain_encoder_until_eos(
    codec: &mut MediaCodec,
    pkt_tx: &mpsc::UnboundedSender<Result<EncodedAudioPacket, Error>>,
) -> Result<(), Error> {
    for _ in 0..5000 {
        match codec.dequeue_output(1000) {
            Ok(out) => {
                let out: mediacodec::CodecOutputBuffer = out;
                if BufferFlag::EndOfStream.is_contained_in(out.info().flags as i32) {
                    return Ok(());
                }
                let info = out.info();
                let ts = std::time::Duration::from_micros(info.presentation_time_us as u64);
                let payload = if let Some(slice) = out.buffer_slice() {
                    bytes::Bytes::copy_from_slice(slice)
                } else {
                    bytes::Bytes::new()
                };
                let pkt = EncodedAudioPacket {
                    payload,
                    timestamp: ts,
                    keyframe: false,
                };
                if pkt_tx.send(Ok(pkt)).is_err() {
                    return Err(Error::Dropped);
                }
            }
            Err(_) => thread::sleep(std::time::Duration::from_millis(1)),
        }
    }
    Err(Error::Platform("drain_encoder_until_eos timed out".into()))
}

fn audio_encode_loop(
    mut codec: MediaCodec,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    pkt_tx: mpsc::UnboundedSender<Result<EncodedAudioPacket, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    loop {
        match cmd_rx.blocking_recv() {
            Some(Cmd::Frame(frame)) => {
                if let Err(e) = submit_one(&mut codec, &frame, &queue) {
                    let _ = pkt_tx.send(Err(e));
                    return;
                }
            }
            Some(Cmd::Flush(done)) => {
                let res = (|| -> Result<(), Error> {
                    send_audio_eos(&mut codec)?;
                    drain_encoder_until_eos(&mut codec, &pkt_tx)?;
                    codec.flush().map_err(|e| Error::Platform(format!("{e:?}")))?;
                    Ok(())
                })();
                let _ = done.send(res);
            }
            Some(Cmd::Close) | None => {
                queue.store(0, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        }

        drain_encoded_output(&mut codec, &pkt_tx);
    }
}
