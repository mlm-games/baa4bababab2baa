use std::{
	borrow::Borrow,
	collections::VecDeque,
	rc::Rc,
	sync::{
		atomic::{AtomicU32, Ordering},
		Arc,
	},
	thread,
	time::Duration,
};

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};

use crate::{
	error::Error,
	traits::{VideoEncoderInput, VideoEncoderOutput},
	types::{
		Dimensions, EncodedVideoPacket, PixelFormat, Timestamp, VideoDecoderConfig, VideoEncoderConfig,
		VideoFrame, VideoPlanes,
	},
};

use cros_codecs::{
	encoder::{self, VideoEncoder as CcVideoEncoder},
	FrameLayout, PlaneLayout, Resolution, Fourcc, BlockingMode,
};

use libva::{
	constants::{VA_FOURCC_NV12, VA_RT_FORMAT_YUV420},
	Display, Image, Surface, UsageHint, VAImageFormat,
};

enum Cmd {
	Encode(VideoFrame, Option<bool>),
	Flush(oneshot::Sender<Result<(), Error>>),
	Close,
}

pub struct CrosVideoEncoderInput {
	pub config: VideoEncoderConfig,
	tx: mpsc::UnboundedSender<Cmd>,
	queue: Arc<AtomicU32>,
}

pub struct CrosVideoEncoderOutput {
	rx: mpsc::UnboundedReceiver<Result<EncodedVideoPacket, Error>>,
	decoder_cfg: Option<VideoDecoderConfig>,
}

impl VideoEncoderInput for CrosVideoEncoderInput {
	fn encode(&mut self, frame: VideoFrame, keyframe: Option<bool>) -> Result<(), Error> {
		self.queue.fetch_add(1, Ordering::Relaxed);
		self.tx.send(Cmd::Encode(frame, keyframe)).map_err(|_| Error::Dropped)
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

impl VideoEncoderOutput for CrosVideoEncoderOutput {
	async fn packet(&mut self) -> Result<Option<EncodedVideoPacket>, Error> {
		match self.rx.recv().await {
			Some(r) => r.map(Some),
			None => Ok(None),
		}
	}

	fn decoder_config(&self) -> Option<&VideoDecoderConfig> {
		self.decoder_cfg.as_ref()
	}
}

pub fn create(
	config: VideoEncoderConfig,
) -> Result<(CrosVideoEncoderInput, CrosVideoEncoderOutput), Error> {
	let codec = config.codec.0.as_str();
	if !matches!(codec, "video/avc" | "video/h264" | "video/vp9" | "video/av01" | "video/av1") {
		return Err(Error::Unsupported);
	}

	let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
	let (pkt_tx, pkt_rx) = mpsc::unbounded_channel();

	let queue = Arc::new(AtomicU32::new(0));
	let queue2 = queue.clone();

	thread::spawn(move || worker_loop(config, cmd_rx, pkt_tx, queue2));

	Ok((
		CrosVideoEncoderInput { config, tx: cmd_tx, queue },
		CrosVideoEncoderOutput {
			rx: pkt_rx,
			decoder_cfg: None,
		},
	))
}

fn worker_loop(
	config: VideoEncoderConfig,
	mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
	pkt_tx: mpsc::UnboundedSender<Result<EncodedVideoPacket, Error>>,
	queue: Arc<AtomicU32>,
) {
	let display = match Display::open() {
		Ok(d) => Rc::new(d),
		Err(e) => {
			let _ = pkt_tx.send(Err(Error::Platform(format!("VAAPI Display::open failed: {e:?}"))));
			return;
		}
	};

	let nv12_fmt = match display.query_image_formats() {
		Ok(fmts) => fmts.into_iter().find(|f| f.fourcc == VA_FOURCC_NV12),
		Err(e) => {
			let _ = pkt_tx.send(Err(Error::Platform(format!("query_image_formats failed: {e:?}"))));
			return;
		}
	};
	let Some(nv12_fmt) = nv12_fmt else {
		let _ = pkt_tx.send(Err(Error::Platform(
			"VAAPI driver does not expose NV12 mapping format".into(),
		)));
		return;
	};

	let width = config.dimensions.width;
	let height = config.dimensions.height;

	if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
		let _ = pkt_tx.send(Err(Error::InvalidConfig(
			"dimensions must be non-zero and even (for NV12 4:2:0)".into(),
		)));
		return;
	}

	let coded_size = Resolution { width, height };
	let input_fourcc = Fourcc::from(b"NV12");

	let mut encoder = match create_vaapi_encoder(&display, &config, input_fourcc, coded_size) {
		Ok(e) => e,
		Err(e) => {
			let _ = pkt_tx.send(Err(e));
			return;
		}
	};

	let mut pool = cros_codecs::backend::vaapi::surface_pool::VaSurfacePool::new(
		Rc::clone(&display),
		VA_RT_FORMAT_YUV420,
		Some(UsageHint::USAGE_HINT_ENCODER),
		coded_size,
	);

	if let Err(e) = pool.add_frames(vec![(); 16]) {
		let _ = pkt_tx.send(Err(Error::Platform(format!("create VA surfaces failed: {e:?}"))));
		return;
	}

	let frame_layout = FrameLayout {
		format: (input_fourcc, 0),
		size: coded_size,
		planes: vec![
			PlaneLayout { buffer_index: 0, offset: 0, stride: width as usize },
			PlaneLayout {
				buffer_index: 0,
				offset: (width * height) as usize,
				stride: width as usize,
			},
		],
	};

	let mut pending: VecDeque<(VideoFrame, bool)> = VecDeque::new();
	let mut flushing: Option<oneshot::Sender<Result<(), Error>>> = None;

	loop {
		while let Ok(cmd) = cmd_rx.try_recv() {
			match cmd {
				Cmd::Encode(frame, keyopt) => {
					queue.fetch_sub(1, Ordering::Relaxed);
					let force_keyframe = keyopt.unwrap_or(false);
					pending.push_back((frame, force_keyframe));
				}
				Cmd::Flush(done) => {
					flushing = Some(done);
				}
				Cmd::Close => return,
			}
		}

		while let Some((frame, force_keyframe)) = pending.pop_front() {
			let Some(handle) = pool.get_surface() else {
				pending.push_front((frame, force_keyframe));
				break;
			};

			let surface: &Surface<()> = handle.borrow();

			let nv12 = match to_nv12_bytes(&frame) {
				Ok(v) => v,
				Err(e) => {
					let _ = pkt_tx.send(Err(e));
					return;
				}
			};

			if let Err(e) = upload_nv12(&display, &nv12_fmt, surface, width, height, &nv12) {
				let _ = pkt_tx.send(Err(e));
				return;
			}

			let meta = encoder::FrameMetadata {
				timestamp: frame.timestamp.as_micros() as u64,
				layout: frame_layout.clone(),
				force_keyframe,
			};

			if let Err(e) = encoder.encode(meta, handle) {
				let _ = pkt_tx.send(Err(Error::Platform(format!("encode failed: {e}"))));
				return;
			}
		}

		if let Some(done) = flushing.take() {
			let res = (|| -> Result<(), Error> {
				encoder.drain().map_err(|e| Error::Platform(format!("drain failed: {e}")))?;
				Ok(())
			})();
			let _ = done.send(res);
		}

		loop {
			let coded = match encoder.poll() {
				Ok(c) => c,
				Err(e) => {
					let _ = pkt_tx.send(Err(Error::Platform(format!("poll failed: {e}"))));
					return;
				}
			};

			let Some(coded) = coded else {
				break;
			};

			let ts = Duration::from_micros(coded.metadata.timestamp);
			let keyframe = coded.metadata.force_keyframe;

			let pkt = EncodedVideoPacket {
				payload: Bytes::from(coded.bitstream),
				timestamp: ts,
				keyframe,
			};

			if pkt_tx.send(Ok(pkt)).is_err() {
				return;
			}
		}

		if cmd_rx.is_closed() && pending.is_empty() {
			return;
		}

		thread::sleep(Duration::from_millis(1));
	}
}

fn create_vaapi_encoder(
	display: &Rc<Display>,
	config: &VideoEncoderConfig,
	fourcc: Fourcc,
	coded_size: Resolution,
) -> Result<Box<dyn CcVideoEncoder<cros_codecs::backend::vaapi::surface_pool::PooledVaSurface<()>>>, Error> {
	let bitrate = config.bitrate.unwrap_or(1_200_000) as u64;
	let framerate = config.framerate.unwrap_or(30.0);

	let low_power = false;

	match config.codec.0.as_str() {
		"video/avc" | "video/h264" => {
			use cros_codecs::codec::h264::parser::{Level, Profile};
			use cros_codecs::encoder::{RateControl, Tunings};

			let cfg = cros_codecs::encoder::h264::EncoderConfig {
				resolution: coded_size,
				profile: Profile::Main,
				level: Level::L4,
				initial_tunings: Tunings {
					rate_control: RateControl::ConstantBitrate(bitrate),
					framerate,
					..Default::default()
				},
				..Default::default()
			};

			let enc = cros_codecs::encoder::stateless::h264::StatelessEncoder::new_vaapi(
				Rc::clone(display),
				cfg,
				fourcc,
				coded_size,
				low_power,
				BlockingMode::NonBlocking,
			)
			.map_err(|e| Error::Platform(format!("create h264 encoder failed: {e}")))?;

			Ok(Box::new(enc))
		}

		"video/vp9" => {
			use cros_codecs::encoder::{RateControl, Tunings};

			let cfg = cros_codecs::encoder::vp9::EncoderConfig {
				resolution: coded_size,
				initial_tunings: Tunings {
					rate_control: RateControl::ConstantBitrate(bitrate),
					framerate,
					..Default::default()
				},
				..Default::default()
			};

			let enc = cros_codecs::encoder::stateless::vp9::StatelessEncoder::new_vaapi(
				Rc::clone(display),
				cfg,
				fourcc,
				coded_size,
				low_power,
				BlockingMode::NonBlocking,
			)
			.map_err(|e| Error::Platform(format!("create vp9 encoder failed: {e}")))?;

			Ok(Box::new(enc))
		}

		"video/av01" | "video/av1" => {
			use cros_codecs::encoder::{RateControl, Tunings};

			let cfg = cros_codecs::encoder::av1::EncoderConfig {
				resolution: coded_size,
				initial_tunings: Tunings {
					rate_control: RateControl::ConstantBitrate(bitrate),
					framerate,
					..Default::default()
				},
				..Default::default()
			};

			let enc = cros_codecs::encoder::stateless::av1::StatelessEncoder::new_vaapi(
				Rc::clone(display),
				cfg,
				fourcc,
				coded_size,
				low_power,
				BlockingMode::NonBlocking,
			)
			.map_err(|e| Error::Platform(format!("create av1 encoder failed: {e}")))?;

			Ok(Box::new(enc))
		}

		_ => Err(Error::Unsupported),
	}
}

fn to_nv12_bytes(frame: &VideoFrame) -> Result<Vec<u8>, Error> {
	let Dimensions { width, height } = frame.dimensions;
	let w = width as usize;
	let h = height as usize;

	let expect = w * h * 3 / 2;

	let VideoPlanes::Cpu(buf) = &frame.planes else {
		return Err(Error::InvalidConfig("Linux VAAPI encoder requires CPU frames".into()));
	};

	match frame.format {
		PixelFormat::Nv12 => {
			if buf.len() != expect {
				return Err(Error::InvalidConfig(format!(
					"NV12 buffer wrong size: got {}, expected {}",
					buf.len(),
					expect
				)));
			}
			Ok(buf.clone())
		}

		PixelFormat::Yuv420p => {
			let y_sz = w * h;
			let uv_sz = y_sz / 4;

			if buf.len() != expect {
				return Err(Error::InvalidConfig(format!(
					"I420 buffer wrong size: got {}, expected {}",
					buf.len(),
					expect
				)));
			}

			let y = &buf[..y_sz];
			let u = &buf[y_sz..(y_sz + uv_sz)];
			let v = &buf[(y_sz + uv_sz)..];

			let mut out = vec![0u8; expect];
			out[..y_sz].copy_from_slice(y);

			let uv = &mut out[y_sz..];
			for i in 0..uv_sz {
				uv[2 * i] = u[i];
				uv[2 * i + 1] = v[i];
			}

			Ok(out)
		}

		_ => Err(Error::Unsupported),
	}
}

fn upload_nv12(
	display: &Rc<Display>,
	nv12_fmt: &VAImageFormat,
	surface: &Surface<()>,
	width: u32,
	height: u32,
	data: &[u8],
) -> Result<(), Error> {
	let mut image =
		Image::create_from(surface, *nv12_fmt, (width, height), (width, height))
			.map_err(|e| Error::Platform(format!("Image::create_from failed: {e:?}")))?;

	let va_image = *image.image();
	let dest = image.as_mut();

	let w = width as usize;
	let h = height as usize;

	let y_sz = w * h;

	if data.len() != y_sz + y_sz / 2 {
		return Err(Error::InvalidConfig("upload_nv12: wrong input size".into()));
	}

	{
		let mut src = &data[..y_sz];
		let mut dst = &mut dest[va_image.offsets[0] as usize..];

		for _ in 0..h {
			dst[..w].copy_from_slice(&src[..w]);
			dst = &mut dst[va_image.pitches[0] as usize..];
			src = &src[w..];
		}
	}

	{
		let mut src = &data[y_sz..];
		let mut dst = &mut dest[va_image.offsets[1] as usize..];

		for _ in 0..(h / 2) {
			dst[..w].copy_from_slice(&src[..w]);
			dst = &mut dst[va_image.pitches[1] as usize..];
			src = &src[w..];
		}
	}

	surface
		.sync()
		.map_err(|e| Error::Platform(format!("surface.sync failed: {e:?}")))?;

	drop(image);
	Ok(())
}