//! Z/M dimension support.
//!
//! The column already stores every dimension (GeoArrow's `geometry` union has XY, XYZ, XYM and
//! XYZM children), so this module only reads the stored dimension and rewrites it:
//!
//! * [`GeoColumn::has_z`] / [`GeoColumn::has_m`]: the dimension a row was stored with. An empty
//!   geometry keeps the dimension it was declared with (`POINT Z EMPTY` has Z); a collection
//!   has Z (or M) when any member does.
//! * [`GeoColumn::z`] / [`GeoColumn::m`]: the ordinate of a point; null for every other
//!   geometry, empty points, missing ordinates and NaN (Shapely gives NaN or raises).
//! * [`GeoColumn::force_2d`] / [`GeoColumn::force_3d`]: dimension changes, applied per
//!   geometry (so per member of a collection) by rewriting its ISO WKB, which keeps every
//!   other ordinate bit for bit.

use geo_traits::{CoordTrait, GeometryCollectionTrait, GeometryTrait, GeometryType, PointTrait};
use geoarrow_array::builder::GeometryBuilder;

use crate::ewkb::{MAX_NESTING_DEPTH, NESTING_ERROR};
use crate::GeoColumn;

/// `(has_z, has_m)` of a geometry.
pub(crate) fn dimensions<G: GeometryTrait<T = f64>>(geometry: &G) -> (bool, bool) {
    if let GeometryType::GeometryCollection(collection) = geometry.as_type() {
        return collection
            .geometries()
            .fold((false, false), |(z, m), member| {
                let (mz, mm) = dimensions(&member);
                (z || mz, m || mm)
            });
    }
    flags(geometry.dim())
}

pub(crate) fn flags(dim: geo_traits::Dimensions) -> (bool, bool) {
    use geo_traits::Dimensions::{Unknown, Xy, Xym, Xyz, Xyzm};
    match dim {
        Xy | Unknown(0..=2) => (false, false),
        Xyz | Unknown(3) => (true, false),
        Xym => (false, true),
        Xyzm | Unknown(_) => (true, true),
    }
}

/// `(z, m)` of a point geometry; `None` entries when absent, empty or NaN.
fn point_ordinates<G: GeometryTrait<T = f64>>(geometry: &G) -> (Option<f64>, Option<f64>) {
    let GeometryType::Point(point) = geometry.as_type() else {
        return (None, None);
    };
    let Some(coord) = point.coord() else {
        return (None, None);
    };
    let finite = |value: f64| Some(value).filter(|v| !v.is_nan());
    match flags(coord.dim()) {
        (true, true) => (finite(coord.nth_or_panic(2)), finite(coord.nth_or_panic(3))),
        (true, false) => (finite(coord.nth_or_panic(2)), None),
        (false, true) => (None, finite(coord.nth_or_panic(2))),
        (false, false) => (None, None),
    }
}

impl GeoColumn {
    /// Whether each geometry has a Z ordinate; `false` for null rows.
    pub fn has_z(&self) -> Vec<bool> {
        self.map_scalars(|scalar| dimensions(scalar).0)
            .into_iter()
            .map(|row| row.unwrap_or(false))
            .collect()
    }

    /// Whether each geometry has an M ordinate; `false` for null rows.
    pub fn has_m(&self) -> Vec<bool> {
        self.map_scalars(|scalar| dimensions(scalar).1)
            .into_iter()
            .map(|row| row.unwrap_or(false))
            .collect()
    }

    /// Z ordinate of points; null for every other geometry, empty points, 2D points and nulls.
    pub fn z(&self) -> Vec<Option<f64>> {
        self.map_scalars(|scalar| point_ordinates(scalar).0)
            .into_iter()
            .map(Option::flatten)
            .collect()
    }

    /// M ordinate of points; same rules as [`z`](Self::z).
    pub fn m(&self) -> Vec<Option<f64>> {
        self.map_scalars(|scalar| point_ordinates(scalar).1)
            .into_iter()
            .map(Option::flatten)
            .collect()
    }

    /// Drops Z and M from every geometry.
    pub fn force_2d(&self) -> Result<Self, String> {
        self.rewrite_dimensions(|_| Edit::Drop)
    }

    /// Gives every geometry without Z a Z ordinate of `z`: a single value for all rows or one
    /// per row (a missing value is NaN). Following Shapely 2.2, a geometry that already has
    /// Z keeps it (its M is dropped) and a measured geometry without Z is left unchanged.
    pub fn force_3d(&self, z: &[Option<f64>]) -> Result<Self, String> {
        if z.len() != 1 && z.len() != self.len() {
            return Err(format!(
                "z must be one value or one per row: length {} for {} rows",
                z.len(),
                self.len()
            ));
        }
        self.rewrite_dimensions(|row| {
            let value = if z.len() == 1 { z[0] } else { z[row] };
            Edit::Force3d(value.unwrap_or(f64::NAN))
        })
    }

    fn rewrite_dimensions(&self, edit: impl Fn(usize) -> Edit<'static>) -> Result<Self, String> {
        let mut builder = GeometryBuilder::new(self.array_type());
        for (row, bytes) in self.to_wkb()?.into_iter().enumerate() {
            let rewritten = bytes.map(|b| rewrite(&b, edit(row))).transpose()?;
            let parsed = rewritten
                .as_deref()
                .map(wkb::reader::read_wkb)
                .transpose()
                .map_err(|e| e.to_string())?;
            builder
                .push_geometry(parsed.as_ref())
                .map_err(|e| e.to_string())?;
        }
        Ok(self.with_array(builder.finish()))
    }
}

/// Visits (and maps) one vertex: `(x, y, z, m)` in, `(x, y, z)` out. `z` and `m` are NaN when
/// the geometry lacks them; the output Z is written only when the geometry has Z.
pub(crate) type VertexFn<'a> = dyn Fn(f64, f64, f64, f64) -> Result<(f64, f64, f64), String> + 'a;

#[derive(Clone, Copy)]
pub(crate) enum Edit<'a> {
    Drop,
    Force3d(f64),
    /// Rewrites every vertex with the function; M is dropped, Z is kept.
    Map(&'a VertexFn<'a>),
    /// Reverses the vertex order of every line and ring; keeps Z and M.
    Reverse,
}

/// Rewrites ISO WKB (as written by the column) under `edit`.
pub(crate) fn rewrite(bytes: &[u8], edit: Edit<'_>) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 4);
    let mut pos = 0;
    node(bytes, &mut pos, &mut out, edit, 0)?;
    Ok(out)
}

fn take<'a>(bytes: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], String> {
    let end = pos
        .checked_add(n)
        .filter(|&end| end <= bytes.len())
        .ok_or_else(|| "truncated WKB".to_string())?;
    let slice = &bytes[*pos..end];
    *pos = end;
    Ok(slice)
}

fn node(
    bytes: &[u8],
    pos: &mut usize,
    out: &mut Vec<u8>,
    edit: Edit<'_>,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_NESTING_DEPTH {
        return Err(NESTING_ERROR.to_string());
    }
    let order = take(bytes, pos, 1)?[0];
    let little = order == 1;
    let u32_at = |slice: &[u8]| {
        let raw: [u8; 4] = slice.try_into().expect("4 bytes");
        if little {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        }
    };
    let put_u32 = |out: &mut Vec<u8>, value: u32| {
        out.extend_from_slice(&if little {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        });
    };
    let code = u32_at(take(bytes, pos, 4)?);
    let (base, dim) = (code % 1000, code / 1000);
    let (z, m) = (dim & 1 == 1, dim & 2 == 2);
    let (new_z, new_m, fill) = match edit {
        Edit::Drop => (false, false, f64::NAN),
        Edit::Force3d(fill) if z => (true, false, fill),
        Edit::Force3d(_) if m => (false, true, f64::NAN),
        Edit::Force3d(fill) => (true, false, fill),
        Edit::Map(_) => (z, false, f64::NAN),
        Edit::Reverse => (z, m, f64::NAN),
    };
    out.push(order);
    put_u32(out, base + 1000 * (u32::from(new_z) + 2 * u32::from(new_m)));

    let width = 2 + usize::from(z) + usize::from(m);
    let f64_at = |slice: &[u8]| {
        let raw: [u8; 8] = slice.try_into().expect("8 bytes");
        if little {
            f64::from_le_bytes(raw)
        } else {
            f64::from_be_bytes(raw)
        }
    };
    let coords = |pos: &mut usize, out: &mut Vec<u8>, n: usize| -> Result<(), String> {
        let len = n
            .checked_mul(width * 8)
            .ok_or_else(|| "truncated WKB".to_string())?;
        let block = take(bytes, pos, len)?;
        let mut rows: Vec<Vec<f64>> = Vec::with_capacity(n);
        for coord in block.chunks_exact(width * 8) {
            let values: Vec<f64> = coord
                .as_chunks::<8>()
                .0
                .iter()
                .map(|raw| f64_at(raw))
                .collect();
            let empty_point = values[0].is_nan() && values[1].is_nan();
            let mut row = vec![values[0], values[1]];
            let mapped = match edit {
                Edit::Map(f) if !empty_point => {
                    let zin = if z { values[2] } else { f64::NAN };
                    let min = if m { values[width - 1] } else { f64::NAN };
                    Some(f(values[0], values[1], zin, min)?)
                }
                _ => None,
            };
            if let Some((x, y, _)) = mapped {
                row = vec![x, y];
            }
            if new_z {
                row.push(match mapped {
                    Some((_, _, nz)) => nz,
                    None if z => values[2],
                    None if empty_point => f64::NAN,
                    None => fill,
                });
            }
            if new_m {
                row.push(values[width - 1]);
            }
            rows.push(row);
        }
        if matches!(edit, Edit::Reverse) {
            rows.reverse();
        }
        for value in rows.into_iter().flatten() {
            out.extend_from_slice(&if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            });
        }
        Ok(())
    };
    let count = |pos: &mut usize, out: &mut Vec<u8>| -> Result<usize, String> {
        let n = u32_at(take(bytes, pos, 4)?);
        put_u32(out, n);
        Ok(n as usize)
    };
    match base {
        1 => coords(pos, out, 1)?,
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
                node(bytes, pos, out, edit, depth + 1)?;
            }
        }
        other => return Err(format!("unsupported WKB geometry type {other}")),
    }
    Ok(())
}
