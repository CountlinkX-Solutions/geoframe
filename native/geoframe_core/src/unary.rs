//! Unary constructive operations and accessors on a [`GeoColumn`], with GeoPandas names.
//!
//! Constructive operations go through [`GeoColumn::map_geometry`]: each row is converted to a
//! `geo` geometry, transformed, and the column is rebuilt with the same CRS. Rows that are
//! null, unrepresentable, undefined for the operation (for example `exterior` of a point) or
//! that panic inside `geo` become null. Results are 2D: `geo` has no Z/M.
//!
//! Where Shapely returns an empty geometry we return an empty geometry too (`boundary` of a
//! closed line is `MULTIPOINT EMPTY`); where it raises we return null.

use std::collections::HashMap;

use geo::algorithm::buffer::{BufferStyle, LineCap, LineJoin};
use geo::{
    AffineTransform, BoundingRect, Buffer, Centroid, ConvexHull, Coord, CoordsIter, Geometry,
    GeometryCollection, InteriorPoint, LineString, MakeValid, MultiLineString, MultiPoint,
    MultiPolygon, Point, Polygon, SimplifyVwPreserve,
};
use geo_traits::{GeometryTrait, GeometryType};
use geoarrow_expr_geo::util::to_geo::geometry_to_geo;

use crate::buffer_single_sided::{single_sided_geometry, Sided};
use crate::predicates::guarded;
use crate::zm_bridge::{Affine3, Keep};
use crate::zm_overlay::z_only;
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
    /// Buffer only one side of a line (Shapely `single_sided`); see [`crate::buffer_single_sided`].
    pub single_sided: bool,
}

impl Default for BufferOptions {
    fn default() -> Self {
        Self {
            cap: "round".to_string(),
            join: "round".to_string(),
            quad_segs: 16,
            mitre_limit: 5.0,
            single_sided: false,
        }
    }
}

impl BufferOptions {
    pub(crate) fn style(&self, distance: f64) -> Result<BufferStyle<f64>, String> {
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
    pub(crate) fn resolve(&self, geom: &Geometry<f64>) -> Option<Coord<f64>> {
        match self {
            Self::Center => geom.bounding_rect().map(|r| r.center()),
            Self::Centroid => geom.centroid().map(Point::into),
            Self::Coord(x, y) => Some(Coord { x: *x, y: *y }),
        }
    }
}

fn is_linear(g: &Geometry<f64>) -> bool {
    matches!(
        g,
        Geometry::Line(_) | Geometry::LineString(_) | Geometry::MultiLineString(_)
    )
}

impl GeoColumn {
    /// Buffers every geometry by `distance` (negative shrinks polygons).
    pub fn buffer(&self, distance: f64, options: &BufferOptions) -> Result<Self, String> {
        self.buffer_rows(&vec![Some(distance); self.len()], options)
    }

    /// Buffers every geometry by the distance of its row (`None` nulls the row). With
    /// `options.single_sided` only one side of a line is buffered, see
    /// [`crate::buffer_single_sided`].
    pub fn buffer_rows(
        &self,
        distances: &[Option<f64>],
        options: &BufferOptions,
    ) -> Result<Self, String> {
        if distances.len() != self.len() {
            return Err(format!(
                "distance has {} entries but the column has {} rows",
                distances.len(),
                self.len()
            ));
        }
        // Validates the style names and limits once, whatever the distances are.
        options.style(1.0)?;
        let sided = Sided::new(options)?;
        let rows: Vec<Option<Geometry<f64>>> = self
            .geo_rows()
            .iter()
            .zip(distances)
            .map(|(g, d)| match (g, d) {
                (Some(g), Some(d)) => guarded(|| {
                    if options.single_sided {
                        single_sided_geometry(g, *d, &sided, options)
                    } else {
                        let style = options.style(*d).ok()?;
                        Some(collapse(g.buffer_with_style(style)))
                    }
                })
                .flatten(),
                _ => None,
            })
            .collect();
        self.rebuild(&rows)
    }

    /// Simplifies lines and polygon rings. `preserve_topology` selects Visvalingam-Whyatt with
    /// topology checks (`SimplifyVwPreserve`; `tolerance` is a triangle *area*), otherwise
    /// GEOS's Ramer-Douglas-Peucker (own code, `tolerance` is a *distance*): rings that collapse
    /// are removed. Points are unchanged.
    pub fn simplify(&self, tolerance: f64, preserve_topology: bool) -> Result<Self, String> {
        self.map_geometry_zm(Keep::Z, |g| {
            Some(simplify_geometry(g, tolerance, preserve_topology))
        })
    }

    /// Applies a parameterless constructive operation.
    pub fn unary(&self, op: UnaryOp) -> Result<Self, String> {
        let apply = |g: &Geometry<f64>| match op {
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
        };
        // Boundary and exterior only copy vertices, so they keep Z and M (as GEOS does).
        match op {
            UnaryOp::Boundary => self.map_geometry_zm(Keep::BothUnlessEmpty, apply),
            UnaryOp::Exterior => self.map_geometry_zm(Keep::Both, apply),
            UnaryOp::ConvexHull => self.map_geometry_model(z_only, apply),
            // A valid geometry comes back unchanged (M included); repairing drops M and
            // averages the Z of the two segments at a new node.
            UnaryOp::MakeValid => {
                self.map_geometry_model(|g, (z, m)| (z, m && crate::validity::is_valid(g)), apply)
            }
            // GEOS picks a vertex of a line; for points and areas the point is new (2D).
            UnaryOp::RepresentativePoint => {
                self.map_geometry_model(|g, (z, _)| (z && is_linear(g), false), apply)
            }
            _ => self.map_geometry(apply),
        }
    }

    /// Shifts every coordinate by `(xoff, yoff)`; Z is kept.
    pub fn translate(&self, xoff: f64, yoff: f64) -> Result<Self, String> {
        self.translate3(xoff, yoff, 0.0)
    }

    /// Shifts every coordinate by `(xoff, yoff)` and every Z by `zoff`. M is dropped.
    pub fn translate3(&self, xoff: f64, yoff: f64, zoff: f64) -> Result<Self, String> {
        let transform = Affine3::with_z(AffineTransform::translate(xoff, yoff), 1.0, zoff);
        self.affine3_rows(|_, _| Some(transform))
    }

    /// Scales about `origin`; negative factors mirror. Z is kept.
    pub fn scale(&self, xfact: f64, yfact: f64, origin: &Origin) -> Result<Self, String> {
        self.scale3(xfact, yfact, 1.0, origin)
    }

    /// Scales x, y about `origin` and Z by `zfact` about zero (Shapely's origin has z = 0).
    pub fn scale3(
        &self,
        xfact: f64,
        yfact: f64,
        zfact: f64,
        origin: &Origin,
    ) -> Result<Self, String> {
        self.affine3_rows(|_, g| {
            let at = origin.resolve(g)?;
            Some(Affine3::with_z(
                AffineTransform::scale(xfact, yfact, at),
                zfact,
                0.0,
            ))
        })
    }

    /// Rotates counter-clockwise by `degrees` about `origin`; Z is kept.
    pub fn rotate(&self, degrees: f64, origin: &Origin) -> Result<Self, String> {
        self.affine3_rows(|_, g| {
            let at = origin.resolve(g)?;
            Some(Affine3::flat(AffineTransform::rotate(degrees, at)))
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
pub(crate) fn collapse(mut multi: MultiPolygon<f64>) -> Geometry<f64> {
    if multi.0.len() == 1 {
        Geometry::Polygon(multi.0.remove(0))
    } else if multi.0.is_empty() {
        Geometry::Polygon(Polygon::new(LineString::new(vec![]), vec![]))
    } else {
        Geometry::MultiPolygon(multi)
    }
}

fn simplify_geometry(geom: &Geometry<f64>, tolerance: f64, preserve: bool) -> Geometry<f64> {
    if !preserve {
        return douglas_peucker_geometry(geom, tolerance);
    }
    match geom {
        Geometry::LineString(g) => Geometry::LineString(g.simplify_vw_preserve(tolerance)),
        Geometry::MultiLineString(g) => {
            Geometry::MultiLineString(g.simplify_vw_preserve(tolerance))
        }
        Geometry::Polygon(g) => Geometry::Polygon(g.simplify_vw_preserve(tolerance)),
        Geometry::MultiPolygon(g) => Geometry::MultiPolygon(g.simplify_vw_preserve(tolerance)),
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

// -- Douglas-Peucker, as GEOS --------------------------------------------------------------

/// GEOS `Distance::pointToSegment`.
fn point_to_segment(p: Coord<f64>, a: Coord<f64>, b: Coord<f64>) -> f64 {
    let distance = |u: Coord<f64>, v: Coord<f64>| {
        ((u.x - v.x) * (u.x - v.x) + (u.y - v.y) * (u.y - v.y)).sqrt()
    };
    if a == b {
        return distance(p, a);
    }
    let len2 = (b.x - a.x) * (b.x - a.x) + (b.y - a.y) * (b.y - a.y);
    let r = ((p.x - a.x) * (b.x - a.x) + (p.y - a.y) * (b.y - a.y)) / len2;
    if r <= 0.0 {
        return distance(p, a);
    }
    if r >= 1.0 {
        return distance(p, b);
    }
    let s = ((a.y - p.y) * (b.x - a.x) - (a.x - p.x) * (b.y - a.y)) / len2;
    s.abs() * len2.sqrt()
}

/// GEOS `DouglasPeuckerLineSimplifier`: a section is replaced by its end points when no vertex
/// is farther than `tolerance` from the chord, otherwise it splits at the first farthest
/// vertex. There is no minimum vertex count (`geo` keeps four per ring), so a ring can come out
/// with fewer than four vertices and is then dropped by the caller.
fn douglas_peucker(points: &[Coord<f64>], tolerance: f64) -> Vec<Coord<f64>> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![true; points.len()];
    let mut sections = vec![(0, points.len() - 1)];
    while let Some((i, j)) = sections.pop() {
        if i + 1 >= j {
            continue;
        }
        let (mut max_distance, mut max_index) = (-1.0, i);
        for (k, point) in points.iter().enumerate().take(j).skip(i + 1) {
            let distance = point_to_segment(*point, points[i], points[j]);
            if distance > max_distance {
                max_distance = distance;
                max_index = k;
            }
        }
        if max_distance <= tolerance {
            keep[i + 1..j].fill(false);
        } else {
            sections.push((i, max_index));
            sections.push((max_index, j));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(point, keep)| keep.then_some(*point))
        .collect()
}

fn dp_line(line: &LineString<f64>, tolerance: f64) -> LineString<f64> {
    LineString::new(douglas_peucker(&line.0, tolerance))
}

/// GEOS also simplifies the closing vertex of a ring: when the start vertex lies within the
/// tolerance of the chord between its two neighbours it is dropped and the ring closes on the
/// next vertex. Only done once, and not for closed line strings.
fn drop_redundant_ring_start(ring: &mut Vec<Coord<f64>>, tolerance: f64) {
    if ring.len() <= 3 {
        return;
    }
    let (previous, next) = (ring[ring.len() - 2], ring[1]);
    if point_to_segment(ring[0], previous, next) <= tolerance {
        ring.remove(0);
        ring.pop();
        ring.push(ring[0]);
    }
}

/// A ring needs four vertices; one that collapses below that is `None`.
fn dp_ring(ring: &LineString<f64>, tolerance: f64) -> Option<LineString<f64>> {
    let mut simplified = douglas_peucker(&ring.0, tolerance);
    drop_redundant_ring_start(&mut simplified, tolerance);
    (simplified.len() >= 4).then(|| LineString::new(simplified))
}

/// A polygon whose shell collapses is empty; a hole that collapses is removed.
fn dp_polygon(polygon: &Polygon<f64>, tolerance: f64) -> Polygon<f64> {
    match dp_ring(polygon.exterior(), tolerance) {
        None => Polygon::new(LineString::new(vec![]), vec![]),
        Some(shell) => Polygon::new(
            shell,
            polygon
                .interiors()
                .iter()
                .filter_map(|hole| dp_ring(hole, tolerance))
                .collect(),
        ),
    }
}

/// GEOS builds a multi geometry from what is left of its parts: a single part is returned
/// as itself.
fn from_parts<T>(
    mut parts: Vec<T>,
    single: impl Fn(T) -> Geometry<f64>,
    many: impl Fn(Vec<T>) -> Geometry<f64>,
) -> Geometry<f64> {
    if parts.len() == 1 {
        single(parts.remove(0))
    } else {
        many(parts)
    }
}

fn is_empty_result(geom: &Geometry<f64>) -> bool {
    match geom {
        Geometry::Polygon(p) => p.exterior().0.is_empty(),
        Geometry::MultiPolygon(m) => m.0.is_empty(),
        Geometry::LineString(l) => l.0.is_empty(),
        Geometry::MultiLineString(m) => m.0.is_empty(),
        Geometry::GeometryCollection(c) => c.0.is_empty(),
        _ => false,
    }
}

/// GEOS `DPTransformer::createValidArea`: a polygonal result that is not valid is repaired with
/// `buffer(0)` (positive-winding regions, overlapping and touching parts dissolved). A result
/// the repair cannot process stays as Douglas-Peucker left it.
fn create_valid_area(rough: Geometry<f64>) -> Geometry<f64> {
    let empty = match &rough {
        Geometry::Polygon(p) => p.exterior().0.is_empty(),
        Geometry::MultiPolygon(m) => m.0.is_empty(),
        _ => true,
    };
    if empty || crate::validity::is_valid(&rough) {
        return rough;
    }
    crate::buffer_zero::buffer_zero(&rough).unwrap_or(rough)
}

/// `shapely.simplify(preserve_topology=False)`: GEOS `DouglasPeuckerSimplifier`. An invalid
/// polygonal result is repaired with `buffer(0)` as GEOS does: per polygon, but a multipolygon
/// as a whole (`DPTransformer::transformPolygon` leaves it to the parent).
fn douglas_peucker_geometry(geom: &Geometry<f64>, tolerance: f64) -> Geometry<f64> {
    match geom {
        Geometry::LineString(l) => Geometry::LineString(dp_line(l, tolerance)),
        Geometry::MultiLineString(m) => {
            let parts: Vec<LineString<f64>> =
                m.0.iter()
                    .map(|l| dp_line(l, tolerance))
                    .filter(|l| !l.0.is_empty())
                    .collect();
            from_parts(parts, Geometry::LineString, |p| {
                Geometry::MultiLineString(MultiLineString::new(p))
            })
        }
        Geometry::Polygon(p) => create_valid_area(Geometry::Polygon(dp_polygon(p, tolerance))),
        Geometry::MultiPolygon(m) => {
            let parts: Vec<Polygon<f64>> =
                m.0.iter()
                    .map(|p| dp_polygon(p, tolerance))
                    .filter(|p| !p.exterior().0.is_empty())
                    .collect();
            create_valid_area(from_parts(parts, Geometry::Polygon, |p| {
                Geometry::MultiPolygon(MultiPolygon::new(p))
            }))
        }
        Geometry::GeometryCollection(members) => {
            Geometry::GeometryCollection(GeometryCollection::new_from(
                members
                    .iter()
                    .map(|m| douglas_peucker_geometry(m, tolerance))
                    .filter(|m| !is_empty_result(m))
                    .collect(),
            ))
        }
        // GEOS builds a one-point multipoint as the point.
        Geometry::MultiPoint(m) if m.0.len() == 1 => Geometry::Point(m.0[0]),
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
    // End counts by coordinate in first-seen order (a hash index replaces the linear search
    // that made many lines quadratic). NaN never equals itself, so each NaN end counts alone.
    let mut ends: Vec<(Coord<f64>, usize)> = Vec::new();
    let mut seen: HashMap<(u64, u64), usize> = HashMap::new();
    for line in lines.iter().filter(|l| !l.is_closed() && !l.0.is_empty()) {
        for end in [line.0[0], line.0[line.0.len() - 1]] {
            if end.x.is_nan() || end.y.is_nan() {
                ends.push((end, 1));
                continue;
            }
            let slot = *seen.entry(crate::noding::key(end)).or_insert_with(|| {
                ends.push((end, 0));
                ends.len() - 1
            });
            ends[slot].1 += 1;
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
    if crate::validity::is_valid(geom) {
        return Some(geom.clone());
    }
    let repair = |mp: MultiPolygon<f64>| mp.make_valid().ok().map(collapse);
    match geom {
        Geometry::Polygon(p) => crate::validity::repair_collapsed(std::slice::from_ref(p), repair)
            .or_else(|| p.make_valid().ok().map(collapse)),
        Geometry::MultiPolygon(mp) => crate::validity::repair_collapsed(&mp.0, repair)
            .or_else(|| mp.make_valid().ok().map(collapse)),
        other => Some(other.clone()),
    }
}
