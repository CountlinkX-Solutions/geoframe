//! Datum shifts for `to_crs`: grid files, registered grids and the Helmert fallback.
//!
//! `proj4rs` shifts a datum with a `+nadgrids` list (NTv2 `.gsb` files and the GeoTIFF grids of
//! PROJ-data, read by its `tiff` feature) or a `+towgs84` Helmert set. The bundled EPSG table
//! has `+towgs84` for most datums, `+datum=NAD27` (which means "the NADCON/NTv2 grids or
//! nothing") and a mandatory grid for NZGD49. This module turns that into PROJ's behaviour:
//!
//! 1. **Grids** are looked up by file name in the configured grid directories
//!    ([`set_grid_dirs`]) and then in the directories of `PROJ_NADGRIDS` / `PROJ_DATA`
//!    (separated by `:`), so a PROJ-data checkout or the cache of `pyproj.sync` works as is. A
//!    geographic CRS is mapped to its candidate files by the table in `datum_grids.rs`
//!    (generated from the PROJ database; [`register_grids`] replaces it for one CRS). The
//!    candidates are optional (`+nadgrids=@a,@b`): the ones that exist are used, best accuracy
//!    first, and a point outside every grid is shifted with the Helmert set below, as PROJ moves
//!    on to its next operation. Nothing is ever downloaded.
//! 2. **Fallback**: without a usable grid the CRS keeps its `+towgs84` set. `+datum=NAD27` gets
//!    the CONUS set (-8, 160, 176 m, EPSG:1173, "NAD27 to WGS 84 (4)"), where `proj4rs` alone
//!    refuses to build the projection.
//!
//! Grids are loaded once per file name and kept for the life of the process (`proj4rs` keeps a
//! global catalog), so changing the directories only affects files not yet loaded.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Once, RwLock};

use proj4rs::errors::Error as ProjError;
use proj4rs::nadgrids::{catalog, formats, Catalog};
use proj4rs::Proj;

use crate::datum_grids::DATUM_GRIDS;
use crate::datum_helmert::{DATUM_HELMERT, HELMERT_LISTS};
use crate::wkt1_projjson::{parse, Node};

static DIRS: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());
static REGISTERED: RwLock<Vec<(u32, Vec<String>)>> = RwLock::new(Vec::new());
static READER: Once = Once::new();

/// CONUS "NAD27 to WGS 84 (4)" as a `+towgs84` set, with the NAD27 ellipsoid (Clarke 1866).
const NAD27_FALLBACK: &str = "+ellps=clrk66 +towgs84=-8,160,176";

/// Replaces the directories searched for grid files. Relative names are joined to each in turn.
pub fn set_grid_dirs(dirs: Vec<String>) {
    *DIRS.write().unwrap() = dirs.into_iter().map(PathBuf::from).collect();
}

/// The directories set with [`set_grid_dirs`] (not those of the environment).
pub fn grid_dirs() -> Vec<String> {
    DIRS.read()
        .unwrap()
        .iter()
        .map(|d| d.display().to_string())
        .collect()
}

/// Uses `names` (PROJ-data file names, tried in order) for the geographic CRS `code` instead of
/// the built-in table. An empty list switches grids off for that CRS.
pub fn register_grids(code: u32, names: Vec<String>) {
    let mut registered = REGISTERED.write().unwrap();
    registered.retain(|(c, _)| *c != code);
    registered.push((code, names));
}

/// Removes a [`register_grids`] entry, so the built-in table applies again.
pub fn unregister_grids(code: u32) {
    REGISTERED.write().unwrap().retain(|(c, _)| *c != code);
}

fn env_dirs() -> Vec<PathBuf> {
    ["PROJ_NADGRIDS", "PROJ_DATA"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .flat_map(|paths| {
            paths
                .split(':')
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The file `name` resolves to: itself when absolute, else the first directory holding it.
pub(crate) fn locate(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    let configured = DIRS.read().unwrap().clone();
    configured
        .into_iter()
        .chain(env_dirs())
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Whether the geographic CRS `code` has Helmert operations or grids to WGS 84.
pub(crate) fn has_operations(code: u32) -> bool {
    DATUM_HELMERT
        .binary_search_by_key(&code, |(c, _)| *c)
        .is_ok()
        || DATUM_GRIDS.binary_search_by_key(&code, |(c, _)| *c).is_ok()
}

/// Whether PROJ shifts the geographic CRS `code` to WGS 84 by more than a null operation: a grid,
/// or a Helmert / offset that is not zero.
pub(crate) fn shifts_to_wgs84(code: u32) -> bool {
    if !grid_names(code).is_empty() {
        return true;
    }
    DATUM_HELMERT
        .binary_search_by_key(&code, |(c, _)| *c)
        .is_ok_and(|i| {
            HELMERT_LISTS[DATUM_HELMERT[i].1 as usize]
                .iter()
                .any(|(_, p, kind)| (kind & !CATCH_ALL) > 1 || p.iter().any(|v| *v != 0.0))
        })
}

/// The grid files, best first, that may shift the geographic CRS `code` to WGS 84.
pub(crate) fn grid_names(code: u32) -> Vec<String> {
    if let Some((_, names)) = REGISTERED.read().unwrap().iter().find(|(c, _)| *c == code) {
        return names.clone();
    }
    DATUM_GRIDS
        .binary_search_by_key(&code, |(c, _)| *c)
        .map(|i| DATUM_GRIDS[i].1.iter().map(|n| n.to_string()).collect())
        .unwrap_or_default()
}

/// The builder `proj4rs` calls for a grid it has not loaded: find the file and read it.
fn read_grid(catalog: &Catalog, key: &str) -> Result<(), ProjError> {
    let path = locate(key).ok_or_else(|| ProjError::GridFileNotFound(key.into()))?;
    formats::read(catalog, key, &mut BufReader::new(File::open(path)?))
}

fn install_reader() {
    READER.call_once(|| {
        catalog::set_builder(read_grid);
    });
}

fn find_geographic(node: &Node) -> Option<&Node> {
    if node.keyword == "GEOGCS" {
        return Some(node);
    }
    node.args.iter().find_map(|arg| match arg {
        crate::wkt1_projjson::Arg::Node(child) => find_geographic(child),
        _ => None,
    })
}

/// The EPSG code of the geographic CRS a WKT1 definition is based on (the CRS itself for a
/// geographic one).
pub(crate) fn base_geographic_code(wkt: &str) -> Option<u32> {
    let id = find_geographic(&parse(wkt)?)?.id()?;
    if id["authority"] != "EPSG" {
        return None;
    }
    u32::try_from(id["code"].as_u64()?).ok()
}

/// The unit of a geographic CRS in radians (grads for NTF Paris; degrees for every other CRS, and
/// for projected ones, whose geographic base is not exposed).
pub(crate) fn angular_unit(wkt: &str) -> f64 {
    let degree = std::f64::consts::PI / 180.0;
    let Some(root) = parse(wkt) else {
        return degree;
    };
    if root.keyword != "GEOGCS" {
        return degree;
    }
    root.child("UNIT")
        .and_then(|u| u.number(1))
        .filter(|f| *f > 0.0)
        // The WKT rounds the degree to 0.0174532925199433: keep the exact value.
        .map_or(degree, |f| {
            if (f - degree).abs() < 1e-12 {
                degree
            } else {
                f
            }
        })
}

/// The proj string with the axis directions of the WKT when the table left them out: EPSG:5513
/// and 2065 (Krovak) use southing and westing, which PROJ writes `+axis=swu`. Only definitions
/// that point an axis south or west are changed (`+axis=enu` is the default and a swapped
/// north/east pair is what `always_xy` undoes).
pub(crate) fn with_axes(proj: &str, wkt: &str) -> String {
    let proj = proj.to_string();
    if proj.contains("+axis=") {
        return proj;
    }
    let Some(root) = parse(wkt) else {
        return proj;
    };
    if root.keyword != "PROJCS" {
        return proj;
    }
    let dirs: Vec<char> = root
        .children("AXIS")
        .filter_map(|a| match a.text(1)? {
            "EAST" => Some('e'),
            "WEST" => Some('w'),
            "NORTH" => Some('n'),
            "SOUTH" => Some('s'),
            _ => None,
        })
        .collect();
    if dirs.len() == 2 && dirs.iter().any(|d| matches!(d, 'w' | 's')) {
        format!("{proj} +axis={}{}u", dirs[0], dirs[1])
    } else {
        proj
    }
}

/// The proj string without its grid list. `+nadgrids=@null` stays: it is not a grid but the
/// legacy way of saying "these coordinates are WGS 84" (Web Mercator's definition), and without
/// it `proj4rs` sees no datum and skips the shift to any other datum.
fn without_nadgrids(proj: &str) -> impl Iterator<Item = &str> {
    proj.split_whitespace()
        .filter(|token| !token.starts_with("+nadgrids=") || *token == "+nadgrids=@null")
}

/// The proj string with no grid at all: the Helmert set it carries, or the NAD27 default.
fn helmert_string(proj: &str) -> String {
    without_nadgrids(proj)
        .map(|token| {
            if token.eq_ignore_ascii_case("+datum=NAD27") {
                NAD27_FALLBACK
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The projection to use and, when it relies on grids, the Helmert one for points outside them.
pub(crate) fn build(
    proj: &str,
    base: Option<u32>,
) -> Result<(Proj, Option<Proj>), proj4rs::errors::Error> {
    let helmert = helmert_string(proj);
    let names = base.map(grid_names).unwrap_or_default();
    if !names.is_empty() {
        install_reader();
        let optional: Vec<String> = names.iter().map(|n| format!("@{n}")).collect();
        // The grids replace every `+nadgrids` entry, `@null` included.
        let rest: Vec<&str> = proj
            .split_whitespace()
            .filter(|token| !token.starts_with("+nadgrids="))
            .collect();
        let with_grids = format!("{} +nadgrids={}", rest.join(" "), optional.join(","));
        // Fails with `NadGridNotAvailable` when none of the files exists.
        if let Ok(gridded) = Proj::from_proj_string(&with_grids) {
            return Ok((gridded, Proj::from_proj_string(&helmert).ok()));
        }
    }
    Ok((Proj::from_proj_string(&helmert)?, None))
}

/// One operation by area of use: the projection of the CRS with the operation's datum shift
/// (or, for a geographic offset, with none), and the offset when the operation is one.
pub(crate) struct AreaOp {
    /// `[west, south, east, north]` in degrees (west > east across the antimeridian).
    pub(crate) area: [f64; 4],
    pub(crate) proj: Proj,
    pub(crate) offset: Option<Offset>,
}

/// A `geogoffset` operation, which `proj4rs` has no datum parameter for: the geographic
/// coordinates on the ellipsoid of the CRS (`geo`, Greenwich-based) are moved by `dlat`/`dlon`
/// (radians) and then read as the geographic CRS `post` (WGS 84, or GRS80 with a translation).
/// A null operation is the zero offset (`noop`: PROJ keeps the coordinates whatever the
/// ellipsoids). An exact Helmert step replaces the offset when `exact` is set.
pub(crate) struct Offset {
    pub(crate) dlat: f64,
    pub(crate) dlon: f64,
    pub(crate) geo: Proj,
    pub(crate) post: Proj,
    exact: Option<ExactHelmert>,
}

/// PROJ's `helmert exact` on geocentric coordinates, for rotations too large for the small-angle
/// `+towgs84` formula of `proj4rs` (Saba: 2e-3 rad, which is 9 m).
struct ExactHelmert {
    /// Geocentric coordinates on the ellipsoid of the CRS and on WGS 84.
    cart: Proj,
    wgs_cart: Proj,
    /// The rotation (orthonormal), the scale (1 + ppm) and the translation in metres.
    rotation: [[f64; 3]; 3],
    scale: f64,
    translation: [f64; 3],
}

impl ExactHelmert {
    /// `set` is a `+towgs84` set: translations in metres, rotations in arc-seconds (position
    /// vector convention) and the scale in ppm.
    fn new(set: &[f64; 7], ellipsoid: &str) -> Option<Self> {
        // PROJ builds the rotation of the coordinate frame angles, the position vector ones negated.
        let [cr, cp, cy] = [-set[3], -set[4], -set[5]].map(|a| (a * ARC_SECOND).cos());
        let [sr, sp, sy] = [-set[3], -set[4], -set[5]].map(|a| (a * ARC_SECOND).sin());
        Some(Self {
            cart: Proj::from_proj_string(&format!("+proj=geocent {ellipsoid} +no_defs")).ok()?,
            wgs_cart: Proj::from_proj_string("+proj=geocent +ellps=WGS84 +no_defs").ok()?,
            rotation: [
                [cp * cy, cr * sy + sr * sp * cy, sr * sy - cr * sp * cy],
                [-cp * sy, cr * cy - sr * sp * sy, sr * cy + cr * sp * sy],
                [sp, -sr * cp, cr * cp],
            ],
            scale: 1.0 + set[6] * 1e-6,
            translation: [set[0], set[1], set[2]],
        })
    }

    fn apply(&self, x: [f64; 3], inverse: bool) -> [f64; 3] {
        let r = &self.rotation;
        if inverse {
            let d: Vec<f64> = (0..3)
                .map(|i| (x[i] - self.translation[i]) / self.scale)
                .collect();
            // the transpose of an orthonormal matrix is its inverse
            [0, 1, 2].map(|i| r[0][i] * d[0] + r[1][i] * d[1] + r[2][i] * d[2])
        } else {
            [0, 1, 2].map(|i| {
                self.scale * (r[i][0] * x[0] + r[i][1] * x[1] + r[i][2] * x[2])
                    + self.translation[i]
            })
        }
    }
}

fn step(from: &Proj, to: &Proj, point: (f64, f64, f64)) -> Result<(f64, f64, f64), String> {
    let mut p = point;
    proj4rs::transform::transform(from, to, &mut p)
        .map(|_| p)
        .map_err(|e| e.to_string())
}

impl Offset {
    /// Geographic coordinates (radians) on the ellipsoid of the CRS, as `wgs` (WGS 84) ones.
    pub(crate) fn to_wgs84(&self, wgs: &Proj, lon: f64, lat: f64) -> Result<(f64, f64), String> {
        if let Some(h) = &self.exact {
            let (x, y, z) = step(&self.geo, &h.cart, (lon, lat, 0.0))?;
            let [x, y, z] = h.apply([x, y, z], false);
            return step(&h.wgs_cart, wgs, (x, y, z)).map(|p| (p.0, p.1));
        }
        step(&self.post, wgs, (lon + self.dlon, lat + self.dlat, 0.0)).map(|p| (p.0, p.1))
    }

    /// The inverse of [`Offset::to_wgs84`].
    pub(crate) fn to_geo(&self, wgs: &Proj, lon: f64, lat: f64) -> Result<(f64, f64), String> {
        if let Some(h) = &self.exact {
            let (x, y, z) = step(wgs, &h.wgs_cart, (lon, lat, 0.0))?;
            let [x, y, z] = h.apply([x, y, z], true);
            return step(&h.cart, &self.geo, (x, y, z)).map(|p| (p.0, p.1));
        }
        let (lon, lat, _) = step(wgs, &self.post, (lon, lat, 0.0))?;
        Ok((lon - self.dlon, lat - self.dlat))
    }
}

/// The `kind` of an exact Helmert step in `datum_helmert.rs`.
const EXACT_HELMERT: u8 = 4;

/// Added to the `kind` of the catch-all operation of `datum_helmert.rs`.
const CATCH_ALL: u8 = 16;

const ARC_SECOND: f64 = std::f64::consts::PI / 648_000.0;

/// The operations PROJ can pick from for the geographic CRS `code`, each as the projection `proj`
/// with that operation's `+towgs84` (or offset), in the order they are tried, with the area of
/// use they apply to, ending with the ballpark (no shift) PROJ uses where none applies. Empty
/// when the CRS has none.
pub(crate) fn area_variants(proj: &str, code: Option<u32>) -> Vec<AreaOp> {
    let Some(code) = code else {
        return Vec::new();
    };
    let Ok(i) = DATUM_HELMERT.binary_search_by_key(&code, |(c, _)| *c) else {
        return Vec::new();
    };
    // A named datum carries its own set: replace it with its ellipsoid, the set is explicit below.
    let base: Vec<String> = helmert_string(proj)
        .split_whitespace()
        .filter(|t| !t.starts_with("+towgs84="))
        .map(|t| match t.strip_prefix("+datum=") {
            Some(datum) => format!("+ellps={}", datum_ellipsoid(datum)),
            None => t.to_string(),
        })
        .collect();
    // The ellipsoid alone (no prime meridian): geographic coordinates counted from Greenwich.
    let ellipsoid: Vec<&str> = base
        .iter()
        .map(String::as_str)
        .filter(|t| {
            ["+ellps=", "+a=", "+rf=", "+b=", "+f=", "+R=", "+es="]
                .iter()
                .any(|k| t.starts_with(k))
        })
        .collect();
    let geo = format!("+proj=longlat {} +no_defs", ellipsoid.join(" "));
    // Where no operation applies PROJ falls back to its ballpark transformation, which keeps the
    // coordinates as they are: a projection with no datum shift at all.
    let ballpark = Proj::from_proj_string(&base.join(" "))
        .ok()
        .map(|proj| AreaOp {
            area: [-180.0, -90.0, 180.0, 90.0],
            proj,
            offset: None,
        });
    // PROJ's own first operation, listed last with the world as its area of use, is what a point
    // outside every area gets (Minna at 40E 10N), and the only operation of a datum applies
    // everywhere (RD/83: that one is the whole list). For a projected CRS PROJ first keeps the
    // operations that meet the area of use of that CRS, which is not known here: a point outside
    // every area stays on the ballpark (DHDN / Gauss-Kruger zone 1, in the sea).
    let geographic = proj
        .split_whitespace()
        .any(|t| matches!(t, "+proj=longlat" | "+proj=latlong" | "+proj=geocent"));
    let list = HELMERT_LISTS[DATUM_HELMERT[i].1 as usize];
    list.iter()
        .filter(|(.., kind)| geographic || kind & CATCH_ALL == 0)
        .filter_map(|(area, p, kind)| {
            let kind = &(kind & !CATCH_ALL);
            if *kind == 0 {
                let set: Vec<String> = p.iter().map(|v| v.to_string()).collect();
                let text = format!("{} +towgs84={}", base.join(" "), set.join(","));
                return Proj::from_proj_string(&text).ok().map(|proj| AreaOp {
                    area: *area,
                    proj,
                    offset: None,
                });
            }
            let post = if *kind == 1 || *kind == EXACT_HELMERT {
                "+proj=longlat +datum=WGS84 +no_defs".to_string()
            } else {
                format!(
                    "+proj=longlat +ellps=GRS80 +towgs84={},{},{} +no_defs",
                    p[2], p[3], p[4]
                )
            };
            Some(AreaOp {
                area: *area,
                proj: Proj::from_proj_string(&base.join(" ")).ok()?,
                offset: Some(Offset {
                    dlat: p[0] * ARC_SECOND,
                    dlon: p[1] * ARC_SECOND,
                    geo: Proj::from_proj_string(&geo).ok()?,
                    post: Proj::from_proj_string(&post).ok()?,
                    exact: if *kind == EXACT_HELMERT {
                        Some(ExactHelmert::new(p, &ellipsoid.join(" "))?)
                    } else {
                        None
                    },
                }),
            })
        })
        .chain(ballpark)
        .collect()
}

/// The ellipsoid of the named datums the bundled table uses.
fn datum_ellipsoid(datum: &str) -> &'static str {
    match datum {
        "GGRS87" | "NAD83" => "GRS80",
        "carthage" => "clrk80ign",
        "hermannskogel" | "potsdam" => "bessel",
        "ire65" => "mod_airy",
        "nzgd49" => "intl",
        "OSGB36" => "airy",
        "NAD27" => "clrk66",
        _ => "WGS84",
    }
}
