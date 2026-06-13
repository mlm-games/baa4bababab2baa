use std::{
    path::PathBuf,
    sync::Arc,
    sync::atomic::{AtomicU32, Ordering},
    thread,
};

use tokio::sync::{mpsc, oneshot};

use crate::{
    error::Error,
    traits::{VideoDecoderInput, VideoDecoderOutput},
    types::{
        Dimensions, EncodedVideoPacket, PixelFormat, Timestamp, VideoDecoderConfig, VideoFrame,
        VideoPlanes,
    },
};

use cros_codecs::{
    BlockingMode, EncodedFormat, Fourcc,
    decoder::stateless::{DecodeError, DynStatelessVideoDecoder, StatelessDecoder, StatelessVideoDecoder},
    decoder::stateless::{av1::Av1, h264::H264, h265::H265, vp8::Vp8, vp9::Vp9},
    decoder::{DecodedHandle, DecoderEvent, StreamInfo},
    image_processing::nv12_to_i420,
    libva,
    utils::align_up,
    video_frame::{
        UV_PLANE, VideoFrame as CcVideoFrame, Y_PLANE,
        gbm_video_frame::{GbmDevice, GbmUsage},
        generic_dma_video_frame::GenericDmaVideoFrame,
    },
};

enum Cmd {
    Packet(EncodedVideoPacket),
    Flush(oneshot::Sender<Result<(), Error>>),
    Close,
}

pub struct CrosVideoDecoderInput {
    tx: mpsc::UnboundedSender<Cmd>,
    queue: Arc<AtomicU32>,
}

pub struct CrosVideoDecoderOutput {
    rx: mpsc::UnboundedReceiver<Result<VideoFrame, Error>>,
}

impl Drop for CrosVideoDecoderInput {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

impl VideoDecoderInput for CrosVideoDecoderInput {
    fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error> {
        self.queue.fetch_add(1, Ordering::Relaxed);
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
        self.queue.load(Ordering::Relaxed)
    }
}

impl VideoDecoderOutput for CrosVideoDecoderOutput {
    async fn frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.rx.recv().await {
            Some(r) => r.map(Some),
            None => Ok(None),
        }
    }
}

pub fn create(
    config: VideoDecoderConfig,
) -> Result<(CrosVideoDecoderInput, CrosVideoDecoderOutput), Error> {
    let codec_string = config.codec.0;
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Result<VideoFrame, Error>>();
    let queue = Arc::new(AtomicU32::new(0));

    let queue2 = queue.clone();
    thread::spawn(move || worker_loop(cmd_rx, frame_tx, queue2, codec_string));

    Ok((
        CrosVideoDecoderInput { tx: cmd_tx, queue },
        CrosVideoDecoderOutput { rx: frame_rx },
    ))
}

fn make_decoder(
    fmt: EncodedFormat,
) -> Result<DynStatelessVideoDecoder<GenericDmaVideoFrame>, String> {
    match fmt {
        EncodedFormat::H264 => Ok(StatelessDecoder::<H264, _>::new_vaapi(
            libva::Display::open()
                .ok_or("failed to open libva display")?,
            BlockingMode::NonBlocking,
        )
        .map_err(|_| "failed to create H264 decoder")?
        .into_trait_object()),
        EncodedFormat::H265 => Ok(StatelessDecoder::<H265, _>::new_vaapi(
            libva::Display::open()
                .ok_or("failed to open libva display")?,
            BlockingMode::NonBlocking,
        )
        .map_err(|_| "failed to create H265 decoder")?
        .into_trait_object()),
        EncodedFormat::VP8 => Ok(StatelessDecoder::<Vp8, _>::new_vaapi(
            libva::Display::open()
                .ok_or("failed to open libva display")?,
            BlockingMode::NonBlocking,
        )
        .map_err(|_| "failed to create VP8 decoder")?
        .into_trait_object()),
        EncodedFormat::VP9 => Ok(StatelessDecoder::<Vp9, _>::new_vaapi(
            libva::Display::open()
                .ok_or("failed to open libva display")?,
            BlockingMode::NonBlocking,
        )
        .map_err(|_| "failed to create VP9 decoder")?
        .into_trait_object()),
        EncodedFormat::AV1 => Ok(StatelessDecoder::<Av1, _>::new_vaapi(
            libva::Display::open()
                .ok_or("failed to open libva display")?,
            BlockingMode::NonBlocking,
        )
        .map_err(|_| "failed to create AV1 decoder")?
        .into_trait_object()),
    }
}

fn worker_loop(
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    frame_tx: mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    queue: Arc<AtomicU32>,
    codec_string: String,
) {
    let gbm = Arc::new(
        GbmDevice::open(PathBuf::from("/dev/dri/renderD128"))
            .expect("Could not open GBM device (/dev/dri/renderD128)"),
    );

    let mut frame_queue: Vec<GenericDmaVideoFrame> = Vec::new();
    let mut decoder: Option<DynStatelessVideoDecoder<GenericDmaVideoFrame>> = None;

    fn codec_to_fmt(codec: &str) -> Result<EncodedFormat, String> {
        match codec {
            "video/avc" | "video/h264" => Ok(EncodedFormat::H264),
            "video/hevc" | "video/h265" => Ok(EncodedFormat::H265),
            "video/vp8" => Ok(EncodedFormat::VP8),
            "video/vp9" => Ok(EncodedFormat::VP9),
            "video/av01" | "video/av1" => Ok(EncodedFormat::AV1),
            _ => Err(format!("unsupported codec string: {codec}")),
        }
    }

    let gbm2 = Arc::clone(&gbm);
    let mut alloc_frame = move |stream_info: &StreamInfo| -> GenericDmaVideoFrame {
        let gbm_frame = GbmDevice::new_frame(
            Arc::clone(&gbm2),
            Fourcc::from(b"NV12"),
            stream_info.display_resolution,
            stream_info.coded_resolution,
            GbmUsage::Decode,
        )
        .expect("GBM new_frame failed");
        gbm_frame
            .to_generic_dma_video_frame()
            .expect("GBM->DMA export failed")
    };

    loop {
        let Some(cmd) = cmd_rx.blocking_recv() else {
            queue.store(0, Ordering::Relaxed);
            return;
        };

        match cmd {
            Cmd::Close => {
                queue.store(0, Ordering::Relaxed);
                return;
            }

            Cmd::Flush(done) => {
                let res = (|| -> Result<(), Error> {
                    if let Some(dec) = decoder.as_mut() {
                        dec.flush().map_err(|e| Error::Platform(format!("{e:?}")))?;
                        drain_events(dec, &mut frame_queue, &frame_tx, &mut alloc_frame)?;
                    }
                    Ok(())
                })();
                let _ = done.send(res);
            }

            Cmd::Packet(pkt) => {
                queue.fetch_sub(1, Ordering::Relaxed);

                let res = (|| -> Result<(), Error> {
                    let fmt = codec_to_fmt(&codec_string)?;

                    if decoder.is_none() {
                        decoder = Some(make_decoder(fmt)?);
                    }
                    let dec = decoder.as_mut().unwrap();

                    let mut remaining = pkt.payload.as_ref();
                    let ts_us = pkt.timestamp.as_micros() as u64;

                    while !remaining.is_empty() {
                        match dec.decode(ts_us, remaining, &mut || frame_queue.pop()) {
                            Ok(n) => remaining = &remaining[n..],
                            Err(
                                DecodeError::NotEnoughOutputBuffers(_) | DecodeError::CheckEvents,
                            ) => {
                                drain_events(dec, &mut frame_queue, &frame_tx, &mut alloc_frame)?;
                            }
                            Err(e) => return Err(Error::Platform(format!("{e:?}"))),
                        }
                        drain_events(dec, &mut frame_queue, &frame_tx, &mut alloc_frame)?;
                    }

                    Ok(())
                })();

                if let Err(e) = res {
                    let _ = frame_tx.send(Err(e));
                    queue.store(0, Ordering::Relaxed);
                    return;
                }
            }
        }
    }
}

fn drain_events<F: CcVideoFrame + 'static>(
    dec: &mut DynStatelessVideoDecoder<F>,
    frame_queue: &mut Vec<GenericDmaVideoFrame>,
    frame_tx: &mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    alloc_frame: &mut dyn FnMut(&StreamInfo) -> GenericDmaVideoFrame,
) -> Result<(), Error> {
    while let Some(ev) = dec.next_event() {
        match ev {
            DecoderEvent::FormatChanged => {
                if let Some(info) = dec.stream_info() {
                    frame_queue.clear();
                    for _ in 0..info.min_num_frames {
                        frame_queue.push(alloc_frame(info));
                    }
                }
            }
            DecoderEvent::FrameReady(h) => {
                let handle: &dyn DecodedHandle<Frame = F> = &*h;
                handle
                    .sync()
                    .map_err(|e| Error::Platform(format!("{e:?}")))?;
                let ts = Timestamp::from_micros(handle.timestamp());

                let frame = handle.video_frame();
                let out = nv12_frame_to_i420(&*frame, ts)?;
                frame_tx.send(Ok(out)).map_err(|_| Error::Dropped)?;
            }
        }
    }
    Ok(())
}

fn nv12_frame_to_i420<F: CcVideoFrame>(
    frame: &F,
    timestamp: Timestamp,
) -> Result<VideoFrame, Error> {
    let res = frame.resolution();
    let width = res.width as usize;
    let height = res.height as usize;

    let luma_size = res.get_area();
    let chroma_size =
        align_up(width as u32, 2) as usize / 2 * (align_up(height as u32, 2) as usize / 2);

    let mut data = vec![0u8; luma_size + 2 * chroma_size];
    let (dst_y, dst_uv) = data.split_at_mut(luma_size);
    let (dst_u, dst_v) = dst_uv.split_at_mut(chroma_size);

    let pitches = frame.get_plane_pitch();
    let mapping = frame.map().map_err(|e| Error::Platform(format!("{e:?}")))?;
    let planes = mapping.get();

    nv12_to_i420(
        planes[Y_PLANE],
        pitches[Y_PLANE],
        dst_y,
        width,
        planes[UV_PLANE],
        pitches[UV_PLANE],
        dst_u,
        align_up(width as u32, 2) as usize / 2,
        dst_v,
        align_up(width as u32, 2) as usize / 2,
        width,
        height,
    );

    Ok(VideoFrame {
        dimensions: Dimensions {
            width: res.width,
            height: res.height,
        },
        format: PixelFormat::Yuv420p,
        timestamp,
        planes: VideoPlanes::Cpu(data),
    })
}
