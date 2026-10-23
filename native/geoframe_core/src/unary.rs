//! Unary constructive operations and accessors on a [`GeoColumn`], with GeoPandas names.
//!
//! Constructive operations go through [`GeoColumn::map_geometry`]: each row is converted to a
//! `geo` geometry, transformed, and the column is rebuilt with the same CRS. Rows that are
//! null, unrepresentable, undefined for the operation (for example `exterior` of a point) or
//! that panic inside `geo` become null. Results are 2D: `geo` has no Z/M.
//!
//! Where Shapely returns an empty geometry we return an empty geometry too (`boundary` of a
//! closed line is `MULTIPOINT EMPTY`); where it raises we return null.

use geo::algorithm::buffer::{BufferStyle, LineCap, LineJoin};
use geo::{
    AffineOps, AffineTransform, BoundingRect, Buffer, Centroid, ConvexHull, Coord, CoordsIter,
    Geometry, GeometryCollection, InteriorPoint, LineString, MakeValid, MultiLineString,
    MultiPoint, MultiPolygon, Point, Polygon, Simplify, SimplifyVwPreserve, Validation,
};
use geo_traits::{GeometryTrait, GeometryType};
use geoarrow_expr_geo::util::to_geo::geometry_to_geo;

use crate::GeoColumn;

/// Parameterless constructive operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    ConvexHull,
    Envelope,
    Boundary,
    MakeValid,
    RepresentativePoint,
    Exterior,
}

impl UnaryOp {
    /// Parses the GeoPandas-style operation name used by the NIF layer.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "convex_hull" => Self::ConvexHull,
            "envelope" => Self::Envelope,
            "boundary" => Self::Boundary,
            "make_valid" => Self::MakeValid,
            "representative_point" => Self::RepresentativePoint,
            "exterior" => Self::Exterior,
            _ => return None,
        })
    }
}

/// Style options for [`GeoColumn::buffer`], named after Shapely's.
#[derive(Debug, Clone)]
pub struct BufferOptions {
    /// `"round"`, `"flat"` or `"square"`.
    pub cap: String,
    /// `"round"`, `"mitre"` or `"bevel"`.
    pub join: String,
    /// Segments per quarter circle for round caps and joins (Shapely `quad_segs`).
    pub quad_segs: u32,
    /// Mitre ratio limit (Shapely `mitre_limit`): longest mitre as a multiple of the distance.
    pub mitre_limit: f64,
}

impl Default for BufferOptions {
    fn default() -> Self {
        Self {
            cap: "round".to_string(),
            join: "round".to_string(),
            quad_segs: 16,
            mitre_limit: 5.0,
        }
    }
}

impl BufferOptions {
    fn style(&self, distance: f64) -> Result<BufferStyle<f64>, String> {
        if self.quad_segs == 0 {
            return Err("quad_segs must be positive".to_string());
        }
        if self.mitre_limit.is_nan() || self.mitre_limit < 1.0 {
            return Err("mitre_limit must be at least 1".to_string());
        }
        // geo takes the arc step as an angle; i_overlay clamps it to [0.01 pi, 0.25 pi].
        let step = std::f64::consts::FRAC_PI_2 / f64::from(self.quad_segs);
        let cap = match self.cap.as_str() {
            "round" => LineCap::Round(step),
            "flat" => LineCap::Butt,
            "square" => LineCap::Square,
            other => return Err(format!("unknown cap_style: {other}")),
        };
        let join = match self.join.as_str() {
            "round" => LineJoin::Round(step),
            // A mitre ratio r = 1 / sin(theta / 2) bounds the sharpest corner theta that is
            // still mitred; geo takes that minimum angle.
            "mitre" => LineJoin::Miter(2.0 * (1.0 / self.mitre_limit).asin()),
            "bevel" => LineJoin::Bevel,
            other => return Err(format!("unknown join_style: {other}")),
        };
        Ok(BufferStyle::new(distance).line_cap(cap).line_join(join))
    }
}

/// The fixed point for [`GeoColumn::scale`] and [`GeoColumn::rotate`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Origin {
    /// Centre of each geometry's bounding box (the GeoPandas default).
    Center,
    /// Centroid of each geometry.
    Centroid,
    /// One explicit coordinate for every row.
    Coord(f64, f64),
}

impl Origin {
    fn resolve(&self, geom: &Geometry<f64>) -> Option<Coord<f64>> {
        match self {
            Self::Center => geom.bounding_rect().map(|r| r.center()),
            Self::Centroid => geom.centroid().map(Point::into),
            Self::Coord(x, y) => Some(Coord { x: *x, y: *y }),
        }
    }
}

impl GeoColumn {
    /// Buffers every geometry by `distance` (negative shrinks polygons).
    pub fn buffer(&self, distance: f64, options: &BufferOptions) -> Result<Self, String> {
        let style = options.style(distance)?;
        self.map_geometry(|g| {
            let buffered = g.buffer_with_style(style.clone());
            Some(collapse(buffered))
        })
    }

    /// Simplifies lines and polygon rings. `preserve_topology` selects Visvalingam-Whyatt with
    /// topology checks (`SimplifyVwPreserve`; `tolerance` is a triangle *area*), otherwise
    /// Ramer-Douglas-Peucker (`Simplify`; `tolerance` is a *distance*). Points are unchanged.
    pub fn simplify(&self, tolerance: f64, preserve_topology: bool) -> Result<Self, String> {
        self.map_geometry(|g| Some(simplify_geometry(g, tolerance, preserve_topology)))
    }

    /// Applies a parameterless constructive operation.
    pub fn unary(&self, op: UnaryOp) -> Result<Self, String> {
        self.map_geometry(|g| match op {
            UnaryOp::ConvexHull => convex_hull(g),
            UnaryOp::Envelope => envelope(g),
            UnaryOp::Boundary => boundary(g),
            UnaryOp::MakeValid => make_valid(g),
            UnaryOp::RepresentativePoint => g.interior_point().map(Geometry::Point),
            UnaryOp::Exterior => match g {
                Geometry::Polygon(p) if !p.exterior().0.is_empty() => {
                    Some(Geometry::LineString(p.exterior().clone()))
                }
                _ => None,
            },
        })
    }

    /// Shifts every coordinate by `(xoff, yoff)`.
    pub fn translate(&self, xoff: f64, yoff: f64) -> Result<Self, String> {
        let transform = AffineTransform::translate(xoff, yoff);
        self.map_geometry(|g| Some(g.affine_transform(&transform)))
    }

    /// Scales about `origin`; negative factors mirror.
    pub fn scale(&self, xfact: f64, yfact: f64, origin: &Origin) -> Result<Self, String> {
        self.map_geometry(|g| {
            let at = origin.resolve(g)?;
            Some(g.affine_transform(&AffineTransform::scale(xfact, yfact, at)))
        })
    }

    /// Rotates counter-clockwise by `degrees` about `origin`.
    pub fn rotate(&self, degrees: f64, origin: &Origin) -> Result<Self, String> {
        self.map_geometry(|g| {
            let at = origin.resolve(g)?;
            Some(g.affine_transform(&AffineTransform::rotate(degrees, at)))
        })
    }

    /// X ordinate of points; null for every other geometry, empty points and nulls.
    pub fn x(&self) -> Vec<Option<f64>> {
        self.point_ordinate(|p| p.x())
    }

    /// Y ordinate of points; null for every other geometry, empty points and nulls.
    pub fn y(&self) -> Vec<Option<f64>> {
        self.point_ordinate(|p| p.y())
    }

    fn point_ordinate(&self, f: impl Fn(&Point<f64>) -> f64) -> Vec<Option<f64>> {
        self.geo_rows()
            .iter()
            .map(|row| match row {
                Some(Geometry::Point(p)) => Some(f(p)),
                _ => None,
            })
            .collect()
    }

    /// Shapely/GeoPandas geometry type name per row; null for null rows.
    pub fn geom_type(&self) -> Vec<Option<String>> {
        self.map_scalars(|scalar| type_name(scalar).to_string())
    }

    /// Whether each geometry is empty; null for null rows.
    pub fn is_empty_rows(&self) -> Vec<Option<bool>> {
        self.coordinate_counts()
            .into_iter()
            .map(|count| count.map(|n| n == 0))
            .collect()
    }

    /// Number of coordinates per geometry (a closed ring counts its closing vertex); empty
    /// geometries give 0 and null rows null.
    pub fn count_coordinates(&self) -> Vec<Option<usize>> {
        self.coordinate_counts()
    }

    fn coordinate_counts(&self) -> Vec<Option<usize>> {
        self.map_scalars(|scalar| match geometry_to_geo(scalar) {
            Ok(g) => g.coords_count(),
            // `geo` cannot hold an empty point, the only unrepresentable stored value.
            Err(_) => 0,
        })
    }
}

fn type_name<G: GeometryTrait<T = f64>>(geom: &G) -> &'static str {
    match geom.as_type() {
        GeometryType::Point(_) => "Point",
        GeometryType::LineString(_) | GeometryType::Line(_) => "LineString",
        GeometryType::Polygon(_) | GeometryType::Rect(_) | GeometryType::Triangle(_) => "Polygon",
        GeometryType::MultiPoint(_) => "MultiPoint",
        GeometryType::MultiLineString(_) => "MultiLineString",
        GeometryType::MultiPolygon(_) => "MultiPolygon",
        GeometryType::GeometryCollection(_) => "GeometryCollection",
    }
}

/// A single-part multipolygon becomes a polygon, as Shapely returns it.
fn collapse(mut multi: MultiPolygon<f64>) -> Geometry<f64> {
    if multi.0.len() == 1 {
        Geometry::Polygon(multi.0.remove(0))
    } else if multi.0.is_empty() {
        Geometry::Polygon(Polygon::new(LineString::new(vec![]), vec![]))
    } else {
        Geometry::MultiPolygon(multi)
    }
}

fn simplify_geometry(geom: &Geometry<f64>, tolerance: f64, preserve: bool) -> Geometry<f64> {
    match geom {
        Geometry::LineString(g) if preserve => {
            Geometry::LineString(g.simplify_vw_preserve(tolerance))
        }
        Geometry::LineString(g) => Geometry::LineString(g.simplify(tolerance)),
        Geometry::MultiLineString(g) if preserve => {
            Geometry::MultiLineString(g.simplify_vw_preserve(tolerance))
        }
        Geometry::MultiLineString(g) => Geometry::MultiLineString(g.simplify(tolerance)),
        Geometry::Polygon(g) if preserve => Geometry::Polygon(g.simplify_vw_preserve(tolerance)),
        Geometry::Polygon(g) => Geometry::Polygon(g.simplify(tolerance)),
        Geometry::MultiPolygon(g) if preserve => {
            Geometry::MultiPolygon(g.simplify_vw_preserve(tolerance))
        }
        Geometry::MultiPolygon(g) => Geometry::MultiPolygon(g.simplify(tolerance)),
        Geometry::GeometryCollection(members) => {
            Geometry::GeometryCollection(GeometryCollection::new_from(
                members
                    .iter()
                    .map(|m| simplify_geometry(m, tolerance, preserve))
                    .collect(),
            ))
        }
        other => other.clone(),
    }
}

fn convex_hull(geom: &Geometry<f64>) -> Option<Geometry<f64>> {
    if geom.coords_count() == 0 {
        return None;
    }
    let hull = geom.convex_hull();
    let mut ring: Vec<Coord<f64>> = hull.exterior().0.clone();
    ring.dedup();
    if ring.first() == ring.last() && ring.len() > 1 {
        ring.pop();
    }
    Some(match ring.len() {
        1 => Geometry::Point(Point::from(ring[0])),
        2 => Geometry::LineString(LineString::new(ring)),
        _ => Geometry::Polygon(hull),
    })
}

fn envelope(geom: &Geometry<f64>) -> Option<Geometry<f64>> {
    let rect = geom.bounding_rect()?;
    let (min, max) = (rect.min(), rect.max());
    Some(if min == max {
        Geometry::Point(Point::from(min))
    } else if min.x == max.x || min.y == max.y {
        Geometry::LineString(LineString::new(vec![min, max]))
    } else {
        Geometry::Polygon(rect.to_polygon())
    })
}

fn boundary(geom: &Geometry<f64>) -> Option<Geometry<f64>> {
    match geom {
        Geometry::Polygon(p) => Some(polygon_boundary(std::slice::from_ref(p))),
        Geometry::MultiPolygon(mp) => Some(polygon_boundary(&mp.0)),
        Geometry::LineString(l) => Some(line_boundary(std::slice::from_ref(l))),
        Geometry::MultiLineString(ml) => Some(line_boundary(&ml.0)),
        Geometry::Point(_) | Geometry::MultiPoint(_) => {
            Some(Geometry::GeometryCollection(GeometryCollection::default()))
        }
        _ => None,
    }
}

fn polygon_boundary(polygons: &[Polygon<f64>]) -> Geometry<f64> {
    let mut rings: Vec<LineString<f64>> = polygons
        .iter()
        .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
        .cloned()
        .collect();
    if rings.len() == 1 {
        Geometry::LineString(rings.remove(0))
    } else {
        Geometry::MultiLineString(MultiLineString::new(rings))
    }
}

/// Mod-2 boundary rule: an endpoint belongs to the boundary when an odd number of lines
/// end there; closed lines contribute nothing.
fn line_boundary(lines: &[LineString<f64>]) -> Geometry<f64> {
    let mut ends: Vec<(Coord<f64>, usize)> = Vec::new();
    for line in lines.iter().filter(|l| !l.is_closed() && !l.0.is_empty()) {
        for end in [line.0[0], line.0[line.0.len() - 1]] {
            match ends.iter_mut().find(|(c, _)| *c == end) {
                Some((_, n)) => *n += 1,
                None => ends.push((end, 1)),
            }
        }
    }
    let points = ends
        .into_iter()
        .filter(|(_, n)| n % 2 == 1)
        .map(|(c, _)| Point::from(c))
        .collect();
    Geometry::MultiPoint(MultiPoint::new(points))
}

fn make_valid(geom: &Geometry<f64>) -> Option<Geometry<f64>> {
    if geom.is_valid() {
        return Some(geom.clone());
    }
    match geom {
        Geometry::Polygon(p) => p.make_valid().ok().map(collapse),
        Geometry::MultiPolygon(mp) => mp.make_valid().ok().map(collapse),
        other => Some(other.clone()),
    }
}
