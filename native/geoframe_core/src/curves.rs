//! Curved geometries (WKB types 8-12) are linearized while WKB is normalised, so every
//! WKB-based reader (GeoPackage, PostGIS EWKB, `from_wkb`, GeoParquet / Feather WKB) accepts
//! them. GeoArrow and the `wkb` crate have no curve types; GDAL linearizes on read, and this
//! is a port of its arc stroking so the vertices agree with `pyogrio` / `GeoPandas`:
//!
//! * `OGRGeometryFactory::curveToLineString`, `GetCurveParameters`, `OGRGeometryFactoryStrokeArc`
//!   (`ogr/ogrgeometryfactory.cpp`, GDAL 3.12): the arc through three control points is
//!   stroked with inscribed vertices at equal angles; the step is `OGR_ARC_STEPSIZE` degrees
//!   (default 4) and the number of steps `trunc(|sweep| / step + 0.5)`. GDAL's default
//!   "stealth" mode forces an odd step count of at least 7 and hides the intermediate control
//!   point's angle in the 8 low mantissa bits of the computed vertices; both are reproduced, so
//!   the vertices are bit for bit GDAL's. Collinear points stay a straight segment, a closed
//!   arc is a full circle centred on the middle of the first and middle points, and an arc is
//!   stroked from the larger (x, y) end so a reversed arc gives the reversed vertices.
//! * `OGRCircularString::CurveToLine`: each three-point arc is stroked, the first vertex of
//!   every arc after the first is dropped; fewer than three points give an empty line.
//! * `OGRCompoundCurve::CurveToLineInternal`: members are concatenated, dropping the first
//!   vertex of every member after the first.
//! * `OGRCurvePolygon::CurvePolyToPoly`: every ring is linearized.
//!
//! GDAL interpolates Z linearly in the angle and drops M on arcs; M is interpolated here the
//! same way as Z, which only differs from GDAL where it would lose the ordinate.

use std::f64::consts::PI;
use std::sync::OnceLock;

use crate::ewkb::{read_u32, take};

const FLAG_Z: u32 = 0x8000_0000;
const FLAG_M: u32 = 0x4000_0000;
const FLAG_SRID: u32 = 0x2000_0000;
const TYPE_MASK: u32 = 0x0FFF_FFFF;

/// The default angular step in degrees, like GDAL's `OGR_ARC_STEPSIZE` (`GetDefaultArcStepSize`).
const DEFAULT_STEP_DEGREES: f64 = 4.0;

/// Arc step in degrees. `OGR_ARC_STEPSIZE` (read once per process, clamped to [0.01, 180] as
/// GDAL does) overrides the 4 degree default.
pub(crate) fn default_step() -> f64 {
    static STEP: OnceLock<f64> = OnceLock::new();
    *STEP.get_or_init(|| {
        std::env::var("OGR_ARC_STEPSIZE")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| !v.is_nan())
            .map_or(DEFAULT_STEP_DEGREES, |v| v.clamp(0.01, 180.0))
    })
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Pt {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub m: f64,
}

struct Arc {
    radius: f64,
    cx: f64,
    cy: f64,
    alpha0: f64,
    alpha1: f64,
    alpha2: f64,
}

/// `OGRGeometryFactory::GetCurveParameters`; `None` when the points are collinear or NaN.
fn curve_parameters(p0: Pt, p1: Pt, p2: Pt) -> Option<Arc> {
    let (x0, y0, x1, y1, x2, y2) = (p0.x, p0.y, p1.x, p1.y, p2.x, p2.y);
    if [x0, y0, x1, y1, x2, y2].iter().any(|v| v.is_nan()) {
        return None;
    }
    if x0 == x2 && y0 == y2 {
        if x0 != x1 || y0 != y1 {
            let cx = (x0 + x1) / 2.0;
            let cy = (y0 + y1) / 2.0;
            let alpha0 = (y0 - cy).atan2(x0 - cx);
            return Some(Arc {
                radius: (cx - x0).hypot(cy - y0),
                cx,
                cy,
                alpha0,
                alpha1: alpha0 + PI,
                alpha2: alpha0 + 2.0 * PI,
            });
        }
        return None;
    }
    let mut dx01 = x1 - x0;
    let mut dy01 = y1 - y0;
    let mut dx12 = x2 - x1;
    let mut dy12 = y2 - y1;
    let scale = dx01.abs().max(dy01.abs()).max(dx12.abs()).max(dy12.abs());
    let inv = 1.0 / scale;
    dx01 *= inv;
    dy01 *= inv;
    dx12 *= inv;
    dy12 *= inv;
    let det = dx01 * dy12 - dx12 * dy01;
    if det.abs() < 1.0e-8 || det.is_nan() {
        return None;
    }
    let x01_mid = (x0 + x1) * inv;
    let x12_mid = (x1 + x2) * inv;
    let y01_mid = (y0 + y1) * inv;
    let y12_mid = (y1 + y2) * inv;
    let c01 = dx01 * x01_mid + dy01 * y01_mid;
    let c12 = dx12 * x12_mid + dy12 * y12_mid;
    let cx = 0.5 * scale * (c01 * dy12 - c12 * dy01) / det;
    let cy = 0.5 * scale * (-c01 * dx12 + c12 * dx01) / det;
    let alpha0 = ((y0 - cy) * inv).atan2((x0 - cx) * inv);
    let mut alpha1 = ((y1 - cy) * inv).atan2((x1 - cx) * inv);
    let mut alpha2 = ((y2 - cy) * inv).atan2((x2 - cx) * inv);
    if det < 0.0 {
        if alpha1 > alpha0 {
            alpha1 -= 2.0 * PI;
        }
        if alpha2 > alpha1 {
            alpha2 -= 2.0 * PI;
        }
    } else {
        if alpha1 < alpha0 {
            alpha1 += 2.0 * PI;
        }
        if alpha2 < alpha1 {
            alpha2 += 2.0 * PI;
        }
    }
    Some(Arc {
        radius: (cx - x0).hypot(cy - y0),
        cx,
        cy,
        alpha0,
        alpha1,
        alpha2,
    })
}

/// `OGRGeometryFactoryStrokeArc` for the stealth case (the only one a read takes): appends the
/// intermediate vertices between `alpha0` and `alpha1`, Z and M interpolated from `from` to `to`.
fn stroke(out: &mut Vec<Pt>, arc: &Arc, from: Pt, to: Pt, alpha1: f64, step: f64) {
    let sign = if step > 0.0 { 1.0 } else { -1.0 };
    let alpha0 = arc.alpha0;
    let steps = ((alpha1 - alpha0) / step).abs() + 0.5;
    if steps.is_nan() || steps >= f64::from(i32::MAX) {
        return;
    }
    let mut n = steps as i32;
    // At least 6 intermediate vertices, then an even number more (an odd step count).
    n = if n < 7 { 7 } else { 7 + 2 * ((n - 7 + 1) / 2) };
    let step = sign * ((alpha1 - alpha0) / f64::from(n)).abs();
    let mut alpha = alpha0 + step;
    while (alpha - alpha1) * sign < -1e-8 {
        let t = (alpha - alpha0) / (alpha1 - alpha0);
        out.push(Pt {
            x: arc.cx + arc.radius * alpha.cos(),
            y: arc.cy + arc.radius * alpha.sin(),
            z: from.z + (to.z - from.z) * (alpha - alpha0) / (alpha1 - alpha0),
            m: from.m + (to.m - from.m) * t,
        });
        alpha += step;
    }
}

const HIDDEN_ALPHA_SCALE: f64 = 4_294_967_294.0; // 2^32 - 2
const HIDDEN_ALPHA_HALF_WIDTH: u32 = 16;
const HIDDEN_ALPHA_HALF_MASK: u32 = (1 << HIDDEN_ALPHA_HALF_WIDTH) - 1;

/// `OGRGF_SetHiddenValue`: the 16-bit value goes in the lowest mantissa byte of x (low half)
/// and y (high half).
fn set_hidden(value: u16, p: &mut Pt) {
    p.x = f64::from_bits((p.x.to_bits() & !0xFF) | u64::from(value & 0xFF));
    p.y = f64::from_bits((p.y.to_bits() & !0xFF) | u64::from(value >> 8));
}

/// `OGRGeometryFactory::curveToLineString` with GDAL's default options (`ADD_INTERMEDIATE_POINT`
/// = STEALTH): the vertices of the arc through `p0`, `p1`, `p2`, ends included.
pub(crate) fn linearize_arc(p0: Pt, p1: Pt, p2: Pt, step_degrees: f64) -> Vec<Pt> {
    // The same arc walked in both directions gives mirrored vertices.
    if p0.x < p2.x || (p0.x == p2.x && p0.y < p2.y) {
        let mut line = linearize_arc(p2, p1, p0, step_degrees);
        line.reverse();
        return line;
    }
    let Some(arc) = curve_parameters(p0, p1, p2) else {
        return vec![p0, p1, p2];
    };
    let sign = if arc.alpha1 >= arc.alpha0 { 1.0 } else { -1.0 };
    let step_degrees = if step_degrees < 1e-6 {
        default_step()
    } else {
        step_degrees
    };
    let step = (step_degrees / 180.0 * PI).max(0.01 / 180.0 * PI) * sign;

    let mut line = vec![p0];
    stroke(&mut line, &arc, p0, p2, arc.alpha2, step);
    if line.len() > 6 {
        // Hide the intermediate point's angle ratio in the low bits so that
        // `curveFromLineString` can recover it; reproduced for bit-exact vertices.
        let ratio = (arc.alpha1 - arc.alpha0) / (arc.alpha2 - arc.alpha0);
        let mut alpha_ratio = 0.5 + HIDDEN_ALPHA_SCALE * ratio;
        if alpha_ratio < 0.0 {
            alpha_ratio *= -1.0;
        } else if alpha_ratio >= f64::from(u32::MAX) || alpha_ratio.is_nan() {
            alpha_ratio = f64::from(u32::MAX);
        }
        let ratio = alpha_ratio as u32;
        let low = (ratio & HIDDEN_ALPHA_HALF_MASK) as u16;
        let high = (ratio >> HIDDEN_ALPHA_HALF_WIDTH) as u16;
        let n = line.len();
        let mut i = 1;
        while i + 1 < n {
            let (mut first, mut second) = (0xFFFF, 0xFFFF);
            if i == 1 {
                first = low;
                second = high;
            } else if i == n - 2 {
                first = high;
                second = low;
            }
            set_hidden(first, &mut line[i]);
            set_hidden(second, &mut line[i + 1]);
            i += 2;
        }
    }
    line.push(p2);
    line
}

/// `OGRCircularString::CurveToLine` over the control points of one circular string.
pub(crate) fn linearize_circular(points: &[Pt]) -> Vec<Pt> {
    let step = default_step();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < points.len() {
        let arc = linearize_arc(points[i], points[i + 1], points[i + 2], step);
        out.extend(arc.into_iter().skip(usize::from(i > 0)));
        i += 2;
    }
    out
}

fn read_f64(bytes: &[u8], pos: &mut usize, little: bool) -> Result<f64, String> {
    let raw: [u8; 8] = take(bytes, pos, 8)?.try_into().expect("8 bytes");
    Ok(if little {
        f64::from_le_bytes(raw)
    } else {
        f64::from_be_bytes(raw)
    })
}

fn read_points(
    bytes: &[u8],
    pos: &mut usize,
    little: bool,
    z: bool,
    m: bool,
) -> Result<Vec<Pt>, String> {
    let n = read_u32(bytes, pos, little)? as usize;
    let ndim = 2 + usize::from(z) + usize::from(m);
    let len = n
        .checked_mul(ndim * 8)
        .ok_or_else(|| "truncated WKB".to_string())?;
    // Checked up front so a huge count cannot allocate before the bytes prove it is real.
    let mut chunk = take(bytes, pos, len)?;
    let mut points = Vec::with_capacity(n);
    let mut at = 0;
    for _ in 0..n {
        let x = read_f64(chunk, &mut at, little)?;
        let y = read_f64(chunk, &mut at, little)?;
        let zv = if z {
            read_f64(chunk, &mut at, little)?
        } else {
            0.0
        };
        let mv = if m {
            read_f64(chunk, &mut at, little)?
        } else {
            0.0
        };
        points.push(Pt { x, y, z: zv, m: mv });
    }
    chunk = &chunk[at..];
    debug_assert!(chunk.is_empty());
    Ok(points)
}

/// Peeks at the type of the geometry starting at `pos` without consuming it.
pub(crate) fn peek_base(bytes: &[u8], pos: usize) -> Result<u32, String> {
    let mut at = pos;
    let order = take(bytes, &mut at, 1)?[0];
    let little = match order {
        0 => false,
        1 => true,
        other => return Err(format!("invalid WKB byte order {other}")),
    };
    Ok((read_u32(bytes, &mut at, little)? & TYPE_MASK) % 1000)
}

/// The linear vertices of the line-like geometry (LineString 2, CircularString 8, CompoundCurve
/// 9) whose header (byte order and type word) starts at `pos`.
pub(crate) fn read_curve_member(
    bytes: &[u8],
    pos: &mut usize,
    depth: usize,
) -> Result<Vec<Pt>, String> {
    if depth > crate::ewkb::MAX_NESTING_DEPTH {
        return Err(crate::ewkb::NESTING_ERROR.to_string());
    }
    let order = take(bytes, pos, 1)?[0];
    let little = match order {
        0 => false,
        1 => true,
        other => return Err(format!("invalid WKB byte order {other}")),
    };
    let code = read_u32(bytes, pos, little)?;
    let low = code & TYPE_MASK;
    let (base, iso_dim) = (low % 1000, low / 1000);
    let z = code & FLAG_Z != 0 || iso_dim == 1 || iso_dim == 3;
    let m = code & FLAG_M != 0 || iso_dim == 2 || iso_dim == 3;
    if code & FLAG_SRID != 0 {
        read_u32(bytes, pos, little)?;
    }
    read_curve_body(bytes, pos, little, base, z, m, depth)
}

/// The body (after the header) of a LineString, CircularString or CompoundCurve, linearized.
pub(crate) fn read_curve_body(
    bytes: &[u8],
    pos: &mut usize,
    little: bool,
    base: u32,
    z: bool,
    m: bool,
    depth: usize,
) -> Result<Vec<Pt>, String> {
    match base {
        2 => read_points(bytes, pos, little, z, m),
        8 => Ok(linearize_circular(&read_points(bytes, pos, little, z, m)?)),
        9 => {
            let members = read_u32(bytes, pos, little)? as usize;
            let mut line: Vec<Pt> = Vec::new();
            for index in 0..members {
                if !matches!(peek_base(bytes, *pos)?, 2 | 8) {
                    return Err("a compound curve holds only line and circular strings".into());
                }
                let part = read_curve_member(bytes, pos, depth + 1)?;
                line.extend(part.into_iter().skip(usize::from(index > 0)));
            }
            Ok(line)
        }
        other => Err(format!("unsupported WKB curve member type {other}")),
    }
}

/// Appends a point count and the coordinates of `points` in the given dimension.
pub(crate) fn write_points(out: &mut Vec<u8>, points: &[Pt], little: bool, z: bool, m: bool) {
    let put = |out: &mut Vec<u8>, v: f64| {
        out.extend_from_slice(&if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        });
    };
    let n = points.len() as u32;
    out.extend_from_slice(&if little {
        n.to_le_bytes()
    } else {
        n.to_be_bytes()
    });
    for p in points {
        put(out, p.x);
        put(out, p.y);
        if z {
            put(out, p.z);
        }
        if m {
            put(out, p.m);
        }
    }
}
