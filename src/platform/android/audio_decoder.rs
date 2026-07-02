use std::thread;

use mediacodec::{
    CodecInputBuffer, CodecOutputBuffer, MediaCodec, MediaFormat, SampleFormat as McSampleFormat,
};
use tokio::sync::{mpsc, oneshot};

use crate::{
    error::Error,
    traits::{AudioDecoderInput, AudioDecoderOutput},
    types::{AudioDecoderConfig, AudioFrame, EncodedAudioPacket, SampleFormat},
};

enum Cmd {
    Packet(EncodedAudioPacket),
    Flush(oneshot::Sender<Result<(), Error>>),
    Close,
}

pub struct AndroidAudioDecoderInput {
    tx: mpsc::UnboundedSender<Cmd>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

pub struct AndroidAudioDecoderOutput {
    rx: mpsc::UnboundedReceiver<Result<AudioFrame, Error>>,
}

impl AudioDecoderInput for AndroidAudioDecoderInput {
    fn decode(&mut self, packet: EncodedAudioPacket) -> Result<(), Error> {
        self.queue
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.tx.send(Cmd::Packet(packet)).map_err(|_| Error::Dropped)
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

impl AudioDecoderOutput for AndroidAudioDecoderOutput {
    async fn frame(&mut self) -> Result<Option<AudioFrame>, Error> {
        match self.rx.recv().await {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }
}

impl Drop for AndroidAudioDecoderInput {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

pub fn create(
    config: AudioDecoderConfig,
) -> Result<(AndroidAudioDecoderInput, AndroidAudioDecoderOutput), Error> {
    let mut format =
        MediaFormat::new().map_err(|_| Error::Platform("Failed to create MediaFormat".into()))?;
    let _ = format.set_string("mime", &config.codec.0);
    let _ = format.set_i32("channel-count", config.channel_count as i32);
    let _ = format.set_i32("sample-rate", config.sample_rate as i32);

    let mime = config.codec.0.clone();
    let mut codec = MediaCodec::create_decoder(&mime)
        .map_err(|e| Error::Platform(format!("No audio decoder for {mime}: {e:?}")))?;

    codec
        .init(&format, None, 0)
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    codec
        .start()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Result<AudioFrame, Error>>();
    let queue = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let queue2 = queue.clone();
    let fallback_channels = config.channel_count;
    let fallback_sample_rate = config.sample_rate;

    thread::spawn(move || {
        audio_decode_loop(codec, cmd_rx, frame_tx, queue2, fallback_channels, fallback_sample_rate)
    });

    Ok((
        AndroidAudioDecoderInput { tx: cmd_tx, queue },
        AndroidAudioDecoderOutput { rx: frame_rx },
    ))
}

fn submit_one(
    codec: &mut MediaCodec,
    pkt: &EncodedAudioPacket,
    queue: &std::sync::Arc<std::sync::atomic::AtomicU32>,
) -> Result<(), Error> {
    if let Ok(buf) = codec.dequeue_input(0) {
        let mut buf: CodecInputBuffer = buf;
        let (ptr, cap) = buf.buffer();
        if pkt.payload.len() > cap {
            return Err(Error::Platform(format!(
                "audio packet too large: {} > {}",
                pkt.payload.len(),
                cap
            )));
        }
        unsafe {
            std::ptr::copy_nonoverlapping(pkt.payload.as_ptr(), ptr, pkt.payload.len());
        }
        buf.set_write_size(pkt.payload.len());
        buf.set_time(pkt.timestamp.as_micros() as u64);
        queue.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    } else {
        Err(Error::Platform("no input buffer available".into()))
    }
}

fn drain_output(
    codec: &mut MediaCodec,
    frame_tx: &mpsc::UnboundedSender<Result<AudioFrame, Error>>,
    fallback_channels: u32,
    fallback_sample_rate: u32,
) {
    while let Ok(out_buf) = codec.dequeue_output(0) {
        let out_buf: CodecOutputBuffer = out_buf;
        let fmt = out_buf.format();
        let channels = fmt.get_i32("channel-count").map(|v| v as u32).unwrap_or(fallback_channels);
        let sample_rate = fmt.get_i32("sample-rate").map(|v| v as u32).unwrap_or(fallback_sample_rate);
        let ts =
            std::time::Duration::from_micros(out_buf.info().presentation_time_us as u64);

        if let Some(mediacodec::Frame::Audio(audio)) = out_buf.frame() {
            let audio_fmt = audio.format();
            let (fmt_out, samples) = match audio_fmt {
                McSampleFormat::S16(buf) => {
                    let bytes: Vec<u8> =
                        buf.iter().flat_map(|s| i16::to_le_bytes(*s)).collect();
                    (SampleFormat::S16, bytes)
                }
                McSampleFormat::F32(buf) => {
                    let bytes: Vec<u8> =
                        buf.iter().flat_map(|s| f32::to_le_bytes(*s)).collect();
                    (SampleFormat::F32, bytes)
                }
            };

            let frame = AudioFrame {
                timestamp: ts,
                sample_rate,
                channels,
                format: fmt_out,
                samples,
            };

            if frame_tx.send(Ok(frame)).is_err() {
                return;
            }
        }
    }
}

fn audio_decode_loop(
    mut codec: MediaCodec,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    frame_tx: mpsc::UnboundedSender<Result<AudioFrame, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
    fallback_channels: u32,
    fallback_sample_rate: u32,
) {
    loop {
        match cmd_rx.blocking_recv() {
            Some(Cmd::Packet(pkt)) => {
                match submit_one(&mut codec, &pkt, &queue) {
                    Ok(()) => {}
                    Err(e) => {
                        let _ = frame_tx.send(Err(e));
                        return;
                    }
                }
            }
            Some(Cmd::Flush(done)) => {
                let res = (|| -> Result<(), Error> {
                    send_audio_eos(&mut codec)?;
                    drain_audio_until_eos(&mut codec, &frame_tx, fallback_channels, fallback_sample_rate)?;
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

        drain_output(&mut codec, &frame_tx, fallback_channels, fallback_sample_rate);
    }
}

fn send_audio_eos(codec: &mut MediaCodec) -> Result<(), Error> {
    for _ in 0..5000 {
        if let Ok(buf) = codec.dequeue_input(0) {
            let mut buf: mediacodec::CodecInputBuffer = buf;
            buf.set_flags(mediacodec::BufferFlag::EndOfStream as u32);
            return Ok(());
        }
        thread::sleep(std::time::Duration::from_millis(1));
    }
    Err(Error::Platform("send_audio_eos timed out".into()))
}

fn drain_audio_until_eos(
    codec: &mut MediaCodec,
    frame_tx: &mpsc::UnboundedSender<Result<AudioFrame, Error>>,
    fallback_channels: u32,
    fallback_sample_rate: u32,
) -> Result<(), Error> {
    for _ in 0..5000 {
        match codec.dequeue_output(1000) {
            Ok(out_buf) => {
                let out_buf: CodecOutputBuffer = out_buf;
                if mediacodec::BufferFlag::EndOfStream.is_contained_in(out_buf.info().flags as i32) {
                    return Ok(());
                }
                let fmt = out_buf.format();
                let channels = fmt.get_i32("channel-count").map(|v| v as u32).unwrap_or(fallback_channels);
                let sample_rate = fmt.get_i32("sample-rate").map(|v| v as u32).unwrap_or(fallback_sample_rate);
                let ts =
                    std::time::Duration::from_micros(out_buf.info().presentation_time_us as u64);

                if let Some(mediacodec::Frame::Audio(audio)) = out_buf.frame() {
                    let audio_fmt = audio.format();
                    let (fmt_out, samples) = match audio_fmt {
                        McSampleFormat::S16(buf) => {
                            let bytes: Vec<u8> =
                                buf.iter().flat_map(|s| i16::to_le_bytes(*s)).collect();
                            (SampleFormat::S16, bytes)
                        }
                        McSampleFormat::F32(buf) => {
                            let bytes: Vec<u8> =
                                buf.iter().flat_map(|s| f32::to_le_bytes(*s)).collect();
                            (SampleFormat::F32, bytes)
                        }
                    };

                    let frame = AudioFrame {
                        timestamp: ts,
                        sample_rate,
                        channels,
                        format: fmt_out,
                        samples,
                    };

                    if frame_tx.send(Ok(frame)).is_err() {
                        return Err(Error::Dropped);
                    }
                }
            }
            Err(_) => thread::sleep(std::time::Duration::from_millis(1)),
        }
    }
    Err(Error::Platform("drain_audio_until_eos timed out".into()))
}
