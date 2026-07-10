use std::{
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
    BlockingMode, EncodedFormat, Fourcc, FrameLayout, PlaneLayout, Resolution,
    decoder::stateless::{
        DecodeError, DynStatelessVideoDecoder, StatelessDecoder, StatelessVideoDecoder,
    },
    decoder::stateless::{av1::Av1, h264::H264, h265::H265, vp8::Vp8, vp9::Vp9},
    decoder::{DecodedHandle, DecoderEvent, StreamInfo},
    image_processing::nv12_to_i420,
    libva,
    utils::align_up,
    video_frame::{
        UV_PLANE, VideoFrame as CcVideoFrame, Y_PLANE,
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

    fn try_frame(&mut self) -> Result<Option<VideoFrame>, Error> {
        match self.rx.try_recv() {
            Ok(Ok(frame)) => Ok(Some(frame)),
            Ok(Err(e)) => Err(e),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => Err(Error::Dropped),
        }
    }
}

pub fn create(
    config: VideoDecoderConfig,
) -> Result<(CrosVideoDecoderInput, CrosVideoDecoderOutput), Error> {
    let codec = config.codec.clone();
    let fmt = codec_to_fmt(&codec).map_err(|e| Error::Platform(e))?;

    // Open VA display and validate codec support synchronously (for easier fallback)
    let va_display = libva::Display::open()
        .ok_or_else(|| Error::Platform("Could not open VA display".into()))?;
    let profiles = va_display
        .query_config_profiles()
        .map_err(|e| Error::Platform(format!("VA profiles: {e}")))?;
    check_codec_supported(fmt, &profiles).map_err(|e| Error::Platform(e))?;

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Result<VideoFrame, Error>>();
    let queue = Arc::new(AtomicU32::new(0));

    let queue2 = queue.clone();
    thread::spawn(move || worker_loop(cmd_rx, frame_tx, queue2, codec, va_display));

    Ok((
        CrosVideoDecoderInput { tx: cmd_tx, queue },
        CrosVideoDecoderOutput { rx: frame_rx },
    ))
}

fn check_codec_supported(
    fmt: EncodedFormat,
    profiles: &[libva::VAProfile::Type],
) -> Result<(), String> {
    use libva::VAProfile;
    match fmt {
        EncodedFormat::H265 => {
            // HEVC has multiple profiles (Main, Main10, REXT, SCC); check if the
            // driver supports at least one of the common decode profiles.
            if !profiles.iter().any(|p| {
                matches!(
                    *p,
                    VAProfile::VAProfileHEVCMain
                        | VAProfile::VAProfileHEVCMain10
                        | VAProfile::VAProfileHEVCMain12
                        | VAProfile::VAProfileHEVCMain422_10
                        | VAProfile::VAProfileHEVCMain422_12
                        | VAProfile::VAProfileHEVCMain444
                        | VAProfile::VAProfileHEVCMain444_10
                        | VAProfile::VAProfileHEVCMain444_12
                        | VAProfile::VAProfileHEVCSccMain
                        | VAProfile::VAProfileHEVCSccMain10
                        | VAProfile::VAProfileHEVCSccMain444
                        | VAProfile::VAProfileHEVCSccMain444_10
                )
            }) {
                return Err(format!("{fmt:?} not supported by VA driver"));
            }
        }
        _ => {
            let profile = match fmt {
                EncodedFormat::H264 => VAProfile::VAProfileH264Main,
                EncodedFormat::VP8 => VAProfile::VAProfileVP8Version0_3,
                EncodedFormat::VP9 => VAProfile::VAProfileVP9Profile0,
                EncodedFormat::AV1 => VAProfile::VAProfileAV1Profile0,
                _ => return Err(format!("{fmt:?} not supported by VA driver")),
            };
            if !profiles.contains(&profile) {
                return Err(format!("{fmt:?} not supported by VA driver"));
            }
        }
    }
    Ok(())
}

fn make_decoder(
    fmt: EncodedFormat,
    display: &Arc<libva::Display>,
) -> Result<DynStatelessVideoDecoder<GenericDmaVideoFrame>, String> {
    match fmt {
        EncodedFormat::H264 => Ok(StatelessDecoder::<H264, _>::new_vaapi(
            display.clone(),
            BlockingMode::NonBlocking,
        )
        .map_err(|e| format!("failed to create H264 decoder: {e}"))?
        .into_trait_object()),
        EncodedFormat::H265 => Ok(StatelessDecoder::<H265, _>::new_vaapi(
            display.clone(),
            BlockingMode::NonBlocking,
        )
        .map_err(|e| format!("failed to create H265 decoder: {e}"))?
        .into_trait_object()),
        EncodedFormat::VP8 => Ok(StatelessDecoder::<Vp8, _>::new_vaapi(
            display.clone(),
            BlockingMode::NonBlocking,
        )
        .map_err(|e| format!("failed to create VP8 decoder: {e}"))?
        .into_trait_object()),
        EncodedFormat::VP9 => Ok(StatelessDecoder::<Vp9, _>::new_vaapi(
            display.clone(),
            BlockingMode::NonBlocking,
        )
        .map_err(|e| format!("failed to create VP9 decoder: {e}"))?
        .into_trait_object()),
        EncodedFormat::AV1 => Ok(StatelessDecoder::<Av1, _>::new_vaapi(
            display.clone(),
            BlockingMode::NonBlocking,
        )
        .map_err(|e| format!("failed to create AV1 decoder: {e}"))?
        .into_trait_object()),
        _ => Err(format!("unsupported codec: {fmt:?}")),
    }
}

fn codec_to_fmt(codec: &crate::types::VideoCodecId) -> Result<EncodedFormat, String> {
    match codec {
        crate::types::VideoCodecId::H264 { .. } => Ok(EncodedFormat::H264),
        crate::types::VideoCodecId::Hevc => Ok(EncodedFormat::H265),
        crate::types::VideoCodecId::Vp8 => Ok(EncodedFormat::VP8),
        crate::types::VideoCodecId::Vp9 => Ok(EncodedFormat::VP9),
        crate::types::VideoCodecId::Av1 => Ok(EncodedFormat::AV1),
        crate::types::VideoCodecId::Other(s) => Err(format!("unsupported codec: {s}")),
    }
}

fn worker_loop(
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    frame_tx: mpsc::UnboundedSender<Result<VideoFrame, Error>>,
    queue: Arc<AtomicU32>,
    codec: crate::types::VideoCodecId,
    va_display: Arc<libva::Display>,
) {
    let mut frame_queue: Vec<GenericDmaVideoFrame> = Vec::new();
    let mut decoder: Option<DynStatelessVideoDecoder<GenericDmaVideoFrame>> = None;
    let mut have_cache = false;
    let mut cached_w = 0u32;
    let mut cached_h = 0u32;
    let mut cached_display = Resolution {
        width: 0,
        height: 0,
    };
    let mut cached_format = cros_codecs::DecodedFormat::NV12;

    let va_display_clone = va_display.clone();
    let mut alloc_frame = move |stream_info: &StreamInfo| -> Result<GenericDmaVideoFrame, Error> {
        create_video_frame(
            stream_info.coded_resolution.width,
            stream_info.coded_resolution.height,
            stream_info.format,
            stream_info.display_resolution,
            &va_display_clone,
        )
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
                        let _ = drain_events(
                            dec,
                            &mut frame_queue,
                            &frame_tx,
                            &mut alloc_frame,
                            &va_display,
                            &mut have_cache,
                            &mut cached_w,
                            &mut cached_h,
                            &mut cached_display,
                            &mut cached_format,
                        )?;
                    }
                    Ok(())
                })();
                let _ = done.send(res);
            }

            Cmd::Packet(pkt) => {
                queue.fetch_sub(1, Ordering::Relaxed);

                let res = (|| -> Result<(), Error> {
                    let fmt = codec_to_fmt(&codec)?;

                    if decoder.is_none() {
                        decoder = Some(make_decoder(fmt, &va_display)?);
                    }
                    let dec = decoder.as_mut().unwrap();

                    let mut remaining = pkt.payload.as_ref();
                    let ts_us = pkt.timestamp.as_micros() as u64;

                    let mut iter = 0u64;
                    let mut no_progress_count = 0u64;
                    while !remaining.is_empty() {
                        iter += 1;
                        if iter > 1000 {
                            eprintln!("[VAAPI] loop limit 1000 reached, aborting");
                            return Err(Error::Platform("decode loop limit".into()));
                        }
                        match dec.decode(ts_us, remaining, &mut || frame_queue.pop()) {
                            Ok(n) => {
                                remaining = &remaining[n..];
                                no_progress_count = 0;
                            }
                            Err(
                                DecodeError::NotEnoughOutputBuffers(_) | DecodeError::CheckEvents,
                            ) => {
                                no_progress_count += 1;
                                if no_progress_count > 50 {
                                    return Err(Error::Platform("no decode progress".into()));
                                }
                                // Back off to avoid tight spin
                                thread::sleep(std::time::Duration::from_micros(100));
                            }
                            Err(e) => return Err(Error::Platform(format!("{e:?}"))),
                        }
                        let _ = drain_events(
                            dec,
                            &mut frame_queue,
                            &frame_tx,
                            &mut alloc_frame,
                            &va_display,
                            &mut have_cache,
                            &mut cached_w,
                            &mut cached_h,
                            &mut cached_display,
                            &mut cached_format,
                        )?;
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
    alloc_frame: &mut dyn FnMut(&StreamInfo) -> Result<GenericDmaVideoFrame, Error>,
    va_display: &Arc<libva::Display>,
    have_cache: &mut bool,
    cached_w: &mut u32,
    cached_h: &mut u32,
    cached_display: &mut Resolution,
    cached_format: &mut cros_codecs::DecodedFormat,
) -> Result<usize, Error> {
    let mut count = 0;
    while let Some(ev) = dec.next_event() {
        match ev {
            DecoderEvent::FormatChanged => {
                if let Some(info) = dec.stream_info() {
                    *cached_w = info.coded_resolution.width;
                    *cached_h = info.coded_resolution.height;
                    *cached_display = info.display_resolution;
                    *cached_format = info.format;
                    *have_cache = true;
                    frame_queue.clear();
                    for _ in 0..info.min_num_frames {
                        frame_queue.push(alloc_frame(info)?);
                    }
                }
            }
            DecoderEvent::FrameReady(h) => {
                let handle: &dyn DecodedHandle<Frame = F> = &*h;
                handle
                    .sync()
                    .map_err(|e| Error::Platform(format!("{e:?}")))?;
                let ts = Timestamp::from_micros(handle.timestamp());

                let frame_arc = handle.video_frame();
                let out = match *cached_format {
                    cros_codecs::DecodedFormat::I010
                    | cros_codecs::DecodedFormat::I210
                    | cros_codecs::DecodedFormat::I410 => p010_frame_to_i010(&*frame_arc, ts),
                    _ => nv12_frame_to_i420(&*frame_arc, ts).or_else(|e| {
                        eprintln!(
                            "[VAAPI] nv12_frame_to_i420 failed: {e:?}, trying VA-API fallback"
                        );
                        nv12_frame_to_i420_via_vaapi(&*frame_arc, ts, va_display)
                    }),
                }
                .map_err(|e| {
                    eprintln!("[VAAPI] drain_events FrameReady failed: {e:?}");
                    e
                })?;
                frame_tx.send(Ok(out)).map_err(|_| Error::Dropped)?;
                count += 1;

                if *have_cache {
                    frame_queue.push(create_video_frame(
                        *cached_w,
                        *cached_h,
                        *cached_format,
                        *cached_display,
                        va_display,
                    )?);
                }
            }
        }
    }
    Ok(count)
}

fn rt_format_from_format(format: cros_codecs::DecodedFormat) -> u32 {
    use cros_codecs::DecodedFormat;
    match format {
        DecodedFormat::NV12 | DecodedFormat::I420 => libva::VA_RT_FORMAT_YUV420,
        DecodedFormat::I422 => libva::VA_RT_FORMAT_YUV422,
        DecodedFormat::I444 => libva::VA_RT_FORMAT_YUV444,
        DecodedFormat::I010 => libva::VA_RT_FORMAT_YUV420_10,
        DecodedFormat::I012 => libva::VA_RT_FORMAT_YUV420_12,
        DecodedFormat::I210 => libva::VA_RT_FORMAT_YUV422_10,
        DecodedFormat::I212 => libva::VA_RT_FORMAT_YUV422_12,
        DecodedFormat::I410 => libva::VA_RT_FORMAT_YUV444_10,
        DecodedFormat::I412 => libva::VA_RT_FORMAT_YUV444_12,
        DecodedFormat::MM21 => libva::VA_RT_FORMAT_YUV420,
        _ => libva::VA_RT_FORMAT_YUV420,
    }
}

fn create_video_frame(
    w: u32,
    h: u32,
    format: cros_codecs::DecodedFormat,
    display_resolution: Resolution,
    va_display: &Arc<libva::Display>,
) -> Result<GenericDmaVideoFrame, Error> {
    let rt_format = rt_format_from_format(format);
    let surfaces = va_display
        .create_surfaces::<()>(
            rt_format,
            Some(u32::from(Fourcc::from(b"NV12"))),
            w,
            h,
            Some(libva::UsageHint::USAGE_HINT_DECODER),
            vec![()],
        )
        .map_err(|e| Error::Platform(format!("VA surface allocation failed: {e:?}")))?;
    let surface = surfaces
        .into_iter()
        .next()
        .ok_or_else(|| Error::Platform("VA surface allocation returned zero surfaces".into()))?;
    let desc = surface
        .export_prime()
        .map_err(|e| Error::Platform(format!("VA surface export failed: {e:?}")))?;
    let layer = &desc.layers[0];
    let planes: Vec<_> = (0..layer.num_planes as usize)
        .map(|i| PlaneLayout {
            buffer_index: layer.object_index[i] as usize,
            offset: layer.offset[i] as usize,
            stride: layer.pitch[i] as usize,
        })
        .collect();
    let mut dma_handles = Vec::new();
    for obj in &desc.objects {
        dma_handles.push(std::fs::File::from(
            obj.fd
                .try_clone()
                .map_err(|e| Error::Platform(format!("FD clone failed: {e}")))?,
        ));
    }
    GenericDmaVideoFrame::new(
        dma_handles,
        FrameLayout {
            format: (
                Fourcc::from(desc.fourcc),
                desc.objects[0].drm_format_modifier,
            ),
            size: display_resolution,
            planes,
        },
    )
    .map_err(|e| Error::Platform(format!("GenericDmaVideoFrame construction failed: {e:?}")))
}

fn nv12_frame_to_i420_via_vaapi<F: 'static + CcVideoFrame>(
    frame: &F,
    timestamp: Timestamp,
    display: &Arc<libva::Display>,
) -> Result<VideoFrame, Error> {
    let gdma = (frame as &dyn std::any::Any)
        .downcast_ref::<GenericDmaVideoFrame>()
        .ok_or_else(|| Error::Platform("not a GenericDmaVideoFrame".into()))?;

    let res = gdma.resolution();
    let width = res.width as usize;
    let height = res.height as usize;

    let luma_size = res.get_area();
    let chroma_size =
        align_up(width as u32, 2) as usize / 2 * (align_up(height as u32, 2) as usize / 2);
    let mut data = vec![0u8; luma_size + 2 * chroma_size];
    let (dst_y, dst_uv) = data.split_at_mut(luma_size);
    let (dst_u, dst_v) = dst_uv.split_at_mut(chroma_size);

    let surface = gdma.to_native_handle(display).map_err(|e| {
        let msg = format!("to_native_handle for VA-API readback: {e}");
        eprintln!("[VAAPI] {msg}");
        Error::Platform(msg)
    })?;

    let mut format: libva::VAImageFormat = unsafe { std::mem::zeroed() };
    format.fourcc = u32::from(Fourcc::from(b"NV12"));
    let image = libva::Image::create_from(
        &surface,
        format,
        (res.width, res.height),
        (res.width, res.height),
    )
    .map_err(|e| {
        let msg = format!("Image::create_from: {e:?}");
        eprintln!("[VAAPI] {msg}");
        Error::Platform(msg)
    })?;

    let va_image = image.image();
    let luma_stride = va_image.pitches[0] as usize;
    let chroma_stride = va_image.pitches[1] as usize;
    let y_off = va_image.offsets[0] as usize;
    let uv_off = va_image.offsets[1] as usize;
    let raw = image.as_ref();

    let chroma_height = align_up(height as u32, 2) as usize / 2;
    if y_off + luma_stride * height > raw.len()
        || uv_off + chroma_stride * chroma_height > raw.len()
    {
        return Err(Error::Platform("derived image data too small".into()));
    }

    nv12_to_i420(
        &raw[y_off..],
        luma_stride,
        dst_y,
        width,
        &raw[uv_off..],
        chroma_stride,
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

fn p010_frame_to_i010<F: CcVideoFrame>(
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

    let src_y = planes[Y_PLANE];
    let src_uv = planes[UV_PLANE];
    let y_pitch = pitches[Y_PLANE];
    let uv_pitch = pitches[UV_PLANE];

    // P010 stores 10-bit samples in 16-bit words (little-endian).  The high
    // byte of each word is a reasonable 8-bit approximation (≙ >> 2 for
    // high-10 convention, ≙ >> 8 for low-10 convention).
    for y in 0..height {
        for x in 0..width {
            dst_y[y * width + x] = src_y[y * y_pitch + x * 2 + 1];
        }
    }

    let chroma_h = align_up(height as u32, 2) as usize / 2;
    let chroma_w = align_up(width as u32, 2) as usize;
    for y in 0..chroma_h {
        for x in 0..chroma_w / 2 {
            let off = y * uv_pitch + x * 4;
            dst_u[y * (chroma_w / 2) + x] = src_uv[off + 1]; // U high byte
            dst_v[y * (chroma_w / 2) + x] = src_uv[off + 3]; // V high byte
        }
    }

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
