//! Batch C: linear referencing and linestring operations.
//!
//! Everything is planar and 2D, with the same null policy as the other kernels: a row is null
//! when it, or its partner, is null or cannot be expressed as a `geo` geometry, when the
//! operation is undefined for its type (`interpolate` of a polygon), or when `geo` panics.
//!
//! # Semantics worth knowing
//!
//! * `interpolate`/`project` treat a `MultiLineString` as its parts laid end to end (GEOS
//!   linear referencing): the gap between two parts has no length.
//! * `hausdorff_distance` is the *discrete* GEOS algorithm: the largest distance from a vertex
//!   of one geometry to the *whole* other geometry (not to its nearest vertex), both ways.
//!   `densify` adds vertices at `k / round(1 / densify)` along every segment first.
//! * `frechet_distance` is the discrete Fréchet distance over vertices and needs two
//!   linestrings; anything else is null.
//! * `remove_repeated_points` with a tolerance keeps the first and the last vertex of every
//!   line and ring, so endpoints and ring closure survive.

use geo::line_measures::FrechetDistance;
use geo::orient::{Direction, Orient};
use geo::winding_order::Winding;
use geo::{
    Closest, ClosestPoint, Coord, CoordsIter, Densify, Distance, Euclidean, Geometry,
    GeometryCollection, Intersects, Length, Line, LineLocatePoint, LineString, MultiLineString,
    MultiPoint, MultiPolygon, Point, Polygon, RemoveRepeatedPoints,
};

use crate::predicates::guarded;
use crate::GeoColumn;

// -- linear referencing -----------------------------------------------------------------

/// The linear parts of a geometry, or `None` when it is not a line.
fn line_parts(g: &Geometry<f64>) -> Option<Vec<LineString<f64>>> {
    let parts = match g {
        Geometry::LineString(l) => vec![l.clone()],
        Geometry::Line(l) => vec![LineString::from(vec![l.start, l.end])],
        Geometry::MultiLineString(m) => m.0.clone(),
        _ => return None,
    };
    let parts: Vec<_> = parts.into_iter().filter(|p| p.0.len() >= 2).collect();
    (!parts.is_empty()).then_some(parts)
}

fn parts_length(parts: &[LineString<f64>]) -> f64 {
    parts.iter().map(|p| Euclidean.length(p)).sum()
}

/// The point `distance` along the parts laid end to end; clamped to the ends.
fn point_along(parts: &[LineString<f64>], distance: f64) -> Option<Point<f64>> {
    let total = parts_length(parts);
    if distance.is_nan() || !total.is_finite() {
        return None;
    }
    let target = distance.clamp(0.0, total);
    let mut travelled = 0.0;
    for segment in parts.iter().flat_map(|p| p.lines()) {
        let length = Euclidean.length(&segment);
        if length > 0.0 && travelled + length >= target {
            let t = (target - travelled) / length;
            return Some(Point::new(
                segment.start.x + (segment.end.x - segment.start.x) * t,
                segment.start.y + (segment.end.y - segment.start.y) * t,
            ));
        }
        travelled += length;
    }
    // Zero total length: every vertex coincides.
    Some(Point::from(parts[0].0[0]))
}

/// Distance along the parts to the point on them nearest `p`.
fn locate(parts: &[LineString<f64>], p: &Point<f64>) -> Option<f64> {
    let mut best: Option<(f64, f64)> = None;
    let mut before = 0.0;
    for part in parts {
        let length = Euclidean.length(part);
        let gap = Euclidean.distance(part, p);
        if best.is_none_or(|(d, _)| gap < d) {
            best = Some((gap, before + part.line_locate_point(p)? * length));
        }
        before += length;
    }
    best.map(|(_, along)| along)
}

impl GeoColumn {
    /// The point at `distance[i]` along each line (negative: from the end; beyond either end:
    /// clamped). With `normalized` the distance is a fraction of the length. Rows that are
    /// not lines, or whose distance is `None`, are null.
    pub fn interpolate(&self, distance: &[Option<f64>], normalized: bool) -> Result<Self, String> {
        if distance.len() != self.len() {
            return Err(format!(
                "distance has {} entries but the series has {} rows",
                distance.len(),
                self.len()
            ));
        }
        let rows: Vec<Option<Geometry<f64>>> = self
            .geo_rows()
            .iter()
            .zip(distance)
            .map(|(row, d)| {
                let (g, d) = (row.as_ref()?, (*d)?);
                guarded(|| {
                    let parts = line_parts(g)?;
                    let total = parts_length(&parts);
                    let d = if normalized { d * total } else { d };
                    let d = if d < 0.0 { total + d } else { d };
                    point_along(&parts, d).map(Geometry::Point)
                })
                .flatten()
            })
            .collect();
        self.rebuild(&rows)
    }

    /// Distance along each line to the point nearest `other` (a point column that pairs
    /// positionally or broadcasts); a fraction of the length when `normalized`.
    pub fn project(&self, other: &GeoColumn, normalized: bool) -> Result<Vec<Option<f64>>, String> {
        let locate_one = |a: &Geometry<f64>, b: &Geometry<f64>| {
            guarded(|| {
                let (parts, Geometry::Point(p)) = (line_parts(a)?, b) else {
                    return None;
                };
                let along = locate(&parts, p)?;
                let total = parts_length(&parts);
                Some(match (normalized, total > 0.0) {
                    (true, true) => along / total,
                    (true, false) => 0.0,
                    _ => along,
                })
            })
            .flatten()
        };
        self.zip_pairs(
            other,
            locate_one,
            |scalar| scalar.clone(),
            |scalar, row| locate_one(row, scalar),
        )
    }

    /// The line joining the nearest points of each pair (first geometry's point first). A
    /// pair that intersects gives a zero-length line at a common point.
    pub fn shortest_line(&self, other: &GeoColumn) -> Result<Self, String> {
        let pairs = self.zip_pairs(
            other,
            |a, b| guarded(|| nearest_pair(a, b)).flatten(),
            |scalar| scalar.clone(),
            |scalar, row| guarded(|| nearest_pair(row, scalar)).flatten(),
        )?;
        let rows: Vec<_> = pairs
            .into_iter()
            .map(|pair| pair.map(|(p, q)| Geometry::LineString(LineString::from(vec![p.0, q.0]))))
            .collect();
        self.rebuild(&rows)
    }
}

/// Nearest points `(on a, on b)`. The minimum distance between two geometries is attained at a
/// vertex of one of them, so the best vertex-to-other-geometry projection is exact; geometries
/// that intersect are handled first because crossings need not touch a vertex.
fn nearest_pair(a: &Geometry<f64>, b: &Geometry<f64>) -> Option<(Point<f64>, Point<f64>)> {
    if a.coords_count() == 0 || b.coords_count() == 0 {
        return None;
    }
    if a.intersects(b) {
        let common = a
            .coords_iter()
            .map(Point::from)
            .find(|p| b.intersects(p))
            .or_else(|| b.coords_iter().map(Point::from).find(|p| a.intersects(p)))
            .or_else(|| crossing(a, b))?;
        return Some((common, common));
    }
    let toward = |from: &Geometry<f64>, to: &Geometry<f64>| {
        from.coords_iter()
            .filter_map(|c| {
                let v = Point::from(c);
                match to.closest_point(&v) {
                    Closest::Intersection(p) | Closest::SinglePoint(p) => {
                        Some((Euclidean.distance(v, p), v, p))
                    }
                    Closest::Indeterminate => None,
                }
            })
            .min_by(|x, y| x.0.total_cmp(&y.0))
    };
    let from_a = toward(a, b);
    let from_b = toward(b, a);
    match (from_a, from_b) {
        (Some(x), Some(y)) if y.0 < x.0 => Some((y.2, y.1)),
        (Some(x), _) => Some((x.1, x.2)),
        (None, Some(y)) => Some((y.2, y.1)),
        (None, None) => None,
    }
}

/// Every segment of every line and ring in `g`.
fn segments(g: &Geometry<f64>) -> Vec<Line<f64>> {
    let polygon = |p: &Polygon<f64>| -> Vec<Line<f64>> {
        std::iter::once(p.exterior())
            .chain(p.interiors())
            .flat_map(|r| r.lines())
            .collect()
    };
    match g {
        Geometry::Line(l) => vec![*l],
        Geometry::LineString(l) => l.lines().collect(),
        Geometry::MultiLineString(m) => m.0.iter().flat_map(|l| l.lines()).collect(),
        Geometry::Polygon(p) => polygon(p),
        Geometry::MultiPolygon(m) => m.0.iter().flat_map(polygon).collect(),
        Geometry::Rect(r) => polygon(&r.to_polygon()),
        Geometry::Triangle(t) => polygon(&t.to_polygon()),
        Geometry::GeometryCollection(c) => c.0.iter().flat_map(segments).collect(),
        Geometry::Point(_) | Geometry::MultiPoint(_) => Vec::new(),
    }
}

/// A point where an edge of `a` crosses an edge of `b`.
fn crossing(a: &Geometry<f64>, b: &Geometry<f64>) -> Option<Point<f64>> {
    use geo::algorithm::line_intersection::{line_intersection, LineIntersection};
    let b_lines = segments(b);
    segments(a).into_iter().find_map(|la| {
        b_lines
            .iter()
            .find_map(|lb| match line_intersection(la, *lb)? {
                LineIntersection::SinglePoint { intersection, .. } => {
                    Some(Point::from(intersection))
                }
                LineIntersection::Collinear { intersection } => {
                    Some(Point::from(intersection.start))
                }
            })
    })
}

// -- discrete distances -------------------------------------------------------------------

/// Validates a GEOS densify fraction and returns the number of sub-segments per segment.
fn sub_segments(densify: Option<f64>) -> Result<Option<usize>, String> {
    match densify {
        None => Ok(None),
        Some(f) if f.is_finite() && f > 0.0 && f <= 1.0 => Ok(Some((1.0 / f).round() as usize)),
        Some(f) => Err(format!("densify must be in (0, 1], got {f}")),
    }
}

fn densified(line: &LineString<f64>, pieces: usize) -> LineString<f64> {
    let mut out: Vec<Coord<f64>> = Vec::with_capacity(line.0.len() * pieces);
    for segment in line.lines() {
        for k in 0..pieces {
            let t = k as f64 / pieces as f64;
            out.push(Coord {
                x: segment.start.x + (segment.end.x - segment.start.x) * t,
                y: segment.start.y + (segment.end.y - segment.start.y) * t,
            });
        }
    }
    out.extend(line.0.last());
    LineString::new(out)
}

/// The vertices of `g`, plus the densified points along its segments when `pieces` is set.
fn sample_coords(g: &Geometry<f64>, pieces: Option<usize>) -> Vec<Coord<f64>> {
    let Some(pieces) = pieces else {
        return g.coords_iter().collect();
    };
    match g {
        Geometry::Point(_) | Geometry::MultiPoint(_) => g.coords_iter().collect(),
        Geometry::GeometryCollection(c) => {
            c.0.iter()
                .flat_map(|g| sample_coords(g, Some(pieces)))
                .collect()
        }
        _ => {
            let lines: Vec<LineString<f64>> = segments(g)
                .into_iter()
                .map(|l| LineString::from(vec![l.start, l.end]))
                .collect();
            lines.iter().flat_map(|l| densified(l, pieces).0).collect()
        }
    }
}

fn directed_hausdorff(
    from: &Geometry<f64>,
    to: &Geometry<f64>,
    pieces: Option<usize>,
) -> Option<f64> {
    sample_coords(from, pieces)
        .into_iter()
        .map(|c| Euclidean.distance(&Geometry::Point(Point::from(c)), to))
        .reduce(f64::max)
}

fn as_line_string(g: &Geometry<f64>) -> Option<LineString<f64>> {
    match g {
        Geometry::LineString(l) if !l.0.is_empty() => Some(l.clone()),
        Geometry::Line(l) => Some(LineString::from(vec![l.start, l.end])),
        _ => None,
    }
}

impl GeoColumn {
    /// Discrete Hausdorff distance between each pair (see the module notes).
    pub fn hausdorff_distance(
        &self,
        other: &GeoColumn,
        densify: Option<f64>,
    ) -> Result<Vec<Option<f64>>, String> {
        let pieces = sub_segments(densify)?;
        let both = |a: &Geometry<f64>, b: &Geometry<f64>| {
            guarded(|| {
                Some(directed_hausdorff(a, b, pieces)?.max(directed_hausdorff(b, a, pieces)?))
            })
            .flatten()
        };
        self.zip_pairs(
            other,
            both,
            |scalar| scalar.clone(),
            |scalar, row| both(row, scalar),
        )
    }

    /// Discrete Fréchet distance between each pair of linestrings; other types are null.
    pub fn frechet_distance(
        &self,
        other: &GeoColumn,
        densify: Option<f64>,
    ) -> Result<Vec<Option<f64>>, String> {
        let pieces = sub_segments(densify)?;
        let both = |a: &Geometry<f64>, b: &Geometry<f64>| {
            guarded(|| {
                let (a, b) = (as_line_string(a)?, as_line_string(b)?);
                let (a, b) = match pieces {
                    Some(n) => (densified(&a, n), densified(&b, n)),
                    None => (a, b),
                };
                Some(Euclidean.frechet_distance(&a, &b))
            })
            .flatten()
        };
        self.zip_pairs(
            other,
            both,
            |scalar| scalar.clone(),
            |scalar, row| both(row, scalar),
        )
    }
}

// -- constructive linestring operations ---------------------------------------------------

/// Applies `f` to a geometry and, recursively, to the members of a collection.
fn map_members(g: &Geometry<f64>, f: &impl Fn(&Geometry<f64>) -> Geometry<f64>) -> Geometry<f64> {
    match g {
        Geometry::GeometryCollection(c) => Geometry::GeometryCollection(GeometryCollection(
            c.0.iter().map(|m| map_members(m, f)).collect(),
        )),
        other => f(other),
    }
}

fn reversed_line(l: &LineString<f64>) -> LineString<f64> {
    LineString::new(l.0.iter().rev().copied().collect())
}

fn reversed_polygon(p: &Polygon<f64>) -> Polygon<f64> {
    Polygon::new(
        reversed_line(p.exterior()),
        p.interiors().iter().map(reversed_line).collect(),
    )
}

fn reverse_one(g: &Geometry<f64>) -> Geometry<f64> {
    match g {
        Geometry::LineString(l) => Geometry::LineString(reversed_line(l)),
        Geometry::Line(l) => Geometry::Line(Line::new(l.end, l.start)),
        Geometry::MultiLineString(m) => Geometry::MultiLineString(MultiLineString::new(
            m.0.iter().map(reversed_line).collect(),
        )),
        Geometry::Polygon(p) => Geometry::Polygon(reversed_polygon(p)),
        Geometry::MultiPolygon(m) => Geometry::MultiPolygon(MultiPolygon::new(
            m.0.iter().map(reversed_polygon).collect(),
        )),
        Geometry::Rect(r) => Geometry::Polygon(reversed_polygon(&r.to_polygon())),
        Geometry::Triangle(t) => Geometry::Polygon(reversed_polygon(&t.to_polygon())),
        other => other.clone(),
    }
}

fn orient_one(g: &Geometry<f64>, direction: Direction) -> Geometry<f64> {
    match g {
        Geometry::Polygon(p) => Geometry::Polygon(p.orient(direction)),
        Geometry::MultiPolygon(m) => Geometry::MultiPolygon(m.orient(direction)),
        other => other.clone(),
    }
}

fn segmentize_one(g: &Geometry<f64>, max: f64) -> Geometry<f64> {
    match g {
        Geometry::Line(l) => Geometry::LineString(Euclidean.densify(l, max)),
        Geometry::LineString(l) => Geometry::LineString(Euclidean.densify(l, max)),
        Geometry::MultiLineString(m) => Geometry::MultiLineString(Euclidean.densify(m, max)),
        Geometry::Polygon(p) => Geometry::Polygon(Euclidean.densify(p, max)),
        Geometry::MultiPolygon(m) => Geometry::MultiPolygon(Euclidean.densify(m, max)),
        Geometry::Rect(r) => Geometry::Polygon(Euclidean.densify(r, max)),
        Geometry::Triangle(t) => Geometry::Polygon(Euclidean.densify(t, max)),
        other => other.clone(),
    }
}

/// Drops every vertex within `tolerance` of the last kept one. The first and last vertices
/// survive, so a line keeps its endpoints (and at least two vertices) and a ring stays closed.
fn thin(coords: &[Coord<f64>], tolerance: f64) -> Vec<Coord<f64>> {
    let Some((&first, rest)) = coords.split_first() else {
        return Vec::new();
    };
    let near = |a: Coord<f64>, b: Coord<f64>| (a.x - b.x).hypot(a.y - b.y) <= tolerance;
    let mut kept = vec![first];
    for &c in rest {
        if !near(c, *kept.last().unwrap()) {
            kept.push(c);
        }
    }
    let last = *coords.last().unwrap();
    if kept.last() != Some(&last) {
        if kept.len() > 1 {
            kept.pop();
        }
        kept.push(last);
    }
    kept
}

fn thin_ring(ring: &LineString<f64>, tolerance: f64) -> Option<LineString<f64>> {
    let kept = thin(&ring.0, tolerance);
    (kept.len() >= 4).then(|| LineString::new(kept))
}

fn thin_polygon(p: &Polygon<f64>, tolerance: f64) -> Polygon<f64> {
    match thin_ring(p.exterior(), tolerance) {
        None => Polygon::new(LineString::new(Vec::new()), Vec::new()),
        Some(shell) => Polygon::new(
            shell,
            p.interiors()
                .iter()
                .filter_map(|r| thin_ring(r, tolerance))
                .collect(),
        ),
    }
}

fn thin_line(l: &LineString<f64>, tolerance: f64) -> LineString<f64> {
    LineString::new(thin(&l.0, tolerance))
}

fn thin_one(g: &Geometry<f64>, tolerance: f64) -> Geometry<f64> {
    match g {
        Geometry::LineString(l) => Geometry::LineString(thin_line(l, tolerance)),
        Geometry::MultiLineString(m) => Geometry::MultiLineString(MultiLineString::new(
            m.0.iter().map(|l| thin_line(l, tolerance)).collect(),
        )),
        Geometry::Polygon(p) => Geometry::Polygon(thin_polygon(p, tolerance)),
        Geometry::MultiPolygon(m) => Geometry::MultiPolygon(MultiPolygon::new(
            m.0.iter().map(|p| thin_polygon(p, tolerance)).collect(),
        )),
        Geometry::MultiPoint(m) => {
            let points: Vec<Coord<f64>> = m.0.iter().map(|p| p.0).collect();
            Geometry::MultiPoint(MultiPoint::new(
                thin(&points, tolerance)
                    .into_iter()
                    .map(Point::from)
                    .collect(),
            ))
        }
        other => other.clone(),
    }
}

impl GeoColumn {
    /// Splits segments so none is longer than `max_segment_length` (Euclidean). Points are
    /// unchanged. The length must be finite and positive.
    pub fn segmentize(&self, max_segment_length: f64) -> Result<Self, String> {
        if !(max_segment_length.is_finite() && max_segment_length > 0.0) {
            return Err(format!(
                "max_segment_length must be finite and positive, got {max_segment_length}"
            ));
        }
        self.map_geometry(|g| Some(map_members(g, &|m| segmentize_one(m, max_segment_length))))
    }

    /// Drops consecutive vertices that repeat, or lie within `tolerance` of the last kept one.
    /// `tolerance` must be finite and non-negative; `0` removes exact repeats only.
    pub fn remove_repeated_points(&self, tolerance: f64) -> Result<Self, String> {
        if !(tolerance.is_finite() && tolerance >= 0.0) {
            return Err(format!(
                "tolerance must be finite and non-negative, got {tolerance}"
            ));
        }
        self.map_geometry(|g| {
            Some(if tolerance == 0.0 {
                g.remove_repeated_points()
            } else {
                map_members(g, &|m| thin_one(m, tolerance))
            })
        })
    }

    /// Reverses the vertex order of every line and ring. Parts of a collection keep their
    /// order. Points are unchanged.
    pub fn reverse(&self) -> Result<Self, String> {
        self.map_geometry(|g| Some(map_members(g, &reverse_one)))
    }

    /// Orients polygon rings: counter-clockwise shells and clockwise holes, or the opposite
    /// with `exterior_cw`. Other types are unchanged.
    pub fn orient_polygons(&self, exterior_cw: bool) -> Result<Self, String> {
        let direction = if exterior_cw {
            Direction::Reversed
        } else {
            Direction::Default
        };
        self.map_geometry(|g| Some(map_members(g, &|m| orient_one(m, direction))))
    }

    /// Whether each geometry is a closed linestring wound counter-clockwise. Open lines and
    /// every other type give `false` (Shapely `is_ccw`); only null rows are null.
    pub fn is_ccw(&self) -> Vec<Option<bool>> {
        self.geo_rows()
            .iter()
            .zip(self.null_rows())
            .map(|(row, is_null)| match row {
                Some(Geometry::LineString(l)) => Some(guarded(|| l.is_ccw()).unwrap_or(false)),
                Some(_) => Some(false),
                None => (!is_null).then_some(false),
            })
            .collect()
    }
}
