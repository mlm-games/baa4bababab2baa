use std::sync::{
    Arc,
    OnceLock,
    atomic::{AtomicU32, Ordering},
};
use std::thread;
use std::time::Duration;
use std::collections::VecDeque;

use mediacodec::{BufferFlag, MediaCodec, MediaFormat};
use tokio::sync::{mpsc, oneshot};

use super::cmd::{self, Cmd};
use crate::{
    error::Error,
    traits::{VideoEncoderInput, VideoEncoderOutput},
    types::{Dimensions, EncodedVideoPacket, VideoDecoderConfig, VideoEncoderConfig, VideoFrame, VideoPlanes},
};

pub struct AndroidVideoEncoderInput {
    tx: mpsc::UnboundedSender<Cmd<(VideoFrame, Option<bool>)>>,
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
    decoder_cfg: Option<VideoDecoderConfig>,
    decoder_cfg_shared: Arc<OnceLock<VideoDecoderConfig>>,
}

impl VideoEncoderInput for AndroidVideoEncoderInput {
    fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error> {
        self.queue.fetch_add(1, Ordering::Relaxed);
        self.tx.send(Cmd::Item((frame, keyframe))).map_err(|_| Error::Dropped)
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
        self.decoder_cfg.as_ref().or_else(|| {
            self.decoder_cfg_shared.get()
        })
    }
}

pub fn create(
    config: VideoEncoderConfig,
) -> Result<(AndroidVideoEncoderInput, AndroidVideoEncoderOutput), Error> {
    let mut format =
        MediaFormat::new().map_err(|_| Error::Platform("Failed to create MediaFormat".into()))?;

    let _ = format.set_string("mime", &config.codec.0);
    let _ = format.set_i32("width", config.dimensions.width as i32);
    let _ = format.set_i32("height", config.dimensions.height as i32);
    if let Some(br) = config.bitrate {
        let _ = format.set_i32("bitrate", br as i32);
    }
    if let Some(fr) = config.framerate {
        let _ = format.set_i32("frame-rate", fr as i32);
    }
    let _ = format.set_i32("i-frame-interval", 1);
    let _ = format.set_i32("color-format", 21); // COLOR_FormatYUV420SemiPlanar (NV12)

    let mime = config.codec.0.clone();
    let mut codec = MediaCodec::create_encoder(&mime)
        .map_err(|e| Error::Platform(format!("No encoder for {mime}: {e:?}")))?;

    codec
        .init(&format, None, 1)
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    codec
        .start()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd<(VideoFrame, Option<bool>)>>();
    let (pkt_tx, pkt_rx) = mpsc::unbounded_channel::<Result<EncodedVideoPacket, Error>>();
    let queue = Arc::new(AtomicU32::new(0));
    let queue2 = queue.clone();
    let decoder_cfg = Arc::new(OnceLock::new());
    let decoder_cfg2 = decoder_cfg.clone();

    thread::spawn(move || {
        encode_loop(codec, cmd_rx, pkt_tx, queue2, decoder_cfg2, config.dimensions, config.codec.clone());
    });

    Ok((
        AndroidVideoEncoderInput {
            tx: cmd_tx,
            queue,
            config,
        },
        AndroidVideoEncoderOutput {
            rx: pkt_rx,
            decoder_cfg: None,
            decoder_cfg_shared: decoder_cfg,
        },
    ))
}

fn drain_pending_frames(
    codec: &mut MediaCodec,
    pending: &mut VecDeque<(VideoFrame, Option<bool>)>,
    queue: &Arc<AtomicU32>,
    _pkt_tx: &mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
) -> Result<(), Error> {
    while let Some((frame, keyframe)) = pending.pop_front() {
        if let Ok(buf) = codec.dequeue_input(0) {
            let mut buf: mediacodec::CodecInputBuffer = buf;
            let (ptr, cap): (*mut u8, usize) = buf.buffer();
            let data = match &frame.planes {
                VideoPlanes::Cpu(d) => d,
                _ => return Err(Error::InvalidConfig("Android encoder requires CPU frames".into())),
            };
            if data.len() > cap {
                return Err(Error::Platform(format!(
                    "frame too large: {} > {}",
                    data.len(),
                    cap
                )));
            }
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            }
            buf.set_write_size(data.len());
            buf.set_time(frame.timestamp.as_micros() as u64);
            if keyframe.unwrap_or(false) {
                buf.set_flags(BufferFlag::KeyFrame as u32);
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
    decoder_cfg: &Arc<OnceLock<VideoDecoderConfig>>,
    dimensions: Dimensions,
    codec_id: crate::types::VideoCodecId,
) -> Result<(), Error> {
    while let Ok(out) = codec.dequeue_output(0) {
        let out_buf: mediacodec::CodecOutputBuffer = out;
        let info = out_buf.info();
        let flags = info.flags as i32;
        let is_key = BufferFlag::KeyFrame.is_contained_in(flags);
        let ts = Duration::from_micros(info.presentation_time_us as u64);

        if BufferFlag::EndOfStream.is_contained_in(flags) {
            continue;
        }
        if BufferFlag::CodecConfig.is_contained_in(flags) {
            if decoder_cfg.get().is_none() {
                if let Some(slice) = out_buf.buffer_slice() {
                    let data = slice.to_vec();
                    let _ = decoder_cfg.set(VideoDecoderConfig {
                        codec: codec_id.clone(),
                        resolution: Some(Dimensions::new(dimensions.width, dimensions.height)),
                        description: Some(bytes::Bytes::from(data)),
                        hardware_acceleration: None,
                    });
                }
            }
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

fn encode_loop(
    mut codec: MediaCodec,
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd<(VideoFrame, Option<bool>)>>,
    pkt_tx: mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
    queue: Arc<AtomicU32>,
    decoder_cfg: Arc<OnceLock<VideoDecoderConfig>>,
    dimensions: Dimensions,
    codec_id: crate::types::VideoCodecId,
) {
    let mut pending: VecDeque<(VideoFrame, Option<bool>)> = VecDeque::new();

    loop {
        match cmd_rx.blocking_recv() {
            Some(Cmd::Item((frame, keyframe))) => pending.push_back((frame, keyframe)),
            Some(Cmd::Flush(done)) => {
                let res = (|| -> Result<(), Error> {
                    drain_pending_frames(&mut codec, &mut pending, &queue, &pkt_tx)?;
                    cmd::send_eos(&mut codec)?;
                    cmd::drain_until_eos(&mut codec, |out| {
                        let flags = out.info().flags as i32;
                        if BufferFlag::CodecConfig.is_contained_in(flags) {
                            return Ok(());
                        }
                        let is_key = BufferFlag::KeyFrame.is_contained_in(flags);
                        let ts = Duration::from_micros(out.info().presentation_time_us as u64);
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
                        pkt_tx.send(Ok(pkt)).map_err(|_| Error::Dropped)
                    })?;
                    codec.flush().map_err(|e| Error::Platform(format!("{e:?}")))?;
                    Ok(())
                })();
                let _ = done.send(res);
            }
            Some(Cmd::Close) | None => {
                let _ = drain_encoded_output(&mut codec, &pkt_tx, &decoder_cfg, dimensions, codec_id.clone());
                queue.store(0, Ordering::Relaxed);
                return;
            }
        }

        let _ = drain_pending_frames(&mut codec, &mut pending, &queue, &pkt_tx);
        let _ = drain_encoded_output(&mut codec, &pkt_tx, &decoder_cfg, dimensions, codec_id.clone());

        if cmd_rx.is_closed() && pending.is_empty() {
            let _ = drain_encoded_output(&mut codec, &pkt_tx, &decoder_cfg, dimensions, codec_id.clone());
            queue.store(0, Ordering::Relaxed);
            return;
        }
    }
}
