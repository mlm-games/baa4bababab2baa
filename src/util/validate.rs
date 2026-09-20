use crate::error::Error;

pub(crate) fn video_dims(width: u32, height: u32) -> Result<(), Error> {
    if width == 0 || height == 0 {
        return Err(Error::InvalidConfig("dimensions must be non-zero".into()));
    }
    Ok(())
}

pub(crate) fn video_dims_even(width: u32, height: u32) -> Result<(), Error> {
    if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
        return Err(Error::InvalidConfig(
            "dimensions must be non-zero and even (for NV12 4:2:0)".into(),
        ));
    }
    Ok(())
}

pub(crate) fn audio_encoder_config(channels: u32, sample_rate: u32) -> Result<(), Error> {
    if channels == 0 {
        return Err(Error::InvalidConfig("audio channels must be non-zero".into()));
    }
    if sample_rate == 0 {
        return Err(Error::InvalidConfig(
            "audio sample rate must be non-zero".into(),
        ));
    }
    Ok(())
}

pub(crate) fn audio_decoder_config(channels: u32, sample_rate: u32) -> Result<(), Error> {
    if channels == 0 {
        return Err(Error::InvalidConfig("audio channels must be non-zero".into()));
    }
    if sample_rate == 0 {
        return Err(Error::InvalidConfig("audio sample rate must be non-zero".into()));
    }
    Ok(())
}

pub(crate) fn audio_frame(channels: u32, sample_rate: u32) -> Result<(), Error> {
    if channels == 0 {
        return Err(Error::InvalidConfig("audio frame has 0 channels".into()));
    }
    if sample_rate == 0 {
        return Err(Error::InvalidConfig("audio frame has 0 sample rate".into()));
    }
    Ok(())
}

#[cfg(test)]
mod codec_goldens {
    use crate::types::{
        AudioCodecId, AudioDecoderConfig, AudioEncoderConfig, VideoCodecId, VideoDecoderConfig,
        VideoDescriptionFormat, VideoEncoderConfig,
    };
    use crate::{Dimensions, MediaFailure, MediaFailureCode};

    #[test]
    fn video_mime_roundtrip() {
        for (mime, canonical) in [
            ("video/avc", "video/avc"),
            ("video/h264", "video/avc"),
            ("video/hevc", "video/hevc"),
            ("video/h265", "video/hevc"),
            ("video/vp8", "video/vp8"),
            ("video/vp9", "video/vp9"),
            ("video/av1", "video/av1"),
            ("video/av01", "video/av1"),
        ] {
            let c = VideoCodecId::from_mime(mime);
            assert_eq!(c.to_mime(), canonical, "mime {mime}");
            assert_eq!(c.to_string(), canonical, "display {mime}");
        }
        let other = VideoCodecId::from_mime("video/x-foo");
        assert_eq!(other, VideoCodecId::Other("video/x-foo".into()));
        assert_eq!(other.to_mime(), "video/x-foo");
        assert_eq!(other.to_webcodecs_strings(), vec!["video/x-foo".to_string()]);
        assert_eq!(VideoCodecId::from("video/avc"), VideoCodecId::from_mime("video/avc"));
    }

    #[test]
    fn audio_mime_roundtrip() {
        for (mime, canonical) in [
            ("audio/mp4a-latm", "audio/mp4a-latm"),
            ("audio/aac", "audio/mp4a-latm"),
            ("audio/opus", "audio/opus"),
            ("audio/mpeg", "audio/mpeg"),
            ("audio/vorbis", "audio/vorbis"),
            ("audio/flac", "audio/flac"),
        ] {
            let c = AudioCodecId::from_mime(mime);
            assert_eq!(c.to_mime(), canonical, "mime {mime}");
            assert_eq!(c.to_string(), canonical, "display {mime}");
        }
        let other = AudioCodecId::from_mime("audio/x-y");
        assert_eq!(other, AudioCodecId::Other("audio/x-y".into()));
        assert_eq!(other.to_mime(), "audio/x-y");
    }

    #[test]
    fn webcodecs_strings_golden() {
        assert_eq!(
            VideoCodecId::H264 {
                profile: None,
                level: None
            }
            .to_webcodecs_strings(),
            vec![
                "avc1.42001E",
                "avc1.42E01E",
                "avc1.4D001E",
                "avc1.4D401E",
                "avc1.64001E",
                "avc1.640028",
                "avc1.640032",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        );
        let high = VideoCodecId::H264 {
            profile: Some(100),
            level: None,
        }
        .to_webcodecs_strings();
        assert!(high[0].starts_with("avc1.64"), "high profile first: {high:?}");
        let leveled = VideoCodecId::H264 {
            profile: None,
            level: Some(40),
        }
        .to_webcodecs_strings();
        assert!(leveled.iter().all(|s| s.ends_with("28")), "{leveled:?}");
        assert_eq!(
            VideoCodecId::Hevc.to_webcodecs_strings()[..4],
            [
                "hvc1.1.6.L93.B0",
                "hev1.1.6.L93.B0",
                "hvc1.1.6.L120.B0",
                "hev1.1.6.L120.B0",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()[..]
        );
        assert_eq!(
            VideoCodecId::Av1.to_webcodecs_strings(),
            vec!["av01.0.04M.08".to_string()]
        );
        assert_eq!(
            VideoCodecId::Vp9.to_webcodecs_strings(),
            vec!["vp09.00.10.08".to_string()]
        );
        assert_eq!(
            VideoCodecId::Vp8.to_webcodecs_strings(),
            vec!["vp8".to_string()]
        );
    }

    #[test]
    fn media_failure_display_golden() {
        let f = MediaFailure::new(MediaFailureCode::UnsupportedOutputFormat, "m")
            .backend("android")
            .codec("video/avc");
        assert_eq!(format!("{f}"), "unsupported-output-format: m");
        assert_eq!(format!("{}", MediaFailureCode::Timeout), "timeout");
        assert_eq!(
            format!("{}", MediaFailureCode::BackendDisconnected),
            "backend-disconnected"
        );
    }

    #[test]
    fn error_display_golden() {
        use crate::Error;
        assert_eq!(
            format!("{}", Error::Dropped),
            "dropped: sender or receiver was released"
        );
        assert_eq!(
            format!("{}", Error::Unsupported),
            "unsupported codec or config on this platform"
        );
        assert_eq!(
            format!("{}", Error::InvalidConfig("x".into())),
            "invalid configuration: x"
        );
        assert_eq!(
            format!("{}", Error::Platform("y".into())),
            "platform error: y"
        );
        assert_eq!(
            format!("{}", Error::NoBackend),
            "no backend available for this platform"
        );
    }

    #[test]
    fn default_configs_golden() {
        let enc = VideoEncoderConfig::default();
        assert_eq!(enc.codec.to_mime(), "video/avc");
        assert_eq!(enc.dimensions, Dimensions::new(1920, 1080));
        let dec = VideoDecoderConfig::default();
        assert_eq!(dec.codec.to_mime(), "video/avc");
        assert!(dec.description.is_none());
        assert!(dec.description_format.is_none());
        assert_eq!(dec.hardware_acceleration, Some(true));
        let aenc = AudioEncoderConfig::default();
        assert_eq!(aenc.codec.to_mime(), "audio/mp4a-latm");
        assert_eq!((aenc.sample_rate, aenc.channels), (48_000, 2));
        let adec = AudioDecoderConfig::default();
        assert_eq!((adec.sample_rate, adec.channel_count), (48_000, 2));
        assert!(adec.description.is_none());
    }

    #[test]
    fn description_format_explicit() {
        let cfg = VideoDecoderConfig {
            description_format: Some(VideoDescriptionFormat::AvcC),
            ..VideoDecoderConfig::default()
        };
        assert_eq!(cfg.description_format, Some(VideoDescriptionFormat::AvcC));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(r: Result<(), Error>) -> String {
        format!("{}", r.unwrap_err())
    }

    #[test]
    fn zero_dims_rejected() {
        assert_eq!(
            msg(video_dims(0, 1080)),
            "invalid configuration: dimensions must be non-zero"
        );
        assert_eq!(
            msg(video_dims(1920, 0)),
            "invalid configuration: dimensions must be non-zero"
        );
    }

    #[test]
    fn nonzero_dims_accepted() {
        assert!(video_dims(1920, 1080).is_ok());
        assert!(video_dims(1919, 1079).is_ok());
    }

    #[test]
    fn odd_dims_rejected_for_nv12() {
        assert_eq!(
            msg(video_dims_even(1919, 1080)),
            "invalid configuration: dimensions must be non-zero and even (for NV12 4:2:0)"
        );
        assert_eq!(
            msg(video_dims_even(1920, 1079)),
            "invalid configuration: dimensions must be non-zero and even (for NV12 4:2:0)"
        );
        assert_eq!(
            msg(video_dims_even(0, 1080)),
            "invalid configuration: dimensions must be non-zero and even (for NV12 4:2:0)"
        );
    }

    #[test]
    fn even_dims_accepted() {
        assert!(video_dims_even(1920, 1080).is_ok());
        assert!(video_dims_even(2, 2).is_ok());
    }

    #[test]
    fn audio_encoder_zero_channels() {
        assert_eq!(
            msg(audio_encoder_config(0, 48_000)),
            "invalid configuration: audio channels must be non-zero"
        );
    }

    #[test]
    fn audio_encoder_zero_rate() {
        assert_eq!(
            msg(audio_encoder_config(2, 0)),
            "invalid configuration: audio sample rate must be non-zero"
        );
    }

    #[test]
    fn audio_encoder_valid() {
        assert!(audio_encoder_config(2, 48_000).is_ok());
    }

    #[test]
    fn audio_frame_zero_channels() {
        assert_eq!(
            msg(audio_frame(0, 48_000)),
            "invalid configuration: audio frame has 0 channels"
        );
    }

    #[test]
    fn audio_frame_zero_rate() {
        assert_eq!(
            msg(audio_frame(2, 0)),
            "invalid configuration: audio frame has 0 sample rate"
        );
    }
}
