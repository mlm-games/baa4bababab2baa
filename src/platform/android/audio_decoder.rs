use std::collections::VecDeque;
use std::thread;

use mediacodec::{
    CodecInputBuffer, CodecOutputBuffer, MediaCodec, MediaFormat, SampleFormat as McSampleFormat,
};
use tokio::sync::{mpsc, oneshot};

use super::cmd::{self, Cmd};
use crate::{
    error::Error,
    traits::{AudioDecoderInput, AudioDecoderOutput},
    types::{AudioDecoderConfig, AudioFrame, EncodedAudioPacket, SampleFormat},
};

pub struct AndroidAudioDecoderInput {
    tx: mpsc::UnboundedSender<Cmd<EncodedAudioPacket>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

pub struct AndroidAudioDecoderOutput {
    rx: mpsc::UnboundedReceiver<Result<AudioFrame, Error>>,
}

impl AudioDecoderInput for AndroidAudioDecoderInput {
    fn decode(&mut self, packet: EncodedAudioPacket) -> Result<(), Error> {
        self.queue
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.tx.send(Cmd::Item(packet)).map_err(|_| Error::Dropped)
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

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd<EncodedAudioPacket>>();
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

fn drain_pending_inputs(
    codec: &mut MediaCodec,
    pending: &mut VecDeque<EncodedAudioPacket>,
    queue: &std::sync::Arc<std::sync::atomic::AtomicU32>,
) -> Result<(), Error> {
    while let Some(pkt) = pending.pop_front() {
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
        } else {
            pending.push_front(pkt);
            break;
        }
    }
    Ok(())
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
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd<EncodedAudioPacket>>,
    frame_tx: mpsc::UnboundedSender<Result<AudioFrame, Error>>,
    queue: std::sync::Arc<std::sync::atomic::AtomicU32>,
    fallback_channels: u32,
    fallback_sample_rate: u32,
) {
    let mut pending: VecDeque<EncodedAudioPacket> = VecDeque::new();

    loop {
        match cmd_rx.blocking_recv() {
            Some(Cmd::Item(pkt)) => pending.push_back(pkt),
            Some(Cmd::Flush(done)) => {
                let res = (|| -> Result<(), Error> {
                    drain_pending_inputs(&mut codec, &mut pending, &queue)?;
                    cmd::send_eos(&mut codec)?;
                    cmd::drain_until_eos(&mut codec, |out_buf| {
                        let fmt = out_buf.format();
                        let channels = fmt.get_i32("channel-count").map(|v| v as u32).unwrap_or(fallback_channels);
                        let sample_rate = fmt.get_i32("sample-rate").map(|v| v as u32).unwrap_or(fallback_sample_rate);
                        let ts = std::time::Duration::from_micros(out_buf.info().presentation_time_us as u64);
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
                            frame_tx.send(Ok(frame)).map_err(|_| Error::Dropped)?;
                        }
                        Ok(())
                    })?;
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

        let _ = drain_pending_inputs(&mut codec, &mut pending, &queue);
        drain_output(&mut codec, &frame_tx, fallback_channels, fallback_sample_rate);

        if cmd_rx.is_closed() && pending.is_empty() {
            queue.store(0, std::sync::atomic::Ordering::Relaxed);
            return;
        }
    }
}


