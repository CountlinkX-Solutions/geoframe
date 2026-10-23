//! Batch I1: simplicity, canonical form, precision reduction and vertex constructors.
//!
//! # `is_simple` / `self_intersects`
//!
//! Shapely (GEOS `IsSimpleOp`) semantics. Points are simple, multipoints unless a point
//! repeats. A line is simple when no two of its segments meet except at the shared vertex of
//! consecutive segments and, for a closed line, at its start. A multi-line also needs its
//! members to meet only at end points that belong to both (the end points of a closed line
//! count as interior). A polygon or multipolygon is simple when each of its rings is, whether
//! or not the polygon is valid. A collection is never simple (Shapely answers `False`, also for
//! an empty one). Segments are paired through an R-tree, so the cost is roughly
//! `O((n + k) log n)` for `n` segments and `k` close pairs.
//!
//! `self_intersects` (GeoPolars) is defined here as the inverse of `is_simple` for lines and
//! multi-lines; every other type is `false`.
//!
//! # `normalize`
//!
//! The GEOS canonical form: a polygon shell is clockwise and its holes counter-clockwise, each
//! ring starts at its smallest coordinate (x, then y), holes are sorted in descending order;
//! an open line is reversed when its far end is the smaller one, a closed line is normalised
//! like a clockwise ring; the parts of a multi-geometry or collection are normalised and sorted
//! in descending order (type rank, then vertex count, then coordinates, as `Geometry::compareTo`).
//!
//! # `set_precision`
//!
//! Grid snapping with GEOS' arithmetic (`floor(x / grid + 0.5) * grid`, halves round up also
//! for negative numbers). `Pointwise` only moves the coordinates. `ValidOutput` also drops
//! repeated vertices, collapsed lines and polygons (and holes), and repairs a polygon that the
//! snapping made invalid with `geo`'s `make_valid`, then snaps again; `KeepCollapsed` is the same
//! but a line that collapses to one point stays a zero-length line. Deviations from GEOS'
//! snap-rounding overlay: lines are not noded, so crossings between snapped lines do not get a
//! vertex; repaired polygons can differ in vertex order and start (topologically equal in the
//! reference tests); an invalid input polygon is repaired instead of raising.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};

use geo::algorithm::kernels::{Kernel, Orientation, RobustKernel};
use geo::line_intersection::{line_intersection, LineIntersection};
use geo::{
    BooleanOps, Coord, CoordsIter, Geometry, GeometryCollection, Line, LineString, MakeValid,
    MapCoords, MultiLineString, MultiPoint, MultiPolygon, Point, Polygon, Validation,
};
use geoarrow_expr_geo::util::to_geo::geometry_to_geo;
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, AABB};

use crate::parity_a::normalize as to_stored;
use crate::predicates::guarded;
use crate::GeoColumn;

// -- is_simple ----------------------------------------------------------------------------

struct Segment {
    line: Line<f64>,
    owner: usize,
    index: usize,
}

fn envelope(line: &Line<f64>) -> AABB<[f64; 2]> {
    AABB::from_corners(
        [line.start.x.min(line.end.x), line.start.y.min(line.end.y)],
        [line.start.x.max(line.end.x), line.start.y.max(line.end.y)],
    )
}

/// Consecutive repeated vertices removed.
fn deduplicated(line: &LineString<f64>) -> Vec<Coord<f64>> {
    let mut coords = line.0.clone();
    coords.dedup();
    coords
}

/// Whether the lines, taken as one multi-line, are simple (see the module documentation).
fn lines_are_simple(lines: &[&LineString<f64>]) -> bool {
    let cleaned: Vec<Vec<Coord<f64>>> = lines
        .iter()
        .map(|l| deduplicated(l))
        .filter(|c| c.len() >= 2)
        .collect();
    let closed: Vec<bool> = cleaned.iter().map(|c| c.first() == c.last()).collect();
    let mut segments = Vec::new();
    for (owner, coords) in cleaned.iter().enumerate() {
        for (index, pair) in coords.windows(2).enumerate() {
            segments.push(Segment {
                line: Line::new(pair[0], pair[1]),
                owner,
                index,
            });
        }
    }
    let tree = RTree::bulk_load(
        segments
            .iter()
            .enumerate()
            .map(|(i, s)| GeomWithData::new(Rectangle::from_aabb(envelope(&s.line)), i))
            .collect(),
    );
    let is_end = |owner: usize, p: Coord<f64>| {
        let coords = &cleaned[owner];
        !closed[owner] && (coords.first() == Some(&p) || coords.last() == Some(&p))
    };
    for (i, a) in segments.iter().enumerate() {
        for entry in tree.locate_in_envelope_intersecting(&envelope(&a.line)) {
            let j = entry.data;
            if j <= i {
                continue;
            }
            let b = &segments[j];
            let Some(hit) = line_intersection(a.line, b.line) else {
                continue;
            };
            let LineIntersection::SinglePoint {
                intersection: at, ..
            } = hit
            else {
                return false;
            };
            let allowed = if a.owner == b.owner {
                let last = cleaned[a.owner].len() - 2;
                if b.index == a.index + 1 {
                    at == a.line.end
                } else {
                    closed[a.owner] && a.index == 0 && b.index == last && at == a.line.start
                }
            } else {
                is_end(a.owner, at) && is_end(b.owner, at)
            };
            if !allowed {
                return false;
            }
        }
    }
    true
}

fn rings_are_simple(polygon: &Polygon<f64>) -> bool {
    std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .all(|ring| lines_are_simple(&[ring]))
}

fn points_are_distinct(points: &[Point<f64>]) -> bool {
    let mut seen = HashSet::new();
    points
        .iter()
        .all(|p| seen.insert(((p.x() + 0.0).to_bits(), (p.y() + 0.0).to_bits())))
}

fn simple(geom: &Geometry<f64>) -> bool {
    match to_stored(geom) {
        Geometry::Point(_) => true,
        Geometry::MultiPoint(m) => points_are_distinct(&m.0),
        Geometry::LineString(l) => lines_are_simple(&[&l]),
        Geometry::MultiLineString(m) => lines_are_simple(&m.0.iter().collect::<Vec<_>>()),
        Geometry::Polygon(p) => rings_are_simple(&p),
        Geometry::MultiPolygon(m) => m.0.iter().all(rings_are_simple),
        _ => false,
    }
}

fn is_linear(geom: &Geometry<f64>) -> bool {
    matches!(
        to_stored(geom),
        Geometry::LineString(_) | Geometry::MultiLineString(_)
    )
}

impl GeoColumn {
    /// Shapely `is_simple`: see the module documentation. Null rows are null; an empty point
    /// (which `geo` cannot represent) is simple.
    pub fn is_simple(&self) -> Vec<Option<bool>> {
        self.map_scalars(|scalar| match geometry_to_geo(scalar) {
            Ok(g) => catch_unwind(AssertUnwindSafe(|| simple(&g))).unwrap_or(false),
            Err(_) => true,
        })
    }

    /// GeoPolars `self_intersects`: the inverse of [`GeoColumn::is_simple`] for lines and
    /// multi-lines, `false` for every other type, null for null rows.
    pub fn self_intersects(&self) -> Vec<Option<bool>> {
        self.map_scalars(|scalar| match geometry_to_geo(scalar) {
            Ok(g) if is_linear(&g) => {
                catch_unwind(AssertUnwindSafe(|| !simple(&g))).unwrap_or(false)
            }
            _ => false,
        })
    }
}

// -- normalize ----------------------------------------------------------------------------

fn cmp_coord(a: &Coord<f64>, b: &Coord<f64>) -> Ordering {
    match a.x.partial_cmp(&b.x) {
        Some(Ordering::Equal) | None => a.y.partial_cmp(&b.y).unwrap_or(Ordering::Equal),
        Some(order) => order,
    }
}

/// `Geometry::getSortIndex` of GEOS.
fn sort_index(g: &Geometry<f64>) -> u8 {
    match g {
        Geometry::Point(_) => 0,
        Geometry::MultiPoint(_) => 1,
        Geometry::LineString(_) | Geometry::Line(_) => 2,
        Geometry::MultiLineString(_) => 4,
        Geometry::Polygon(_) | Geometry::Rect(_) | Geometry::Triangle(_) => 5,
        Geometry::MultiPolygon(_) => 6,
        Geometry::GeometryCollection(_) => 7,
    }
}

fn first_difference<T>(a: &[T], b: &[T], cmp: impl Fn(&T, &T) -> Ordering) -> Ordering {
    a.iter()
        .zip(b)
        .map(|(x, y)| cmp(x, y))
        .find(|o| o.is_ne())
        .unwrap_or(Ordering::Equal)
}

fn cmp_lines(a: &LineString<f64>, b: &LineString<f64>) -> Ordering {
    a.0.len()
        .cmp(&b.0.len())
        .then_with(|| first_difference(&a.0, &b.0, cmp_coord))
}

fn cmp_polygons(a: &Polygon<f64>, b: &Polygon<f64>) -> Ordering {
    cmp_lines(a.exterior(), b.exterior())
        .then_with(|| a.interiors().len().cmp(&b.interiors().len()))
        .then_with(|| first_difference(a.interiors(), b.interiors(), cmp_lines))
}

fn cmp_sequences(a: &[Geometry<f64>], b: &[Geometry<f64>]) -> Ordering {
    first_difference(a, b, cmp_geometry).then_with(|| a.len().cmp(&b.len()))
}

fn members<T: Clone>(items: &[T], wrap: impl Fn(T) -> Geometry<f64>) -> Vec<Geometry<f64>> {
    items.iter().cloned().map(wrap).collect()
}

/// `Geometry::compareTo` of GEOS: type rank first, empty before non-empty, then by content.
fn cmp_geometry(a: &Geometry<f64>, b: &Geometry<f64>) -> Ordering {
    let (a, b) = (to_stored(a), to_stored(b));
    let rank = sort_index(&a).cmp(&sort_index(&b));
    if rank.is_ne() {
        return rank;
    }
    match (a.coords_count() == 0, b.coords_count() == 0) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        _ => {}
    }
    match (&a, &b) {
        (Geometry::Point(x), Geometry::Point(y)) => cmp_coord(&x.0, &y.0),
        (Geometry::LineString(x), Geometry::LineString(y)) => cmp_lines(x, y),
        (Geometry::Polygon(x), Geometry::Polygon(y)) => cmp_polygons(x, y),
        (Geometry::MultiPoint(x), Geometry::MultiPoint(y)) => cmp_sequences(
            &members(&x.0, Geometry::Point),
            &members(&y.0, Geometry::Point),
        ),
        (Geometry::MultiLineString(x), Geometry::MultiLineString(y)) => cmp_sequences(
            &members(&x.0, Geometry::LineString),
            &members(&y.0, Geometry::LineString),
        ),
        (Geometry::MultiPolygon(x), Geometry::MultiPolygon(y)) => cmp_sequences(
            &members(&x.0, Geometry::Polygon),
            &members(&y.0, Geometry::Polygon),
        ),
        (Geometry::GeometryCollection(x), Geometry::GeometryCollection(y)) => {
            cmp_sequences(&x.0, &y.0)
        }
        _ => Ordering::Equal,
    }
}

/// JTS/GEOS `Orientation::isCCW` of a closed coordinate list; `false` for degenerate rings.
fn is_ccw(ring: &[Coord<f64>]) -> bool {
    let n = ring.len().saturating_sub(1);
    if n < 3 {
        return false;
    }
    let mut up_hi = ring[0];
    let mut prev_y = up_hi.y;
    let mut up_low = None;
    let mut i_up_hi = 0;
    for (i, c) in ring.iter().enumerate().skip(1) {
        if c.y > prev_y && c.y >= up_hi.y {
            up_hi = *c;
            up_low = Some(ring[i - 1]);
            i_up_hi = i;
        }
        prev_y = c.y;
    }
    let Some(up_low) = up_low.filter(|_| i_up_hi != 0) else {
        return false;
    };
    let mut i_down_low = i_up_hi;
    loop {
        i_down_low = (i_down_low + 1) % n;
        if i_down_low == i_up_hi || ring[i_down_low].y != up_hi.y {
            break;
        }
    }
    let down_low = ring[i_down_low];
    let down_hi = ring[if i_down_low > 0 {
        i_down_low - 1
    } else {
        n - 1
    }];
    if up_hi == down_hi {
        if up_low == up_hi || down_low == up_hi || up_low == down_low {
            return false;
        }
        RobustKernel::orient2d(up_low, up_hi, down_low) == Orientation::CounterClockwise
    } else {
        down_hi.x - up_hi.x < 0.0
    }
}

/// A closed coordinate list rotated to start at its smallest coordinate and wound clockwise
/// (`clockwise`) or counter-clockwise, as `Polygon::normalize(LinearRing*, bool)` of GEOS.
fn normalized_ring(coords: &[Coord<f64>], clockwise: bool) -> Vec<Coord<f64>> {
    if coords.len() < 2 {
        return coords.to_vec();
    }
    let mut open = coords[..coords.len() - 1].to_vec();
    let mut min = 0;
    for (i, c) in open.iter().enumerate() {
        if cmp_coord(c, &open[min]) == Ordering::Less {
            min = i;
        }
    }
    open.rotate_left(min);
    open.push(open[0]);
    if is_ccw(&open) == clockwise {
        open.reverse();
    }
    open
}

fn normalized_line(line: &LineString<f64>) -> LineString<f64> {
    let coords = &line.0;
    if coords.is_empty() {
        return line.clone();
    }
    if coords.first() == coords.last() {
        return LineString::new(normalized_ring(coords, true));
    }
    let n = coords.len();
    for i in 0..n / 2 {
        let j = n - 1 - i;
        if coords[i] != coords[j] {
            if cmp_coord(&coords[i], &coords[j]) == Ordering::Greater {
                return LineString::new(coords.iter().rev().copied().collect());
            }
            break;
        }
    }
    line.clone()
}

fn normalized_polygon(polygon: &Polygon<f64>) -> Polygon<f64> {
    if polygon.exterior().0.is_empty() {
        return polygon.clone();
    }
    let shell = LineString::new(normalized_ring(&polygon.exterior().0, true));
    let mut holes: Vec<LineString<f64>> = polygon
        .interiors()
        .iter()
        .map(|h| LineString::new(normalized_ring(&h.0, false)))
        .collect();
    holes.sort_by(|a, b| cmp_lines(b, a));
    Polygon::new(shell, holes)
}

fn sorted_descending(mut parts: Vec<Geometry<f64>>) -> Vec<Geometry<f64>> {
    parts.sort_by(|a, b| cmp_geometry(b, a));
    parts
}

fn canonical(geom: &Geometry<f64>) -> Geometry<f64> {
    match to_stored(geom) {
        Geometry::LineString(l) => Geometry::LineString(normalized_line(&l)),
        Geometry::Polygon(p) => Geometry::Polygon(normalized_polygon(&p)),
        Geometry::MultiPoint(m) => Geometry::MultiPoint(MultiPoint::new(
            sorted_descending(members(&m.0, Geometry::Point))
                .into_iter()
                .filter_map(|g| Point::try_from(g).ok())
                .collect(),
        )),
        Geometry::MultiLineString(m) => Geometry::MultiLineString(MultiLineString::new(
            sorted_descending(
                m.0.iter()
                    .map(|l| Geometry::LineString(normalized_line(l)))
                    .collect(),
            )
            .into_iter()
            .filter_map(|g| LineString::try_from(g).ok())
            .collect(),
        )),
        Geometry::MultiPolygon(m) => Geometry::MultiPolygon(MultiPolygon::new(
            sorted_descending(
                m.0.iter()
                    .map(|p| Geometry::Polygon(normalized_polygon(p)))
                    .collect(),
            )
            .into_iter()
            .filter_map(|g| Polygon::try_from(g).ok())
            .collect(),
        )),
        Geometry::GeometryCollection(c) => Geometry::GeometryCollection(
            GeometryCollection::new_from(sorted_descending(c.0.iter().map(canonical).collect())),
        ),
        other => other,
    }
}

impl GeoColumn {
    /// The GEOS canonical form of every geometry; see the module documentation. Null rows stay
    /// null.
    pub fn normalize(&self) -> Result<Self, String> {
        self.map_geometry(|g| Some(canonical(g)))
    }
}

// -- set_precision ------------------------------------------------------------------------

/// What [`GeoColumn::set_precision`] does with geometry that collapses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecisionMode {
    ValidOutput,
    Pointwise,
    KeepCollapsed,
}

impl PrecisionMode {
    /// Parses the Shapely mode names.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "valid_output" => Ok(Self::ValidOutput),
            "pointwise" => Ok(Self::Pointwise),
            "keep_collapsed" => Ok(Self::KeepCollapsed),
            other => Err(format!(
                "unknown precision mode {other:?}; expected valid_output, pointwise or keep_collapsed"
            )),
        }
    }
}

/// How many times a snapped polygon is repaired and snapped again before it is returned as is.
const REPAIR_ROUNDS: usize = 3;

struct Grid {
    scale: f64,
}

impl Grid {
    fn snap(&self, v: f64) -> f64 {
        (v * self.scale + 0.5).floor() / self.scale
    }

    fn coord(&self, c: Coord<f64>) -> Coord<f64> {
        Coord {
            x: self.snap(c.x),
            y: self.snap(c.y),
        }
    }

    /// Snapped vertices with consecutive repeats removed.
    fn snapped_line(&self, line: &LineString<f64>) -> Vec<Coord<f64>> {
        let mut coords: Vec<Coord<f64>> = line.0.iter().map(|&c| self.coord(c)).collect();
        coords.dedup();
        coords
    }

    fn line(&self, line: &LineString<f64>, keep_collapsed: bool) -> LineString<f64> {
        let mut coords = self.snapped_line(line);
        if coords.len() == 1 && keep_collapsed {
            coords.push(coords[0]);
        }
        if coords.len() < 2 {
            coords.clear();
        }
        LineString::new(coords)
    }

    /// A snapped ring, or `None` when it collapsed (too few vertices or no area).
    fn ring(&self, ring: &LineString<f64>) -> Option<LineString<f64>> {
        let mut coords = self.snapped_line(ring);
        if coords.is_empty() {
            return None;
        }
        if coords.first() != coords.last() {
            coords.push(coords[0]);
        }
        let ring = LineString::new(coords);
        (ring.0.len() >= 4 && ring_area(&ring) != 0.0).then_some(ring)
    }

    /// The polygons a polygon becomes: none when it collapses, several when repair splits it.
    fn polygon(&self, polygon: &Polygon<f64>, rounds: usize) -> Vec<Polygon<f64>> {
        let Some(shell) = self.ring(polygon.exterior()) else {
            return Vec::new();
        };
        let holes = polygon
            .interiors()
            .iter()
            .filter_map(|h| self.ring(h))
            .collect();
        let snapped = Polygon::new(shell, holes);
        if snapped.is_valid() || rounds == 0 {
            return vec![snapped];
        }
        // Holes that now overlap each other or leave the shell: shell minus all of them.
        let shell_only = Polygon::new(snapped.exterior().clone(), vec![]);
        let repaired = if shell_only.is_valid() {
            guarded(|| {
                snapped.interiors().iter().fold(
                    MultiPolygon::new(vec![shell_only.clone()]),
                    |area, hole| {
                        area.difference(&MultiPolygon::new(vec![Polygon::new(
                            hole.clone(),
                            vec![],
                        )]))
                    },
                )
            })
        } else {
            snapped.make_valid().ok()
        };
        match repaired {
            Some(repaired) => repaired
                .0
                .iter()
                .flat_map(|p| self.polygon(p, rounds - 1))
                .collect(),
            None => vec![snapped],
        }
    }

    /// Snapped polygons that overlap or share edges are merged, as GEOS' union does.
    fn polygons(&self, polygons: &[Polygon<f64>]) -> MultiPolygon<f64> {
        let multi = MultiPolygon::new(
            polygons
                .iter()
                .flat_map(|p| self.polygon(p, REPAIR_ROUNDS))
                .collect(),
        );
        if multi.0.len() < 2 || multi.is_valid() {
            return multi;
        }
        let merged = guarded(|| {
            multi.0.iter().fold(MultiPolygon::new(vec![]), |acc, p| {
                acc.union(&MultiPolygon::new(vec![p.clone()]))
            })
        });
        match merged {
            Some(merged) => {
                MultiPolygon::new(merged.0.iter().flat_map(|p| self.polygon(p, 1)).collect())
            }
            None => multi,
        }
    }

    fn reduce(&self, geom: &Geometry<f64>, keep_collapsed: bool) -> Geometry<f64> {
        match to_stored(geom) {
            Geometry::LineString(l) => Geometry::LineString(self.line(&l, keep_collapsed)),
            Geometry::MultiLineString(m) => Geometry::MultiLineString(MultiLineString::new(
                m.0.iter()
                    .map(|l| self.line(l, keep_collapsed))
                    .filter(|l| !l.0.is_empty())
                    .collect(),
            )),
            Geometry::Polygon(p) => {
                let mut parts = self.polygons(std::slice::from_ref(&p)).0;
                match parts.len() {
                    0 => Geometry::Polygon(Polygon::new(LineString::new(vec![]), vec![])),
                    1 => Geometry::Polygon(parts.remove(0)),
                    _ => Geometry::MultiPolygon(MultiPolygon::new(parts)),
                }
            }
            Geometry::MultiPolygon(m) => Geometry::MultiPolygon(self.polygons(&m.0)),
            Geometry::GeometryCollection(c) => {
                Geometry::GeometryCollection(GeometryCollection::new_from(
                    c.0.iter()
                        .map(|g| self.reduce(g, keep_collapsed))
                        .filter(|g| g.coords_count() > 0)
                        .collect(),
                ))
            }
            point_like => point_like.map_coords(|c| self.coord(c)),
        }
    }
}

/// Signed shoelace area of a closed ring; exactly `0.0` for a ring without area.
fn ring_area(ring: &LineString<f64>) -> f64 {
    ring.0
        .windows(2)
        .map(|w| w[0].x * w[1].y - w[1].x * w[0].y)
        .sum::<f64>()
        / 2.0
}

/// GEOS rebuilds a collection through `buildGeometry`, which turns a one-member multi
/// geometry inside it into its member; outside a collection a multi keeps its type.
fn unwrap_single_members(geom: Geometry<f64>) -> Geometry<f64> {
    let Geometry::GeometryCollection(collection) = geom else {
        return geom;
    };
    Geometry::GeometryCollection(GeometryCollection::new_from(
        collection
            .0
            .into_iter()
            .map(|member| match member {
                Geometry::MultiPoint(mut m) if m.0.len() == 1 => Geometry::Point(m.0.remove(0)),
                Geometry::MultiLineString(mut m) if m.0.len() == 1 => {
                    Geometry::LineString(m.0.remove(0))
                }
                Geometry::MultiPolygon(mut m) if m.0.len() == 1 => Geometry::Polygon(m.0.remove(0)),
                other => other,
            })
            .collect(),
    ))
}

impl GeoColumn {
    /// Snaps every coordinate to a grid of `grid_size` (the sign is ignored; `0` returns the
    /// column unchanged), as Shapely `set_precision`; see the module documentation for what each
    /// [`PrecisionMode`] does with collapsed geometry.
    pub fn set_precision(&self, grid_size: f64, mode: PrecisionMode) -> Result<Self, String> {
        if !grid_size.is_finite() {
            return Err(format!("grid_size must be finite, got {grid_size}"));
        }
        let grid_size = grid_size.abs();
        if grid_size == 0.0 {
            return Ok(self.clone());
        }
        let scale = 1.0 / grid_size;
        if !scale.is_finite() {
            return Err(format!("grid_size {grid_size} is too small"));
        }
        let grid = Grid { scale };
        self.map_geometry(|g| {
            let snapped = match mode {
                PrecisionMode::Pointwise => to_stored(g).map_coords(|c| grid.coord(c)),
                PrecisionMode::ValidOutput => grid.reduce(g, false),
                PrecisionMode::KeepCollapsed => grid.reduce(g, true),
            };
            Some(unwrap_single_members(snapped))
        })
    }
}

// -- constructors from vertices -----------------------------------------------------------

/// Which geometry [`GeoColumn::from_vertices`] builds from each row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexKind {
    LineString,
    MultiPoint,
    Polygon,
    MultiLineString,
    MultiPolygon,
}

impl VertexKind {
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "line_string" => Ok(Self::LineString),
            "multi_point" => Ok(Self::MultiPoint),
            "polygon" => Ok(Self::Polygon),
            "multi_line_string" => Ok(Self::MultiLineString),
            "multi_polygon" => Ok(Self::MultiPolygon),
            other => Err(format!("unknown vertex geometry kind {other:?}")),
        }
    }
}

/// One row of vertices: parts, each a list of rings, each a list of `(x, y)`.
pub type VertexParts = Vec<Vec<Vec<(f64, f64)>>>;

fn vertices(ring: &[(f64, f64)], row: usize) -> Result<Vec<Coord<f64>>, String> {
    ring.iter()
        .map(|&(x, y)| {
            if x.is_finite() && y.is_finite() {
                Ok(Coord { x, y })
            } else {
                Err(format!("row {row}: non-finite coordinate ({x}, {y})"))
            }
        })
        .collect()
}

fn line_from(ring: &[(f64, f64)], row: usize) -> Result<LineString<f64>, String> {
    let coords = vertices(ring, row)?;
    if coords.len() == 1 {
        return Err(format!("row {row}: a line needs at least two vertices"));
    }
    Ok(LineString::new(coords))
}

fn polygon_from(rings: &[Vec<(f64, f64)>], row: usize) -> Result<Polygon<f64>, String> {
    let mut built = Vec::with_capacity(rings.len());
    for ring in rings {
        let mut coords = vertices(ring, row)?;
        if coords.first() != coords.last() {
            coords.push(coords[0]);
        }
        let distinct: HashSet<(u64, u64)> = coords
            .iter()
            .map(|c| ((c.x + 0.0).to_bits(), (c.y + 0.0).to_bits()))
            .collect();
        if distinct.len() < 3 {
            return Err(format!(
                "row {row}: a ring needs at least three distinct vertices"
            ));
        }
        built.push(LineString::new(coords));
    }
    let mut built = built.into_iter();
    match built.next() {
        Some(shell) => Ok(Polygon::new(shell, built.collect())),
        None => Ok(Polygon::new(LineString::new(vec![]), vec![])),
    }
}

fn single_ring(part: &[Vec<(f64, f64)>], row: usize) -> Result<&[(f64, f64)], String> {
    match part {
        [ring] => Ok(ring),
        _ => Err(format!("row {row}: expected one list of vertices")),
    }
}

fn geometry_from(
    kind: VertexKind,
    parts: &VertexParts,
    row: usize,
) -> Result<Geometry<f64>, String> {
    match kind {
        VertexKind::LineString | VertexKind::MultiPoint => {
            let [part] = parts.as_slice() else {
                return Err(format!("row {row}: expected one list of vertices"));
            };
            let ring = single_ring(part, row)?;
            if kind == VertexKind::LineString {
                Ok(Geometry::LineString(line_from(ring, row)?))
            } else {
                Ok(Geometry::MultiPoint(MultiPoint::new(
                    vertices(ring, row)?.into_iter().map(Point::from).collect(),
                )))
            }
        }
        VertexKind::Polygon => {
            let [part] = parts.as_slice() else {
                return Err(format!("row {row}: expected one list of rings"));
            };
            Ok(Geometry::Polygon(polygon_from(part, row)?))
        }
        VertexKind::MultiLineString => Ok(Geometry::MultiLineString(MultiLineString::new(
            parts
                .iter()
                .map(|part| line_from(single_ring(part, row)?, row))
                .collect::<Result<_, _>>()?,
        ))),
        VertexKind::MultiPolygon => Ok(Geometry::MultiPolygon(MultiPolygon::new(
            parts
                .iter()
                .map(|part| polygon_from(part, row))
                .collect::<Result<_, _>>()?,
        ))),
    }
}

impl GeoColumn {
    /// Builds a column from vertex lists: each row is `None` (null) or a list of parts, each a
    /// list of rings, each a list of `(x, y)`; `kind` says how a row nests (one part with one
    /// ring for a line or multipoint, one part with the rings for a polygon, one single-ring
    /// part per line for a multi-line, one ring list per polygon for a multipolygon). Open
    /// rings are closed. Errors name the row.
    pub fn from_vertices(
        kind: VertexKind,
        rows: &[Option<VertexParts>],
        crs: Option<&str>,
    ) -> Result<Self, String> {
        let geoms: Vec<Option<Geometry<f64>>> = rows
            .iter()
            .enumerate()
            .map(|(row, parts)| {
                parts
                    .as_ref()
                    .map(|p| geometry_from(kind, p, row))
                    .transpose()
            })
            .collect::<Result<_, _>>()?;
        GeoColumn::from_wkt::<&str>(&[], crs)?.rebuild(&geoms)
    }
}
