use std::thread;

use mediacodec::{MediaCodec, MediaFormat};
use tokio::sync::mpsc;

use crate::{
    error::Error,
    traits::{AudioDecoderInput, AudioDecoderOutput},
    types::{AudioDecoderConfig, AudioFrame, EncodedAudioPacket, SampleFormat},
};

pub struct AndroidAudioDecoderInput {
    tx: mpsc::UnboundedSender<EncodedAudioPacket>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

pub struct AndroidAudioDecoderOutput {
    rx: mpsc::UnboundedReceiver<Result<AudioFrame, Error>>,
}

impl AudioDecoderInput for AndroidAudioDecoderInput {
    fn decode(&mut self, packet: EncodedAudioPacket) -> Result<(), Error> {
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

impl AudioDecoderOutput for AndroidAudioDecoderOutput {
    async fn frame(&mut self) -> Result<Option<AudioFrame>, Error> {
        match self.rx.recv().await {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }
}

pub fn create(
    config: AudioDecoderConfig,
) -> Result<(AndroidAudioDecoderInput, AndroidAudioDecoderOutput), Error> {
    let mut format =
        MediaFormat::new().ok_or_else(|| Error::Platform("Failed to create MediaFormat".into()))?;
    format.set_string("mime", &config.codec.0);
    format.set_i32("channel-count", config.channel_count as i32);
    format.set_i32("sample-rate", config.sample_rate as i32);

    let mime = config.codec.0.clone();
    let mut codec = MediaCodec::create_decoder(&mime)
        .ok_or_else(|| Error::Platform(format!("No audio decoder for {mime}")))?;

    codec
        .init(&format, None, 0)
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    codec
        .start()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;

    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel::<EncodedAudioPacket>();
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Result<AudioFrame, Error>>();
    let queue = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let queue2 = queue.clone();

    thread::spawn(move || audio_decode_loop(codec, pkt_rx, frame_tx, queue2));

    Ok((
        AndroidAudioDecoderInput { tx: pkt_tx, queue },
        AndroidAudioDecoderOutput { rx: frame_rx },
    ))
}

fn audio_decode_loop(
    mut codec: MediaCodec,
    mut pkt_rx: mpsc::UnboundedReceiver<EncodedAudioPacket>,
    frame_tx: mpsc::UnboundedSender<Result<AudioFrame, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    loop {
        if let Ok(pkt) = pkt_rx.try_recv() {
            if let Ok(mut buf) = codec.dequeue_input() {
                let (ptr, cap) = buf.buffer();
                let copy_len = pkt.payload.len().min(cap);
                unsafe {
                    std::ptr::copy_nonoverlapping(pkt.payload.as_ptr(), ptr, copy_len);
                }
                buf.set_write_size(copy_len);
                buf.set_time(pkt.timestamp.as_micros() as u64);
                queue.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        while let Ok(out) = codec.dequeue_output() {
            let fmt = out.format();
            let channels = fmt.get_i32("channel-count").unwrap_or(2) as u32;
            let sample_rate = fmt.get_i32("sample-rate").unwrap_or(48_000) as u32;
            let ts = std::time::Duration::from_micros(out.info().presentation_time_us as u64);

            if let Some(mediacodec::Frame::Audio(audio)) = out.frame() {
                let (format, samples) = match audio.format() {
                    mediacodec::SampleFormat::S16(buf) => {
                        let bytes: Vec<u8> = buf.iter().flat_map(|s| s.to_le_bytes()).collect();
                        (SampleFormat::S16, bytes)
                    }
                    mediacodec::SampleFormat::F32(buf) => {
                        let bytes: Vec<u8> = buf.iter().flat_map(|s| s.to_le_bytes()).collect();
                        (SampleFormat::F32, bytes)
                    }
                };

                let frame = AudioFrame {
                    timestamp: ts,
                    sample_rate,
                    channels,
                    format,
                    samples,
                };

                if frame_tx.send(Ok(frame)).is_err() {
                    return;
                }
            }
        }

        thread::sleep(std::time::Duration::from_micros(100));
        if pkt_rx.is_closed() {
            return;
        }
    }
}
