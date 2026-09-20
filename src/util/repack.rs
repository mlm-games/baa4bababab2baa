use crate::error::Error;
use crate::types::PixelFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorLayout {
    Nv12,
    I420,
    P010,
    Unsupported,
}

pub(crate) fn layout_from_color_format_raw(v: i32) -> ColorLayout {
    match v {
        21 | 39 | 2135033992 | 2141391872 => ColorLayout::Nv12,
        19 | 20 => ColorLayout::I420,
        54 => ColorLayout::P010,
        _ => ColorLayout::Unsupported,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub vis_w: u32,
    pub vis_h: u32,
    pub stride: usize,
    pub slice_h: usize,
    pub crop_left: usize,
    pub crop_top: usize,
}

pub(crate) fn resolve_visible(
    crop_left: u32,
    crop_top: u32,
    crop_right: u32,
    crop_bottom: u32,
    fmt_w: u32,
    fmt_h: u32,
) -> Result<(u32, u32), Error> {
    // Per the MediaCodec docs the crop keys are INCLUSIVE: right/bottom are
    // the last valid column/row, so visible = right - left + 1. The docs
    // compute width only when BOTH left+right are present (and height only
    // when both top+bottom are), so a half-present rect is malformed rather
    // than a fallback: (right>0)xor(bottom>0) is an error, (0,0) is absent.
    // Docs also require callers to check key presence; since this helper only
    // sees values, (0, 0) means "absent" and falls back to width/height.
    let right_present = crop_right > 0;
    let bottom_present = crop_bottom > 0;
    if right_present != bottom_present {
        return Err(Error::InvalidConfig("incomplete crop rectangle".into()));
    }
    if right_present && bottom_present {
        if crop_left > crop_right || crop_top > crop_bottom {
            return Err(Error::InvalidConfig("invalid crop rectangle".into()));
        }
        let (w, h) = (
            crop_right - crop_left + 1,
            crop_bottom - crop_top + 1,
        );
        if w % 2 != 0 || h % 2 != 0 {
            return Err(Error::InvalidConfig(
                "crop rectangle must be even for 4:2:0 output".into(),
            ));
        }
        Ok((w, h))
    } else {
        Ok((fmt_w, fmt_h))
    }
}

pub(crate) fn repack(
    raw: &[u8],
    layout: ColorLayout,
    geom: Geometry,
) -> Result<(PixelFormat, Vec<u8>), Error> {
    match layout {
        ColorLayout::Nv12 => repack_nv12(raw, geom).map(|v| (PixelFormat::Nv12, v)),
        ColorLayout::I420 => repack_i420(raw, geom).map(|v| (PixelFormat::Yuv420p, v)),
        ColorLayout::P010 => repack_p010(raw, geom).map(|v| (PixelFormat::Nv12, v)),
        ColorLayout::Unsupported => Err(Error::InvalidConfig(
            "unsupported color layout for CPU repack".into(),
        )),
    }
}

fn check_visible(geom: Geometry) -> Result<(usize, usize), Error> {
    if geom.vis_w == 0 || geom.vis_h == 0 {
        return Err(Error::InvalidConfig(
            "visible dimensions must be non-zero".into(),
        ));
    }
    if geom.stride == 0 || geom.slice_h == 0 {
        return Err(Error::InvalidConfig(
            "stride and slice-height must be non-zero".into(),
        ));
    }
    let (w, h) = (geom.vis_w as usize, geom.vis_h as usize);
    let y = (w as u64)
        .checked_mul(h as u64)
        .ok_or_else(|| Error::Platform("visible dimensions overflow".into()))?;
    y.checked_mul(3)
        .and_then(|v| v.checked_div(2))
        .ok_or_else(|| Error::Platform("visible dimensions overflow".into()))?;
    Ok((w, h))
}

fn out_nv12_size(w: usize, h: usize) -> Result<usize, Error> {
    (w as u64)
        .checked_mul(h as u64)
        .and_then(|v| v.checked_mul(3))
        .and_then(|v| v.checked_div(2))
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| Error::Platform("visible dimensions overflow".into()))
}

fn repack_nv12(raw: &[u8], geom: Geometry) -> Result<Vec<u8>, Error> {
    let (w, h) = check_visible(geom)?;
    if w % 2 != 0 || h % 2 != 0 {
        return Err(Error::InvalidConfig(
            "NV12 visible dimensions must be even".into(),
        ));
    }
    let uv_h = h / 2;
    let uv_crop_top = geom.crop_top / 2;
    let y_size = geom
        .stride
        .checked_mul(geom.slice_h)
        .ok_or_else(|| Error::Platform("NV12 geometry overflow".into()))?;
    let uv_need = uv_h
        .checked_mul(geom.stride)
        .ok_or_else(|| Error::Platform("NV12 geometry overflow".into()))?;
    let expected = y_size
        .checked_add(uv_need)
        .ok_or_else(|| Error::Platform("NV12 geometry overflow".into()))?;
    if raw.len() < expected {
        return Err(Error::Platform(format!(
            "NV12 buffer too small: {} < {}",
            raw.len(),
            expected
        )));
    }
    let crop_ok = geom.crop_left.checked_add(w).is_some_and(|e| e <= geom.stride)
        && geom.crop_top.checked_add(h).is_some_and(|e| e <= geom.slice_h)
        && uv_crop_top
            .checked_add(uv_h)
            .and_then(|rows| rows.checked_mul(geom.stride))
            .and_then(|uv_end| y_size.checked_add(uv_end))
            .is_some_and(|end| end <= raw.len());
    if !crop_ok {
        return Err(Error::Platform("NV12 crop out of bounds".into()));
    }
    let out_len = out_nv12_size(w, h)?;
    let mut out = vec![0u8; out_len];
    let (out_y, out_uv) = out.split_at_mut(w * h);
    for row in 0..h {
        let src_start = geom
            .crop_top
            .checked_add(row)
            .and_then(|r| r.checked_mul(geom.stride))
            .and_then(|v| v.checked_add(geom.crop_left))
            .ok_or_else(|| Error::Platform("NV12 crop out of bounds".into()))?;
        let dst_start = row * w;
        out_y[dst_start..dst_start + w].copy_from_slice(&raw[src_start..src_start + w]);
    }
    for row in 0..uv_h {
        let src_start = y_size
            .checked_add(
                uv_crop_top
                    .checked_add(row)
                    .and_then(|r| r.checked_mul(geom.stride))
                    .ok_or_else(|| Error::Platform("NV12 crop out of bounds".into()))?,
            )
            .and_then(|v| v.checked_add(geom.crop_left))
            .ok_or_else(|| Error::Platform("NV12 crop out of bounds".into()))?;
        let dst_start = row * w;
        out_uv[dst_start..dst_start + w].copy_from_slice(&raw[src_start..src_start + w]);
    }
    Ok(out)
}

fn repack_i420(raw: &[u8], geom: Geometry) -> Result<Vec<u8>, Error> {
    let (w, h) = check_visible(geom)?;
    if w % 2 != 0 || h % 2 != 0 {
        return Err(Error::InvalidConfig(
            "I420 visible dimensions must be even".into(),
        ));
    }
    let uv_h = h / 2;
    let uv_stride = geom.stride / 2;
    if uv_stride == 0 {
        return Err(Error::InvalidConfig(
            "stride and slice-height must be non-zero".into(),
        ));
    }
    let y_size = geom
        .stride
        .checked_mul(geom.slice_h)
        .ok_or_else(|| Error::Platform("I420 geometry overflow".into()))?;
    let u_size = uv_stride
        .checked_mul(uv_h)
        .ok_or_else(|| Error::Platform("I420 geometry overflow".into()))?;
    let expected = y_size
        .checked_add(2 * u_size)
        .ok_or_else(|| Error::Platform("I420 geometry overflow".into()))?;
    if raw.len() < expected {
        return Err(Error::Platform(format!(
            "I420 buffer too small: {} < {}",
            raw.len(),
            expected
        )));
    }
    let uv_crop_l = geom.crop_left / 2;
    let uv_crop_top = geom.crop_top / 2;
    if geom.crop_left.checked_add(w).is_none_or(|e| e > geom.stride)
        || geom.crop_top.checked_add(h).is_none_or(|e| e > geom.slice_h)
        || uv_crop_l.checked_add(w / 2).is_none_or(|e| e > uv_stride)
        || uv_crop_top.checked_add(uv_h).is_none_or(|e| {
            e.checked_mul(uv_stride)
                .and_then(|v| y_size.checked_add(v))
                .and_then(|v| u_size.checked_add(v))
                .is_none_or(|v| v > raw.len())
        })
    {
        return Err(Error::Platform("I420 crop out of bounds".into()));
    }
    let out_len = out_nv12_size(w, h)?;
    let mut out = vec![0u8; out_len];
    let (out_y, out_uv) = out.split_at_mut(w * h);
    let (out_u, out_v) = out_uv.split_at_mut(w * h / 4);
    for row in 0..h {
        let src_start = geom
            .crop_top
            .checked_add(row)
            .and_then(|r| r.checked_mul(geom.stride))
            .and_then(|v| v.checked_add(geom.crop_left))
            .ok_or_else(|| Error::Platform("I420 crop out of bounds".into()))?;
        let dst_start = row * w;
        out_y[dst_start..dst_start + w].copy_from_slice(&raw[src_start..src_start + w]);
    }
    for row in 0..uv_h {
        let src_start = y_size
            .checked_add(
                uv_crop_top
                    .checked_add(row)
                    .and_then(|r| r.checked_mul(uv_stride))
                    .ok_or_else(|| Error::Platform("I420 crop out of bounds".into()))?,
            )
            .and_then(|v| v.checked_add(uv_crop_l))
            .ok_or_else(|| Error::Platform("I420 crop out of bounds".into()))?;
        let dst_start = row * (w / 2);
        out_u[dst_start..dst_start + w / 2]
            .copy_from_slice(&raw[src_start..src_start + w / 2]);
    }
    for row in 0..uv_h {
        let src_start = y_size
            .checked_add(u_size)
            .and_then(|base| {
                uv_crop_top
                    .checked_add(row)
                    .and_then(|r| r.checked_mul(uv_stride))
                    .and_then(|v| base.checked_add(v))
            })
            .and_then(|v| v.checked_add(uv_crop_l))
            .ok_or_else(|| Error::Platform("I420 crop out of bounds".into()))?;
        let dst_start = row * (w / 2);
        out_v[dst_start..dst_start + w / 2]
            .copy_from_slice(&raw[src_start..src_start + w / 2]);
    }
    Ok(out)
}

fn repack_p010(raw: &[u8], geom: Geometry) -> Result<Vec<u8>, Error> {
    let (w, h) = check_visible(geom)?;
    if w % 2 != 0 || h % 2 != 0 {
        return Err(Error::InvalidConfig(
            "P010 visible dimensions must be even".into(),
        ));
    }
    let uv_h = h / 2;
    let uv_crop_top = geom.crop_top / 2;
    let y_size = geom
        .stride
        .checked_mul(geom.slice_h)
        .and_then(|v| v.checked_mul(2))
        .ok_or_else(|| Error::Platform("P010 geometry overflow".into()))?;
    let uv_need = uv_h
        .checked_mul(geom.stride)
        .and_then(|v| v.checked_mul(2))
        .ok_or_else(|| Error::Platform("P010 geometry overflow".into()))?;
    let expected = y_size
        .checked_add(uv_need)
        .ok_or_else(|| Error::Platform("P010 geometry overflow".into()))?;
    if raw.len() < expected {
        return Err(Error::Platform(format!(
            "P010 buffer too small: {} < {}",
            raw.len(),
            expected
        )));
    }
    if geom.crop_left.checked_add(w).is_none_or(|e| e > geom.stride)
        || geom.crop_top.checked_add(h).is_none_or(|e| e > geom.slice_h)
    {
        return Err(Error::Platform("P010 crop out of bounds".into()));
    }
    let out_len = out_nv12_size(w, h)?;
    let mut out = vec![0u8; out_len];
    let (out_y, out_uv) = out.split_at_mut(w * h);
    for row in 0..h {
        let src_row = geom
            .crop_top
            .checked_add(row)
            .and_then(|r| r.checked_mul(geom.stride))
            .and_then(|v| v.checked_mul(2))
            .and_then(|v| v.checked_add(geom.crop_left.checked_mul(2)?))
            .ok_or_else(|| Error::Platform("P010 crop out of bounds".into()))?;
        let dst_start = row * w;
        for col in 0..w {
            out_y[dst_start + col] = raw[src_row + col * 2 + 1];
        }
    }
    for row in 0..uv_h {
        let src_row = y_size
            .checked_add(
                uv_crop_top
                    .checked_add(row)
                    .and_then(|r| r.checked_mul(geom.stride))
                    .and_then(|v| v.checked_mul(2))
                    .ok_or_else(|| Error::Platform("P010 crop out of bounds".into()))?,
            )
            .and_then(|v| v.checked_add(geom.crop_left.checked_mul(2)?))
            .ok_or_else(|| Error::Platform("P010 crop out of bounds".into()))?;
        let dst_start = row * w;
        for col in 0..w {
            out_uv[dst_start + col] = raw[src_row + col * 2 + 1];
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom(w: u32, h: u32, stride: usize, slice_h: usize) -> Geometry {
        Geometry {
            vis_w: w,
            vis_h: h,
            stride,
            slice_h,
            crop_left: 0,
            crop_top: 0,
        }
    }

    #[test]
    fn layout_mapping_matches_anodecs() {
        assert_eq!(layout_from_color_format_raw(21), ColorLayout::Nv12);
        assert_eq!(layout_from_color_format_raw(39), ColorLayout::Nv12);
        assert_eq!(layout_from_color_format_raw(2135033992), ColorLayout::Nv12);
        assert_eq!(layout_from_color_format_raw(2141391872), ColorLayout::Nv12);
        assert_eq!(layout_from_color_format_raw(19), ColorLayout::I420);
        assert_eq!(layout_from_color_format_raw(20), ColorLayout::I420);
        assert_eq!(layout_from_color_format_raw(54), ColorLayout::P010);
        assert_eq!(
            layout_from_color_format_raw(2130706688),
            ColorLayout::Unsupported
        );
        assert_eq!(
            layout_from_color_format_raw(2130708361),
            ColorLayout::Unsupported
        );
        assert_eq!(layout_from_color_format_raw(0), ColorLayout::Unsupported);
        assert_eq!(layout_from_color_format_raw(-1), ColorLayout::Unsupported);
        assert_eq!(layout_from_color_format_raw(999), ColorLayout::Unsupported);
    }

    #[test]
    fn resolve_prefers_crop_rect() {
        assert_eq!(resolve_visible(0, 0, 1919, 1079, 1920, 1088).unwrap(), (1920, 1080));
    }

    #[test]
    fn resolve_falls_back_to_format_size() {
        assert_eq!(resolve_visible(0, 0, 0, 0, 1280, 720).unwrap(), (1280, 720));
    }

    #[test]
    fn resolve_rejects_inverted_crop() {
        assert!(resolve_visible(100, 0, 50, 100, 1920, 1080).is_err());
        assert!(resolve_visible(0, 200, 100, 100, 1920, 1080).is_err());
    }

    #[test]
    fn resolve_rejects_partial_crop() {
        assert!(resolve_visible(0, 0, 1919, 0, 1920, 1088).is_err());
        assert!(resolve_visible(0, 0, 0, 1079, 1920, 1088).is_err());
    }

    #[test]
    fn resolve_rejects_odd_crop() {
        assert!(resolve_visible(0, 0, 1918, 1079, 1920, 1088).is_err());
        assert!(resolve_visible(0, 0, 1919, 1078, 1920, 1088).is_err());
    }

    #[test]
    fn resolve_rejects_oversize_crop() {
        assert!(resolve_visible(0, 0, 100, 100, 64, 64).is_err());
    }

    #[test]
    fn nv12_crop_rect_must_fit_stride_and_slice() {
        let y: Vec<u8> = (0..16).collect();
        let uv: Vec<u8> = (100..108).collect();
        let raw: Vec<u8> = y.iter().chain(uv.iter()).copied().collect();
        let g = Geometry {
            vis_w: 4,
            vis_h: 4,
            stride: 4,
            slice_h: 4,
            crop_left: 0,
            crop_top: 0,
        };
        assert!(repack(&raw, ColorLayout::Nv12, g).is_ok());
        let g = Geometry { slice_h: 2, ..g };
        assert!(repack(&raw, ColorLayout::Nv12, g).is_err());
        let g = Geometry { stride: 2, slice_h: 4, ..g };
        assert!(repack(&raw, ColorLayout::Nv12, g).is_err());
    }

    #[test]
    fn slice_zero_rejected() {
        let raw = vec![0u8; 24];
        assert!(repack(&raw, ColorLayout::Nv12, geom(4, 4, 4, 0)).is_err());
    }

    #[test]
    fn nv12_tight_roundtrip() {
        let y: Vec<u8> = (0..16).collect();
        let uv: Vec<u8> = (100..108).collect();
        let raw: Vec<u8> = y.iter().chain(uv.iter()).copied().collect();
        let (fmt, out) = repack(&raw, ColorLayout::Nv12, geom(4, 4, 4, 4)).unwrap();
        assert_eq!(fmt, PixelFormat::Nv12);
        assert_eq!(out, raw);
    }

    #[test]
    fn nv12_stride_padded_crops_to_tight() {
        let mut raw = vec![9u8; 16 + 2 * 4];
        raw[0] = 1;
        raw[1] = 2;
        raw[4] = 3;
        raw[5] = 4;
        raw[16] = 5;
        raw[17] = 6;
        let (_, out) = repack(&raw, ColorLayout::Nv12, geom(2, 2, 4, 4)).unwrap();
        assert_eq!(out, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn nv12_crop_offset() {
        let y: Vec<u8> = (0..16).collect();
        let uv: Vec<u8> = (100..108).collect();
        let raw: Vec<u8> = y.iter().chain(uv.iter()).copied().collect();
        let g = Geometry {
            vis_w: 2,
            vis_h: 2,
            stride: 4,
            slice_h: 4,
            crop_left: 1,
            crop_top: 1,
        };
        let (_, out) = repack(&raw, ColorLayout::Nv12, g).unwrap();
        assert_eq!(out, vec![5, 6, 9, 10, 101, 102]);
    }

    #[test]
    fn nv12_short_buffer_errors() {
        let raw = vec![0u8; 10];
        let err = repack(&raw, ColorLayout::Nv12, geom(4, 4, 4, 4)).unwrap_err();
        assert_eq!(format!("{err}"), "platform error: NV12 buffer too small: 10 < 24");
    }

    #[test]
    fn nv12_zero_dims_rejected() {
        let raw = vec![0u8; 24];
        let err = repack(&raw, ColorLayout::Nv12, geom(0, 4, 4, 4)).unwrap_err();
        assert_eq!(
            format!("{err}"),
            "invalid configuration: visible dimensions must be non-zero"
        );
    }

    #[test]
    fn huge_dims_rejected_without_alloc() {
        let raw: [u8; 0] = [];
        let g = Geometry {
            vis_w: u32::MAX,
            vis_h: u32::MAX,
            stride: usize::MAX,
            slice_h: usize::MAX,
            crop_left: 0,
            crop_top: 0,
        };
        for layout in [ColorLayout::Nv12, ColorLayout::I420, ColorLayout::P010] {
            let err = repack(&raw, layout, g).unwrap_err();
            let msg = format!("{err}");
            assert!(
                msg.contains("overflow") || msg.contains("too small"),
                "layout {layout:?}: {msg}"
            );
        }
    }

    #[test]
    fn huge_crop_rejected_without_panic() {
        let raw = vec![0u8; 24];
        let g = Geometry {
            vis_w: 4,
            vis_h: 4,
            stride: 4,
            slice_h: 4,
            crop_left: usize::MAX - 1,
            crop_top: usize::MAX - 1,
        };
        for layout in [ColorLayout::Nv12, ColorLayout::I420, ColorLayout::P010] {
            assert!(repack(&raw, layout, g).is_err());
        }
    }

    #[test]
    fn nv12_crop_oob_rejected() {
        let raw = vec![0u8; 24];
        let g = Geometry {
            vis_w: 4,
            vis_h: 4,
            stride: 4,
            slice_h: 4,
            crop_left: 2,
            crop_top: 0,
        };
        assert!(repack(&raw, ColorLayout::Nv12, g).is_err());
    }

    #[test]
    fn i420_tight_roundtrip() {
        let y: Vec<u8> = (0..16).collect();
        let u: Vec<u8> = vec![50, 51, 52, 53];
        let v: Vec<u8> = vec![60, 61, 62, 63];
        let raw: Vec<u8> = y.iter().chain(u.iter()).chain(v.iter()).copied().collect();
        let (fmt, out) = repack(&raw, ColorLayout::I420, geom(4, 4, 4, 4)).unwrap();
        assert_eq!(fmt, PixelFormat::Yuv420p);
        assert_eq!(out, raw);
    }

    #[test]
    fn i420_stride_padded() {
        let mut raw = vec![9u8; 16 + 2 + 2];
        raw[0] = 1;
        raw[1] = 2;
        raw[4] = 3;
        raw[5] = 4;
        raw[16] = 60;
        raw[18] = 70;
        let (_, out) = repack(&raw, ColorLayout::I420, geom(2, 2, 4, 4)).unwrap();
        assert_eq!(out, vec![1, 2, 3, 4, 60, 70]);
    }

    #[test]
    fn i420_short_buffer_errors() {
        let raw = vec![0u8; 10];
        let err = repack(&raw, ColorLayout::I420, geom(4, 4, 4, 4)).unwrap_err();
        assert_eq!(format!("{err}"), "platform error: I420 buffer too small: 10 < 24");
    }

    #[test]
    fn i420_odd_dims_rejected() {
        let raw = vec![0u8; 24];
        assert!(repack(&raw, ColorLayout::I420, geom(3, 4, 4, 4)).is_err());
    }

    #[test]
    fn p010_downconverts_high_bytes() {
        let raw: Vec<u8> = vec![0x00, 0x11, 0x00, 0x22, 0x00, 0x33, 0x00, 0x44, 0x00, 0x55, 0x00, 0x66];
        let (fmt, out) = repack(&raw, ColorLayout::P010, geom(2, 2, 2, 2)).unwrap();
        assert_eq!(fmt, PixelFormat::Nv12);
        assert_eq!(out, vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
    }

    #[test]
    fn p010_short_buffer_errors() {
        let raw = vec![0u8; 5];
        let err = repack(&raw, ColorLayout::P010, geom(2, 2, 2, 2)).unwrap_err();
        assert_eq!(format!("{err}"), "platform error: P010 buffer too small: 5 < 12");
    }

    #[test]
    fn unsupported_layout_rejected() {
        let raw = vec![0u8; 24];
        assert!(repack(&raw, ColorLayout::Unsupported, geom(4, 4, 4, 4)).is_err());
    }

    #[test]
    fn odd_height_rejected_for_420() {
        let raw = vec![0u8; 4 * 5 + 2 * 4];
        let err = repack(&raw, ColorLayout::Nv12, geom(4, 5, 4, 5)).unwrap_err();
        assert_eq!(
            format!("{err}"),
            "invalid configuration: NV12 visible dimensions must be even"
        );
        let err = repack(&raw, ColorLayout::I420, geom(4, 5, 4, 5)).unwrap_err();
        assert_eq!(
            format!("{err}"),
            "invalid configuration: I420 visible dimensions must be even"
        );
        let raw10 = vec![0u8; (4 * 5 + 2 * 4) * 2];
        let err = repack(&raw10, ColorLayout::P010, geom(4, 5, 4, 5)).unwrap_err();
        assert_eq!(
            format!("{err}"),
            "invalid configuration: P010 visible dimensions must be even"
        );
    }

    #[test]
    fn odd_width_rejected_for_420() {
        let raw = vec![0u8; 64];
        for layout in [ColorLayout::Nv12, ColorLayout::I420, ColorLayout::P010] {
            assert!(repack(&raw, layout, geom(3, 4, 4, 4)).is_err());
        }
    }

    #[test]
    fn real_frame_bbb360_nv12_tight_identity() {
        let raw = include_bytes!("../../assets-test/bbb360-nv12.bin");
        assert_eq!(raw.len(), 640 * 360 * 3 / 2);
        let (fmt, out) = repack(&raw[..], ColorLayout::Nv12, geom(640, 360, 640, 360)).unwrap();
        assert_eq!(fmt, PixelFormat::Nv12);
        assert_eq!(out, raw.to_vec());
    }

    #[test]
    fn real_frame_bbb360_stride_padded_roundtrip() {
        let tight = include_bytes!("../../assets-test/bbb360-nv12.bin");
        let (w, h, stride, slice_h) = (640usize, 360usize, 672usize, 368usize);
        let mut raw = vec![0xCDu8; stride * slice_h + (h / 2) * stride];
        for row in 0..h {
            raw[row * stride..row * stride + w]
                .copy_from_slice(&tight[row * w..row * w + w]);
        }
        let y_size = stride * slice_h;
        for row in 0..h / 2 {
            raw[y_size + row * stride..y_size + row * stride + w]
                .copy_from_slice(&tight[w * h + row * w..w * h + row * w + w]);
        }
        let (_, out) = repack(
            &raw,
            ColorLayout::Nv12,
            geom(w as u32, h as u32, stride, slice_h),
        )
        .unwrap();
        assert_eq!(out, tight.to_vec());
    }
}
