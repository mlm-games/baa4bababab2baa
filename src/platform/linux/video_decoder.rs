use std::{path::PathBuf, sync::atomic::{AtomicU32, Ordering}, sync::Arc, thread};

use tokio::sync::{mpsc, oneshot};

use crate::{
	error::Error,
	traits::{VideoDecoderInput, VideoDecoderOutput},
	types::{Dimensions, EncodedVideoPacket, PixelFormat, Timestamp, VideoDecoderConfig, VideoFrame, VideoPlanes},
};

use cros_codecs::{
	decoder::{DecodedHandle, DecoderEvent, StreamInfo},
	decoder::stateless::{DecodeError, StatelessDecoder, StatelessVideoDecoder},
	decoder::stateless::{av1::Av1, h264::H264, h265::H265, vp8::Vp8, vp9::Vp9},
	image_processing::nv12_to_i420,
	utils::align_up,
	video_frame::{
		frame_pool::FramePool,
		generic_dma_video_frame::GenericDmaVideoFrame,
		gbm_video_frame::{GbmDevice, GbmUsage},
		VideoFrame as CcVideoFrame,
		UV_PLANE, Y_PLANE,
	},
	BlockingMode, EncodedFormat, Fourcc,
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

impl VideoDecoderInput for CrosVideoDecoderInput {
	fn decode(&mut self, packet: EncodedVideoPacket) -> Result<(), Error> {
		self.queue.fetch_add(1, Ordering::Relaxed);
		self.tx.send(Cmd::Packet(packet)).map_err(|_| Error::Dropped)
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
	_config: VideoDecoderConfig,
) -> Result<(CrosVideoDecoderInput, CrosVideoDecoderOutput), Error> {
	let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd>();
	let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Result<VideoFrame, Error>>();
	let queue = Arc::new(AtomicU32::new(0));

	let queue2 = queue.clone();
	thread::spawn(move || worker_loop(cmd_rx, frame_tx, queue2));

	Ok((
		CrosVideoDecoderInput { tx: cmd_tx, queue },
		CrosVideoDecoderOutput { rx: frame_rx },
	))
}

fn worker_loop(
	mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
	frame_tx: mpsc::UnboundedSender<Result<VideoFrame, Error>>,
	queue: Arc<AtomicU32>,
) {
	let gbm = Arc::new(
		GbmDevice::open(PathBuf::from("/dev/dri/renderD128"))
			.expect("Could not open GBM device (/dev/dri/renderD128)"),
	);

	let mut pool = FramePool::new({
		let gbm = gbm.clone();
		move |stream_info: &StreamInfo| {
			gbm.new_frame(
				Fourcc::from(b"NV12"),
				stream_info.display_resolution,
				stream_info.coded_resolution,
				GbmUsage::Decode,
			)
			.expect("GBM new_frame failed")
			.to_generic_dma_video_frame()
			.expect("GBM->DMA export failed")
		}
	});

	let mut alloc = || pool.alloc();

	let mut decoder_h264 = None::<cros_codecs::decoder::stateless::DynStatelessVideoDecoder<_>>;
	let mut decoder_h265 = None::<cros_codecs::decoder::stateless::DynStatelessVideoDecoder<_>>;
	let mut decoder_vp8  = None::<cros_codecs::decoder::stateless::DynStatelessVideoDecoder<_>>;
	let mut decoder_vp9  = None::<cros_codecs::decoder::stateless::DynStatelessVideoDecoder<_>>;
	let mut decoder_av1  = None::<cros_codecs::decoder::stateless::DynStatelessVideoDecoder<_>>;

	let mut get_decoder = |codec: &str| -> Result<&mut cros_codecs::decoder::stateless::DynStatelessVideoDecoder<_>, String> {
		let mk = |fmt: EncodedFormat| -> Result<cros_codecs::decoder::stateless::DynStatelessVideoDecoder<_>, String> {
			match fmt {
				EncodedFormat::H264 => StatelessDecoder::<H264, _>::new_vaapi(
					libva::Display::open().ok_or("failed to open libva display")?.into(),
					BlockingMode::NonBlocking,
				).map_err(|_| "failed to create H264 decoder")?.into_trait_object().into(),
				EncodedFormat::H265 => StatelessDecoder::<H265, _>::new_vaapi(
					libva::Display::open().ok_or("failed to open libva display")?.into(),
					BlockingMode::NonBlocking,
				).map_err(|_| "failed to create H265 decoder")?.into_trait_object().into(),
				EncodedFormat::VP8 => StatelessDecoder::<Vp8, _>::new_vaapi(
					libva::Display::open().ok_or("failed to open libva display")?.into(),
					BlockingMode::NonBlocking,
				).map_err(|_| "failed to create VP8 decoder")?.into_trait_object().into(),
				EncodedFormat::VP9 => StatelessDecoder::<Vp9, _>::new_vaapi(
					libva::Display::open().ok_or("failed to open libva display")?.into(),
					BlockingMode::NonBlocking,
				).map_err(|_| "failed to create VP9 decoder")?.into_trait_object().into(),
				EncodedFormat::AV1 => StatelessDecoder::<Av1, _>::new_vaapi(
					libva::Display::open().ok_or("failed to open libva display")?.into(),
					BlockingMode::NonBlocking,
				).map_err(|_| "failed to create AV1 decoder")?.into_trait_object().into(),
			}
		};

		let fmt = match codec {
			"video/avc" | "video/h264" => EncodedFormat::H264,
			"video/hevc" | "video/h265" => EncodedFormat::H265,
			"video/vp8" => EncodedFormat::VP8,
			"video/vp9" => EncodedFormat::VP9,
			"video/av01" | "video/av1" => EncodedFormat::AV1,
			_ => return Err(format!("unsupported codec string: {codec}")),
		};

		Ok(match fmt {
			EncodedFormat::H264 => decoder_h264.get_or_insert(mk(fmt)?),
			EncodedFormat::H265 => decoder_h265.get_or_insert(mk(fmt)?),
			EncodedFormat::VP8  => decoder_vp8.get_or_insert(mk(fmt)?),
			EncodedFormat::VP9  => decoder_vp9.get_or_insert(mk(fmt)?),
			EncodedFormat::AV1  => decoder_av1.get_or_insert(mk(fmt)?),
		})
	};

	let codec_string = "video/avc";

	loop {
		let Some(cmd) = cmd_rx.blocking_recv() else {
			return;
		};

		match cmd {
			Cmd::Close => return,

			Cmd::Flush(done) => {
				let res = (|| {
					let dec = get_decoder(codec_string).map_err(Error::Platform)?;
					dec.flush().map_err(|e| Error::Platform(format!("{e:?}")))?;
					drain_events(dec, &mut pool, &frame_tx)?;
					Ok(())
				})();
				let _ = done.send(res);
			}

			Cmd::Packet(pkt) => {
				queue.fetch_sub(1, Ordering::Relaxed);

				let res = (|| {
					let dec = get_decoder(codec_string).map_err(Error::Platform)?;

					let mut remaining = pkt.payload.as_ref();
					let ts_us = pkt.timestamp.as_micros() as u64;

					while !remaining.is_empty() {
						match dec.decode(ts_us, remaining, &mut alloc) {
							Ok(n) => remaining = &remaining[n..],
							Err(DecodeError::NotEnoughOutputBuffers(_) | DecodeError::CheckEvents) => {
								drain_events(dec, &mut pool, &frame_tx)?;
							}
							Err(e) => return Err(Error::Platform(format!("{e:?}"))),
						}
						drain_events(dec, &mut pool, &frame_tx)?;
					}

					Ok(())
				})();

				if let Err(e) = res {
					let _ = frame_tx.send(Err(e));
					return;
				}
			}
		}
	}
}

fn drain_events<F: CcVideoFrame + 'static>(
	dec: &mut dyn StatelessVideoDecoder<Handle = Box<dyn DecodedHandle<Frame = F>>>,
	pool: &mut FramePool<GenericDmaVideoFrame>,
	frame_tx: &mpsc::UnboundedSender<Result<VideoFrame, Error>>,
) -> Result<(), Error> {
	while let Some(ev) = dec.next_event() {
		match ev {
			DecoderEvent::FormatChanged => {
				if let Some(info) = dec.stream_info() {
					pool.resize(info);
				}
			}
			DecoderEvent::FrameReady(handle) => {
				handle.sync().map_err(|e| Error::Platform(format!("{e:?}")))?;
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
	let chroma_size = align_up(width as u32, 2) as usize / 2 * (align_up(height as u32, 2) as usize / 2);

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
		dimensions: Dimensions { width: res.width, height: res.height },
		format: PixelFormat::Yuv420p,
		timestamp,
		planes: VideoPlanes::Cpu(data),
	})
}