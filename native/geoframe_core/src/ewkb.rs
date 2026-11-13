//! EWKB (PostGIS extended WKB) support. EWKB differs from ISO WKB in two ways: the type
//! word carries flag bits (`0x80000000` Z, `0x40000000` M, `0x20000000` SRID present) instead
//! of the ISO `+1000/+2000/+3000` offsets, and a top-level geometry may be followed by a
//! 4-byte SRID. The `wkb` reader only understands ISO, so rows are rewritten to ISO first
//! (the SRID is returned separately), and standard WKB is rewritten to EWKB on output.

use crate::curves;

const FLAG_Z: u32 = 0x8000_0000;
const FLAG_M: u32 = 0x4000_0000;
const FLAG_SRID: u32 = 0x2000_0000;
const TYPE_MASK: u32 = 0x0FFF_FFFF;

/// Maximum collection nesting accepted when rewriting (the `wkb` 0.9.3 reader enforces its own
/// `MAX_WKB_NESTING_DEPTH` of 128; ours is stricter so the recursive rewrite cannot overflow the
/// stack first).
pub const MAX_NESTING_DEPTH: usize = 64;

/// Error text produced when [`MAX_NESTING_DEPTH`] is exceeded; callers treat it as fatal.
pub const NESTING_ERROR: &str = "WKB nesting too deep";

/// Rewrites EWKB (or ISO WKB, which passes through unchanged) to ISO WKB and returns the SRID
/// when the top-level geometry carries one.
pub fn ewkb_to_iso(bytes: &[u8]) -> Result<(Vec<u8>, Option<i32>), String> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut pos = 0;
    let srid = rewrite(bytes, &mut pos, &mut out, Target::Iso, true, None, 0)?;
    Ok((out, srid))
}

/// Rewrites WKB (ISO or EWKB) to EWKB with the given SRID on the top-level geometry.
pub fn wkb_to_ewkb(bytes: &[u8], srid: i32) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(bytes.len() + 4);
    let mut pos = 0;
    rewrite(bytes, &mut pos, &mut out, Target::Ewkb, true, Some(srid), 0)?;
    Ok(out)
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Iso,
    Ewkb,
}

pub(crate) fn take<'a>(bytes: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], String> {
    let end = pos
        .checked_add(n)
        .filter(|&end| end <= bytes.len())
        .ok_or_else(|| "truncated WKB".to_string())?;
    let slice = &bytes[*pos..end];
    *pos = end;
    Ok(slice)
}

pub(crate) fn read_u32(bytes: &[u8], pos: &mut usize, little: bool) -> Result<u32, String> {
    let raw: [u8; 4] = take(bytes, pos, 4)?.try_into().expect("4 bytes");
    Ok(if little {
        u32::from_le_bytes(raw)
    } else {
        u32::from_be_bytes(raw)
    })
}

fn write_u32(out: &mut Vec<u8>, value: u32, little: bool) {
    out.extend_from_slice(&if little {
        value.to_le_bytes()
    } else {
        value.to_be_bytes()
    });
}

fn rewrite(
    bytes: &[u8],
    pos: &mut usize,
    out: &mut Vec<u8>,
    target: Target,
    top: bool,
    srid_out: Option<i32>,
    depth: usize,
) -> Result<Option<i32>, String> {
    if depth > MAX_NESTING_DEPTH {
        return Err(NESTING_ERROR.to_string());
    }
    let order = take(bytes, pos, 1)?[0];
    let little = match order {
        0 => false,
        1 => true,
        other => return Err(format!("invalid WKB byte order {other}")),
    };
    let code = read_u32(bytes, pos, little)?;
    let low = code & TYPE_MASK;
    let (src_base, iso_dim) = (low % 1000, low / 1000);
    let z = code & FLAG_Z != 0 || iso_dim == 1 || iso_dim == 3;
    let m = code & FLAG_M != 0 || iso_dim == 2 || iso_dim == 3;
    let srid = if code & FLAG_SRID != 0 {
        Some(read_u32(bytes, pos, little)? as i32)
    } else {
        None
    };

    out.push(order);
    // Curves are written as their linear equivalents (see `curves`).
    let base = match src_base {
        8 | 9 => 2,
        10 => 3,
        11 => 5,
        12 => 6,
        other => other,
    };
    let new_code = match target {
        Target::Iso => base + 1000 * (u32::from(z) + 2 * u32::from(m)),
        Target::Ewkb => {
            let mut c = base;
            if z {
                c |= FLAG_Z;
            }
            if m {
                c |= FLAG_M;
            }
            if top && srid_out.is_some() {
                c |= FLAG_SRID;
            }
            c
        }
    };
    write_u32(out, new_code, little);
    if let (Target::Ewkb, true, Some(s)) = (target, top, srid_out) {
        write_u32(out, s as u32, little);
    }

    let ndim = 2 + usize::from(z) + usize::from(m);
    let coords = |pos: &mut usize, out: &mut Vec<u8>, n: usize| -> Result<(), String> {
        let len = n
            .checked_mul(ndim * 8)
            .ok_or_else(|| "truncated WKB".to_string())?;
        out.extend_from_slice(take(bytes, pos, len)?);
        Ok(())
    };
    let count = |pos: &mut usize, out: &mut Vec<u8>| -> Result<usize, String> {
        let n = read_u32(bytes, pos, little)?;
        write_u32(out, n, little);
        Ok(n as usize)
    };
    match src_base {
        1 => coords(pos, out, 1)?,
        8 | 9 => {
            let points = curves::read_curve_body(bytes, pos, little, src_base, z, m, depth)?;
            curves::write_points(out, &points, little, z, m);
        }
        10 => {
            let rings = read_u32(bytes, pos, little)?;
            write_u32(out, rings, little);
            for _ in 0..rings {
                if !matches!(curves::peek_base(bytes, *pos)?, 2 | 8 | 9) {
                    return Err("a curve polygon ring must be a line or curve".into());
                }
                let ring = curves::read_curve_member(bytes, pos, depth + 1)?;
                curves::write_points(out, &ring, little, z, m);
            }
        }
        11 | 12 => {
            let allowed: &[u32] = if src_base == 11 { &[2, 8, 9] } else { &[3, 10] };
            for _ in 0..count(pos, out)? {
                if !allowed.contains(&curves::peek_base(bytes, *pos)?) {
                    return Err(format!("invalid member of WKB type {src_base}"));
                }
                rewrite(bytes, pos, out, target, false, None, depth + 1)?;
            }
        }
        2 => {
            let n = count(pos, out)?;
            coords(pos, out, n)?;
        }
        3 => {
            for _ in 0..count(pos, out)? {
                let n = count(pos, out)?;
                coords(pos, out, n)?;
            }
        }
        4..=7 => {
            for _ in 0..count(pos, out)? {
                rewrite(bytes, pos, out, target, false, None, depth + 1)?;
            }
        }
        other => return Err(format!("unsupported WKB geometry type {other}")),
    }
    Ok(if top { srid } else { None })
}
