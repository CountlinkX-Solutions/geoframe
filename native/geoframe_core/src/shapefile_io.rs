//! ESRI Shapefile read and write.
//!
//! Shapes are read with the pure-Rust `shapefile` crate; the `.shp`/`.shx` writer is local
//! because the crate's writer cannot emit null-shape records, which GeoFrame needs for null
//! geometries. The `.dbf` attribute table is handled by [`crate::dbf`] (types, encodings and
//! GDAL's field-name truncation are described there).
//!
//! # Geometry
//!
//! A shapefile holds one shape type. Points, multipoints, lines (`LineString` and
//! `MultiLineString` both become polyline shapes) and polygons (`Polygon` and `MultiPolygon`
//! both become polygon shapes) are supported; a frame that mixes these, or holds a
//! `GeometryCollection`, is rejected like GDAL does. Null and empty geometries become null
//! shapes. On read, a polyline with one part is a `LineString` and with several a
//! `MultiLineString`, a polygon shape with one outer ring a `Polygon` and with several a
//! `MultiPolygon`. Written polygon rings follow the format's orientation (exterior clockwise,
//! holes counter-clockwise). Z and M values are dropped on read; output is 2D.
//!
//! # Sidecar files
//!
//! Writing `a.shp` creates `a.shx`, `a.dbf`, `a.cpg` (the encoding) and, when the CRS has a
//! definition, `a.prj` (see [`crate::crs_wkt`]); a stale `.prj` from an earlier write is
//! removed when there is no CRS. A path ending in `.zip` reads and writes a zip archive
//! holding one shapefile instead. Reading finds the sidecars next to the `.shp` (any case);
//! a missing `.dbf` gives geometry only.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use geo::{Coord, Geometry, LineString, Polygon};

use crate::attributes::{ipc_from_columns, Table};
use crate::column::ColumnBuilder;
use crate::crs_wkt::{crs_to_prj_wkt, prj_wkt_to_crs};
use crate::dbf::{self, Encoding};
use crate::gpkg_io::LayerInfo;
use crate::{GeoColumn, VectorRead};

// -- reading ------------------------------------------------------------------------------

/// The files of one shapefile, loaded into memory.
struct Parts {
    name: String,
    shp: Vec<u8>,
    shx: Option<Vec<u8>>,
    dbf: Option<Vec<u8>>,
    prj: Option<String>,
    cpg: Option<String>,
}

fn is_zip(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".zip")
}

fn sibling(path: &Path, extension: &str) -> Option<PathBuf> {
    [extension.to_string(), extension.to_ascii_uppercase()]
        .into_iter()
        .map(|e| path.with_extension(e))
        .find(|p| p.is_file())
}

fn load_parts(path: &str) -> Result<Parts, String> {
    if is_zip(path) {
        return load_zip(path);
    }
    let shp_path = Path::new(path);
    let shp = fs::read(shp_path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let read = |extension: &str| sibling(shp_path, extension).and_then(|p| fs::read(p).ok());
    Ok(Parts {
        name: shp_path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        shx: read("shx"),
        dbf: read("dbf"),
        prj: read("prj").map(|b| String::from_utf8_lossy(&b).into_owned()),
        cpg: read("cpg").map(|b| String::from_utf8_lossy(&b).into_owned()),
        shp,
    })
}

fn load_zip(path: &str) -> Result<Parts, String> {
    let file = fs::File::open(path).map_err(|e| format!("cannot open {path}: {e}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("{path} is not a readable zip file: {e}"))?;
    let names: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .map(|n| n.into_owned())
        .collect();
    let shapes: Vec<&String> = names
        .iter()
        .filter(|n| n.to_ascii_lowercase().ends_with(".shp") && !n.starts_with("__MACOSX/"))
        .collect();
    let stem = match shapes.as_slice() {
        [one] => one[..one.len() - 4].to_string(),
        [] => return Err(format!("{path} contains no .shp file")),
        _ => {
            return Err(format!(
                "{path} contains several .shp files; extract the one you want"
            ))
        }
    };
    let mut member = |extension: &str| -> Option<Vec<u8>> {
        let wanted = format!("{stem}.{extension}").to_ascii_lowercase();
        let name = names.iter().find(|n| n.to_ascii_lowercase() == wanted)?;
        let mut bytes = Vec::new();
        archive.by_name(name).ok()?.read_to_end(&mut bytes).ok()?;
        Some(bytes)
    };
    let shp = member("shp").ok_or_else(|| format!("cannot read the .shp in {path}"))?;
    Ok(Parts {
        name: Path::new(&stem)
            .file_name()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        shx: member("shx"),
        dbf: member("dbf"),
        prj: member("prj").map(|b| String::from_utf8_lossy(&b).into_owned()),
        cpg: member("cpg").map(|b| String::from_utf8_lossy(&b).into_owned()),
        shp,
    })
}

/// An explicit `encoding` wins, then the `.cpg`, then UTF-8.
fn resolve_encoding(explicit: Option<&str>, cpg: Option<&str>) -> Result<Encoding, String> {
    match (explicit, cpg.map(str::trim).filter(|c| !c.is_empty())) {
        (Some(name), _) => Encoding::parse(name),
        (None, Some(name)) => Encoding::parse(name)
            .map_err(|e| format!("{e} (from the .cpg file; pass encoding: to override)")),
        (None, None) => Ok(Encoding::Utf8),
    }
}

/// Reads a shapefile (`.shp` path, or a `.zip` holding one). `encoding` overrides the `.cpg`.
pub fn read_shapefile(path: &str, encoding: Option<&str>) -> Result<VectorRead, String> {
    catch_unwind(AssertUnwindSafe(|| read(path, encoding)))
        .unwrap_or_else(|_| Err(format!("{path} is not a readable shapefile")))
}

fn read(path: &str, encoding: Option<&str>) -> Result<VectorRead, String> {
    let parts = load_parts(path)?;
    let encoding = resolve_encoding(encoding, parts.cpg.as_deref())?;
    let crs = parts
        .prj
        .as_deref()
        .map(prj_wkt_to_crs)
        .filter(|c| !c.is_empty());

    let table = parts
        .dbf
        .as_deref()
        .map(|bytes| dbf::read_dbf(bytes, encoding))
        .transpose()
        .map_err(|e| format!("cannot read the attributes of {path}: {e}"))?;
    let shapes = shapefile::ShapeReader::new(Cursor::new(&parts.shp))
        .and_then(shapefile::ShapeReader::read)
        .map_err(|e| format!("cannot read {path} as a shapefile: {e}"))?;

    let deleted = table.as_ref().map(|t| t.deleted.clone());
    if let Some(table) = &table {
        if table.records != shapes.len() {
            return Err(format!(
                "{path} has {} shapes but {} attribute records",
                shapes.len(),
                table.records
            ));
        }
    }
    let keep = |row: usize| deleted.as_ref().is_none_or(|d| !d[row]);

    let mut builder = ColumnBuilder::new(crs.as_deref());
    let mut kept = 0usize;
    for (row, shape) in shapes.into_iter().enumerate() {
        if !keep(row) {
            continue;
        }
        kept += 1;
        match to_geometry(shape)? {
            Some(geometry) => builder.push(Some(&geometry))?,
            None => builder.push(None::<&Geometry<f64>>)?,
        }
    }

    let attributes_ipc = match table {
        Some(table) => {
            let columns = if table.deleted.iter().any(|d| *d) {
                dbf::drop_deleted(&table.columns, &table.deleted)?
            } else {
                table.columns
            };
            ipc_from_columns(table.fields, columns, kept)?
        }
        None => ipc_from_columns(Vec::new(), Vec::new(), kept)?,
    };
    Ok(VectorRead {
        geometry: builder.finish(),
        attributes_ipc,
    })
}

fn to_geometry(shape: shapefile::Shape) -> Result<Option<Geometry<f64>>, String> {
    if matches!(shape, shapefile::Shape::NullShape) {
        return Ok(None);
    }
    let geometry = Geometry::<f64>::try_from(shape).map_err(|e| format!("invalid shape: {e}"))?;
    Ok(Some(match geometry {
        Geometry::MultiLineString(mut lines) if lines.0.len() == 1 => {
            Geometry::LineString(lines.0.remove(0))
        }
        Geometry::MultiPolygon(mut polygons) if polygons.0.len() == 1 => {
            Geometry::Polygon(polygons.0.remove(0))
        }
        other => other,
    }))
}

// -- info -----------------------------------------------------------------------------------

/// Layer facts from the headers, without reading the shapes.
pub fn shapefile_info(path: &str, encoding: Option<&str>) -> Result<LayerInfo, String> {
    catch_unwind(AssertUnwindSafe(|| info(path, encoding)))
        .unwrap_or_else(|_| Err(format!("{path} is not a readable shapefile")))
}

fn info(path: &str, encoding: Option<&str>) -> Result<LayerInfo, String> {
    let parts = load_parts(path)?;
    let encoding = resolve_encoding(encoding, parts.cpg.as_deref())?;
    if parts.shp.len() < 100 {
        return Err(format!(
            "{path} is not a shapefile (the .shp header is truncated)"
        ));
    }
    let shape_type = i32::from_le_bytes(parts.shp[32..36].try_into().expect("4 bytes"));
    let double = |offset: usize| {
        f64::from_le_bytes(parts.shp[offset..offset + 8].try_into().expect("8 bytes"))
    };
    let features = match (&parts.shx, &parts.dbf) {
        (Some(shx), _) => (shx.len().saturating_sub(100) / 8) as u64,
        (None, Some(dbf)) => dbf::parse_header(dbf, encoding)?.records as u64,
        (None, None) => shapefile::ShapeReader::new(Cursor::new(&parts.shp))
            .and_then(shapefile::ShapeReader::read)
            .map_err(|e| format!("cannot read {path} as a shapefile: {e}"))?
            .len() as u64,
    };
    let fields = match &parts.dbf {
        Some(bytes) => dbf::parse_header(bytes, encoding)?
            .fields
            .iter()
            .map(|f| (f.name.clone(), dbf::dtype_name(f).to_string()))
            .collect(),
        None => Vec::new(),
    };
    Ok(LayerInfo {
        name: parts.name,
        driver: "ESRI Shapefile".to_string(),
        crs: parts
            .prj
            .as_deref()
            .map(prj_wkt_to_crs)
            .filter(|c| !c.is_empty()),
        encoding: encoding.cpg_name().to_string(),
        geometry_type: geometry_type_name(shape_type),
        features,
        bounds: (features > 0).then(|| [double(36), double(44), double(52), double(60)]),
        fields,
    })
}

/// The name GDAL / pyogrio give a shapefile shape type.
fn geometry_type_name(shape_type: i32) -> Option<String> {
    let (base, suffix) = match shape_type {
        1 => ("Point", ""),
        3 => ("LineString", ""),
        5 => ("Polygon", ""),
        8 => ("MultiPoint", ""),
        11 => ("Point", " Z"),
        13 => ("LineString", " Z"),
        15 => ("Polygon", " Z"),
        18 => ("MultiPoint", " Z"),
        21 => ("Point", " M"),
        23 => ("LineString", " M"),
        25 => ("Polygon", " M"),
        28 => ("MultiPoint", " M"),
        31 => ("Unknown", ""),
        _ => return None,
    };
    Some(format!("{base}{suffix}"))
}

// -- writing --------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Point,
    MultiPoint,
    Line,
    Polygon,
}

impl Kind {
    fn shape_type(self) -> i32 {
        match self {
            Self::Point => 1,
            Self::Line => 3,
            Self::Polygon => 5,
            Self::MultiPoint => 8,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Point => "Point",
            Self::MultiPoint => "MultiPoint",
            Self::Line => "LineString/MultiLineString",
            Self::Polygon => "Polygon/MultiPolygon",
        }
    }
}

fn kind_of(geometry: &Geometry<f64>) -> Result<Kind, String> {
    Ok(match geometry {
        Geometry::Point(_) => Kind::Point,
        Geometry::MultiPoint(_) => Kind::MultiPoint,
        Geometry::Line(_) | Geometry::LineString(_) | Geometry::MultiLineString(_) => Kind::Line,
        Geometry::Polygon(_)
        | Geometry::MultiPolygon(_)
        | Geometry::Rect(_)
        | Geometry::Triangle(_) => Kind::Polygon,
        Geometry::GeometryCollection(_) => {
            return Err("a shapefile cannot store a GeometryCollection".to_string())
        }
    })
}

/// Writes a shapefile (`.shp` path, or a `.zip`). Returns warnings (changed field names,
/// truncated values, a CRS without a `.prj` definition).
pub fn write_shapefile(
    path: &str,
    geometry: &GeoColumn,
    attributes_ipc: &[u8],
    encoding: Option<&str>,
) -> Result<Vec<String>, String> {
    catch_unwind(AssertUnwindSafe(|| {
        write(path, geometry, attributes_ipc, encoding)
    }))
    .unwrap_or_else(|_| Err("the shapefile encoder failed".to_string()))
}

fn write(
    path: &str,
    geometry: &GeoColumn,
    attributes_ipc: &[u8],
    encoding: Option<&str>,
) -> Result<Vec<String>, String> {
    let encoding = encoding.map_or(Ok(Encoding::Utf8), Encoding::parse)?;
    let rows = geometry.len();
    let table = Table::from_ipc(attributes_ipc, rows)?;
    let shapes = geometry.geo_rows();

    let mut kind: Option<Kind> = None;
    for shape in shapes.iter().flatten() {
        let this = kind_of(shape)?;
        match kind {
            None => kind = Some(this),
            Some(previous) if previous != this => {
                return Err(format!(
                    "a shapefile holds a single geometry type, but the geometries mix {} and {}",
                    previous.label(),
                    this.label()
                ))
            }
            Some(_) => {}
        }
    }
    let kind = kind.unwrap_or(Kind::Point);

    let mut warnings = Vec::new();
    let (shp, shx) = encode_shapes(&shapes, kind);
    let dbf = dbf::write_dbf(&table, rows, encoding, &mut warnings)?;
    let crs = geometry.crs();
    let prj = match crs.as_deref() {
        Some(crs) => {
            let wkt = crs_to_prj_wkt(crs);
            if wkt.is_none() {
                warnings.push(format!(
                    "CRS {crs} has no .prj definition available, so no .prj file was written and the shapefile has no coordinate system"
                ));
            }
            wkt
        }
        None => None,
    };
    let cpg = encoding.cpg_name().as_bytes().to_vec();

    if is_zip(path) {
        write_zip(path, shp, shx, dbf, prj, cpg)?;
    } else {
        write_files(path, shp, shx, dbf, prj, cpg)?;
    }
    Ok(warnings)
}

fn write_files(
    path: &str,
    shp: Vec<u8>,
    shx: Vec<u8>,
    dbf: Vec<u8>,
    prj: Option<String>,
    cpg: Vec<u8>,
) -> Result<(), String> {
    let base = Path::new(path);
    let put = |extension: &str, bytes: &[u8]| {
        let target = base.with_extension(extension);
        fs::write(&target, bytes).map_err(|e| format!("cannot write {}: {e}", target.display()))
    };
    put("shp", &shp)?;
    put("shx", &shx)?;
    put("dbf", &dbf)?;
    put("cpg", &cpg)?;
    match prj {
        Some(wkt) => put("prj", wkt.as_bytes())?,
        None => {
            let _ = fs::remove_file(base.with_extension("prj"));
        }
    }
    Ok(())
}

fn write_zip(
    path: &str,
    shp: Vec<u8>,
    shx: Vec<u8>,
    dbf: Vec<u8>,
    prj: Option<String>,
    cpg: Vec<u8>,
) -> Result<(), String> {
    let stem = Path::new(path)
        .file_stem()
        .map_or_else(|| "layer".to_string(), |s| s.to_string_lossy().into_owned());
    let file = fs::File::create(path).map_err(|e| format!("cannot write {path}: {e}"))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut members: Vec<(&str, Vec<u8>)> =
        vec![("shp", shp), ("shx", shx), ("dbf", dbf), ("cpg", cpg)];
    if let Some(wkt) = prj {
        members.push(("prj", wkt.into_bytes()));
    }
    for (extension, bytes) in members {
        writer
            .start_file(format!("{stem}.{extension}"), options)
            .and_then(|()| writer.write_all(&bytes).map_err(zip::result::ZipError::Io))
            .map_err(|e| format!("cannot write {path}: {e}"))?;
    }
    writer
        .finish()
        .map(|_| ())
        .map_err(|e| format!("cannot write {path}: {e}"))
}

// -- .shp / .shx encoding --------------------------------------------------------------------

/// The points of every part of a shape and the start index of each part.
struct Shape {
    parts: Vec<Vec<Coord<f64>>>,
}

fn signed_area(ring: &[Coord<f64>]) -> f64 {
    ring.windows(2)
        .map(|w| w[0].x * w[1].y - w[1].x * w[0].y)
        .sum::<f64>()
        / 2.0
}

/// The ring's points oriented clockwise (`clockwise`) or counter-clockwise.
fn oriented(ring: &LineString<f64>, clockwise: bool) -> Vec<Coord<f64>> {
    let mut points = ring.0.clone();
    if (signed_area(&points) < 0.0) != clockwise {
        points.reverse();
    }
    points
}

fn polygon_parts(polygon: &Polygon<f64>, parts: &mut Vec<Vec<Coord<f64>>>) {
    if polygon.exterior().0.len() < 4 {
        return;
    }
    parts.push(oriented(polygon.exterior(), true));
    for hole in polygon.interiors().iter().filter(|h| h.0.len() >= 4) {
        parts.push(oriented(hole, false));
    }
}

fn shape_parts(geometry: &Geometry<f64>, kind: Kind) -> Option<Shape> {
    let mut parts: Vec<Vec<Coord<f64>>> = Vec::new();
    match (kind, geometry) {
        (Kind::Point, Geometry::Point(p)) => parts.push(vec![p.0]),
        (Kind::MultiPoint, Geometry::MultiPoint(m)) => {
            parts.push(m.0.iter().map(|p| p.0).collect());
        }
        (Kind::Line, Geometry::Line(l)) => parts.push(vec![l.start, l.end]),
        (Kind::Line, Geometry::LineString(l)) => parts.push(l.0.clone()),
        (Kind::Line, Geometry::MultiLineString(m)) => {
            parts.extend(m.0.iter().filter(|l| l.0.len() >= 2).map(|l| l.0.clone()));
        }
        (Kind::Polygon, Geometry::Polygon(p)) => polygon_parts(p, &mut parts),
        (Kind::Polygon, Geometry::MultiPolygon(m)) => {
            m.0.iter().for_each(|p| polygon_parts(p, &mut parts));
        }
        (Kind::Polygon, Geometry::Rect(r)) => polygon_parts(&r.to_polygon(), &mut parts),
        (Kind::Polygon, Geometry::Triangle(t)) => polygon_parts(&t.to_polygon(), &mut parts),
        _ => return None,
    }
    parts.retain(|p| !p.is_empty());
    (!parts.is_empty()).then_some(Shape { parts })
}

fn push_f64(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i32_le(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn file_header(shape_type: i32, length_words: i32, bounds: [f64; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(100);
    out.extend_from_slice(&9994_i32.to_be_bytes());
    out.extend_from_slice(&[0; 20]);
    out.extend_from_slice(&length_words.to_be_bytes());
    push_i32_le(&mut out, 1000);
    push_i32_le(&mut out, shape_type);
    for value in bounds {
        push_f64(&mut out, value);
    }
    out.extend_from_slice(&[0; 32]); // z and m ranges
    out
}

/// The `.shp` and `.shx` bytes for `rows`; rows without a (non-empty) shape are null shapes.
fn encode_shapes(rows: &[Option<Geometry<f64>>], kind: Kind) -> (Vec<u8>, Vec<u8>) {
    let mut records = Vec::new();
    let mut index = Vec::with_capacity(rows.len() * 8);
    let mut extent: Option<[f64; 4]> = None;

    for (number, row) in rows.iter().enumerate() {
        let offset_words = ((100 + records.len()) / 2) as i32;
        let mut content = Vec::new();
        match row.as_ref().and_then(|g| shape_parts(g, kind)) {
            None => push_i32_le(&mut content, 0),
            Some(shape) => {
                push_i32_le(&mut content, kind.shape_type());
                let points: Vec<&Coord<f64>> = shape.parts.iter().flatten().collect();
                let (mut min_x, mut min_y, mut max_x, mut max_y) =
                    (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                for c in &points {
                    min_x = min_x.min(c.x);
                    min_y = min_y.min(c.y);
                    max_x = max_x.max(c.x);
                    max_y = max_y.max(c.y);
                }
                extent = Some(match extent {
                    None => [min_x, min_y, max_x, max_y],
                    Some(e) => [
                        e[0].min(min_x),
                        e[1].min(min_y),
                        e[2].max(max_x),
                        e[3].max(max_y),
                    ],
                });
                match kind {
                    Kind::Point => {
                        push_f64(&mut content, points[0].x);
                        push_f64(&mut content, points[0].y);
                    }
                    Kind::MultiPoint => {
                        for v in [min_x, min_y, max_x, max_y] {
                            push_f64(&mut content, v);
                        }
                        push_i32_le(&mut content, points.len() as i32);
                        for c in &points {
                            push_f64(&mut content, c.x);
                            push_f64(&mut content, c.y);
                        }
                    }
                    Kind::Line | Kind::Polygon => {
                        for v in [min_x, min_y, max_x, max_y] {
                            push_f64(&mut content, v);
                        }
                        push_i32_le(&mut content, shape.parts.len() as i32);
                        push_i32_le(&mut content, points.len() as i32);
                        let mut start = 0;
                        for part in &shape.parts {
                            push_i32_le(&mut content, start as i32);
                            start += part.len();
                        }
                        for c in &points {
                            push_f64(&mut content, c.x);
                            push_f64(&mut content, c.y);
                        }
                    }
                }
            }
        }
        let content_words = (content.len() / 2) as i32;
        records.extend_from_slice(&((number + 1) as i32).to_be_bytes());
        records.extend_from_slice(&content_words.to_be_bytes());
        records.extend_from_slice(&content);
        index.extend_from_slice(&offset_words.to_be_bytes());
        index.extend_from_slice(&content_words.to_be_bytes());
    }

    let bounds = extent.unwrap_or([0.0; 4]);
    let mut shp = file_header(
        kind.shape_type(),
        ((100 + records.len()) / 2) as i32,
        bounds,
    );
    shp.extend_from_slice(&records);
    let mut shx = file_header(kind.shape_type(), ((100 + index.len()) / 2) as i32, bounds);
    shx.extend_from_slice(&index);
    (shp, shx)
}
