use crate::types::SampleFormat;

pub(crate) fn interleaved_to_planar_f32(
    samples: &[u8],
    channels: usize,
    format: SampleFormat,
) -> Result<Vec<Vec<f32>>, String> {
    if channels == 0 {
        return Err("audio frame has 0 channels".into());
    }
    match format {
        SampleFormat::F32 => {
            let total = samples.len() / 4;
            let frames = total / channels;
            let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); channels];
            for (i, chunk) in samples.chunks_exact(4).enumerate() {
                let s = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                planar[i % channels].push(s);
            }
            Ok(planar)
        }
        SampleFormat::S16 => {
            let total = samples.len() / 2;
            let frames = total / channels;
            let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); channels];
            for (i, chunk) in samples.chunks_exact(2).enumerate() {
                let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                planar[i % channels].push(s as f32 / 32768.0);
            }
            Ok(planar)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le_f32(v: f32) -> [u8; 4] {
        v.to_le_bytes()
    }

    #[test]
    fn f32_stereo_deinterleave() {
        let vals = [0.5f32, -0.5, 0.25, -0.25];
        let mut bytes = Vec::new();
        for v in vals {
            bytes.extend_from_slice(&le_f32(v));
        }
        let planar = interleaved_to_planar_f32(&bytes, 2, SampleFormat::F32).unwrap();
        assert_eq!(planar.len(), 2);
        assert_eq!(planar[0], vec![0.5, 0.25]);
        assert_eq!(planar[1], vec![-0.5, -0.25]);
    }

    #[test]
    fn s16_mono_scales_to_unit() {
        let bytes = [0x00u8, 0x40, 0x00, 0xC0, 0x00, 0x00];
        let planar = interleaved_to_planar_f32(&bytes, 1, SampleFormat::S16).unwrap();
        assert_eq!(planar.len(), 1);
        assert!((planar[0][0] - 0.5).abs() < 1e-6);
        assert!((planar[0][1] + 0.5).abs() < 1e-6);
        assert_eq!(planar[0][2], 0.0);
    }

    #[test]
    fn s16_stereo_extremes() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&32767i16.to_le_bytes());
        bytes.extend_from_slice(&(-32768i16).to_le_bytes());
        let planar = interleaved_to_planar_f32(&bytes, 2, SampleFormat::S16).unwrap();
        assert!((planar[0][0] - 32767.0 / 32768.0).abs() < 1e-6);
        assert_eq!(planar[1][0], -1.0);
    }

    #[test]
    fn zero_channels_rejected() {
        assert!(interleaved_to_planar_f32(&[0u8; 8], 0, SampleFormat::F32).is_err());
    }

    #[test]
    fn trailing_partial_sample_ignored() {
        let mut bytes = 1.0f32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0xAA, 0xBB]);
        let planar = interleaved_to_planar_f32(&bytes, 1, SampleFormat::F32).unwrap();
        assert_eq!(planar[0], vec![1.0]);
    }

    #[test]
    fn trailing_partial_s16_ignored() {
        let bytes = vec![0x01u8, 0x02, 0x03, 0x04, 0xFF];
        let planar = interleaved_to_planar_f32(&bytes, 2, SampleFormat::S16).unwrap();
        assert_eq!(planar[0].len(), 1);
        assert_eq!(planar[1].len(), 1);
    }

    #[test]
    fn incomplete_frame_samples_stay_ragged() {
        let mut bytes = Vec::new();
        for v in [1.0f32, 2.0, 3.0] {
            bytes.extend_from_slice(&le_f32(v));
        }
        let planar = interleaved_to_planar_f32(&bytes, 2, SampleFormat::F32).unwrap();
        assert_eq!(planar[0], vec![1.0, 3.0]);
        assert_eq!(planar[1], vec![2.0]);
    }

    #[test]
    fn empty_input_yields_empty_planes() {
        let planar = interleaved_to_planar_f32(&[], 2, SampleFormat::F32).unwrap();
        assert_eq!(planar, vec![Vec::<f32>::new(), Vec::new()]);
    }

    #[test]
    fn odd_channel_round_robin() {
        let vals = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mut bytes = Vec::new();
        for v in vals {
            bytes.extend_from_slice(&le_f32(v));
        }
        let planar = interleaved_to_planar_f32(&bytes, 3, SampleFormat::F32).unwrap();
        assert_eq!(planar[0], vec![1.0, 4.0]);
        assert_eq!(planar[1], vec![2.0, 5.0]);
        assert_eq!(planar[2], vec![3.0, 6.0]);
    }

    #[test]
    fn real_sample_s16_stereo_matches_ffmpeg_f32() {
        let s16 = include_bytes!("../../assets-test/sample-1s-s16-44100-stereo.bin");
        let expected = include_bytes!("../../assets-test/sample-1s-f32-44100-stereo.bin");
        assert_eq!(s16.len(), 44_100 * 2 * 2);
        assert_eq!(expected.len(), 44_100 * 2 * 4);
        let planar = interleaved_to_planar_f32(&s16[..], 2, SampleFormat::S16).unwrap();
        assert_eq!(planar.len(), 2);
        assert_eq!(planar[0].len(), 44_100);
        assert_eq!(planar[1].len(), 44_100);
        for (i, chunk) in expected.chunks_exact(4).enumerate() {
            let want = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let got = planar[i % 2][i / 2];
            let scale = (i16::from_le_bytes([s16[i * 2], s16[i * 2 + 1]]) as f32 / 32768.0
                - want)
                .abs();
            assert!(scale < 1e-4, "ffmpeg drift at sample {i}: {scale}");
            assert!((got - want).abs() < 1e-6 || scale < 1e-4, "mismatch at {i}");
        }
    }
}
