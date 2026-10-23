//! Z/M plumbing shared by the file formats.
//!
//! The column stores every dimension, but `geo` geometries are 2D, so the readers and writers
//! that must keep Z or M cannot go through `geo`. They use two small tools instead:
//!
//! * [`WkbOut`], an ISO WKB builder, so a reader can describe a geometry (with its Z and M) and
//!   hand the bytes to the column builder;
//! * [`Vertex`] and [`vertex`], which read `(x, y, z, m)` from any `geo_traits` coordinate, so a
//!   writer can walk the column's own WKB.
//!
//! GeoJSON geometry conversion lives here too, because both directions are pure Z handling.

use geo_traits::{
    CoordTrait, GeometryCollectionTrait, GeometryTrait, GeometryType, LineStringTrait,
    MultiLineStringTrait, MultiPointTrait, MultiPolygonTrait, PointTrait, PolygonTrait,
};
use geojson::GeometryValue;
use serde_json::Number;

use crate::zm::flags;

/// One vertex; a missing ordinate is NaN.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Vertex {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub m: f64,
}

/// Reads a coordinate of any dimension.
pub(crate) fn vertex<C: CoordTrait<T = f64>>(coord: &C) -> Vertex {
    let (has_z, has_m) = flags(coord.dim());
    let mut next = 2;
    let mut take = |present: bool| {
        if present {
            next += 1;
            coord.nth_or_panic(next - 1)
        } else {
            f64::NAN
        }
    };
    let z = take(has_z);
    let m = take(has_m);
    Vertex {
        x: coord.x(),
        y: coord.y(),
        z,
        m,
    }
}

// -- ISO WKB builder ----------------------------------------------------------------------

/// Little-endian ISO WKB under construction, with one fixed dimension.
pub(crate) struct WkbOut {
    pub bytes: Vec<u8>,
    z: bool,
    m: bool,
}

impl WkbOut {
    pub fn new(z: bool, m: bool) -> Self {
        Self {
            bytes: Vec::new(),
            z,
            m,
        }
    }

    /// Byte order and type code of a geometry (`base` is 1 to 7).
    pub fn header(&mut self, base: u32) {
        self.bytes.push(1);
        let code = base + 1000 * u32::from(self.z) + 2000 * u32::from(self.m);
        self.bytes.extend_from_slice(&code.to_le_bytes());
    }

    pub fn count(&mut self, n: usize) -> Result<(), String> {
        let n = u32::try_from(n).map_err(|_| "too many elements for WKB".to_string())?;
        self.bytes.extend_from_slice(&n.to_le_bytes());
        Ok(())
    }

    /// One vertex: x, y, then the ordinates this geometry has.
    pub fn vertex(&mut self, v: Vertex) {
        let mut put = |value: f64| self.bytes.extend_from_slice(&value.to_le_bytes());
        put(v.x);
        put(v.y);
        if self.z {
            put(v.z);
        }
        if self.m {
            put(v.m);
        }
    }

    pub fn vertices(&mut self, vertices: &[Vertex]) -> Result<(), String> {
        self.count(vertices.len())?;
        vertices.iter().for_each(|v| self.vertex(*v));
        Ok(())
    }
}

// -- GeoJSON -> WKB -----------------------------------------------------------------------

/// ISO WKB of a GeoJSON geometry. A geometry has Z when any position (of any member of a
/// collection) has three or more values; positions with two values then get a NaN Z. Values
/// past the third are ignored, as GDAL does.
pub(crate) fn geojson_to_wkb(geometry: &geojson::Geometry) -> Result<Vec<u8>, String> {
    let mut out = WkbOut::new(has_z(&geometry.value), false);
    write_value(&mut out, &geometry.value)?;
    Ok(out.bytes)
}

fn has_z(value: &GeometryValue) -> bool {
    let line = |l: &[geojson::Position]| l.iter().any(|p| p.len() >= 3);
    match value {
        GeometryValue::Point { coordinates } => coordinates.len() >= 3,
        GeometryValue::MultiPoint { coordinates } | GeometryValue::LineString { coordinates } => {
            line(coordinates)
        }
        GeometryValue::MultiLineString { coordinates } | GeometryValue::Polygon { coordinates } => {
            coordinates.iter().any(|l| line(l))
        }
        GeometryValue::MultiPolygon { coordinates } => {
            coordinates.iter().any(|p| p.iter().any(|l| line(l)))
        }
        GeometryValue::GeometryCollection { geometries } => {
            geometries.iter().any(|g| has_z(&g.value))
        }
    }
}

fn position(p: &geojson::Position) -> Result<Vertex, String> {
    match p.as_slice() {
        [x, y] => Ok(Vertex {
            x: *x,
            y: *y,
            z: f64::NAN,
            m: f64::NAN,
        }),
        [x, y, z, ..] => Ok(Vertex {
            x: *x,
            y: *y,
            z: *z,
            m: f64::NAN,
        }),
        _ => Err("a position needs at least two numbers".to_string()),
    }
}

fn positions(list: &[geojson::Position]) -> Result<Vec<Vertex>, String> {
    list.iter().map(position).collect()
}

fn write_value(out: &mut WkbOut, value: &GeometryValue) -> Result<(), String> {
    match value {
        GeometryValue::Point { coordinates } => {
            out.header(1);
            out.vertex(position(coordinates)?);
        }
        GeometryValue::MultiPoint { coordinates } => {
            out.header(4);
            out.count(coordinates.len())?;
            for p in coordinates {
                out.header(1);
                out.vertex(position(p)?);
            }
        }
        GeometryValue::LineString { coordinates } => {
            out.header(2);
            out.vertices(&positions(coordinates)?)?;
        }
        GeometryValue::MultiLineString { coordinates } => {
            out.header(5);
            out.count(coordinates.len())?;
            for line in coordinates {
                out.header(2);
                out.vertices(&positions(line)?)?;
            }
        }
        GeometryValue::Polygon { coordinates } => {
            out.header(3);
            write_rings(out, coordinates)?;
        }
        GeometryValue::MultiPolygon { coordinates } => {
            out.header(6);
            out.count(coordinates.len())?;
            for polygon in coordinates {
                out.header(3);
                write_rings(out, polygon)?;
            }
        }
        GeometryValue::GeometryCollection { geometries } => {
            out.header(7);
            out.count(geometries.len())?;
            for member in geometries {
                write_value(out, &member.value)?;
            }
        }
    }
    Ok(())
}

fn write_rings(out: &mut WkbOut, rings: &[Vec<geojson::Position>]) -> Result<(), String> {
    out.count(rings.len())?;
    for ring in rings {
        out.vertices(&positions(ring)?)?;
    }
    Ok(())
}

// -- WKB -> GeoJSON -----------------------------------------------------------------------

/// GeoJSON text of a geometry, with Z as the third ordinate (M has no place in GeoJSON and is
/// dropped). A vertex whose Z is NaN is written with two values. `None` for an empty point,
/// which GeoJSON cannot express.
pub(crate) fn geometry_to_geojson<G: GeometryTrait<T = f64>>(geometry: &G) -> Option<String> {
    let mut out = String::new();
    write_geometry(&mut out, geometry).then_some(out)
}

fn number(out: &mut String, value: f64) {
    match Number::from_f64(value) {
        Some(n) => out.push_str(&n.to_string()),
        None => out.push_str("null"),
    }
}

fn position_text<C: CoordTrait<T = f64>>(out: &mut String, coord: &C) {
    let v = vertex(coord);
    out.push('[');
    number(out, v.x);
    out.push(',');
    number(out, v.y);
    if !v.z.is_nan() {
        out.push(',');
        number(out, v.z);
    }
    out.push(']');
}

fn list<T>(out: &mut String, items: impl Iterator<Item = T>, mut each: impl FnMut(&mut String, T)) {
    out.push('[');
    for (i, item) in items.enumerate() {
        if i > 0 {
            out.push(',');
        }
        each(out, item);
    }
    out.push(']');
}

fn line_text<L: LineStringTrait<T = f64>>(out: &mut String, line: &L) {
    list(out, line.coords(), |out, c| position_text(out, &c));
}

fn polygon_text<P: PolygonTrait<T = f64>>(out: &mut String, polygon: &P) {
    list(
        out,
        polygon.exterior().into_iter().chain(polygon.interiors()),
        |out, ring| line_text(out, &ring),
    );
}

fn write_geometry<G: GeometryTrait<T = f64>>(out: &mut String, geometry: &G) -> bool {
    let tagged = |out: &mut String, name: &str| {
        out.push_str(r#"{"type":""#);
        out.push_str(name);
        out.push_str(r#"","coordinates":"#);
    };
    match geometry.as_type() {
        GeometryType::Point(point) => {
            let Some(coord) = point.coord() else {
                return false;
            };
            tagged(out, "Point");
            position_text(out, &coord);
        }
        GeometryType::MultiPoint(multi) => {
            tagged(out, "MultiPoint");
            list(
                out,
                multi.points().filter(|p| p.coord().is_some()),
                |out, p| {
                    if let Some(c) = p.coord() {
                        position_text(out, &c);
                    }
                },
            );
        }
        GeometryType::LineString(line) => {
            tagged(out, "LineString");
            line_text(out, line);
        }
        GeometryType::MultiLineString(multi) => {
            tagged(out, "MultiLineString");
            list(out, multi.line_strings(), |out, l| line_text(out, &l));
        }
        GeometryType::Polygon(polygon) => {
            tagged(out, "Polygon");
            polygon_text(out, polygon);
        }
        GeometryType::MultiPolygon(multi) => {
            tagged(out, "MultiPolygon");
            list(out, multi.polygons(), |out, p| polygon_text(out, &p));
        }
        GeometryType::GeometryCollection(collection) => {
            out.push_str(r#"{"type":"GeometryCollection","geometries":["#);
            let mut first = true;
            for member in collection.geometries() {
                let mut text = String::new();
                if !write_geometry(&mut text, &member) {
                    continue;
                }
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&text);
            }
            out.push_str("]}");
            return true;
        }
        _ => return false,
    }
    out.push('}');
    true
}
