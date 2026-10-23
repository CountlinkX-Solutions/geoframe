//! OGC GeoPackage 1.3 vector feature tables on `rusqlite` with the bundled SQLite.
//!
//! This is the only C code GeoFrame compiles: `rusqlite`'s `bundled` feature builds the SQLite
//! amalgamation (with the R-tree module) through `cc`, so the NIF stays self-contained and
//! needs no system SQLite or GDAL.
//!
//! # Reading
//!
//! Layers come from `gpkg_contents` (`features` and `attributes` tables, in file order); the
//! geometry column and SRS from `gpkg_geometry_columns` and `gpkg_spatial_ref_sys`. Each
//! geometry is a GeoPackageBinary blob: the `GP` header (version, flags, SRS id, optional
//! envelope) followed by standard WKB. Empty-flagged blobs read as null. The integer primary
//! key (`fid`) is not an attribute. Column types follow the declared SQL type
//! (`BOOLEAN`, `TINYINT`, `SMALLINT`, `MEDIUMINT`, `INT`/`INTEGER`, `FLOAT`, `DOUBLE`/`REAL`,
//! `TEXT`, `BLOB`, `DATE`, `DATETIME`); other declarations fall back to SQLite's type
//! affinity. A file with several feature layers needs the layer name.
//!
//! # Writing
//!
//! Creates the required metadata tables (`gpkg_spatial_ref_sys`, `gpkg_contents`,
//! `gpkg_geometry_columns`, `gpkg_extensions`), `application_id` `GPKG` and `user_version`
//! 10300, a feature table `(fid INTEGER PRIMARY KEY AUTOINCREMENT, geom, attributes...)` and,
//! unless disabled, the `rtree_<table>_<geom>` spatial index with the spec's triggers
//! (`gpkg_rtree_index` extension). The triggers call `ST_MinX`, `ST_MaxX`, `ST_MinY`,
//! `ST_MaxY` and `ST_IsEmpty`, which every connection opened here registers (GDAL does the
//! same), so appending to a file that other software wrote keeps its index current.
//!
//! The geometry type of a layer is the common type of its rows, `GEOMETRY` for a mix; unlike
//! GDAL, single and multi geometries are not promoted to multi, so rows read back exactly as
//! written. Every blob carries an envelope. `append: false` replaces the layer of that name
//! (other layers are kept), `append: true` adds rows to it after checking the columns and the
//! CRS, and creating a missing layer in either mode. A file that exists but is not a
//! GeoPackage is never overwritten.
//!
//! # Z and M
//!
//! The WKB keeps Z and M (ISO dimension codes). `gpkg_geometry_columns.z` and `.m` are 0 when
//! no row has the ordinate, 1 when every non-null row has it and 2 when only some do; appending
//! rows of another dimension turns a layer's flag into 2. A blob's envelope has the Z and/or M
//! range after the XY one (indicator 2, 3 or 4) when the geometry has values for them (NaN is
//! ignored); the R-tree and the layer extent stay 2D.
//!
//! A write that fails leaves no half-made file: the `application_id` and `user_version`
//! pragmas are set inside the creating transaction, and a file the write itself created is
//! removed again on error. Appending checks the rows against the layer: a different geometry
//! type than the layer's declared one (unless it is `GEOMETRY`) or a column whose values do not
//! fit the declared storage class (integer, real, text, blob; integers fit a real column) is
//! an error. On read, a `TINYINT`, `SMALLINT` or `MEDIUMINT` column holding a value outside
//! its declared range is widened to `int64` instead of nulling that value.

use std::path::Path;
use std::sync::Arc;

use arrow_array::builder::{
    BinaryBuilder, BooleanBuilder, Float32Builder, Float64Builder, Int16Builder, Int32Builder,
    Int64Builder, Int8Builder, StringBuilder,
};
use arrow_array::{Array, ArrayRef};
use arrow_cast::cast::{cast_with_options, CastOptions};
use arrow_schema::{DataType, Field, TimeUnit};
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OpenFlags};

use crate::attributes::{ipc_from_columns, Cell, Table};
use crate::crs_wkt::{crs_to_authority, epsg_definition, looks_like_wkt, prj_wkt_to_crs};
use crate::{GeoColumn, VectorRead};

const APPLICATION_ID: i64 = 0x4750_4B47;
const USER_VERSION: i64 = 10300;
/// The SRS id GDAL gives data without a CRS.
const UNDEFINED_SRS: i64 = 99_999;
const UNDEFINED_SRS_WKT: &str = r#"LOCAL_CS["Undefined SRS",LOCAL_DATUM["unknown",32767],UNIT["unknown",0],AXIS["Easting",EAST],AXIS["Northing",NORTH]]"#;

/// Facts about one layer or shapefile, shaped after pyogrio's `read_info`.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerInfo {
    pub name: String,
    pub driver: String,
    pub crs: Option<String>,
    pub encoding: String,
    pub geometry_type: Option<String>,
    pub features: u64,
    /// `[minx, miny, maxx, maxy]`
    pub bounds: Option<[f64; 4]>,
    /// `(name, pandas-style dtype)`
    pub fields: Vec<(String, String)>,
}

/// One entry of the layer list; `geometry_type` is `None` for a table without geometries.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerSummary {
    pub name: String,
    pub geometry_type: Option<String>,
}

pub struct GpkgRead {
    pub layer: String,
    pub vector: VectorRead,
}

#[derive(Debug, Clone, Copy)]
pub struct GpkgWriteOptions {
    /// Add to an existing layer instead of replacing it.
    pub append: bool,
    /// Create the R-tree index for a new layer.
    pub spatial_index: bool,
}

// -- GeoPackageBinary -----------------------------------------------------------------------

/// A decoded GeoPackageBinary header and its WKB payload.
pub(crate) struct Blob<'a> {
    /// The SRS id of the header (the layer's own SRS is used for reading).
    #[cfg_attr(not(test), allow(dead_code))]
    pub srs_id: i32,
    /// `[minx, miny, maxx, maxy]`
    pub envelope: Option<[f64; 4]>,
    /// `[minz, maxz]` when the envelope has a Z range (indicator 2 or 4).
    #[cfg_attr(not(test), allow(dead_code))]
    pub z_range: Option<[f64; 2]>,
    /// `[minm, maxm]` when the envelope has an M range (indicator 3 or 4).
    #[cfg_attr(not(test), allow(dead_code))]
    pub m_range: Option<[f64; 2]>,
    pub empty: bool,
    pub wkb: &'a [u8],
}

/// `GP` header (version 0, little-endian, XY envelope when given) followed by `wkb`.
pub(crate) fn encode_geometry_blob(wkb: &[u8], srs_id: i32, envelope: Option<[f64; 4]>) -> Vec<u8> {
    encode_geometry_blob_zm(wkb, srs_id, envelope, None, None)
}

/// Like [`encode_geometry_blob`] with Z and/or M ranges: the envelope indicator becomes 2 (XYZ),
/// 3 (XYM) or 4 (XYZM) and the ranges follow the XY ones. Ranges need an XY envelope.
pub(crate) fn encode_geometry_blob_zm(
    wkb: &[u8],
    srs_id: i32,
    envelope: Option<[f64; 4]>,
    z: Option<[f64; 2]>,
    m: Option<[f64; 2]>,
) -> Vec<u8> {
    let indicator: u8 = match (envelope.is_some(), z.is_some(), m.is_some()) {
        (false, _, _) => 0,
        (true, false, false) => 1,
        (true, true, false) => 2,
        (true, false, true) => 3,
        (true, true, true) => 4,
    };
    let mut out = Vec::with_capacity(8 + 64 + wkb.len());
    out.extend_from_slice(b"GP");
    out.push(0);
    out.push(0b0000_0001 | (indicator << 1));
    out.extend_from_slice(&srs_id.to_le_bytes());
    if let Some([min_x, min_y, max_x, max_y]) = envelope {
        for value in [min_x, max_x, min_y, max_y] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in z.into_iter().chain(m).flatten() {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out.extend_from_slice(wkb);
    out
}

pub(crate) fn decode_geometry_blob(blob: &[u8]) -> Result<Blob<'_>, String> {
    if blob.len() < 8 {
        return Err("geometry blob is shorter than the GeoPackage header".to_string());
    }
    if &blob[..2] != b"GP" {
        return Err("geometry blob does not start with the GeoPackage magic \"GP\"".to_string());
    }
    let flags = blob[3];
    if flags & 0b0010_0000 != 0 {
        return Err("ExtendedGeoPackageBinary geometries are not supported".to_string());
    }
    let little = flags & 1 == 1;
    let indicator = (flags >> 1) & 0b111;
    let envelope_bytes = match indicator {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        other => {
            return Err(format!(
                "invalid envelope indicator {other} in the geometry header"
            ))
        }
    };
    if blob.len() < 8 + envelope_bytes {
        return Err("geometry blob is shorter than its header".to_string());
    }
    let srs_raw: [u8; 4] = blob[4..8].try_into().expect("4 bytes");
    let srs_id = if little {
        i32::from_le_bytes(srs_raw)
    } else {
        i32::from_be_bytes(srs_raw)
    };
    let double = |index: usize| {
        let raw: [u8; 8] = blob[8 + index * 8..16 + index * 8]
            .try_into()
            .expect("8 bytes");
        if little {
            f64::from_le_bytes(raw)
        } else {
            f64::from_be_bytes(raw)
        }
    };
    let envelope = (indicator != 0).then(|| [double(0), double(2), double(1), double(3)]);
    let range = |first: usize| Some([double(first), double(first + 1)]);
    let z_range = if matches!(indicator, 2 | 4) {
        range(4)
    } else {
        None
    };
    let m_range = match indicator {
        3 => range(4),
        4 => range(6),
        _ => None,
    };
    Ok(Blob {
        srs_id,
        envelope,
        z_range,
        m_range,
        empty: flags & 0b0001_0000 != 0,
        wkb: &blob[8 + envelope_bytes..],
    })
}

// -- connections ----------------------------------------------------------------------------

fn sql_error(e: rusqlite::Error) -> String {
    e.to_string()
}

fn user_error(message: String) -> rusqlite::Error {
    rusqlite::Error::UserFunctionError(message.into())
}

/// Envelope of a blob: from the header, or computed from the WKB when the writer omitted it
/// (GDAL does for points).
fn blob_bounds(blob: &[u8]) -> Result<Option<[f64; 4]>, String> {
    let decoded = decode_geometry_blob(blob)?;
    if decoded.empty {
        return Ok(None);
    }
    if let Some(envelope) = decoded.envelope {
        return Ok(Some(envelope));
    }
    let column = GeoColumn::from_wkb(&[Some(decoded.wkb)], None)?;
    Ok(column.bounds().into_iter().next().flatten())
}

fn register_functions(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    for (name, index) in [
        ("ST_MinX", 0usize),
        ("ST_MinY", 1),
        ("ST_MaxX", 2),
        ("ST_MaxY", 3),
    ] {
        conn.create_scalar_function(name, 1, flags, move |ctx| {
            let blob: Option<Vec<u8>> = ctx.get(0)?;
            match blob {
                None => Ok(None),
                Some(blob) => blob_bounds(&blob)
                    .map(|bounds| bounds.map(|b| b[index]))
                    .map_err(user_error),
            }
        })?;
    }
    conn.create_scalar_function("ST_IsEmpty", 1, flags, |ctx| {
        let blob: Option<Vec<u8>> = ctx.get(0)?;
        match blob {
            None => Ok(None),
            Some(blob) => decode_geometry_blob(&blob)
                .map(|b| Some(i64::from(b.empty)))
                .map_err(user_error),
        }
    })?;
    Ok(())
}

fn open_read(path: &str) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("cannot open {path}: {e}"))?;
    register_functions(&conn).map_err(sql_error)?;
    ensure_geopackage(&conn, path)?;
    Ok(conn)
}

fn ensure_geopackage(conn: &Connection, path: &str) -> Result<(), String> {
    let has_contents: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'gpkg_contents'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("{path} is not a GeoPackage: {e}"))?;
    if has_contents == 0 {
        return Err(format!(
            "{path} is not a GeoPackage (it has no gpkg_contents table)"
        ));
    }
    Ok(())
}

/// A SQL string literal.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

// -- layers ---------------------------------------------------------------------------------

struct LayerRow {
    name: String,
    data_type: String,
    geometry_type: Option<String>,
    z: i64,
    m: i64,
}

fn layer_rows(conn: &Connection) -> Result<Vec<LayerRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT c.table_name, c.data_type, g.geometry_type_name, g.z, g.m \
             FROM gpkg_contents c LEFT JOIN gpkg_geometry_columns g ON g.table_name = c.table_name \
             WHERE c.data_type IN ('features', 'attributes') ORDER BY c.rowid",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map([], |r| {
            Ok(LayerRow {
                name: r.get(0)?,
                data_type: r.get(1)?,
                geometry_type: r.get(2)?,
                z: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                m: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
            })
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    Ok(rows)
}

/// pyogrio's name for a GeoPackage geometry type.
fn display_geometry_type(name: &str, z: i64, m: i64) -> String {
    let base = match name.to_ascii_uppercase().as_str() {
        "POINT" => "Point",
        "LINESTRING" => "LineString",
        "POLYGON" => "Polygon",
        "MULTIPOINT" => "MultiPoint",
        "MULTILINESTRING" => "MultiLineString",
        "MULTIPOLYGON" => "MultiPolygon",
        "GEOMETRYCOLLECTION" => "GeometryCollection",
        "GEOMETRY" => "Unknown",
        _ => name,
    };
    let suffix = match (z == 1, m == 1) {
        (true, true) => " ZM",
        (true, false) => " Z",
        (false, true) => " M",
        _ => "",
    };
    format!("{base}{suffix}")
}

/// The layers of a GeoPackage in file order.
pub fn gpkg_layers(path: &str) -> Result<Vec<LayerSummary>, String> {
    let conn = open_read(path)?;
    Ok(layer_rows(&conn)?
        .into_iter()
        .map(|row| LayerSummary {
            geometry_type: row
                .geometry_type
                .as_deref()
                .map(|t| display_geometry_type(t, row.z, row.m)),
            name: row.name,
        })
        .collect())
}

/// Picks the layer to read. Without a name a lone feature layer is chosen; with several,
/// `strict` reports the choice as an error while a lenient caller gets the first one.
fn select_layer(
    rows: &[LayerRow],
    layer: Option<&str>,
    strict: bool,
    path: &str,
) -> Result<usize, String> {
    let listing = || {
        rows.iter()
            .map(|r| format!("{:?}", r.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(wanted) = layer {
        return rows
            .iter()
            .position(|r| r.name == wanted)
            .or_else(|| {
                rows.iter()
                    .position(|r| r.name.eq_ignore_ascii_case(wanted))
            })
            .ok_or_else(|| {
                format!(
                    "layer {wanted:?} not found in {path}; layers: {}",
                    listing()
                )
            });
    }
    let features: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.data_type == "features")
        .map(|(i, _)| i)
        .collect();
    match features.as_slice() {
        [] => Err(format!("{path} has no feature layers")),
        [one] => Ok(*one),
        many if strict => Err(format!(
            "{path} has several layers ({}); pass layer: to choose one",
            many.iter()
                .map(|&i| format!("{:?}", rows[i].name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        many => Ok(many[0]),
    }
}

// -- CRS ------------------------------------------------------------------------------------

/// The CRS string of a `gpkg_spatial_ref_sys` row; `None` for the undefined SRS ids.
fn crs_for_srs(conn: &Connection, srs_id: i64) -> Result<Option<String>, String> {
    if srs_id <= 0 {
        return Ok(None);
    }
    let row: Option<(String, i64, String)> = conn
        .query_row(
            "SELECT organization, organization_coordsys_id, definition \
             FROM gpkg_spatial_ref_sys WHERE srs_id = ?1",
            [srs_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })
        .map_err(sql_error)?;
    Ok(row.and_then(|(organization, code, definition)| {
        let organization = organization.trim().to_ascii_uppercase();
        if organization == "GDAL" && code == UNDEFINED_SRS {
            return None;
        }
        if organization == "NONE" || organization.is_empty() {
            return (looks_like_wkt(&definition)).then(|| prj_wkt_to_crs(&definition));
        }
        Some(format!("{organization}:{code}"))
    }))
}

fn first_word(wkt: &str) -> Option<String> {
    let open = wkt.find('[')?;
    let rest = wkt[open + 1..].trim_start().strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

/// Finds or creates the `gpkg_spatial_ref_sys` row for `crs` and returns its `srs_id`.
fn resolve_srs(
    conn: &Connection,
    crs: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<i64, String> {
    let Some(crs) = crs.map(str::trim) else {
        // GDAL's own "Undefined SRS": GDAL and GeoPandas read it as no CRS, whereas the
        // spec's -1 / 0 ids come back as an engineering CRS.
        conn.execute(
            "INSERT OR IGNORE INTO gpkg_spatial_ref_sys \
             (srs_name, srs_id, organization, organization_coordsys_id, definition, description) \
             VALUES ('Undefined SRS', 99999, 'GDAL', 99999, ?1, 'Custom undefined coordinate reference system')",
            [UNDEFINED_SRS_WKT],
        )
        .map_err(sql_error)?;
        return Ok(UNDEFINED_SRS);
    };
    if let Some((organization, code)) = crs_to_authority(crs) {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT srs_id FROM gpkg_spatial_ref_sys \
                 WHERE upper(organization) = ?1 AND organization_coordsys_id = ?2",
                params![organization, code],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })
            .map_err(sql_error)?;
        if let Some(id) = existing {
            return Ok(id);
        }
        let definition = if organization == "EPSG" {
            epsg_definition(code)
        } else {
            None
        };
        if definition.is_none() {
            warnings.push(format!(
                "no definition is available for {organization}:{code}; it is stored as \"undefined\" in gpkg_spatial_ref_sys"
            ));
        }
        let name = definition
            .and_then(first_word)
            .unwrap_or_else(|| format!("{organization}:{code}"));
        let taken: bool = conn
            .query_row(
                "SELECT count(*) FROM gpkg_spatial_ref_sys WHERE srs_id = ?1",
                [code],
                |r| r.get::<_, i64>(0),
            )
            .map_err(sql_error)?
            > 0;
        let id = if taken { next_custom_srs(conn)? } else { code };
        conn.execute(
            "INSERT INTO gpkg_spatial_ref_sys \
             (srs_name, srs_id, organization, organization_coordsys_id, definition) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                name,
                id,
                organization,
                code,
                definition.unwrap_or("undefined")
            ],
        )
        .map_err(sql_error)?;
        return Ok(id);
    }
    if looks_like_wkt(crs) {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT srs_id FROM gpkg_spatial_ref_sys WHERE definition = ?1",
                [crs],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })
            .map_err(sql_error)?;
        if let Some(id) = existing {
            return Ok(id);
        }
        let id = next_custom_srs(conn)?;
        conn.execute(
            "INSERT INTO gpkg_spatial_ref_sys \
             (srs_name, srs_id, organization, organization_coordsys_id, definition) \
             VALUES (?1, ?2, 'NONE', ?2, ?3)",
            params![
                first_word(crs).unwrap_or_else(|| "custom".to_string()),
                id,
                crs
            ],
        )
        .map_err(sql_error)?;
        return Ok(id);
    }
    Err(format!(
        "CRS {crs:?} cannot be stored in a GeoPackage; use an AUTHORITY:CODE string or WKT"
    ))
}

fn next_custom_srs(conn: &Connection) -> Result<i64, String> {
    let max: i64 = conn
        .query_row(
            "SELECT coalesce(max(srs_id), 0) FROM gpkg_spatial_ref_sys",
            [],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    Ok(max.max(99_999) + 1)
}

// -- reading --------------------------------------------------------------------------------

struct Column {
    name: String,
    declared: String,
    primary_key: bool,
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<Column>, String> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({})", quote(table)))
        .map_err(sql_error)?;
    let columns = statement
        .query_map([], |r| {
            Ok(Column {
                name: r.get(1)?,
                declared: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                primary_key: r.get::<_, i64>(5)? > 0,
            })
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    Ok(columns)
}

struct GeometryColumn {
    name: String,
    srs_id: i64,
    type_name: String,
}

fn geometry_column(conn: &Connection, table: &str) -> Result<GeometryColumn, String> {
    conn.query_row(
        "SELECT column_name, srs_id, geometry_type_name FROM gpkg_geometry_columns \
         WHERE table_name = ?1",
        [table],
        |r| {
            Ok(GeometryColumn {
                name: r.get(0)?,
                srs_id: r.get(1)?,
                type_name: r.get(2)?,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            format!("layer {table:?} has no geometry column (it is an attribute table)")
        }
        e => e.to_string(),
    })
}

/// The integer primary key, which is the feature id rather than an attribute.
fn is_fid(column: &Column, all: &[Column]) -> bool {
    column.primary_key
        && column.declared.to_ascii_uppercase().contains("INT")
        && all.iter().filter(|c| c.primary_key).count() == 1
}

/// Reads one layer (`None` picks the only feature layer). Several feature layers need a name.
pub fn read_gpkg(path: &str, layer: Option<&str>) -> Result<GpkgRead, String> {
    let conn = open_read(path)?;
    let rows = layer_rows(&conn)?;
    let chosen = &rows[select_layer(&rows, layer, true, path)?];
    let table = chosen.name.clone();
    let geometry = geometry_column(&conn, &table)?;
    let crs = crs_for_srs(&conn, geometry.srs_id)?;

    let columns = table_columns(&conn, &table)?;
    let attributes: Vec<&Column> = columns
        .iter()
        .filter(|c| c.name != geometry.name && !is_fid(c, &columns))
        .collect();
    let select = std::iter::once(quote(&geometry.name))
        .chain(attributes.iter().map(|c| quote(&c.name)))
        .collect::<Vec<_>>()
        .join(", ");
    let order = columns
        .iter()
        .find(|c| is_fid(c, &columns))
        .map_or_else(String::new, |c| format!(" ORDER BY {}", quote(&c.name)));
    let mut statement = conn
        .prepare(&format!("SELECT {select} FROM {}{order}", quote(&table)))
        .map_err(sql_error)?;

    let mut blobs: Vec<Option<Vec<u8>>> = Vec::new();
    let mut cells: Vec<Vec<Value>> = vec![Vec::new(); attributes.len()];
    let mut query = statement.query([]).map_err(sql_error)?;
    while let Some(row) = query.next().map_err(sql_error)? {
        let blob: Option<Vec<u8>> = row.get(0).map_err(sql_error)?;
        blobs.push(blob);
        for (index, column) in cells.iter_mut().enumerate() {
            column.push(row.get::<_, Value>(index + 1).map_err(sql_error)?);
        }
    }

    let rows_count = blobs.len();
    let mut wkb_rows: Vec<Option<&[u8]>> = Vec::with_capacity(rows_count);
    for (row, blob) in blobs.iter().enumerate() {
        wkb_rows.push(match blob {
            None => None,
            Some(blob) => {
                let decoded = decode_geometry_blob(blob)
                    .map_err(|e| format!("row {row} of layer {table:?}: {e}"))?;
                (!decoded.empty).then_some(decoded.wkb)
            }
        });
    }
    let column = GeoColumn::from_wkb(&wkb_rows, crs.as_deref())
        .map_err(|e| format!("layer {table:?}: {e}"))?;

    let mut fields = Vec::with_capacity(attributes.len());
    let mut arrays = Vec::with_capacity(attributes.len());
    for (attribute, values) in attributes.iter().zip(&cells) {
        let array = build_column(&attribute.declared, values)?;
        fields.push(Field::new(&attribute.name, array.data_type().clone(), true));
        arrays.push(array);
    }
    Ok(GpkgRead {
        layer: table,
        vector: VectorRead {
            geometry: column,
            attributes_ipc: ipc_from_columns(fields, arrays, rows_count)?,
        },
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Declared {
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    Float32,
    Float64,
    Text,
    Blob,
    Date,
    DateTime,
    Unknown,
}

fn declared_kind(declared: &str) -> Declared {
    let upper = declared.to_ascii_uppercase();
    let name = upper.split('(').next().unwrap_or("").trim();
    match name {
        "BOOLEAN" => Declared::Bool,
        "TINYINT" => Declared::Int8,
        "SMALLINT" => Declared::Int16,
        "MEDIUMINT" => Declared::Int32,
        "INT" | "INTEGER" => Declared::Int64,
        "FLOAT" => Declared::Float32,
        "DOUBLE" | "REAL" => Declared::Float64,
        "TEXT" => Declared::Text,
        "BLOB" => Declared::Blob,
        "DATE" => Declared::Date,
        "DATETIME" => Declared::DateTime,
        _ if name.contains("INT") => Declared::Int64,
        _ if name.contains("CHAR") || name.contains("CLOB") || name.contains("TEXT") => {
            Declared::Text
        }
        _ if name.contains("REAL") || name.contains("FLOA") || name.contains("DOUB") => {
            Declared::Float64
        }
        _ => Declared::Unknown,
    }
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Integer(v) => Some(*v),
        Value::Real(v) if v.fract() == 0.0 => Some(*v as i64),
        Value::Text(t) => t.trim().parse().ok(),
        _ => None,
    }
}

fn as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(v) => Some(*v as f64),
        Value::Real(v) => Some(*v),
        Value::Text(t) => t.trim().parse().ok(),
        _ => None,
    }
}

fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Integer(v) => Some(v.to_string()),
        Value::Real(v) => Some(v.to_string()),
        Value::Text(t) => Some(t.clone()),
        Value::Blob(b) => Some(String::from_utf8_lossy(b).into_owned()),
    }
}

fn build_column(declared: &str, values: &[Value]) -> Result<ArrayRef, String> {
    macro_rules! numeric {
        ($builder:ident, $convert:expr) => {{
            let mut builder = $builder::new();
            for value in values {
                builder.append_option($convert(value));
            }
            Arc::new(builder.finish()) as ArrayRef
        }};
    }
    let mut kind = declared_kind(declared);
    if kind == Declared::Unknown {
        // NUMERIC and untyped columns: decide from what the cells hold.
        let present = || values.iter().filter(|v| !matches!(v, Value::Null));
        kind = if present().all(|v| matches!(v, Value::Integer(_))) {
            Declared::Int64
        } else if present().all(|v| matches!(v, Value::Integer(_) | Value::Real(_))) {
            Declared::Float64
        } else if present().all(|v| matches!(v, Value::Blob(_))) && present().next().is_some() {
            Declared::Blob
        } else {
            Declared::Text
        };
    }
    if matches!(kind, Declared::Int8 | Declared::Int16 | Declared::Int32) {
        // A value outside the declared width would otherwise be nulled; widen instead.
        let fits = |x: i64| match kind {
            Declared::Int8 => i8::try_from(x).is_ok(),
            Declared::Int16 => i16::try_from(x).is_ok(),
            _ => i32::try_from(x).is_ok(),
        };
        if values.iter().filter_map(as_i64).any(|x| !fits(x)) {
            kind = Declared::Int64;
        }
    }
    Ok(match kind {
        Declared::Bool => numeric!(BooleanBuilder, |v: &Value| as_f64(v).map(|x| x != 0.0)),
        Declared::Int8 => numeric!(Int8Builder, |v: &Value| as_i64(v)
            .and_then(|x| i8::try_from(x).ok())),
        Declared::Int16 => numeric!(Int16Builder, |v: &Value| as_i64(v)
            .and_then(|x| i16::try_from(x).ok())),
        Declared::Int32 => numeric!(Int32Builder, |v: &Value| as_i64(v)
            .and_then(|x| i32::try_from(x).ok())),
        Declared::Int64 => numeric!(Int64Builder, as_i64),
        Declared::Float32 => numeric!(Float32Builder, |v: &Value| as_f64(v).map(|x| x as f32)),
        Declared::Float64 => numeric!(Float64Builder, as_f64),
        Declared::Blob => {
            let mut builder = BinaryBuilder::new();
            for value in values {
                match value {
                    Value::Blob(b) => builder.append_value(b),
                    Value::Null => builder.append_null(),
                    other => builder.append_value(as_text(other).unwrap_or_default().as_bytes()),
                }
            }
            Arc::new(builder.finish())
        }
        Declared::Text | Declared::Unknown | Declared::Date | Declared::DateTime => {
            let mut builder = StringBuilder::new();
            for value in values {
                builder.append_option(as_text(value));
            }
            let text: ArrayRef = Arc::new(builder.finish());
            let target = match kind {
                Declared::Date => Some(DataType::Date32),
                Declared::DateTime => Some(DataType::Timestamp(TimeUnit::Microsecond, None)),
                _ => None,
            };
            match target {
                // Unparseable text becomes null rather than failing the whole read.
                Some(target) => cast_with_options(
                    &text,
                    &target,
                    &CastOptions {
                        safe: true,
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())?,
                None => text,
            }
        }
    })
}

// -- info -----------------------------------------------------------------------------------

fn dtype_for(declared: &str) -> &'static str {
    match declared_kind(declared) {
        Declared::Bool => "bool",
        Declared::Int8 => "int8",
        Declared::Int16 => "int16",
        Declared::Int32 => "int32",
        Declared::Int64 => "int64",
        Declared::Float32 => "float32",
        Declared::Float64 => "float64",
        Declared::Date => "datetime64[D]",
        Declared::DateTime => "datetime64[ms]",
        Declared::Text | Declared::Blob | Declared::Unknown => "object",
    }
}

/// Facts about a layer (`None` picks the first feature layer).
pub fn gpkg_info(path: &str, layer: Option<&str>) -> Result<LayerInfo, String> {
    let conn = open_read(path)?;
    let rows = layer_rows(&conn)?;
    let chosen = &rows[select_layer(&rows, layer, false, path)?];
    let table = chosen.name.clone();
    let geometry = geometry_column(&conn, &table)?;
    let columns = table_columns(&conn, &table)?;

    let features: i64 = conn
        .query_row(
            &format!("SELECT count(*) FROM {}", quote(&table)),
            [],
            |r| r.get(0),
        )
        .map_err(sql_error)?;
    let stored: (Option<f64>, Option<f64>, Option<f64>, Option<f64>) = conn
        .query_row(
            "SELECT min_x, min_y, max_x, max_y FROM gpkg_contents WHERE table_name = ?1",
            [&table],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(sql_error)?;
    let bounds = match stored {
        (Some(a), Some(b), Some(c), Some(d)) => Some([a, b, c, d]),
        _ => scan_bounds(&conn, &table, &geometry.name)?,
    };
    Ok(LayerInfo {
        name: table,
        driver: "GPKG".to_string(),
        crs: crs_for_srs(&conn, geometry.srs_id)?,
        encoding: "UTF-8".to_string(),
        geometry_type: chosen
            .geometry_type
            .as_deref()
            .map(|t| display_geometry_type(t, chosen.z, chosen.m)),
        features: u64::try_from(features).unwrap_or(0),
        bounds,
        fields: columns
            .iter()
            .filter(|c| c.name != geometry.name && !is_fid(c, &columns))
            .map(|c| (c.name.clone(), dtype_for(&c.declared).to_string()))
            .collect(),
    })
}

fn scan_bounds(conn: &Connection, table: &str, geometry: &str) -> Result<Option<[f64; 4]>, String> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {g} FROM {t} WHERE {g} IS NOT NULL",
            g = quote(geometry),
            t = quote(table)
        ))
        .map_err(sql_error)?;
    let mut query = statement.query([]).map_err(sql_error)?;
    let mut total: Option<[f64; 4]> = None;
    while let Some(row) = query.next().map_err(sql_error)? {
        let blob: Vec<u8> = row.get(0).map_err(sql_error)?;
        if let Some(b) = blob_bounds(&blob)? {
            total = Some(union(total, b));
        }
    }
    Ok(total)
}

fn union(total: Option<[f64; 4]>, b: [f64; 4]) -> [f64; 4] {
    match total {
        None => b,
        Some(t) => [
            t[0].min(b[0]),
            t[1].min(b[1]),
            t[2].max(b[2]),
            t[3].max(b[3]),
        ],
    }
}

// -- writing --------------------------------------------------------------------------------

const INIT_SQL: &str = r#"
CREATE TABLE gpkg_spatial_ref_sys (
  srs_name TEXT NOT NULL,
  srs_id INTEGER NOT NULL PRIMARY KEY,
  organization TEXT NOT NULL,
  organization_coordsys_id INTEGER NOT NULL,
  definition TEXT NOT NULL,
  description TEXT
);
CREATE TABLE gpkg_contents (
  table_name TEXT NOT NULL PRIMARY KEY,
  data_type TEXT NOT NULL,
  identifier TEXT UNIQUE,
  description TEXT DEFAULT '',
  last_change DATETIME NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
  min_x DOUBLE, min_y DOUBLE, max_x DOUBLE, max_y DOUBLE,
  srs_id INTEGER,
  CONSTRAINT fk_gc_r_srs_id FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys(srs_id)
);
CREATE TABLE gpkg_geometry_columns (
  table_name TEXT NOT NULL,
  column_name TEXT NOT NULL,
  geometry_type_name TEXT NOT NULL,
  srs_id INTEGER NOT NULL,
  z TINYINT NOT NULL,
  m TINYINT NOT NULL,
  CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name),
  CONSTRAINT uk_gc_table_name UNIQUE (table_name),
  CONSTRAINT fk_gc_tn FOREIGN KEY (table_name) REFERENCES gpkg_contents(table_name),
  CONSTRAINT fk_gc_srs FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys (srs_id)
);
INSERT INTO gpkg_spatial_ref_sys VALUES
  ('Undefined Cartesian SRS', -1, 'NONE', -1, 'undefined', 'undefined Cartesian coordinate reference system'),
  ('Undefined geographic SRS', 0, 'NONE', 0, 'undefined', 'undefined geographic coordinate reference system');
"#;

const EXTENSIONS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS gpkg_extensions (
  table_name TEXT,
  column_name TEXT,
  extension_name TEXT NOT NULL,
  definition TEXT NOT NULL,
  scope TEXT NOT NULL,
  CONSTRAINT ge_tce UNIQUE (table_name, column_name, extension_name)
);
"#;

/// Writes `geometry` and the attributes as the layer `layer` of the GeoPackage at `path`.
/// Returns warnings (an SRS without a definition, for instance).
pub fn write_gpkg(
    path: &str,
    geometry: &GeoColumn,
    attributes_ipc: &[u8],
    layer: &str,
    options: GpkgWriteOptions,
) -> Result<Vec<String>, String> {
    let existed = Path::new(path).exists();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        write(path, geometry, attributes_ipc, layer, options)
    }))
    .unwrap_or_else(|_| Err("the GeoPackage encoder failed".to_string()));
    if result.is_err() && !existed {
        // The connection is closed by now; drop the empty or half-made file it created.
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(format!("{path}-journal"));
    }
    result
}

fn write(
    path: &str,
    geometry: &GeoColumn,
    attributes_ipc: &[u8],
    layer: &str,
    options: GpkgWriteOptions,
) -> Result<Vec<String>, String> {
    if layer.is_empty() || layer.contains('\0') {
        return Err("the layer name must be a non-empty string".to_string());
    }
    let rows = geometry.len();
    let table = Table::from_ipc(attributes_ipc, rows)?;
    if table.names.iter().any(|n| n.eq_ignore_ascii_case("fid")) {
        return Err(
            "a column named \"fid\" clashes with the feature id column of a GeoPackage; rename it"
                .to_string(),
        );
    }
    let blobs = geometry.to_wkb()?;
    let bounds = geometry.bounds();
    let (row_z, row_m) = (geometry.has_z(), geometry.has_m());
    let present = blobs.iter().flatten().count();
    let flag = |rows: &[bool]| -> i64 {
        match rows.iter().filter(|z| **z).count() {
            0 => 0,
            n if n == present => 1,
            _ => 2,
        }
    };
    let (layer_z, layer_m) = (flag(&row_z), flag(&row_m));

    let fresh = !Path::new(path).exists() || std::fs::metadata(path).is_ok_and(|m| m.len() == 0);
    let mut conn = Connection::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    register_functions(&conn).map_err(sql_error)?;
    if !fresh {
        ensure_geopackage(&conn, path)?;
    }

    let mut warnings = Vec::new();
    let tx = conn.transaction().map_err(sql_error)?;
    if fresh {
        tx.execute_batch(&format!(
            "PRAGMA application_id = {APPLICATION_ID}; PRAGMA user_version = {USER_VERSION};"
        ))
        .map_err(sql_error)?;
        tx.execute_batch(INIT_SQL).map_err(sql_error)?;
    }
    tx.execute_batch(EXTENSIONS_SQL).map_err(sql_error)?;
    let wgs84 = epsg_definition(4326).unwrap_or("undefined");
    tx.execute(
        "INSERT OR IGNORE INTO gpkg_spatial_ref_sys \
         (srs_name, srs_id, organization, organization_coordsys_id, definition, description) \
         SELECT 'WGS 84 geodetic', 4326, 'EPSG', 4326, ?1, \
                'longitude/latitude coordinates in decimal degrees on the WGS 84 spheroid' \
         WHERE NOT EXISTS (SELECT 1 FROM gpkg_spatial_ref_sys WHERE srs_id = 4326)",
        [wgs84],
    )
    .map_err(sql_error)?;

    let crs = geometry.crs();
    let srs_id = resolve_srs(&tx, crs.as_deref(), &mut warnings)?;
    let existing = layer_rows(&tx)?
        .into_iter()
        .find(|row| row.name.eq_ignore_ascii_case(layer))
        .map(|row| row.name);

    // On append: the layer's Z and M flags and whether it already holds rows.
    let mut prior: Option<(i64, i64, bool)> = None;
    let (target, geometry_name, insert_columns) = match (&existing, options.append) {
        (Some(name), true) => {
            let (geometry_name, columns) =
                check_append(&tx, name, &table, geometry, srs_id, crs.as_deref())?;
            let (old_z, old_m): (i64, i64) = tx
                .query_row(
                    "SELECT z, m FROM gpkg_geometry_columns WHERE table_name = ?1",
                    [name],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(sql_error)?;
            let had_rows: bool = tx
                .query_row(
                    &format!("SELECT EXISTS (SELECT 1 FROM {})", quote(name)),
                    [],
                    |r| r.get(0),
                )
                .map_err(sql_error)?;
            prior = Some((old_z, old_m, had_rows));
            (name.clone(), geometry_name, columns)
        }
        (existing, _) => {
            if let Some(name) = existing {
                drop_layer(&tx, name)?;
            }
            let geometry_name = create_layer(
                &tx,
                layer,
                geometry,
                &table,
                srs_id,
                options.spatial_index,
                (layer_z, layer_m),
            )?;
            (
                layer.to_string(),
                geometry_name,
                table.names.iter().cloned().enumerate().collect(),
            )
        }
    };

    let mut total: Option<[f64; 4]> = None;
    {
        let columns = std::iter::once(quote(&geometry_name))
            .chain(insert_columns.iter().map(|(_, name)| quote(name)))
            .collect::<Vec<_>>()
            .join(", ");
        let placeholders = (1..=insert_columns.len() + 1)
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut insert = tx
            .prepare(&format!(
                "INSERT INTO {} ({columns}) VALUES ({placeholders})",
                quote(&target)
            ))
            .map_err(sql_error)?;
        for row in 0..rows {
            let mut values: Vec<Value> = Vec::with_capacity(insert_columns.len() + 1);
            values.push(match &blobs[row] {
                None => Value::Null,
                Some(wkb) => {
                    if let Some(b) = bounds[row] {
                        total = Some(union(total, b));
                    }
                    let (z, m) = if row_z[row] || row_m[row] {
                        crate::zm_io::zm_ranges(wkb)
                    } else {
                        (None, None)
                    };
                    Value::Blob(encode_row_blob(wkb, bounds[row], srs_id, z, m))
                }
            });
            for (column, _) in &insert_columns {
                values.push(cell_value(&table, *column, row)?);
            }
            insert
                .execute(params_from_iter(values))
                .map_err(|e| format!("cannot insert row {row}: {e}"))?;
        }
    }

    if let (Some((old_z, old_m, had_rows)), true) = (prior, present > 0) {
        // Rows of another dimension make the ordinate optional (2); an empty layer takes the
        // flags of the new rows.
        let merge = |old: i64, new: i64| if !had_rows || old == new { new } else { 2 };
        tx.execute(
            "UPDATE gpkg_geometry_columns SET z = ?1, m = ?2 WHERE table_name = ?3",
            params![merge(old_z, layer_z), merge(old_m, layer_m), target],
        )
        .map_err(sql_error)?;
    }
    if let Some(b) = total {
        tx.execute(
            "UPDATE gpkg_contents SET \
               min_x = CASE WHEN min_x IS NULL OR ?1 < min_x THEN ?1 ELSE min_x END, \
               min_y = CASE WHEN min_y IS NULL OR ?2 < min_y THEN ?2 ELSE min_y END, \
               max_x = CASE WHEN max_x IS NULL OR ?3 > max_x THEN ?3 ELSE max_x END, \
               max_y = CASE WHEN max_y IS NULL OR ?4 > max_y THEN ?4 ELSE max_y END \
             WHERE table_name = ?5",
            params![b[0], b[1], b[2], b[3], target],
        )
        .map_err(sql_error)?;
    }
    tx.execute(
        "UPDATE gpkg_contents SET last_change = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE table_name = ?1",
        [&target],
    )
    .map_err(sql_error)?;
    tx.commit().map_err(sql_error)?;
    Ok(warnings)
}

fn encode_row_blob(
    wkb: &[u8],
    bounds: Option<[f64; 4]>,
    srs_id: i64,
    z: Option<[f64; 2]>,
    m: Option<[f64; 2]>,
) -> Vec<u8> {
    let srs = i32::try_from(srs_id).unwrap_or(-1);
    match bounds {
        Some(b) => encode_geometry_blob_zm(wkb, srs, Some(b), z, m),
        None => {
            // A geometry without bounds is empty: set the empty flag, no envelope.
            let mut blob = encode_geometry_blob(wkb, srs, None);
            blob[3] |= 0b0001_0000;
            blob
        }
    }
}

/// Whether an ISO timestamp already says its offset (`Z`, `+02:00`, `-0500`).
fn has_utc_offset(text: &str) -> bool {
    text.ends_with('Z')
        || text
            .split_once('T')
            .is_some_and(|(_, time)| time.contains(['+', '-']))
}

fn cell_value(table: &Table, column: usize, row: usize) -> Result<Value, String> {
    let text_type = matches!(table.source_types[column], DataType::Timestamp(_, _));
    Ok(match table.cell(column, row) {
        Cell::Null => Value::Null,
        Cell::Bool(v) => Value::Integer(i64::from(v)),
        Cell::Int(v) => Value::Integer(v),
        Cell::UInt(v) => Value::Integer(i64::try_from(v).map_err(|_| {
            format!(
                "column {:?} holds {v}, which does not fit a GeoPackage integer",
                table.names[column]
            )
        })?),
        Cell::Float(v) => Value::Real(v),
        Cell::Str(text) if text_type && !has_utc_offset(text) => Value::Text(format!("{text}Z")),
        Cell::Str(text) => Value::Text(text.to_string()),
        Cell::Bytes(bytes) => Value::Blob(bytes.to_vec()),
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Integer,
    Real,
    Text,
    Blob,
}

/// The SQLite storage class a declared type stores; `None` when it says nothing.
fn storage_class(kind: Declared) -> Option<Class> {
    match kind {
        Declared::Bool | Declared::Int8 | Declared::Int16 | Declared::Int32 | Declared::Int64 => {
            Some(Class::Integer)
        }
        Declared::Float32 | Declared::Float64 => Some(Class::Real),
        Declared::Text | Declared::Date | Declared::DateTime => Some(Class::Text),
        Declared::Blob => Some(Class::Blob),
        Declared::Unknown => None,
    }
}

fn declared_type(data_type: &DataType) -> &'static str {
    match data_type {
        DataType::Boolean => "BOOLEAN",
        DataType::Int8 => "TINYINT",
        DataType::Int16 | DataType::UInt8 => "SMALLINT",
        DataType::Int32 | DataType::UInt16 => "MEDIUMINT",
        DataType::Int64 | DataType::UInt32 | DataType::UInt64 => "INTEGER",
        DataType::Float32 => "FLOAT",
        DataType::Float64 | DataType::Float16 => "REAL",
        DataType::Binary | DataType::LargeBinary => "BLOB",
        DataType::Date32 | DataType::Date64 => "DATE",
        DataType::Timestamp(_, _) => "DATETIME",
        _ => "TEXT",
    }
}

/// The geometry type name of a layer: the rows' common type, else `GEOMETRY`.
fn geometry_type_name(geometry: &GeoColumn) -> &'static str {
    use geo::Geometry as G;
    let mut names = geometry.geo_rows().into_iter().flatten().map(|g| match g {
        G::Point(_) => "POINT",
        G::Line(_) | G::LineString(_) => "LINESTRING",
        G::Polygon(_) | G::Rect(_) | G::Triangle(_) => "POLYGON",
        G::MultiPoint(_) => "MULTIPOINT",
        G::MultiLineString(_) => "MULTILINESTRING",
        G::MultiPolygon(_) => "MULTIPOLYGON",
        G::GeometryCollection(_) => "GEOMETRYCOLLECTION",
    });
    match names.next() {
        Some(first) if names.all(|n| n == first) => first,
        _ => "GEOMETRY",
    }
}

fn create_layer(
    tx: &Connection,
    layer: &str,
    geometry: &GeoColumn,
    table: &Table,
    srs_id: i64,
    spatial_index: bool,
    (z, m): (i64, i64),
) -> Result<String, String> {
    let taken = |name: &str| table.names.iter().any(|n| n.eq_ignore_ascii_case(name));
    let geometry_name = ["geom", "geometry", "shape"]
        .iter()
        .map(|s| s.to_string())
        .chain((1..).map(|i| format!("geom_{i}")))
        .find(|name| !taken(name))
        .expect("an unbounded iterator");
    let type_name = geometry_type_name(geometry);

    let mut definition = format!(
        "{} INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, {} {type_name}",
        quote("fid"),
        quote(&geometry_name)
    );
    for (name, kind) in table.names.iter().zip(&table.source_types) {
        definition.push_str(&format!(", {} {}", quote(name), declared_type(kind)));
    }
    tx.execute_batch(&format!("CREATE TABLE {} ( {definition} )", quote(layer)))
        .map_err(|e| format!("cannot create layer {layer:?}: {e}"))?;
    tx.execute(
        "INSERT INTO gpkg_contents (table_name, data_type, identifier, srs_id) \
         VALUES (?1, 'features', ?1, ?2)",
        params![layer, srs_id],
    )
    .map_err(sql_error)?;
    tx.execute(
        "INSERT INTO gpkg_geometry_columns VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![layer, geometry_name, type_name, srs_id, z, m],
    )
    .map_err(sql_error)?;
    if spatial_index {
        create_rtree(tx, layer, &geometry_name)?;
    }
    Ok(geometry_name)
}

fn create_rtree(tx: &Connection, table: &str, column: &str) -> Result<(), String> {
    let index = format!("rtree_{table}_{column}");
    let (t, c, i) = (quote(table), quote(column), quote("fid"));
    let ix = quote(&index);
    let bounds = |prefix: &str| {
        format!(
            "{prefix}{i}, ST_MinX({prefix}{c}), ST_MaxX({prefix}{c}), ST_MinY({prefix}{c}), ST_MaxY({prefix}{c})"
        )
    };
    let new_bounds = bounds("NEW.");
    let trigger = |suffix: &str| quote(&format!("{index}_{suffix}"));
    let sql = format!(
        "CREATE VIRTUAL TABLE {ix} USING rtree(id, minx, maxx, miny, maxy);
         INSERT INTO gpkg_extensions (table_name, column_name, extension_name, definition, scope)
           VALUES ({table_literal}, {column_literal}, 'gpkg_rtree_index',
                   'http://www.geopackage.org/spec120/#extension_rtree', 'write-only');
         CREATE TRIGGER {insert} AFTER INSERT ON {t}
           WHEN (new.{c} NOT NULL AND NOT ST_IsEmpty(NEW.{c}))
           BEGIN INSERT OR REPLACE INTO {ix} VALUES ({new_bounds}); END;
         CREATE TRIGGER {update1} AFTER UPDATE OF {c} ON {t}
           WHEN OLD.{i} = NEW.{i} AND (NEW.{c} NOTNULL AND NOT ST_IsEmpty(NEW.{c}))
           BEGIN INSERT OR REPLACE INTO {ix} VALUES ({new_bounds}); END;
         CREATE TRIGGER {update2} AFTER UPDATE OF {c} ON {t}
           WHEN OLD.{i} = NEW.{i} AND (NEW.{c} IS NULL OR ST_IsEmpty(NEW.{c}))
           BEGIN DELETE FROM {ix} WHERE id = OLD.{i}; END;
         CREATE TRIGGER {update3} AFTER UPDATE ON {t}
           WHEN OLD.{i} != NEW.{i} AND (NEW.{c} NOTNULL AND NOT ST_IsEmpty(NEW.{c}))
           BEGIN DELETE FROM {ix} WHERE id = OLD.{i};
                 INSERT OR REPLACE INTO {ix} VALUES ({new_bounds}); END;
         CREATE TRIGGER {update4} AFTER UPDATE ON {t}
           WHEN OLD.{i} != NEW.{i} AND (NEW.{c} IS NULL OR ST_IsEmpty(NEW.{c}))
           BEGIN DELETE FROM {ix} WHERE id IN (OLD.{i}, NEW.{i}); END;
         CREATE TRIGGER {delete} AFTER DELETE ON {t}
           WHEN old.{c} NOT NULL
           BEGIN DELETE FROM {ix} WHERE id = OLD.{i}; END;",
        table_literal = literal(table),
        column_literal = literal(column),
        insert = trigger("insert"),
        update1 = trigger("update1"),
        update2 = trigger("update2"),
        update3 = trigger("update3"),
        update4 = trigger("update4"),
        delete = trigger("delete"),
    );
    tx.execute_batch(&sql).map_err(sql_error)?;
    // Index rows already present (appending an index to an existing layer).
    tx.execute_batch(&format!(
        "INSERT OR REPLACE INTO {ix} SELECT {i}, ST_MinX({c}), ST_MaxX({c}), ST_MinY({c}), ST_MaxY({c}) \
         FROM {t} WHERE {c} NOT NULL AND NOT ST_IsEmpty({c})"
    ))
    .map_err(sql_error)?;
    Ok(())
}

/// Removes a layer: its table (with triggers), R-tree and metadata rows.
fn drop_layer(tx: &Connection, table: &str) -> Result<(), String> {
    let geometry: Option<String> = tx
        .query_row(
            "SELECT column_name FROM gpkg_geometry_columns WHERE table_name = ?1",
            [table],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })
        .map_err(sql_error)?;
    tx.execute_batch(&format!("DROP TABLE {}", quote(table)))
        .map_err(sql_error)?;
    if let Some(column) = &geometry {
        tx.execute_batch(&format!(
            "DROP TABLE IF EXISTS {}",
            quote(&format!("rtree_{table}_{column}"))
        ))
        .map_err(sql_error)?;
    }
    for sql in [
        "DELETE FROM gpkg_extensions WHERE lower(table_name) = lower(?1)",
        "DELETE FROM gpkg_geometry_columns WHERE lower(table_name) = lower(?1)",
        "DELETE FROM gpkg_contents WHERE lower(table_name) = lower(?1)",
    ] {
        tx.execute(sql, [table]).map_err(sql_error)?;
    }
    // GDAL's feature-count bookkeeping table, when present.
    let _ = tx.execute(
        "DELETE FROM gpkg_ogr_contents WHERE lower(table_name) = lower(?1)",
        [table],
    );
    Ok(())
}

/// Validates appending `table`'s columns and CRS to the existing layer `name`. Returns the
/// geometry column and the `(frame column index, table column name)` pairs to insert.
fn check_append(
    tx: &Connection,
    name: &str,
    table: &Table,
    new_geometry: &GeoColumn,
    srs_id: i64,
    crs: Option<&str>,
) -> Result<(String, Vec<(usize, String)>), String> {
    let geometry = geometry_column(tx, name)?;
    let layer_type = geometry.type_name.to_ascii_uppercase();
    let new_type = geometry_type_name(new_geometry);
    let has_rows = new_geometry
        .geo_rows()
        .into_iter()
        .flatten()
        .next()
        .is_some();
    if has_rows && layer_type != "GEOMETRY" && layer_type != new_type {
        return Err(format!(
            "geometry type mismatch: layer {name:?} holds {layer_type} but the frame holds {new_type}"
        ));
    }
    let existing_crs = crs_for_srs(tx, geometry.srs_id)?;
    let new_crs = crs_for_srs(tx, srs_id)?;
    if existing_crs != new_crs {
        return Err(format!(
            "CRS mismatch: layer {name:?} uses {} but the frame uses {}",
            existing_crs.as_deref().unwrap_or("no CRS"),
            new_crs.as_deref().or(crs).unwrap_or("no CRS")
        ));
    }
    let columns = table_columns(tx, name)?;
    let mut pairs = Vec::with_capacity(table.names.len());
    for (index, column) in table.names.iter().enumerate() {
        let found = columns
            .iter()
            .find(|c| {
                c.name.eq_ignore_ascii_case(column)
                    && c.name != geometry.name
                    && !is_fid(c, &columns)
            })
            .ok_or_else(|| format!("layer {name:?} has no column {column:?} to append to"))?;
        let have = storage_class(declared_kind(&found.declared));
        let want = storage_class(declared_kind(declared_type(&table.source_types[index])));
        let fits = match (have, want) {
            (Some(have), Some(want)) => {
                have == want || (have == Class::Real && want == Class::Integer)
            }
            _ => true,
        };
        if !fits {
            return Err(format!(
                "column {column:?} of layer {name:?} is {} but the frame's column is {}",
                found.declared,
                declared_type(&table.source_types[index])
            ));
        }
        pairs.push((index, found.name.clone()));
    }
    Ok((geometry.name, pairs))
}
