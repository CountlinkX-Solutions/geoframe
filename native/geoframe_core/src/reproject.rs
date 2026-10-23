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

/// A resolved CRS: canonical label plus the engine's projection.
struct Resolved {
    label: String,
    proj: Proj,
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
    let (label, proj_string) = match authority.trim().to_ascii_uppercase().as_str() {
        "EPSG" => {
            let code = parse_epsg_code(code, input)?;
            let def = crs_definitions::from_code(code)
                .ok_or_else(|| format!("unknown EPSG code {code}: not in the bundled table"))?;
            (format!("EPSG:{code}"), def.proj4)
        }
        "OGC" if code.trim().eq_ignore_ascii_case("CRS84") => ("OGC:CRS84".to_string(), CRS84_PROJ),
        other => {
            return Err(format!(
                "unsupported CRS authority {other:?} in {input:?}: expected EPSG or OGC:CRS84"
            ))
        }
    };
    let proj = Proj::from_proj_string(proj_string)
        .map_err(|e| format!("cannot build projection for {label}: {e}"))?;
    Ok(Resolved { label, proj })
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

/// Transforms one coordinate; geographic CRSs are in degrees outside, radians inside.
fn transform_coord(src: &Proj, dst: &Proj, c: Coord<f64>) -> Result<Coord<f64>, String> {
    let (mut x, mut y) = (c.x, c.y);
    if src.is_latlong() {
        x = x.to_radians();
        y = y.to_radians();
    }
    let mut point = (x, y);
    proj4rs::transform::transform(src, dst, &mut point).map_err(|e| e.to_string())?;
    let (mut x, mut y) = point;
    if dst.is_latlong() {
        x = x.to_degrees();
        y = y.to_degrees();
    }
    if x.is_finite() && y.is_finite() {
        Ok(Coord { x, y })
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
        let rows: Vec<Option<Geometry<f64>>> = self
            .geo_rows()
            .iter()
            .map(|row| {
                row.as_ref().and_then(|g| {
                    catch_unwind(AssertUnwindSafe(|| {
                        g.try_map_coords(|c| transform_coord(&src.proj, &dst.proj, c))
                            .ok()
                    }))
                    .ok()
                    .flatten()
                })
            })
            .collect();
        Ok(self.rebuild(&rows)?.set_crs(Some(&dst.label)))
    }

    /// The WGS 84 UTM CRS (`EPSG:326xx` north, `EPSG:327xx` south) whose zone contains the
    /// centre of the total bounds, like GeoPandas `estimate_utm_crs`. Fails without a CRS,
    /// without bounds, or outside the UTM latitude range (80 S to 84 N).
    pub fn estimate_utm_crs(&self) -> Result<String, String> {
        let source = self.crs().ok_or_else(|| {
            "cannot estimate a UTM CRS for naive geometries; set_crs first".to_string()
        })?;
        let [minx, miny, maxx, maxy] = self
            .total_bounds()
            .ok_or_else(|| "cannot estimate a UTM CRS: no non-empty geometries".to_string())?;
        let centre = Coord {
            x: (minx + maxx) / 2.0,
            y: (miny + maxy) / 2.0,
        };
        let (src, wgs84) = (resolve(&source)?, resolve("OGC:CRS84")?);
        let lonlat = transform_coord(&src.proj, &wgs84.proj, centre)?;
        if !(-80.0..=84.0).contains(&lonlat.y) {
            return Err(format!(
                "latitude {} is outside the UTM range (-80 to 84)",
                lonlat.y
            ));
        }
        let zone = (((lonlat.x + 180.0) / 6.0).floor() as i64).rem_euclid(60) + 1;
        let base = if lonlat.y >= 0.0 { 32600 } else { 32700 };
        Ok(format!("EPSG:{}", base + zone))
    }
}
