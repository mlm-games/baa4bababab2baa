//! Biplanar 4:2:0 pictures (NV12, and P010 with its 10-bit samples in the high bits of 16-bit
//! words) as a mapped decoder surface, into the tight I420 buffer this crate's frames carry.
//!
//! Ported from filmcraft's `biplanar` (MIT OR Apache-2.0), trimmed to the 8-bit output contract
//! the rest of the crate uses (10-bit pictures keep the high byte, as the Linux P010 path does).

/// A mapped biplanar surface: a luma plane followed by interleaved chroma, `stride` bytes per row.
pub struct Biplanar<'a> {
    pub luma: &'a [u8],
    pub chroma: &'a [u8],
    pub stride: usize,
    /// Rows in the luma plane (the surface height, which may exceed the picture's).
    pub rows: usize,
}

fn row(plane: &[u8], stride: usize, y: usize, x: usize, len: usize) -> Result<&[u8], String> {
    let start = y
        .checked_mul(stride)
        .and_then(|s| s.checked_add(x))
        .ok_or("plane offset overflows")?;
    plane
        .get(start..start.checked_add(len).ok_or("plane offset overflows")?)
        .ok_or_else(|| "plane row out of range".to_string())
}

/// Copy the picture at `crop` (x, y, width, height in luma samples) out of `src` into a tight
/// I420 buffer. `bits` is 8 (NV12) or 10 (P010; samples are shifted down from the high bits).
pub fn to_i420(src: &Biplanar, crop: (u32, u32, u32, u32), bits: u32) -> Result<Vec<u8>, String> {
    let (cx, cy, w, h) = crop;
    let (ox, oy, w, h) = (cx as usize, cy as usize, w as usize, h as usize);
    if w == 0 || h == 0 || !matches!(bits, 8 | 10) {
        return Err(format!("cannot read a {w}x{h} picture of {bits} bits"));
    }
    let bps = if bits > 8 { 2 } else { 1 };
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let (cox, coy) = (ox / 2, oy / 2);
    let needs_rows = oy.checked_add(h).ok_or("picture size overflows")?;
    let needs_cols = ox
        .checked_add(w)
        .and_then(|c| c.checked_mul(bps))
        .ok_or("picture size overflows")?;
    if src.rows < needs_rows || src.stride < needs_cols || src.stride < (cox + cw) * 2 * bps {
        return Err(format!(
            "decoded surface ({} rows, {} byte rows) is smaller than the {w}x{h} picture at {ox},{oy}",
            src.rows, src.stride
        ));
    }

    let mut out = vec![0u8; w * h + 2 * cw * ch];
    let (dst_y, dst_uv) = out.split_at_mut(w * h);
    let (dst_u, dst_v) = dst_uv.split_at_mut(cw * ch);

    if bps == 1 {
        for y in 0..h {
            dst_y[y * w..y * w + w].copy_from_slice(row(src.luma, src.stride, oy + y, ox, w)?);
        }
        for y in 0..ch {
            let pairs = row(src.chroma, src.stride, coy + y, cox * 2, cw * 2)?
                .as_chunks::<2>()
                .0;
            let u_row = &mut dst_u[y * cw..y * cw + cw];
            let v_row = &mut dst_v[y * cw..y * cw + cw];
            for (x, p) in pairs.iter().enumerate() {
                u_row[x] = p[0];
                v_row[x] = p[1];
            }
        }
    } else {
        // P010: 10-bit samples in the high bits of little-endian 16-bit words; the high byte is
        // the 8-bit approximation (the same convention as the Linux P010 readback).
        for y in 0..h {
            let src_row = row(src.luma, src.stride, oy + y, ox * 2, w * 2)?;
            let dst_row = &mut dst_y[y * w..y * w + w];
            for (x, dst) in dst_row.iter_mut().enumerate() {
                *dst = src_row[x * 2 + 1];
            }
        }
        for y in 0..ch {
            let quads = row(src.chroma, src.stride, coy + y, cox * 4, cw * 4)?
                .as_chunks::<4>()
                .0;
            let u_row = &mut dst_u[y * cw..y * cw + cw];
            let v_row = &mut dst_v[y * cw..y * cw + cw];
            for (x, q) in quads.iter().enumerate() {
                u_row[x] = q[1];
                v_row[x] = q[3];
            }
        }
    }
    Ok(out)
}
