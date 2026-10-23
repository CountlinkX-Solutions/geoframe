//! Bounding shapes: minimum rotated rectangle, minimum bounding circle (Welzl) and maximum
//! inscribed circle (the JTS cell search).

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::f64::consts::{SQRT_2, TAU};

use geo::{
    BoundingRect, Centroid, Contains, Coord, Geometry, Line, LineString, MinimumRotatedRect,
    MultiPoint, Point, Polygon,
};

use super::concave::degenerate_hull;
use super::unique_coords;

/// Segments of the polygon drawn for a circle: Shapely buffers the center with 8 segments per
/// quarter circle.
const CIRCLE_SEGMENTS: usize = 32;

/// The minimum-area rectangle around the vertices; a point or a segment for degenerate input.
pub(super) fn minimum_rotated_rectangle(geom: &Geometry<f64>) -> Option<Geometry<f64>> {
    let points = unique_coords(geom);
    if points.len() < 3 || is_collinear(&points) {
        return Some(degenerate_hull(&points));
    }
    let cloud = MultiPoint::from(points.into_iter().map(Point::from).collect::<Vec<_>>());
    cloud.minimum_rotated_rect().map(Geometry::Polygon)
}

fn is_collinear(points: &[Coord<f64>]) -> bool {
    let (a, b) = (points[0], points[1]);
    points[2..]
        .iter()
        .all(|c| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x) == 0.0)
}

// -- minimum bounding circle -----------------------------------------------------------

#[derive(Clone, Copy)]
pub(super) struct Circle {
    pub center: Coord<f64>,
    pub radius: f64,
}

impl Circle {
    fn contains(&self, p: Coord<f64>) -> bool {
        (p.x - self.center.x).hypot(p.y - self.center.y) <= self.radius * (1.0 + 1e-12) + 1e-300
    }

    fn from_two(a: Coord<f64>, b: Coord<f64>) -> Self {
        Self {
            center: Coord {
                x: (a.x + b.x) / 2.0,
                y: (a.y + b.y) / 2.0,
            },
            radius: (a.x - b.x).hypot(a.y - b.y) / 2.0,
        }
    }

    fn from_three(a: Coord<f64>, b: Coord<f64>, c: Coord<f64>) -> Self {
        let (bx, by) = (b.x - a.x, b.y - a.y);
        let (cx, cy) = (c.x - a.x, c.y - a.y);
        let d = 2.0 * (bx * cy - by * cx);
        if d == 0.0 {
            // Collinear: the circle on the two farthest points.
            let pairs = [(a, b), (a, c), (b, c)];
            let (p, q) = pairs
                .into_iter()
                .max_by(|x, y| {
                    let dx = (x.0.x - x.1.x).hypot(x.0.y - x.1.y);
                    let dy = (y.0.x - y.1.x).hypot(y.0.y - y.1.y);
                    dx.total_cmp(&dy)
                })
                .unwrap();
            return Self::from_two(p, q);
        }
        let b2 = bx * bx + by * by;
        let c2 = cx * cx + cy * cy;
        let ux = (cy * b2 - by * c2) / d;
        let uy = (bx * c2 - cx * b2) / d;
        Self {
            center: Coord {
                x: a.x + ux,
                y: a.y + uy,
            },
            radius: ux.hypot(uy),
        }
    }
}

/// The smallest circle around the points (Welzl's algorithm, move-to-front, iterative).
pub(super) fn smallest_circle(points: &[Coord<f64>]) -> Option<Circle> {
    let mut points = points.to_vec();
    // A fixed pseudo-random shuffle keeps the expected linear time and the output stable.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    for i in (1..points.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        points.swap(i, (state >> 33) as usize % (i + 1));
    }
    let mut circle = Circle {
        center: *points.first()?,
        radius: 0.0,
    };
    for i in 1..points.len() {
        if circle.contains(points[i]) {
            continue;
        }
        circle = Circle {
            center: points[i],
            radius: 0.0,
        };
        for j in 0..i {
            if circle.contains(points[j]) {
                continue;
            }
            circle = Circle::from_two(points[i], points[j]);
            for k in 0..j {
                if !circle.contains(points[k]) {
                    circle = Circle::from_three(points[i], points[j], points[k]);
                }
            }
        }
    }
    Some(circle)
}

pub(super) fn bounding_circle(geom: &Geometry<f64>) -> Option<Circle> {
    smallest_circle(&unique_coords(geom))
}

/// The circle as Shapely draws it: `buffer(radius, quad_segs=8)` of the center, which starts at
/// angle 0 and runs clockwise. A zero radius gives the point.
pub(super) fn circle_polygon(circle: Circle) -> Geometry<f64> {
    if circle.radius == 0.0 {
        return Geometry::Point(Point::from(circle.center));
    }
    let mut ring: Vec<Coord<f64>> = (0..CIRCLE_SEGMENTS)
        .map(|k| {
            let angle = -(k as f64) * TAU / CIRCLE_SEGMENTS as f64;
            Coord {
                x: circle.center.x + circle.radius * angle.cos(),
                y: circle.center.y + circle.radius * angle.sin(),
            }
        })
        .collect();
    ring.push(ring[0]);
    Geometry::Polygon(Polygon::new(LineString::new(ring), vec![]))
}

// -- maximum inscribed circle ----------------------------------------------------------

fn polygons_of(geom: &Geometry<f64>) -> Option<Vec<&Polygon<f64>>> {
    match geom {
        Geometry::Polygon(p) => Some(vec![p]),
        Geometry::MultiPolygon(m) => Some(m.0.iter().collect()),
        _ => None,
    }
}

fn segments<'a>(polygons: &'a [&'a Polygon<f64>]) -> impl Iterator<Item = Line<f64>> + 'a {
    polygons
        .iter()
        .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors().iter()))
        .flat_map(|ring| ring.lines())
}

fn segment_distance_point(p: Coord<f64>, line: &Line<f64>) -> (f64, Coord<f64>) {
    let (a, b) = (line.start, line.end);
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    };
    let nearest = Coord {
        x: a.x + t * dx,
        y: a.y + t * dy,
    };
    ((p.x - nearest.x).hypot(p.y - nearest.y), nearest)
}

/// Distance from `p` to the nearest boundary, negative outside the polygons, and that point.
fn signed_boundary_distance(polygons: &[&Polygon<f64>], p: Coord<f64>) -> (f64, Coord<f64>) {
    let (distance, nearest) = segments(polygons)
        .map(|line| segment_distance_point(p, &line))
        .fold(
            (f64::INFINITY, p),
            |best, cur| {
                if cur.0 < best.0 {
                    cur
                } else {
                    best
                }
            },
        );
    let inside = polygons.iter().any(|poly| poly.contains(&Point::from(p)));
    (if inside { distance } else { -distance }, nearest)
}

#[derive(Clone, Copy)]
struct Cell {
    x: f64,
    y: f64,
    half: f64,
    distance: f64,
    potential: f64,
    seq: usize,
}

impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Cell {}

impl PartialOrd for Cell {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cell {
    /// Most promising first; earlier cells win ties so the output is deterministic.
    fn cmp(&self, other: &Self) -> Ordering {
        self.potential
            .total_cmp(&other.potential)
            .then(other.seq.cmp(&self.seq))
    }
}

/// Upper bound on the cells examined, so a tiny tolerance cannot run away.
const MAX_CELLS: usize = 5_000_000;

/// The line from the center of the largest circle inside the polygon to the nearest boundary
/// point. The center is within `tolerance` of the optimum (default: the larger side of the
/// bounding box over 1000), found by the same cell search as JTS/GEOS.
pub(super) fn maximum_inscribed_circle(
    geom: &Geometry<f64>,
    tolerance: Option<f64>,
) -> Option<Geometry<f64>> {
    let polygons = polygons_of(geom)?;
    let bounds = geom.bounding_rect()?;
    let (width, height) = (bounds.width(), bounds.height());
    let cell_size = width.min(height);
    if cell_size <= 0.0 {
        return None;
    }
    let tolerance = tolerance.unwrap_or_else(|| width.max(height) / 1000.0);
    if tolerance.is_nan() || tolerance <= 0.0 {
        return None;
    }

    let mut seq = 0usize;
    let mut make = |x: f64, y: f64, half: f64| {
        let (distance, _) = signed_boundary_distance(&polygons, Coord { x, y });
        seq += 1;
        Cell {
            x,
            y,
            half,
            distance,
            potential: distance + half * SQRT_2,
            seq,
        }
    };

    let mut queue = BinaryHeap::new();
    let half = cell_size / 2.0;
    let mut x = bounds.min().x;
    while x < bounds.max().x {
        let mut y = bounds.min().y;
        while y < bounds.max().y {
            queue.push(make(x + half, y + half, half));
            y += cell_size;
        }
        x += cell_size;
    }

    let centroid = geom.centroid()?;
    let mut best = make(centroid.x(), centroid.y(), 0.0);
    let mut examined = 0usize;
    while let Some(cell) = queue.pop() {
        if cell.distance > best.distance {
            best = cell;
        }
        if cell.potential - best.distance > tolerance && examined < MAX_CELLS {
            let h = cell.half / 2.0;
            for (dx, dy) in [(-h, -h), (h, -h), (-h, h), (h, h)] {
                queue.push(make(cell.x + dx, cell.y + dy, h));
            }
            examined += 4;
        }
    }

    let center = Coord {
        x: best.x,
        y: best.y,
    };
    let (_, nearest) = signed_boundary_distance(&polygons, center);
    Some(Geometry::LineString(LineString::new(vec![center, nearest])))
}
