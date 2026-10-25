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
pub(crate) struct Offset {
    pub(crate) dlat: f64,
    pub(crate) dlon: f64,
    pub(crate) geo: Proj,
    pub(crate) post: Proj,
}

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
    HELMERT_LISTS[DATUM_HELMERT[i].1 as usize]
        .iter()
        .filter_map(|(area, p, kind)| {
            if *kind == 0 {
                let set: Vec<String> = p.iter().map(|v| v.to_string()).collect();
                let text = format!("{} +towgs84={}", base.join(" "), set.join(","));
                return Proj::from_proj_string(&text).ok().map(|proj| AreaOp {
                    area: *area,
                    proj,
                    offset: None,
                });
            }
            let post = if *kind == 1 {
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
