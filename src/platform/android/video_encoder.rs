use std::collections::VecDeque;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU32, Ordering},
};
use std::thread;
use std::time::Duration;

use log::info;
use mediacodec::{BufferFlag, MediaCodec, MediaFormat};
use tokio::sync::{mpsc, oneshot};

use super::cmd::{self, Cmd};
use crate::{
    error::Error,
    traits::{VideoEncoderInput, VideoEncoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, VideoDecoderConfig, VideoEncoderConfig, VideoFrame,
        VideoPlanes,
    },
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
        self.tx
            .send(Cmd::Item((frame, keyframe)))
            .map_err(|_| Error::Dropped)
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
        self.decoder_cfg
            .as_ref()
            .or_else(|| self.decoder_cfg_shared.get())
    }
}

pub fn create(
    config: VideoEncoderConfig,
) -> Result<(AndroidVideoEncoderInput, AndroidVideoEncoderOutput), Error> {
    let mut format =
        MediaFormat::new().map_err(|_| Error::Platform("Failed to create MediaFormat".into()))?;

    let _ = format.set_string("mime", config.codec.to_mime());
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

    info!(
        "encoder format: mime={}, {}x{}, bitrate={:?}, framerate={:?}, i-frame-interval=1, color-format=21",
        config.codec.to_mime(),
        config.dimensions.width,
        config.dimensions.height,
        config.bitrate,
        config.framerate
    );

    let mime = config.codec.to_mime().to_string();
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
    let dims = config.dimensions;
    let codec_id = config.codec.clone();

    thread::spawn(move || {
        encode_loop(codec, cmd_rx, pkt_tx, queue2, decoder_cfg2, dims, codec_id);
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

fn codec_config_to_annexb(data: &[u8]) -> Vec<u8> {
    if data.len() < 4 {
        return data.to_vec();
    }
    if data[..4] == [0x00, 0x00, 0x00, 0x01] || data[..3] == [0x00, 0x00, 0x01] {
        return data.to_vec();
    }
    if data.len() >= 23 && data[0] == 1 {
        let r = parse_hvcc_annexb(data);
        if !r.is_empty() {
            return r;
        }
    }
    if data.len() >= 6 && data[0] == 1 {
        let r = parse_avcc_annexb(data);
        if !r.is_empty() {
            return r;
        }
    }
    data.to_vec()
}

fn parse_avcc_annexb(data: &[u8]) -> Vec<u8> {
    if data.len() < 6 || data[0] != 1 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut pos = 6usize;
    let num_sps = (data[5] & 0x1F) as usize;
    for _ in 0..num_sps {
        if pos + 2 > data.len() {
            break;
        }
        let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        if pos + len > data.len() {
            break;
        }
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        out.extend_from_slice(&data[pos..pos + len]);
        pos += len;
    }
    if pos >= data.len() {
        return out;
    }
    let num_pps = data[pos] as usize;
    pos += 1;
    for _ in 0..num_pps {
        if pos + 2 > data.len() {
            break;
        }
        let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        if pos + len > data.len() {
            break;
        }
        out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        out.extend_from_slice(&data[pos..pos + len]);
        pos += len;
    }
    out
}

fn parse_hvcc_annexb(data: &[u8]) -> Vec<u8> {
    if data.len() < 23 || data[0] != 1 {
        return Vec::new();
    }
    let num_arrays = data[22] as usize;
    let mut out = Vec::new();
    let mut pos = 23usize;
    for _ in 0..num_arrays {
        if pos >= data.len() {
            break;
        }
        pos += 1;
        if pos + 2 > data.len() {
            break;
        }
        let num_nalus = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        for _ in 0..num_nalus {
            if pos + 2 > data.len() {
                break;
            }
            let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
            pos += 2;
            if pos + len > data.len() {
                break;
            }
            out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            out.extend_from_slice(&data[pos..pos + len]);
            pos += len;
        }
    }
    out
}

fn drain_pending_frames(
    codec: &mut MediaCodec,
    pending: &mut VecDeque<(VideoFrame, Option<bool>)>,
    queue: &Arc<AtomicU32>,
    _pkt_tx: &mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
) -> Result<bool, Error> {
    let mut submitted = false;
    while let Some((frame, keyframe)) = pending.pop_front() {
        if let Ok(buf) = codec.dequeue_input(0) {
            let mut buf: mediacodec::CodecInputBuffer = buf;
            let (ptr, cap): (*mut u8, usize) = buf.buffer();
            let data = match &frame.planes {
                VideoPlanes::Cpu(d) => d,
                _ => {
                    return Err(Error::InvalidConfig(
                        "Android encoder requires CPU frames".into(),
                    ));
                }
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
            submitted = true;
        } else {
            pending.push_front((frame, keyframe));
            break;
        }
    }
    Ok(submitted)
}

fn send_encoded_packet(
    pkt_tx: &mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
    info: &mediacodec::BufferInfo,
    payload: &[u8],
    codec_config_pending: &mut Option<Vec<u8>>,
    first_packet_sent: &mut bool,
) -> Result<(), Error> {
    let is_key = BufferFlag::KeyFrame.is_contained_in(info.flags);
    let ts = Duration::from_micros(info.presentation_time_us as u64);

    let mut data = payload.to_vec();
    if !*first_packet_sent {
        *first_packet_sent = true;
        if let Some(config) = &*codec_config_pending {
            let mut combined = Vec::with_capacity(config.len() + data.len());
            combined.extend_from_slice(config);
            combined.extend_from_slice(&data);
            data = combined;
        }
    }

    let pkt = EncodedVideoPacket {
        payload: bytes::Bytes::from(data),
        timestamp: ts,
        keyframe: is_key,
    };
    pkt_tx.send(Ok(pkt)).map_err(|_| Error::Dropped)?;
    Ok(())
}

fn handle_flush(
    codec: &mut MediaCodec,
    pending: &mut VecDeque<(VideoFrame, Option<bool>)>,
    queue: &Arc<AtomicU32>,
    pkt_tx: &mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
    codec_config_pending: &mut Option<Vec<u8>>,
    first_packet_sent: &mut bool,
) -> Result<(), Error> {
    let _ = drain_pending_frames(codec, pending, queue, pkt_tx)?;
    cmd::send_eos(codec)?;
    cmd::drain_until_eos(codec, |out| {
        let flags = out.info().flags;
        if BufferFlag::CodecConfig.is_contained_in(flags) {
            if let Some(slice) = out.buffer_slice() {
                *codec_config_pending = Some(codec_config_to_annexb(slice));
            }
            return Ok(());
        }
        let payload = if let Some(slice) = out.buffer_slice() {
            slice
        } else {
            &[]
        };
        send_encoded_packet(
            pkt_tx,
            out.info(),
            payload,
            codec_config_pending,
            first_packet_sent,
        )
    })?;
    codec
        .flush()
        .map_err(|e| Error::Platform(format!("{e:?}")))?;
    Ok(())
}

fn drain_encoded_output(
    codec: &mut MediaCodec,
    pkt_tx: &mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
    decoder_cfg: &Arc<OnceLock<VideoDecoderConfig>>,
    dimensions: Dimensions,
    codec_id: crate::types::VideoCodecId,
    codec_config_pending: &mut Option<Vec<u8>>,
    first_packet_sent: &mut bool,
) -> Result<bool, Error> {
    use mediacodec::DequeueOutputError;
    let mut had_output = false;
    loop {
        match codec.dequeue_output(0) {
            Ok(out) => {
                let out_buf: mediacodec::CodecOutputBuffer = out;
                let info = out_buf.info();
                let flags = info.flags;

                if BufferFlag::EndOfStream.is_contained_in(flags) {
                    continue;
                }
                if BufferFlag::CodecConfig.is_contained_in(flags) {
                    if let Some(slice) = out_buf.buffer_slice() {
                        let data = slice.to_vec();
                        let annexb = codec_config_to_annexb(&data);
                        info!(
                            "encoder CodecConfig: {} bytes, annexb {} bytes, first_byte=0x{:02x}, is_annexb={}",
                            data.len(),
                            annexb.len(),
                            data.first().copied().unwrap_or(0),
                            (data.len() >= 4
                                && (data[..4] == [0x00, 0x00, 0x00, 0x01]
                                    || data[..3] == [0x00, 0x00, 0x01]))
                        );
                        *codec_config_pending = Some(annexb);
                        if decoder_cfg.get().is_none() {
                            let _ = decoder_cfg.set(VideoDecoderConfig {
                                codec: codec_id.clone(),
                                resolution: Some(Dimensions::new(
                                    dimensions.width,
                                    dimensions.height,
                                )),
                                description: Some(bytes::Bytes::from(data)),
                                hardware_acceleration: None,
                            });
                        }
                    }
                    continue;
                }

                let payload = if let Some(slice) = out_buf.buffer_slice() {
                    slice
                } else {
                    &[]
                };

                had_output = true;
                send_encoded_packet(
                    pkt_tx,
                    &info,
                    payload,
                    codec_config_pending,
                    first_packet_sent,
                )?;
            }
            Err(DequeueOutputError::TryAgainLater) => break,
            Err(DequeueOutputError::OutputFormatChanged)
            | Err(DequeueOutputError::OutputBuffersChanged) => {
                // Normal encoder lifecycle events; format/buffers already refreshed
                // by the wrapper. Continue polling.
            }
            Err(DequeueOutputError::CodecError(e)) => {
                return Err(Error::Platform(format!("codec error: {e:?}")));
            }
        }
    }
    Ok(had_output)
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
    let mut codec_config_pending: Option<Vec<u8>> = None;
    let mut first_packet_sent = false;
    let mut work_pending = false;

    info!("encode_loop started for {}", codec_id.to_mime());

    loop {
        // Block only when no work is pending and nothing queued
        if !work_pending && pending.is_empty() {
            match cmd_rx.blocking_recv() {
                Some(Cmd::Item((frame, keyframe))) => {
                    pending.push_back((frame, keyframe));
                }
                Some(Cmd::Flush(done)) => {
                    info!("encode_loop: flush");
                    let res = handle_flush(
                        &mut codec,
                        &mut pending,
                        &queue,
                        &pkt_tx,
                        &mut codec_config_pending,
                        &mut first_packet_sent,
                    );
                    work_pending = false;
                    let _ = done.send(res);
                }
                Some(Cmd::Close) | None => {
                    info!("encode_loop: close");
                    let _ = drain_encoded_output(
                        &mut codec,
                        &pkt_tx,
                        &decoder_cfg,
                        dimensions,
                        codec_id.clone(),
                        &mut codec_config_pending,
                        &mut first_packet_sent,
                    );
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            }
        }

        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Cmd::Item((frame, keyframe)) => pending.push_back((frame, keyframe)),
                Cmd::Flush(done) => {
                    let res = handle_flush(
                        &mut codec,
                        &mut pending,
                        &queue,
                        &pkt_tx,
                        &mut codec_config_pending,
                        &mut first_packet_sent,
                    );
                    work_pending = false;
                    let _ = done.send(res);
                }
                Cmd::Close => {
                    let _ = drain_encoded_output(
                        &mut codec,
                        &pkt_tx,
                        &decoder_cfg,
                        dimensions,
                        codec_id.clone(),
                        &mut codec_config_pending,
                        &mut first_packet_sent,
                    );
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            }
        }

        let submitted =
            drain_pending_frames(&mut codec, &mut pending, &queue, &pkt_tx).unwrap_or(false);
        if submitted {
            work_pending = true;
        }
        let got_output = drain_encoded_output(
            &mut codec,
            &pkt_tx,
            &decoder_cfg,
            dimensions,
            codec_id.clone(),
            &mut codec_config_pending,
            &mut first_packet_sent,
        )
        .unwrap_or(false);
        if got_output {
            work_pending = false;
        }

        if cmd_rx.is_closed() && pending.is_empty() {
            info!("encode_loop: closed+empty, final drain");
            let _ = drain_encoded_output(
                &mut codec,
                &pkt_tx,
                &decoder_cfg,
                dimensions,
                codec_id.clone(),
                &mut codec_config_pending,
                &mut first_packet_sent,
            );
            queue.store(0, Ordering::Relaxed);
            return;
        }

        if work_pending || !pending.is_empty() {
            thread::sleep(Duration::from_millis(1));
        }
    }
}
