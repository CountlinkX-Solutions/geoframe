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

use crate::proj_ext::{self, Ext};
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
    /// Helmert operations of the datum by area of use (see [`crate::datum::area_variants`]):
    /// the first whose area holds the point replaces the single `+towgs84` set of the table.
    areas: Vec<([f64; 4], Proj)>,
    /// Set for a projection `proj4rs` lacks (see [`crate::proj_ext`]); `proj` is then the
    /// geographic CRS under it and coordinates go through this first.
    ext: Option<Ext>,
    /// Radians per unit of a geographic CRS (grads for the Paris-meridian CRSs).
    angular: f64,
}

impl Resolved {
    /// Whether coordinates are angles (a geographic CRS).
    fn is_geographic(&self) -> bool {
        self.ext.is_none() && self.proj.is_latlong()
    }
}

/// Definitions where the bundled table differs from the current EPSG registry (as PROJ 9.8 has it).
fn table_fix(code: u16) -> Option<&'static str> {
    match code {
        // False easting is 40 000 chains (804 670.24 m), not 40 000 m.
        3167 => Some(
            "+proj=omerc +lat_0=4 +lonc=102.25 +alpha=323.0257905 +k=0.99984 +x_0=804670.24 \
             +y_0=0 +no_uoff +gamma=323.1301023611111 +a=6377295.664 +b=6356094.667915204 \
             +to_meter=20.116756 +no_defs",
        ),
        _ => None,
    }
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
    let (label, proj_string, base, angular) = match authority.trim().to_ascii_uppercase().as_str() {
        "EPSG" => {
            let code = parse_epsg_code(code, input)?;
            let def = crs_definitions::from_code(code)
                .ok_or_else(|| format!("unknown EPSG code {code}: not in the bundled table"))?;
            (
                format!("EPSG:{code}"),
                crate::datum::with_axes(table_fix(code).unwrap_or(def.proj4), def.wkt),
                crate::datum::base_geographic_code(def.wkt),
                crate::datum::angular_unit(def.wkt),
            )
        }
        "OGC" if code.trim().eq_ignore_ascii_case("CRS84") => (
            "OGC:CRS84".to_string(),
            CRS84_PROJ.to_string(),
            None,
            std::f64::consts::PI / 180.0,
        ),
        other => {
            return Err(format!(
                "unsupported CRS authority {other:?} in {input:?}: expected EPSG or OGC:CRS84"
            ))
        }
    };
    let (proj_string, ext) = if proj_ext::handles(&proj_string) {
        let (ext, geographic) = proj_ext::build(&proj_string)
            .map_err(|e| format!("cannot build projection for {label}: {e}"))?;
        (geographic, Some(ext))
    } else {
        (proj_string, None)
    };
    let (proj, fallback) = crate::datum::build(&proj_string, base)
        .map_err(|e| format!("cannot build projection for {label}: {e}"))?;
    let areas = crate::datum::area_variants(&proj_string, base);
    Ok(Resolved {
        label,
        proj,
        fallback,
        base,
        areas,
        ext,
        angular,
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
    resolve(input).ok().map(|r| r.is_geographic())
}

thread_local! {
    static WGS84: Proj = Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs")
        .expect("WGS 84 is a valid proj string");
}

/// Whether `at` (lon, lat in degrees) lies in `area` (`west > east` across the antimeridian).
fn in_area(area: &[f64; 4], at: (f64, f64)) -> bool {
    let [w, s, e, n] = *area;
    let lon_ok = if w <= e {
        at.0 >= w && at.0 <= e
    } else {
        at.0 >= w || at.0 <= e
    };
    lon_ok && at.1 >= s && at.1 <= n
}

impl Resolved {
    /// The Helmert variant of this CRS for a point: the first operation by area of use that holds
    /// it, else the one `+towgs84` set of the table.
    fn helmert_at(&self, at: Option<(f64, f64)>) -> &Proj {
        at.and_then(|at| {
            self.areas
                .iter()
                .find(|(area, _)| in_area(area, at))
                .map(|(_, p)| p)
        })
        .unwrap_or_else(|| self.fallback.as_ref().unwrap_or(&self.proj))
    }

    /// The projection tried first: the grids when it has them, else the Helmert variant.
    fn primary_at(&self, at: Option<(f64, f64)>) -> &Proj {
        if self.fallback.is_some() {
            &self.proj
        } else {
            self.helmert_at(at)
        }
    }
}

/// Longitude and latitude in degrees (WGS 84, to the metres that matter for choosing an area) of
/// a point given in the engine coordinates of `src`; `None` when neither CRS has areas to choose
/// from.
fn locate(src: &Resolved, dst: &Resolved, x: f64, y: f64) -> Option<(f64, f64)> {
    if src.areas.is_empty() && dst.areas.is_empty() {
        return None;
    }
    if src.ext.is_some() || src.proj.is_latlong() {
        let pm = src.proj.from_greenwich();
        return Some(((x + pm).to_degrees(), y.to_degrees()));
    }
    let mut point = (x, y);
    WGS84
        .with(|w| proj4rs::transform::transform(&src.proj, w, &mut point))
        .ok()?;
    Some((point.0.to_degrees(), point.1.to_degrees()))
}

/// Runs `f` with the projections of both CRSs, then, when a point lies outside every datum
/// grid, once more with their Helmert variants (what PROJ does by trying its next operation).
fn with_fallback<T>(
    src: &Resolved,
    dst: &Resolved,
    at: Option<(f64, f64)>,
    f: impl Fn(&Proj, &Proj) -> Result<T, proj4rs::errors::Error>,
) -> Result<T, String> {
    match f(src.primary_at(at), dst.primary_at(at)) {
        Err(proj4rs::errors::Error::PointOutsideNadShiftArea)
            if src.fallback.is_some() || dst.fallback.is_some() =>
        {
            f(src.helmert_at(at), dst.helmert_at(at))
        }
        other => other,
    }
    .map_err(|e| e.to_string())
}

/// Angle in radians; `unit` is radians per unit. Degrees go through `to_radians` so that a round
/// trip is exact.
fn to_radians(v: f64, unit: f64) -> f64 {
    if unit == std::f64::consts::PI / 180.0 {
        v.to_radians()
    } else {
        v * unit
    }
}

fn to_degrees(v: f64, unit: f64) -> f64 {
    if unit == std::f64::consts::PI / 180.0 {
        v.to_degrees()
    } else {
        v / unit
    }
}

/// CRS coordinates to what the `proj4rs` projection of `res` takes: angles in radians (from the
/// unit of a geographic CRS, or through the extension projection) or the coordinates as they are.
fn to_engine(res: &Resolved, x: f64, y: f64) -> Result<(f64, f64), String> {
    if let Some(ext) = &res.ext {
        ext.inverse(x, y)
    } else if res.proj.is_latlong() {
        Ok((to_radians(x, res.angular), to_radians(y, res.angular)))
    } else {
        Ok((x, y))
    }
}

/// The inverse of [`to_engine`].
fn from_engine(res: &Resolved, x: f64, y: f64) -> Result<(f64, f64), String> {
    if let Some(ext) = &res.ext {
        ext.forward(x, y)
    } else if res.proj.is_latlong() {
        Ok((to_degrees(x, res.angular), to_degrees(y, res.angular)))
    } else {
        Ok((x, y))
    }
}

/// Transforms one coordinate; geographic CRSs are in their own angular unit outside (degrees,
/// or grads for the Paris-meridian CRSs), radians inside.
fn transform_coord(src: &Resolved, dst: &Resolved, c: Coord<f64>) -> Result<Coord<f64>, String> {
    let (x, y) = to_engine(src, c.x, c.y)?;
    let at = locate(src, dst, x, y);
    let (x, y) = with_fallback(src, dst, at, |src, dst| {
        let mut point = (x, y);
        proj4rs::transform::transform(src, dst, &mut point)?;
        Ok(point)
    })?;
    let (x, y) = from_engine(dst, x, y)?;
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
    let (x, y) = to_engine(src, c.x, c.y)?;
    let at = locate(src, dst, x, y);
    let (x, y, nz) = with_fallback(src, dst, at, |src, dst| {
        let mut point = (x, y, z);
        proj4rs::transform::transform(src, dst, &mut point)?;
        Ok(point)
    })?;
    let (x, y) = from_engine(dst, x, y)?;
    if x.is_finite() && y.is_finite() && nz.is_finite() {
        Ok((Coord { x, y }, nz))
    } else {
        Err("non-finite result".to_string())
    }
}

/// Transforms one `(x, y)` position between two CRS labels (the engine behind
/// [`GeoColumn::to_crs`]); used by the sweep example and tests.
pub fn transform_xy(source: &str, target: &str, x: f64, y: f64) -> Result<(f64, f64), String> {
    let (src, dst) = (resolve(source)?, resolve(target)?);
    catch_unwind(AssertUnwindSafe(|| {
        transform_coord(&src, &dst, Coord { x, y })
    }))
    .map_err(|_| "transform panicked".to_string())?
    .map(|c| (c.x, c.y))
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
        let [minx, miny, maxx, maxy] = if src.is_geographic() {
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
