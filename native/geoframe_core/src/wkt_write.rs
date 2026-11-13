//! WKT writing in the `wkt` crate's format, except for empty members, which GEOS writes as
//! `EMPTY`:
//!
//! - an empty member point of a `MULTIPOINT` (`MULTIPOINT (EMPTY, (1 1))`): the crate's writer
//!   unwraps every member's coordinate and panics. GeoArrow stores the member as a point with
//!   NaN coordinates, which is also how GEOS writes it to WKB;
//! - an empty member line of a `MULTILINESTRING` and an empty ring of a `POLYGON`
//!   (`POLYGON ((0 0, 9 0, 9 9, 0 0), EMPTY)`): the crate writes `()`, which no reader accepts.
//!
//! Every other geometry, and every member without an empty part, is written by the crate.

use geo_traits::{
    GeometryCollectionTrait, GeometryTrait, GeometryType, LineStringTrait, MultiLineStringTrait,
    MultiPointTrait, MultiPolygonTrait, PointTrait, PolygonTrait,
};

/// Whether the polygon has an empty ring.
fn has_empty_ring<P: PolygonTrait<T = f64>>(polygon: &P) -> bool {
    polygon
        .exterior()
        .into_iter()
        .chain(polygon.interiors())
        .any(|ring| ring.num_coords() == 0)
}

/// Whether `geometry` holds an empty member the crate cannot write, at any depth.
pub(crate) fn has_empty_member<G: GeometryTrait<T = f64>>(geometry: &G) -> bool {
    match geometry.as_type() {
        GeometryType::MultiPoint(multi) => multi.points().any(|p| p.coord().is_none()),
        GeometryType::MultiLineString(multi) => {
            multi.line_strings().any(|line| line.num_coords() == 0)
        }
        GeometryType::Polygon(polygon) => has_empty_ring(polygon),
        GeometryType::MultiPolygon(multi) => multi.polygons().any(|p| has_empty_ring(&p)),
        GeometryType::GeometryCollection(collection) => collection
            .geometries()
            .any(|member| has_empty_member(&member)),
        _ => false,
    }
}

/// The WKT of one geometry, as the `wkt` crate writes it, with an empty member written
/// `EMPTY`.
pub(crate) fn geometry_wkt<G: GeometryTrait<T = f64>>(geometry: &G) -> Result<String, String> {
    let mut out = String::new();
    write(&mut out, geometry)?;
    Ok(out)
}

/// The crate's dimension suffix after a type name.
fn tag(dim: geo_traits::Dimensions) -> Result<&'static str, String> {
    use geo_traits::Dimensions::{Unknown, Xy, Xym, Xyz, Xyzm};
    match dim {
        Xy | Unknown(2) => Ok(""),
        Xyz | Unknown(3) => Ok(" Z"),
        Xym => Ok(" M"),
        Xyzm | Unknown(4) => Ok(" ZM"),
        Unknown(n) => Err(format!("cannot write a geometry with {n} dimensions")),
    }
}

/// The parenthesized body the crate writes after the type name, from `text` (`POINT Z(1 2 3)`
/// gives `(1 2 3)`).
fn body(text: &str) -> Result<&str, String> {
    text.find('(')
        .map(|at| &text[at..])
        .ok_or_else(|| format!("no coordinates in {text}"))
}

/// A line or ring as a member: `(x y,...)`, or `EMPTY`.
fn line_body<L: LineStringTrait<T = f64>>(out: &mut String, line: &L) -> Result<(), String> {
    if line.num_coords() == 0 {
        out.push_str("EMPTY");
        return Ok(());
    }
    let mut text = String::new();
    wkt::to_wkt::write_linestring(&mut text, line).map_err(|e| e.to_string())?;
    out.push_str(body(&text)?);
    Ok(())
}

/// A polygon as a member: `(ring,...)`, or `EMPTY` without rings.
fn polygon_body<P: PolygonTrait<T = f64>>(out: &mut String, polygon: &P) -> Result<(), String> {
    let rings: Vec<_> = polygon
        .exterior()
        .into_iter()
        .chain(polygon.interiors())
        .collect();
    if rings.is_empty() {
        out.push_str("EMPTY");
        return Ok(());
    }
    out.push('(');
    for (i, ring) in rings.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        line_body(out, ring)?;
    }
    out.push(')');
    Ok(())
}

/// `NAME tag(member,...)`, the members written by `each`.
fn members<I>(
    out: &mut String,
    name: &str,
    dim: geo_traits::Dimensions,
    items: impl Iterator<Item = I>,
    mut each: impl FnMut(&mut String, I) -> Result<(), String>,
) -> Result<(), String> {
    out.push_str(name);
    out.push_str(tag(dim)?);
    out.push('(');
    for (i, item) in items.enumerate() {
        if i > 0 {
            out.push(',');
        }
        each(out, item)?;
    }
    out.push(')');
    Ok(())
}

fn write<G: GeometryTrait<T = f64>>(out: &mut String, geometry: &G) -> Result<(), String> {
    if !has_empty_member(geometry) {
        return wkt::to_wkt::write_geometry(out, geometry).map_err(|e| e.to_string());
    }
    match geometry.as_type() {
        GeometryType::MultiPoint(multi) => members(
            out,
            "MULTIPOINT",
            multi.dim(),
            multi.points(),
            |out, point| {
                if point.coord().is_none() {
                    out.push_str("EMPTY");
                    return Ok(());
                }
                let mut text = String::new();
                wkt::to_wkt::write_point(&mut text, &point).map_err(|e| e.to_string())?;
                out.push_str(body(&text)?);
                Ok(())
            },
        ),
        GeometryType::MultiLineString(multi) => members(
            out,
            "MULTILINESTRING",
            multi.dim(),
            multi.line_strings(),
            |out, line| line_body(out, &line),
        ),
        GeometryType::Polygon(polygon) => {
            out.push_str("POLYGON");
            out.push_str(tag(polygon.dim())?);
            polygon_body(out, polygon)
        }
        GeometryType::MultiPolygon(multi) => members(
            out,
            "MULTIPOLYGON",
            multi.dim(),
            multi.polygons(),
            |out, polygon| polygon_body(out, &polygon),
        ),
        GeometryType::GeometryCollection(collection) => members(
            out,
            "GEOMETRYCOLLECTION",
            collection.dim(),
            collection.geometries(),
            |out, member| write(out, &member),
        ),
        _ => wkt::to_wkt::write_geometry(out, geometry).map_err(|e| e.to_string()),
    }
}
