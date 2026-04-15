use std::thread;

use mediacodec::{BufferFlag, MediaCodec, MediaFormat};
use tokio::sync::mpsc;

use crate::{
    error::Error,
    traits::{AudioEncoderInput, AudioEncoderOutput},
    types::{AudioDecoderConfig, AudioEncoderConfig, AudioFrame, EncodedAudioPacket},
};

pub struct AndroidAudioEncoderInput {
    tx: mpsc::UnboundedSender<AudioFrame>,
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
        self.tx.send(frame).map_err(|_| Error::Dropped)
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

pub fn create(
    config: AudioEncoderConfig,
) -> Result<(AndroidAudioEncoderInput, AndroidAudioEncoderOutput), Error> {
    let mut format =
        MediaFormat::new().ok_or_else(|| Error::Platform("Failed to create MediaFormat".into()))?;
    format.set_string("mime", &config.codec.0);
    format.set_i32("channel-count", config.channels as i32);
    format.set_i32("sample-rate", config.sample_rate as i32);
    if let Some(br) = config.bitrate {
        format.set_i32("bitrate", br as i32);
    }

    let mime = config.codec.0.clone();
    let mut codec = MediaCodec::create_encoder(&mime)
        .ok_or_else(|| Error::Platform(format!("No audio encoder for {mime}")))?;

    codec
        .init(&format, None, 1)
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    codec
        .start()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;

    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<AudioFrame>();
    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel::<Result<EncodedAudioPacket, Error>>();
    let queue = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let queue2 = queue.clone();

    thread::spawn(move || audio_encode_loop(codec, frame_rx, pkt_tx, queue2));

    Ok((
        AndroidAudioEncoderInput {
            tx: frame_tx,
            queue,
            config,
        },
        AndroidAudioEncoderOutput { rx: pkt_rx },
    ))
}

fn audio_encode_loop(
    mut codec: MediaCodec,
    mut frame_rx: mpsc::UnboundedReceiver<AudioFrame>,
    pkt_tx: mpsc::UnboundedSender<Result<EncodedAudioPacket, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    loop {
        if let Ok(frame) = frame_rx.try_recv() {
            if let Ok(buf) = codec.dequeue_input() {
                let mut buf: mediacodec::CodecInputBuffer = buf;
                let (ptr, cap): (*mut u8, usize) = buf.buffer();
                let copy_len = frame.samples.len().min(cap);
                unsafe {
                    std::ptr::copy_nonoverlapping(frame.samples.as_ptr(), ptr, copy_len);
                }
                buf.set_write_size(copy_len);
                buf.set_time(frame.timestamp.as_micros() as u64);
                queue.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        while let Ok(out) = codec.dequeue_output() {
            let out_buf: mediacodec::CodecOutputBuffer = out;
            let info = out_buf.info();
            let is_key = BufferFlag::CodecConfig.is_contained_in(info.flags as i32);
            let ts = std::time::Duration::from_micros(info.presentation_time_us as u64);

            let payload = if let Some(slice) = out_buf.buffer_slice_pub() {
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

        thread::sleep(std::time::Duration::from_micros(100));
        if frame_rx.is_closed() {
            return;
        }
    }
}
