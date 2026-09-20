pub(crate) fn codec_config_to_annexb(data: &[u8]) -> Vec<u8> {
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

pub(crate) fn parse_avcc_annexb(data: &[u8]) -> Vec<u8> {
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

pub(crate) fn parse_hvcc_annexb(data: &[u8]) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annexb_4byte_passthrough() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB];
        assert_eq!(codec_config_to_annexb(&data), data.to_vec());
    }

    #[test]
    fn annexb_3byte_passthrough() {
        let data = [0x00, 0x00, 0x01, 0x65, 0xAA];
        assert_eq!(codec_config_to_annexb(&data), data.to_vec());
    }

    #[test]
    fn short_passthrough() {
        let data = [0x01, 0x64, 0x00];
        assert_eq!(codec_config_to_annexb(&data), data.to_vec());
    }

    #[test]
    fn empty_passthrough() {
        let data: [u8; 0] = [];
        assert_eq!(codec_config_to_annexb(&data), Vec::new());
    }

    #[test]
    fn length_prefixed_passthrough() {
        let data = [0x00, 0x00, 0x00, 0x05, 0x65, 0xAA, 0xBB, 0xCC, 0xDD];
        assert_eq!(codec_config_to_annexb(&data), data.to_vec());
    }

    #[test]
    fn avcc_sps_pps_to_annexb() {
        let sps = [0x67, 0x42, 0xC0, 0x1E];
        let pps = [0x68, 0xCE, 0x3C, 0x80];
        let mut avcc = vec![0x01, 0x64, 0x00, 0x1E, 0xFF, 0xE1];
        avcc.extend_from_slice(&[0x00, 0x04]);
        avcc.extend_from_slice(&sps);
        avcc.push(0x01);
        avcc.extend_from_slice(&[0x00, 0x04]);
        avcc.extend_from_slice(&pps);
        let mut expected = vec![0x00, 0x00, 0x00, 0x01];
        expected.extend_from_slice(&sps);
        expected.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        expected.extend_from_slice(&pps);
        assert_eq!(codec_config_to_annexb(&avcc), expected);
    }

    #[test]
    fn hvcc_single_nalu_to_annexb() {
        let mut hvcc = vec![0u8; 23];
        hvcc[0] = 0x01;
        hvcc[22] = 0x01;
        hvcc.extend_from_slice(&[0x40, 0x00, 0x01, 0x00, 0x03, 0x26, 0x01, 0xCC]);
        let expected = vec![0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xCC];
        assert_eq!(codec_config_to_annexb(&hvcc), expected);
    }

    #[test]
    fn truncated_avcc_falls_back_to_input() {
        let data = [0x01, 0x64, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x0A, 0x67];
        assert_eq!(codec_config_to_annexb(&data), data.to_vec());
    }

    #[test]
    fn truncated_hvcc_falls_back_to_input() {
        let mut data = vec![0u8; 23];
        data[0] = 0x01;
        data[22] = 0x01;
        data.extend_from_slice(&[0x40, 0x00, 0x05]);
        assert_eq!(codec_config_to_annexb(&data), data.clone());
    }

    #[test]
    fn avcc_multi_sps_pps_all_prefixed() {
        let sps1 = [0x67, 0x42, 0xC0, 0x1E, 0x01];
        let sps2 = [0x67, 0x4D, 0x00, 0x20];
        let pps1 = [0x68, 0xCE, 0x3C];
        let pps2 = [0x68, 0xEF, 0x04, 0x05];
        let mut avcc = vec![0x01, 0x64, 0x00, 0x1E, 0xFF, 0xE2];
        for sps in [&sps1[..], &sps2[..]] {
            avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
            avcc.extend_from_slice(sps);
        }
        avcc.push(0x02);
        for pps in [&pps1[..], &pps2[..]] {
            avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
            avcc.extend_from_slice(pps);
        }
        let out = codec_config_to_annexb(&avcc);
        assert_eq!(out.len(), 4 * 4 + sps1.len() + sps2.len() + pps1.len() + pps2.len());
        assert_eq!(out.windows(4).filter(|w| *w == [0x00, 0x00, 0x00, 0x01]).count(), 4);
    }

    #[test]
    fn hvcc_multi_array_all_prefixed() {
        let mut hvcc = vec![0u8; 23];
        hvcc[0] = 0x01;
        hvcc[22] = 0x03;
        for nalu in [&[0x40u8, 0x01][..], &[0x42u8, 0x01, 0xAA][..], &[0x26u8, 0x01][..]] {
            hvcc.push(0x00);
            hvcc.extend_from_slice(&1u16.to_be_bytes());
            hvcc.extend_from_slice(&(nalu.len() as u16).to_be_bytes());
            hvcc.extend_from_slice(nalu);
        }
        let out = codec_config_to_annexb(&hvcc);
        assert_eq!(out.windows(4).filter(|w| *w == [0x00, 0x00, 0x00, 0x01]).count(), 3);
        assert!(out.windows(2).any(|w| w == [0x40, 0x01]));
        assert!(out.windows(3).any(|w| w == [0x42, 0x01, 0xAA]));
    }

    #[test]
    fn avcc_zero_sps_falls_back_to_input() {
        let data = vec![0x01, 0x64, 0x00, 0x1E, 0xFF, 0xE0];
        assert_eq!(codec_config_to_annexb(&data), data.clone());
    }

    #[test]
    fn avcc_chromium_style_config_converts() {
        let avcc: Vec<u8> = vec![
            0x01, 0x64, 0x00, 0x1F, 0xFF, 0xE1, 0x00, 0x19, 0x67, 0x64, 0x00, 0x1F,
            0xAC, 0xD9, 0x40, 0x78, 0x02, 0x27, 0xE5, 0xC0, 0x44, 0x00, 0x00, 0x03,
            0x00, 0x04, 0x00, 0x00, 0x03, 0x00, 0xF1, 0x83, 0x19, 0x01, 0x00, 0x04,
            0x68, 0xE9, 0x7B, 0xCB,
        ];
        let out = codec_config_to_annexb(&avcc);
        assert_eq!(out.len(), 4 + 0x19 + 4 + 4);
        assert_eq!(&out[..4], &[0x00, 0x00, 0x00, 0x01]);
        assert_eq!(out[4], 0x67);
        assert_eq!(&out[out.len() - 8..out.len() - 4], &[0x00, 0x00, 0x00, 0x01]);
        assert_eq!(out[out.len() - 4], 0x68);
    }

    #[test]
    fn avcc_with_sps_extension_parses_pps() {
        let sps = [0x67, 0x64, 0x00, 0x1F, 0xAA];
        let ext = [0xFD, 0xF8, 0xF8, 0x00];
        let pps = [0x68, 0xE9, 0x7B];
        let mut avcc = vec![0x01, 0x64, 0x00, 0x1F, 0xFF, 0xE1];
        avcc.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&sps);
        avcc.push(0x01);
        avcc.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        avcc.extend_from_slice(&pps);
        avcc.extend_from_slice(&ext);
        let out = codec_config_to_annexb(&avcc);
        assert_eq!(out.windows(4).filter(|w| *w == [0x00, 0x00, 0x00, 0x01]).count(), 2);
        assert!(out.windows(sps.len()).any(|w| w == sps));
        assert!(out.windows(pps.len()).any(|w| w == pps));
        assert!(!out.windows(ext.len()).any(|w| w == ext));
    }

    #[test]
    fn non_config_byte_passthrough() {
        let data = [0x02, 0x00, 0x00, 0x00, 0x01, 0xAA, 0xBB, 0xCC];
        assert_eq!(codec_config_to_annexb(&data), data.to_vec());
    }
}
