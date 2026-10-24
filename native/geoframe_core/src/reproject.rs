//! Coordinate reprojection in pure Rust (`proj4rs` + the `crs-definitions` EPSG table).
//!
//! # CRS identification
//!
//! A CRS is identified by an authority code: `EPSG:<code>` (any code in the bundled
//! `crs-definitions` table) or `OGC:CRS84`. A CRS stored as PROJJSON is accepted when it
//! carries a top-level `id` with an authority and code; PROJJSON without one is rejected
//! because the definition cannot be mapped to a proj string without a full PROJ database.
//! Labels are canonicalised (`epsg:3857` becomes `EPSG:3857`).
//!
//! # Axis order
//!
//! Like GeoPandas with `always_xy=True`, x is always easting/longitude and y northing/latitude,
//! for every CRS including `EPSG:4326` (whose official axis order is lat/lon) and `OGC:CRS84`.
//! This matches GeoArrow and GeoParquet storage. `proj4rs` works in radians for geographic
//! CRSs, so degrees are converted at the boundaries.
//!
//! # Null policy and dimensions
//!
//! Rows that are null, unrepresentable, or whose transform fails for any vertex (outside the
//! projection's domain, non-finite result) become null. Output is 2D; Z/M are dropped.

use std::panic::{catch_unwind, AssertUnwindSafe};

use geo::{Coord, Geometry, MapCoords};
use proj4rs::Proj;
use serde_json::Value;

use crate::GeoColumn;

/// Proj string for `OGC:CRS84`: WGS 84 as lon/lat.
const CRS84_PROJ: &str = "+proj=longlat +datum=WGS84 +no_defs";

/// A resolved CRS: canonical label plus the engine's projection. `fallback` is the Helmert
/// variant of `proj` when `proj` shifts its datum with grids (see [`crate::datum`]); points
/// outside every grid use it.
struct Resolved {
    label: String,
    proj: Proj,
    fallback: Option<Proj>,
    /// EPSG code of the geographic CRS the definition is based on, the key of its grids.
    base: Option<u32>,
}

fn parse_epsg_code(text: &str, input: &str) -> Result<u16, String> {
    let code: u32 = text
        .trim()
        .parse()
        .map_err(|_| format!("invalid CRS {input:?}: EPSG code must be an integer"))?;
    u16::try_from(code).map_err(|_| format!("unknown EPSG code {code}: not in the bundled table"))
}

/// Pulls `AUTHORITY:code` out of a PROJJSON object's top-level `id`.
fn projjson_label(value: &Value) -> Result<String, String> {
    let id = value.get("id").ok_or_else(|| {
        "unsupported PROJJSON CRS: no top-level `id` with authority and code".to_string()
    })?;
    let authority = id.get("authority").and_then(Value::as_str);
    let code = id.get("code").and_then(|c| match c {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    });
    match (authority, code) {
        (Some(a), Some(c)) => Ok(format!("{a}:{c}")),
        _ => Err("unsupported PROJJSON CRS: `id` lacks authority or code".to_string()),
    }
}

fn resolve(input: &str) -> Result<Resolved, String> {
    let text = input.trim();
    if text.starts_with('{') {
        let value: Value =
            serde_json::from_str(text).map_err(|e| format!("invalid PROJJSON CRS: {e}"))?;
        return resolve(&projjson_label(&value)?);
    }
    let (authority, code) = text
        .split_once(':')
        .ok_or_else(|| format!("unsupported CRS {input:?}: expected EPSG:<code> or OGC:CRS84"))?;
    let (label, proj_string, base) = match authority.trim().to_ascii_uppercase().as_str() {
        "EPSG" => {
            let code = parse_epsg_code(code, input)?;
            let def = crs_definitions::from_code(code)
                .ok_or_else(|| format!("unknown EPSG code {code}: not in the bundled table"))?;
            (
                format!("EPSG:{code}"),
                def.proj4,
                crate::datum::base_geographic_code(def.wkt),
            )
        }
        "OGC" if code.trim().eq_ignore_ascii_case("CRS84") => {
            ("OGC:CRS84".to_string(), CRS84_PROJ, None)
        }
        other => {
            return Err(format!(
                "unsupported CRS authority {other:?} in {input:?}: expected EPSG or OGC:CRS84"
            ))
        }
    };
    let (proj, fallback) = crate::datum::build(proj_string, base)
        .map_err(|e| format!("cannot build projection for {label}: {e}"))?;
    Ok(Resolved {
        label,
        proj,
        fallback,
        base,
    })
}

/// Uses the grid files `names` (PROJ-data names, tried in order) for the datum of `input`, a
/// geographic CRS or a projected one on it, instead of the built-in table; an empty list turns
/// grids off for that datum. Returns the EPSG code of the geographic CRS the entry is keyed on.
pub fn register_grids_for(input: &str, names: Vec<String>) -> Result<u32, String> {
    let base = datum_of(input)?;
    crate::datum::register_grids(base, names);
    Ok(base)
}

/// Removes a [`register_grids_for`] entry; the built-in table applies again.
pub fn unregister_grids_for(input: &str) -> Result<u32, String> {
    let base = datum_of(input)?;
    crate::datum::unregister_grids(base);
    Ok(base)
}

fn datum_of(input: &str) -> Result<u32, String> {
    resolve(input)?
        .base
        .ok_or_else(|| format!("{input:?} is not based on an EPSG geographic CRS"))
}

/// The grid files that may shift the datum of `input` and where each is installed (`None`:
/// not found in the grid directories). Empty for a CRS whose datum has no grid.
pub fn grid_status(input: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let base = resolve(input)?.base;
    Ok(base
        .map(crate::datum::grid_names)
        .unwrap_or_default()
        .into_iter()
        .map(|name| {
            let path = crate::datum::locate(&name).map(|p| p.display().to_string());
            (name, path)
        })
        .collect())
}

/// Normalises a CRS string (`EPSG:<code>`, `OGC:CRS84`, or PROJJSON with an `id`) to its
/// canonical label, failing for unknown or unsupported CRSs.
pub fn normalize_crs(input: &str) -> Result<String, String> {
    resolve(input).map(|r| r.label)
}

/// Whether two CRS labels name the same CRS for compatibility checks. Labels are compared
/// after resolution, so case differences (`epsg:4326`) do not matter and `OGC:CRS84` equals
/// `EPSG:4326` (GeoFrame always uses x = longitude, y = latitude). Labels that cannot be
/// resolved fall back to a case-insensitive comparison of the trimmed text. Stored labels are
/// never rewritten.
pub fn crs_equivalent(a: &str, b: &str) -> bool {
    let canonical = |label: String| {
        if label == "OGC:CRS84" {
            "EPSG:4326".to_string()
        } else {
            label
        }
    };
    match (normalize_crs(a), normalize_crs(b)) {
        (Ok(a), Ok(b)) => canonical(a) == canonical(b),
        _ => a.trim().eq_ignore_ascii_case(b.trim()),
    }
}

/// Whether `input` names a geographic (lon/lat) CRS; `None` when it cannot be resolved.
pub fn crs_is_geographic(input: &str) -> Option<bool> {
    resolve(input).ok().map(|r| r.proj.is_latlong())
}

/// Runs `f` with the projections of both CRSs, then, when a point lies outside every datum
/// grid, once more with their Helmert variants (what PROJ does by trying its next operation).
fn with_fallback<T>(
    src: &Resolved,
    dst: &Resolved,
    f: impl Fn(&Proj, &Proj) -> Result<T, proj4rs::errors::Error>,
) -> Result<T, String> {
    match f(&src.proj, &dst.proj) {
        Err(proj4rs::errors::Error::PointOutsideNadShiftArea)
            if src.fallback.is_some() || dst.fallback.is_some() =>
        {
            f(
                src.fallback.as_ref().unwrap_or(&src.proj),
                dst.fallback.as_ref().unwrap_or(&dst.proj),
            )
        }
        other => other,
    }
    .map_err(|e| e.to_string())
}

/// Transforms one coordinate; geographic CRSs are in degrees outside, radians inside.
fn transform_coord(src: &Resolved, dst: &Resolved, c: Coord<f64>) -> Result<Coord<f64>, String> {
    let (x, y) = with_fallback(src, dst, |src, dst| {
        let (mut x, mut y) = (c.x, c.y);
        if src.is_latlong() {
            x = x.to_radians();
            y = y.to_radians();
        }
        let mut point = (x, y);
        proj4rs::transform::transform(src, dst, &mut point)?;
        let (mut x, mut y) = point;
        if dst.is_latlong() {
            x = x.to_degrees();
            y = y.to_degrees();
        }
        Ok((x, y))
    })?;
    if x.is_finite() && y.is_finite() {
        Ok(Coord { x, y })
    } else {
        Err("non-finite result".to_string())
    }
}

/// Like [`transform_coord`] with a height. The height is unchanged (as PROJ leaves it between
/// two CRSs that are not geocentric and share a datum, which is every pair the bundled table
/// offers for 2D use); it is transformed only when either CRS is geocentric. A NaN `z` (no Z
/// ordinate) stays NaN.
fn transform_coord3(
    src: &Resolved,
    dst: &Resolved,
    c: Coord<f64>,
    z: f64,
) -> Result<(Coord<f64>, f64), String> {
    if z.is_nan() || !(src.proj.is_geocent() || dst.proj.is_geocent()) {
        return Ok((transform_coord(src, dst, c)?, z));
    }
    let (x, y, nz) = with_fallback(src, dst, |src, dst| {
        let (mut x, mut y) = (c.x, c.y);
        if src.is_latlong() {
            x = x.to_radians();
            y = y.to_radians();
        }
        let mut point = (x, y, z);
        proj4rs::transform::transform(src, dst, &mut point)?;
        let (mut x, mut y, nz) = point;
        if dst.is_latlong() {
            x = x.to_degrees();
            y = y.to_degrees();
        }
        Ok((x, y, nz))
    })?;
    if x.is_finite() && y.is_finite() && nz.is_finite() {
        Ok((Coord { x, y }, nz))
    } else {
        Err("non-finite result".to_string())
    }
}

impl GeoColumn {
    /// Reprojects every coordinate from this column's CRS to `target` and sets `target` as the
    /// CRS. Fails when this column has no CRS or either CRS is unknown. Rows whose transform
    /// fails become null. Reprojecting to the same CRS returns an unchanged copy.
    pub fn to_crs(&self, target: &str) -> Result<Self, String> {
        let source = self
            .crs()
            .ok_or_else(|| "cannot transform naive geometries; set_crs first".to_string())?;
        let (src, dst) = (resolve(&source)?, resolve(target)?);
        if src.label == dst.label {
            return Ok(self.clone());
        }
        let moved = if self.any_zm() {
            // Z rides along: heights are transformed only where the CRS pair needs it
            // (geographic 3D to geocentric); otherwise they are unchanged. M is dropped.
            self.map_vertices(|_, _| {
                let (src, dst) = (&src, &dst);
                Some(Box::new(move |x, y, z, _m| {
                    let c = transform_coord3(src, dst, Coord { x, y }, z)?;
                    Ok((c.0.x, c.0.y, c.1))
                }))
            })?
        } else {
            let rows: Vec<Option<Geometry<f64>>> = self
                .geo_rows()
                .iter()
                .map(|row| {
                    row.as_ref().and_then(|g| {
                        catch_unwind(AssertUnwindSafe(|| {
                            g.try_map_coords(|c| transform_coord(&src, &dst, c)).ok()
                        }))
                        .ok()
                        .flatten()
                    })
                })
                .collect();
            self.rebuild(&rows)?
        };
        Ok(moved.set_crs(Some(&dst.label)))
    }

    /// The UTM CRS whose area of use holds the centre of the total bounds, like GeoPandas
    /// `estimate_utm_crs` (`pyproj.database.query_utm_crs_info` with the centre as area of
    /// interest, first match by EPSG code). `datum_name` is the datum in the CRS name
    /// (default `"WGS 84"`; `"NAD83"`, `"NAD27"`, `"ETRS89"`, ...), compared without spaces.
    ///
    /// A geographic CRS uses the centre of its raw bounds. Any other CRS has its bounds
    /// converted to lon/lat first (edges densified, poles and the antimeridian handled like
    /// PROJ's `transform_bounds`) and the centre of that box is used. The Norway and Svalbard
    /// exceptions of the MGRS grid are not applied, because pyproj does not apply them either.
    /// Fails without a CRS, without bounds, or when no UTM CRS of that datum covers the centre
    /// (WGS 84: outside 80 S to 84 N).
    pub fn estimate_utm_crs(&self, datum_name: Option<&str>) -> Result<String, String> {
        let source = self.crs().ok_or_else(|| {
            "cannot estimate a UTM CRS for naive geometries; set_crs first".to_string()
        })?;
        let bounds = self
            .total_bounds()
            .ok_or_else(|| "cannot estimate a UTM CRS: no non-empty geometries".to_string())?;
        let src = resolve(&source)?;
        let [minx, miny, maxx, maxy] = if src.proj.is_latlong() {
            bounds
        } else {
            bounds_to_lonlat(&src, &resolve("OGC:CRS84")?, bounds)?
        };
        let lon = if minx > maxx {
            // Crossed the antimeridian: shift maxx to [0, 360), centre, shift back.
            (((minx + maxx + 360.0) / 2.0 + 180.0).rem_euclid(360.0)) - 180.0
        } else {
            (minx + maxx) / 2.0
        };
        let lat = (miny + maxy) / 2.0;
        // Round-off from the inverse projection (1e-16 degrees below the equator, say) must not
        // move a centre across a zone edge or the equator.
        let snap = |v: f64| (v * 1e9).round() / 1e9;
        let (lon, lat) = (snap(lon), snap(lat));
        let datum = datum_name.unwrap_or("WGS 84");
        crate::utm::utm_crs_code(datum, lon, lat)
            .map(|code| format!("EPSG:{code}"))
            .ok_or_else(|| {
                format!(
                    "Unable to determine UTM CRS: no UTM CRS with datum {datum:?} covers \
                     lon {lon}, lat {lat}"
                )
            })
    }
}

/// Number of points added along each edge when converting bounds (pyproj's default).
const DENSIFY_POINTS: usize = 21;

/// The lon/lat bounds `[west, south, east, north]` of a box in `src` (west > east when the box
/// crosses the antimeridian), like `Transformer.transform_bounds` with the default
/// densification: the box edges are sampled, a pole inside the box widens it to every
/// longitude, and a jump of more than 180 degrees between neighbouring samples marks an
/// antimeridian crossing.
fn bounds_to_lonlat(src: &Resolved, lonlat: &Resolved, b: [f64; 4]) -> Result<[f64; 4], String> {
    let [minx, miny, maxx, maxy] = b;
    let steps = DENSIFY_POINTS + 1;
    let mut ring = Vec::with_capacity(4 * steps);
    for i in 0..steps {
        let t = i as f64 / steps as f64;
        ring.push(Coord {
            x: minx + t * (maxx - minx),
            y: miny,
        });
    }
    for i in 0..steps {
        let t = i as f64 / steps as f64;
        ring.push(Coord {
            x: maxx,
            y: miny + t * (maxy - miny),
        });
    }
    for i in 0..steps {
        let t = i as f64 / steps as f64;
        ring.push(Coord {
            x: maxx - t * (maxx - minx),
            y: maxy,
        });
    }
    for i in 0..steps {
        let t = i as f64 / steps as f64;
        ring.push(Coord {
            x: minx,
            y: maxy - t * (maxy - miny),
        });
    }
    let moved: Vec<Coord<f64>> = ring
        .into_iter()
        .filter_map(|c| transform_coord(src, lonlat, c).ok())
        .collect();
    if moved.is_empty() {
        return Err("cannot estimate a UTM CRS: the bounds cannot be converted to lon/lat".into());
    }
    let (mut west, mut east) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut south, mut north) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut west_positive, mut east_negative) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut crosses = false;
    for (i, c) in moved.iter().enumerate() {
        west = west.min(c.x);
        east = east.max(c.x);
        south = south.min(c.y);
        north = north.max(c.y);
        if c.x >= 0.0 {
            west_positive = west_positive.min(c.x);
        } else {
            east_negative = east_negative.max(c.x);
        }
        let prev = moved[(i + moved.len() - 1) % moved.len()];
        if (c.x - prev.x).abs() > 180.0 {
            crosses = true;
        }
    }
    let inside = |pole_lat: f64| {
        transform_coord(
            lonlat,
            src,
            Coord {
                x: 0.0,
                y: pole_lat,
            },
        )
        .is_ok_and(|p| p.x >= minx && p.x <= maxx && p.y >= miny && p.y <= maxy)
    };
    Ok(if inside(90.0) {
        [-180.0, south, 180.0, 90.0]
    } else if inside(-90.0) {
        [-180.0, -90.0, 180.0, north]
    } else if crosses {
        [west_positive, south, east_negative, north]
    } else {
        [west, south, east, north]
    })
}
