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

fn without_nadgrids(proj: &str) -> impl Iterator<Item = &str> {
    proj.split_whitespace()
        .filter(|token| !token.starts_with("+nadgrids="))
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
        let with_grids = format!(
            "{} +nadgrids={}",
            without_nadgrids(proj).collect::<Vec<_>>().join(" "),
            optional.join(",")
        );
        // Fails with `NadGridNotAvailable` when none of the files exists.
        if let Ok(gridded) = Proj::from_proj_string(&with_grids) {
            return Ok((gridded, Proj::from_proj_string(&helmert).ok()));
        }
    }
    Ok((Proj::from_proj_string(&helmert)?, None))
}
