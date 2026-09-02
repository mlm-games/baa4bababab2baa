use std::fmt;

use bytes::Bytes;

use crate::types::common::{Dimensions, PixelFormat, Timestamp};

/// A video codec identifier with type-safe variants and a raw escape hatch.
///
/// Use [`to_mime()`](VideoCodecId::to_mime) to get an Android MediaCodec MIME string,
/// and [`to_webcodecs_strings()`](VideoCodecId::to_webcodecs_strings) for WASM
/// WebCodecs full codec strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoCodecId {
    H264 {
        profile: Option<u32>,
        level: Option<u32>,
    },
    Hevc,
    Vp8,
    Vp9,
    Av1,
    /// Raw MIME string for codecs not covered by variants above.
    Other(String),
}

impl VideoCodecId {
    /// Create from a MIME-type string (e.g. `"video/avc"`).
    pub fn from_mime(mime: &str) -> Self {
        match mime {
            "video/avc" | "video/h264" => VideoCodecId::H264 {
                profile: None,
                level: None,
            },
            "video/hevc" | "video/h265" => VideoCodecId::Hevc,
            "video/vp8" => VideoCodecId::Vp8,
            "video/vp9" => VideoCodecId::Vp9,
            "video/av1" | "video/av01" => VideoCodecId::Av1,
            other => VideoCodecId::Other(other.to_string()),
        }
    }

    /// Return the canonical MIME string for this codec (Android MediaCodec format).
    pub fn to_mime(&self) -> &str {
        match self {
            VideoCodecId::H264 { .. } => "video/avc",
            VideoCodecId::Hevc => "video/hevc",
            VideoCodecId::Vp8 => "video/vp8",
            VideoCodecId::Vp9 => "video/vp9",
            VideoCodecId::Av1 => "video/av1",
            VideoCodecId::Other(s) => s.as_str(),
        }
    }

    /// Return WebCodecs full codec strings for this codec (WASM target).
    /// Multiple strings are returned for H.264/HEVC to try different profile/level combinations.
    pub fn to_webcodecs_strings(&self) -> Vec<&str> {
        match self {
            VideoCodecId::H264 { .. } => vec![
                "avc1.42001E",
                "avc1.42E01E",
                "avc1.4D001E",
                "avc1.4D401E",
                "avc1.64001E",
                "avc1.640028",
                "avc1.640032",
            ],
            VideoCodecId::Hevc => vec![
                "hvc1.1.6.L93.B0",
                "hev1.1.6.L93.B0",
                "hvc1.1.6.L120.B0",
                "hev1.1.6.L120.B0",
                "hvc1.1.6.L123.B0",
                "hev1.1.6.L123.B0",
                "hvc1.1.6.L150.B0",
                "hev1.1.6.L150.B0",
                "hvc1.1.6.L153.B0",
                "hev1.1.6.L153.B0",
                "hvc1.2.4.L120.B0",
                "hev1.2.4.L120.B0",
            ],
            VideoCodecId::Av1 => vec!["av01.0.04M.08"],
            VideoCodecId::Vp9 => vec!["vp09.00.10.08"],
            VideoCodecId::Vp8 => vec!["vp8"],
            VideoCodecId::Other(s) => vec![s.as_str()],
        }
    }
}

impl fmt::Display for VideoCodecId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_mime())
    }
}

impl From<&str> for VideoCodecId {
    fn from(s: &str) -> Self {
        VideoCodecId::from_mime(s)
    }
}

/// Maps to `VideoEncoderConfig.avc.format` in WebCodecs on WASM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvcBitstreamFormat {
    /// Annex-B format with start codes (0x00000001). Preferred by many pipelines.
    AnnexB,
    /// AVCC format with 4-byte length prefixes. Default for most WebCodecs encoders.
    Avc,
}

/// Color matrix coefficients for the YUV data fed to the encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoColorSpace {
    /// BT.601 (SD). Most compatible default; most platform encoders assume this.
    #[default]
    Bt601,
    /// BT.709 (HD).
    Bt709,
    /// BT.2020 (HDR).
    Bt2020,
}

#[derive(Debug, Clone)]
pub struct VideoEncoderConfig {
    pub codec: VideoCodecId,
    pub dimensions: Dimensions,
    pub bitrate: Option<u32>,
    pub framerate: Option<f64>,
    pub hardware_acceleration: Option<bool>,
    pub latency_optimized: Option<bool>,
    pub level: Option<u32>,
    /// Request the encoder to use a specific H.264 bitstream format.
    pub avc_bitstream_format: Option<AvcBitstreamFormat>,
    /// Color space of the YUV data. Used to signal color metadata in the
    /// encoded bitstream where the platform supports it.
    pub color_space: Option<VideoColorSpace>,
}

impl Default for VideoEncoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodecId::H264 {
                profile: None,
                level: None,
            },
            dimensions: Dimensions::new(1920, 1080),
            bitrate: None,
            framerate: None,
            hardware_acceleration: None,
            latency_optimized: None,
            level: None,
            avc_bitstream_format: None,
            color_space: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoOutputMode {
    #[default]
    Cpu,
    PreferHardware,
    HardwareOnly,
}

#[derive(Debug, Clone)]
pub struct VideoDecoderConfig {
    pub codec: VideoCodecId,
    pub resolution: Option<Dimensions>,
    pub description: Option<Bytes>,
    pub hardware_acceleration: Option<bool>,
    pub output_mode: VideoOutputMode,
}

impl Default for VideoDecoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodecId::H264 {
                profile: None,
                level: None,
            },
            resolution: None,
            description: None,
            hardware_acceleration: Some(true),
            output_mode: VideoOutputMode::PreferHardware,
        }
    }
}

#[derive(Debug)]
pub struct EncodedVideoPacket {
    pub payload: Bytes,
    pub timestamp: Timestamp,
    pub keyframe: bool,
}

#[derive(Debug)]
pub struct VideoFrame {
    pub dimensions: Dimensions,
    pub format: PixelFormat,
    pub timestamp: Timestamp,
    pub planes: VideoPlanes,
}

#[derive(Debug)]
pub enum VideoPlanes {
    Cpu(Vec<u8>),
    Hardware(HardwareBuffer),
}

impl VideoFrame {
    pub fn is_hardware(&self) -> bool {
        matches!(self.planes, VideoPlanes::Hardware(_))
    }

    pub fn ensure_cpu(&mut self) -> Result<(), crate::Error> {
        if let VideoPlanes::Hardware(hw) = &self.planes {
            let (fmt, data) =
                hw.copy_to_cpu(self.format, self.dimensions.width, self.dimensions.height)?;
            self.format = fmt;
            self.planes = VideoPlanes::Cpu(data);
        }
        Ok(())
    }
}

pub struct HardwareBuffer {
    pub(crate) inner: HardwareBufferInner,
}

impl fmt::Debug for HardwareBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            #[cfg(all(target_os = "linux", feature = "linux"))]
            HardwareBufferInner::DmaBuf(d) => f
                .debug_struct("DmaBuf")
                .field("w", &d.width)
                .field("h", &d.height)
                .field("fourcc", &d.fourcc)
                .field("modifier", &d.modifier)
                .field("fds", &d.fds.len())
                .finish(),
            #[cfg(target_arch = "wasm32")]
            HardwareBufferInner::WebCodecs(_) => f.write_str("WebCodecs(VideoFrame)"),
            #[cfg(target_os = "android")]
            HardwareBufferInner::MediaCodecSurface { .. } => f.write_str("MediaCodecSurface"),
            HardwareBufferInner::Unsupported => f.write_str("Unsupported"),
        }
    }
}

pub(crate) enum HardwareBufferInner {
    #[cfg(all(target_os = "linux", feature = "linux"))]
    DmaBuf(DmaBufFrame),
    #[cfg(target_arch = "wasm32")]
    WebCodecs(wasodecs::VideoFrame),
    #[cfg(target_os = "android")]
    MediaCodecSurface {
        release: Option<Box<dyn FnOnce(bool) + Send>>,
        render_on_drop: bool,
    },
    Unsupported,
}

#[cfg(all(target_os = "linux", feature = "linux"))]
#[derive(Debug)]
pub struct DmaBufFrame {
    pub fds: Vec<std::fs::File>,
    pub fourcc: u32,
    pub modifier: u64,
    pub width: u32,
    pub height: u32,
    pub planes: Vec<DmaBufPlane>,
}

#[cfg(all(target_os = "linux", feature = "linux"))]
#[derive(Debug, Clone)]
pub struct DmaBufPlane {
    pub buffer_index: usize,
    pub offset: usize,
    pub stride: usize,
}

impl HardwareBuffer {
    #[cfg(all(target_os = "linux", feature = "linux"))]
    pub fn as_dmabuf(&self) -> Option<&DmaBufFrame> {
        match &self.inner {
            HardwareBufferInner::DmaBuf(d) => Some(d),
            _ => None,
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn as_web_video_frame(&self) -> Option<&wasodecs::VideoFrame> {
        match &self.inner {
            HardwareBufferInner::WebCodecs(f) => Some(f),
            _ => None,
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn into_web_video_frame(mut self) -> Option<wasodecs::VideoFrame> {
        match std::mem::replace(&mut self.inner, HardwareBufferInner::Unsupported) {
            HardwareBufferInner::WebCodecs(f) => Some(f),
            _ => None,
        }
    }

    pub fn copy_to_cpu(
        &self,
        fmt: PixelFormat,
        w: u32,
        h: u32,
    ) -> Result<(PixelFormat, Vec<u8>), crate::Error> {
        match &self.inner {
            #[cfg(all(target_os = "linux", feature = "linux"))]
            HardwareBufferInner::DmaBuf(_) => {
                crate::platform::linux::video_decoder::dmabuf_copy_to_cpu(self, fmt, w, h)
            }
            _ => Err(crate::Error::Unsupported),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn copy_to_cpu_async(&self) -> Result<Vec<u8>, crate::Error> {
        match &self.inner {
            HardwareBufferInner::WebCodecs(f) => f
                .copy_to_cpu()
                .await
                .map_err(|e| crate::Error::Platform(format!("copy_to_cpu: {e:?}"))),
            _ => Err(crate::Error::Unsupported),
        }
    }
}

impl Drop for HardwareBuffer {
    fn drop(&mut self) {
        #[cfg(target_os = "android")]
        if let HardwareBufferInner::MediaCodecSurface {
            release,
            render_on_drop,
        } = &mut self.inner
        {
            if let Some(f) = release.take() {
                f(*render_on_drop);
            }
        }
    }
}
