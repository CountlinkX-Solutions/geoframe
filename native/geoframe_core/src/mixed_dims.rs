//! Collections whose members have different dimensions.
//!
//! GEOS reads such a collection and keeps each member's own dimension: the collection has Z
//! when any member has Z and M when any member has M, whatever its own header says. A GeoArrow
//! collection stores one dimension for all its members, so here every member is promoted to
//! the collection's dimension and the ordinates it lacks are NaN, which is what Shapely's
//! `get_coordinates` reports for them. The promoted collection is written as ISO WKB and read
//! back by the column builder.

use geo_traits::{
    CoordTrait, Dimensions, GeometryCollectionTrait, GeometryTrait, GeometryType, LineStringTrait,
    MultiLineStringTrait, MultiPointTrait, MultiPolygonTrait, PointTrait, PolygonTrait,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Dims {
    z: bool,
    m: bool,
}

impl Dims {
    /// An unnamed dimension is read by ordinate count (three ordinates are taken as Z).
    fn of(dim: Dimensions) -> Self {
        match dim {
            Dimensions::Xyz | Dimensions::Unknown(3) => Self { z: true, m: false },
            Dimensions::Xym => Self { z: false, m: true },
            Dimensions::Xyzm | Dimensions::Unknown(4) => Self { z: true, m: true },
            _ => Self::default(),
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            z: self.z || other.z,
            m: self.m || other.m,
        }
    }

    fn iso_offset(self) -> u32 {
        u32::from(self.z) * 1000 + u32::from(self.m) * 2000
    }
}

/// The dimension a geometry gives the collection holding it, as GEOS reckons it: a point,
/// line or polygon has its own (empty or not), a multi geometry its own unless empty, and a
/// collection the union of its members (`None` when it adds nothing).
fn contribution<G: GeometryTrait<T = f64>>(geometry: &G) -> Option<Dims> {
    let empty_multi = match geometry.as_type() {
        GeometryType::GeometryCollection(gc) => {
            return gc
                .geometries()
                .filter_map(|member| contribution(&member))
                .reduce(Dims::union)
        }
        GeometryType::MultiPoint(g) => g.num_points() == 0,
        GeometryType::MultiLineString(g) => g.num_line_strings() == 0,
        GeometryType::MultiPolygon(g) => g.num_polygons() == 0,
        _ => false,
    };
    (!empty_multi).then(|| Dims::of(geometry.dim()))
}

/// ISO WKB of a collection with members of different dimensions, every member promoted to the
/// collection's dimension (see the module documentation).
pub(crate) fn promoted_wkb<G: GeometryTrait<T = f64>>(geometry: &G) -> Result<Vec<u8>, String> {
    let dims = contribution(geometry).unwrap_or_default();
    let mut out = Vec::new();
    write_geometry(&mut out, geometry, dims)?;
    Ok(out)
}

/// GEOS's WKT rule: a collection tagged Z, M or ZM requires each member to have exactly that
/// dimension (a member without a tag is XY, also when empty), at any depth; GEOS raises
/// "Cannot mix dimensionality in a geometry" otherwise. An untagged collection is XY in `wkt`
/// and takes members of any dimension.
pub(crate) fn tagged_collection_conflict<G: GeometryTrait<T = f64>>(geometry: &G) -> bool {
    let GeometryType::GeometryCollection(gc) = geometry.as_type() else {
        return false;
    };
    let tag = Dims::of(gc.dim());
    gc.geometries().any(|member| {
        (tag != Dims::default() && Dims::of(member.dim()) != tag)
            || tagged_collection_conflict(&member)
    })
}

fn header(out: &mut Vec<u8>, base: u32, dims: Dims) {
    out.push(1);
    out.extend((base + dims.iso_offset()).to_le_bytes());
}

fn count(out: &mut Vec<u8>, n: usize) -> Result<(), String> {
    let n = u32::try_from(n).map_err(|_| "geometry too large for WKB".to_string())?;
    out.extend(n.to_le_bytes());
    Ok(())
}

fn ordinate(out: &mut Vec<u8>, value: f64) {
    out.extend(value.to_le_bytes());
}

fn coord<C: CoordTrait<T = f64>>(out: &mut Vec<u8>, c: &C, dims: Dims) {
    let own = Dims::of(c.dim());
    ordinate(out, c.x());
    ordinate(out, c.y());
    if dims.z {
        ordinate(out, if own.z { c.nth(2) } else { None }.unwrap_or(f64::NAN));
    }
    if dims.m {
        let index = if own.z { 3 } else { 2 };
        ordinate(
            out,
            if own.m { c.nth(index) } else { None }.unwrap_or(f64::NAN),
        );
    }
}

fn write_point<P: PointTrait<T = f64>>(out: &mut Vec<u8>, point: &P, dims: Dims) {
    header(out, 1, dims);
    match point.coord() {
        Some(c) => coord(out, &c, dims),
        // An empty point is all NaN in WKB.
        None => {
            (0..2 + usize::from(dims.z) + usize::from(dims.m)).for_each(|_| ordinate(out, f64::NAN))
        }
    }
}

fn write_coords<L: LineStringTrait<T = f64>>(
    out: &mut Vec<u8>,
    line: &L,
    dims: Dims,
) -> Result<(), String> {
    count(out, line.num_coords())?;
    line.coords().for_each(|c| coord(out, &c, dims));
    Ok(())
}

fn write_line<L: LineStringTrait<T = f64>>(
    out: &mut Vec<u8>,
    line: &L,
    dims: Dims,
) -> Result<(), String> {
    header(out, 2, dims);
    write_coords(out, line, dims)
}

fn write_polygon<P: PolygonTrait<T = f64>>(
    out: &mut Vec<u8>,
    polygon: &P,
    dims: Dims,
) -> Result<(), String> {
    header(out, 3, dims);
    let exterior = polygon.exterior();
    count(
        out,
        usize::from(exterior.is_some()) + polygon.num_interiors(),
    )?;
    for ring in exterior.into_iter().chain(polygon.interiors()) {
        write_coords(out, &ring, dims)?;
    }
    Ok(())
}

fn write_geometry<G: GeometryTrait<T = f64>>(
    out: &mut Vec<u8>,
    geometry: &G,
    dims: Dims,
) -> Result<(), String> {
    match geometry.as_type() {
        GeometryType::Point(g) => write_point(out, g, dims),
        GeometryType::LineString(g) => write_line(out, g, dims)?,
        GeometryType::Polygon(g) => write_polygon(out, g, dims)?,
        GeometryType::MultiPoint(g) => {
            header(out, 4, dims);
            count(out, g.num_points())?;
            g.points().for_each(|p| write_point(out, &p, dims));
        }
        GeometryType::MultiLineString(g) => {
            header(out, 5, dims);
            count(out, g.num_line_strings())?;
            for line in g.line_strings() {
                write_line(out, &line, dims)?;
            }
        }
        GeometryType::MultiPolygon(g) => {
            header(out, 6, dims);
            count(out, g.num_polygons())?;
            for polygon in g.polygons() {
                write_polygon(out, &polygon, dims)?;
            }
        }
        GeometryType::GeometryCollection(g) => {
            header(out, 7, dims);
            count(out, g.num_geometries())?;
            for member in g.geometries() {
                write_geometry(out, &member, dims)?;
            }
        }
        // Readers never produce these, and `geo` geometries are XY only.
        GeometryType::Rect(_) | GeometryType::Triangle(_) | GeometryType::Line(_) => {
            return Err("unsupported geometry type in a collection with mixed dimensions".into())
        }
    }
    Ok(())
}
