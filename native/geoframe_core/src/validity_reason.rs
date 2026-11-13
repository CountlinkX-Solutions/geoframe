//! The first reason GEOS' `IsValidOp` gives for an invalid geometry, as `<text>[<x> <y>]`.
//!
//! The check order is GEOS': coordinates, too few points, area intersections (any crossing
//! or overlap is `Self-intersection`; a ring touching itself is `Ring Self-intersection`),
//! hole outside shell, nested holes, nested shells, disconnected interior. The text of each
//! class is GEOS'. The reported coordinate is GEOS' whenever one error exists or its
//! intersection point is unambiguous (GEOS' `LineIntersector` rules for collinear overlaps
//! are reproduced); when a geometry has several errors of one class, GEOS' monotone-chain
//! traversal picks the reported one and this module takes the first in ring and segment
//! order instead (`docs/parity.md`).

use geo::algorithm::line_intersection::{line_intersection, LineIntersection};
use geo::coordinate_position::{CoordPos, CoordinatePosition};
use geo::{BoundingRect, Coord, Geometry, Line, LineString, Polygon, Rect};

/// GEOS' reason for the first error of `geom`, `None` when this module finds none.
pub(crate) fn geos_reason(geom: &Geometry<f64>) -> Option<String> {
    match geom {
        Geometry::Point(p) => coordinates_reason(&[p.0]),
        Geometry::MultiPoint(mp) => mp.0.iter().find_map(|p| coordinates_reason(&[p.0])),
        Geometry::Line(l) => line_reason(&LineString::from(vec![l.start, l.end])),
        Geometry::LineString(l) => line_reason(l),
        Geometry::MultiLineString(ml) => ml.0.iter().find_map(line_reason),
        Geometry::Polygon(p) => polygons_reason(std::slice::from_ref(p)),
        Geometry::MultiPolygon(mp) => polygons_reason(&mp.0),
        Geometry::GeometryCollection(gc) => gc.0.iter().find_map(geos_reason),
        Geometry::Rect(_) | Geometry::Triangle(_) => None,
    }
}

fn number(v: f64) -> String {
    if v.is_nan() {
        "nan".to_string()
    } else if v.is_infinite() {
        if v > 0.0 { "inf" } else { "-inf" }.to_string()
    } else {
        v.to_string()
    }
}

fn at(text: &str, c: Coord<f64>) -> String {
    format!("{text}[{} {}]", number(c.x), number(c.y))
}

fn coordinates_reason(coords: &[Coord<f64>]) -> Option<String> {
    coords
        .iter()
        .find(|c| !c.x.is_finite() || !c.y.is_finite())
        .map(|c| at("Invalid Coordinate", *c))
}

/// Consecutive repeated vertices removed, as GEOS does before it analyses a ring.
fn without_repeats(ring: &LineString<f64>) -> Vec<Coord<f64>> {
    let mut out: Vec<Coord<f64>> = Vec::with_capacity(ring.0.len());
    for c in &ring.0 {
        if out.last() != Some(c) {
            out.push(*c);
        }
    }
    out
}

fn too_few(text: &str, ring: &LineString<f64>, minimum: usize) -> Option<String> {
    (!ring.0.is_empty() && without_repeats(ring).len() < minimum).then(|| at(text, ring.0[0]))
}

fn line_reason(line: &LineString<f64>) -> Option<String> {
    coordinates_reason(&line.0).or_else(|| too_few("Too few points in geometry component", line, 2))
}

const SELF_INTERSECTION: &str = "Self-intersection";
const RING_SELF_INTERSECTION: &str = "Ring Self-intersection";
const TOO_FEW: &str = "Too few points in geometry component";

fn polygon_rings(p: &Polygon<f64>) -> impl Iterator<Item = &LineString<f64>> {
    std::iter::once(p.exterior()).chain(p.interiors())
}

fn polygons_reason(polygons: &[Polygon<f64>]) -> Option<String> {
    let rings = || polygons.iter().flat_map(polygon_rings);
    if let Some(r) = rings().find_map(|ring| coordinates_reason(&ring.0)) {
        return Some(r);
    }
    if let Some(r) = rings().find_map(|ring| too_few(TOO_FEW, ring, 4)) {
        return Some(r);
    }
    let mut touches = Vec::new();
    if let Some(r) = area_intersections(polygons, &mut touches) {
        return Some(r);
    }
    let shells: Vec<Polygon<f64>> = polygons
        .iter()
        .map(|p| Polygon::new(p.exterior().clone(), vec![]))
        .collect();
    polygons
        .iter()
        .zip(&shells)
        .find_map(|(p, shell)| hole_outside_shell(p, shell))
        .or_else(|| polygons.iter().find_map(nested_hole))
        .or_else(|| nested_shell(polygons, &shells))
        .or_else(|| disconnected_interior(polygons, &touches))
}

// -- area intersections -----------------------------------------------------------------

/// Ring and segment index of both segments of a pair: the order in which pairs are tried.
type SegmentKey = (usize, usize, usize, usize);

struct Segment {
    ring: usize,
    index: usize,
    line: Line<f64>,
}

/// A single-point touch between two rings that is allowed (a vertex or a T), kept for the
/// interior connectivity check.
struct Touch {
    key: SegmentKey,
    rings: (usize, usize),
    at: Coord<f64>,
}

/// What a pair of segments means for validity.
enum Meeting {
    Invalid(&'static str, Coord<f64>),
    Touch(Coord<f64>),
}

/// Pair tests after which the sweep stops once it holds an error.
const PAIR_BUDGET: usize = 5_000_000;

fn area_intersections(polygons: &[Polygon<f64>], touches: &mut Vec<Touch>) -> Option<String> {
    if let Some(reason) = crate::validity::collapsed_reason(&Geometry::MultiPolygon(
        geo::MultiPolygon::new(polygons.to_vec()),
    )) {
        return Some(reason);
    }
    let rings: Vec<Vec<Coord<f64>>> = polygons
        .iter()
        .flat_map(polygon_rings)
        .map(without_repeats)
        .collect();
    let mut segments: Vec<Segment> = Vec::new();
    for (ring, coords) in rings.iter().enumerate() {
        for (index, w) in coords.windows(2).enumerate() {
            segments.push(Segment {
                ring,
                index,
                line: Line::new(w[0], w[1]),
            });
        }
    }
    let counts: Vec<usize> = rings.iter().map(|r| r.len().saturating_sub(1)).collect();
    let min_x = |s: &Segment| s.line.start.x.min(s.line.end.x);
    let max_x = |s: &Segment| s.line.start.x.max(s.line.end.x);
    let (min_y, max_y) = (
        |s: &Segment| s.line.start.y.min(s.line.end.y),
        |s: &Segment| s.line.start.y.max(s.line.end.y),
    );
    let mut order: Vec<usize> = (0..segments.len()).collect();
    order.sort_by(|&a, &b| min_x(&segments[a]).total_cmp(&min_x(&segments[b])));

    let mut found: Option<(SegmentKey, &'static str, Coord<f64>)> = None;
    let mut tests = 0usize;
    'sweep: for (n, &i) in order.iter().enumerate() {
        let a = &segments[i];
        for &j in &order[n + 1..] {
            let b = &segments[j];
            if min_x(b) > max_x(a) {
                break;
            }
            if min_y(b) > max_y(a) || max_y(b) < min_y(a) {
                continue;
            }
            tests += 1;
            if found.is_some() && tests > PAIR_BUDGET {
                break 'sweep;
            }
            let (p, q) = if (a.ring, a.index) <= (b.ring, b.index) {
                (a, b)
            } else {
                (b, a)
            };
            let adjacent = p.ring == q.ring
                && (q.index == p.index + 1 || (p.index == 0 && q.index + 1 == counts[p.ring]));
            let key = (p.ring, p.index, q.ring, q.index);
            match meeting(p, q, adjacent) {
                Some(Meeting::Invalid(text, c)) => {
                    if found.as_ref().is_none_or(|f| key < f.0) {
                        found = Some((key, text, c));
                    }
                }
                Some(Meeting::Touch(c)) => touches.push(Touch {
                    key,
                    rings: (p.ring, q.ring),
                    at: c,
                }),
                None => {}
            }
        }
    }
    touches.sort_by_key(|t| t.key);
    found.map(|(_, text, c)| at(text, c))
}

fn meeting(p: &Segment, q: &Segment, adjacent: bool) -> Option<Meeting> {
    let same_ring = p.ring == q.ring;
    match line_intersection(p.line, q.line)? {
        LineIntersection::Collinear { .. } => Some(Meeting::Invalid(
            "Self-intersection",
            collinear_point(p.line, q.line),
        )),
        LineIntersection::SinglePoint {
            intersection,
            is_proper,
        } => {
            if is_proper {
                Some(Meeting::Invalid(SELF_INTERSECTION, intersection))
            } else if !same_ring {
                Some(Meeting::Touch(intersection))
            } else if adjacent {
                None
            } else {
                Some(Meeting::Invalid(RING_SELF_INTERSECTION, intersection))
            }
        }
    }
}

/// First point of a collinear overlap, in the order of GEOS' `LineIntersector`.
fn collinear_point(p: Line<f64>, q: Line<f64>) -> Coord<f64> {
    let within = |s: Line<f64>, c: Coord<f64>| {
        c.x >= s.start.x.min(s.end.x)
            && c.x <= s.start.x.max(s.end.x)
            && c.y >= s.start.y.min(s.end.y)
            && c.y <= s.start.y.max(s.end.y)
    };
    let (q1_in_p, q2_in_p) = (within(p, q.start), within(p, q.end));
    let (p1_in_q, p2_in_q) = (within(q, p.start), within(q, p.end));
    if q1_in_p {
        q.start
    } else if p1_in_q {
        p.start
    } else if q2_in_p {
        q.end
    } else if p2_in_q {
        p.end
    } else {
        p.start
    }
}

// -- holes and shells -------------------------------------------------------------------

/// The first vertex of `ring` that is not on the boundary of `against`, with its position.
fn first_off_boundary(
    ring: &LineString<f64>,
    against: &Polygon<f64>,
) -> Option<(Coord<f64>, CoordPos)> {
    ring.0.iter().find_map(|c| {
        let pos = against.coordinate_position(c);
        (pos != CoordPos::OnBoundary).then_some((*c, pos))
    })
}

fn hole_outside_shell(p: &Polygon<f64>, shell: &Polygon<f64>) -> Option<String> {
    if p.exterior().0.is_empty() {
        return None;
    }
    p.interiors().iter().find_map(|hole| {
        first_off_boundary(hole, shell)
            .filter(|(_, pos)| *pos == CoordPos::Outside)
            .map(|(c, _)| at("Hole lies outside shell", c))
    })
}

fn nested_hole(p: &Polygon<f64>) -> Option<String> {
    let holes: Vec<(Polygon<f64>, Option<Rect<f64>>)> = p
        .interiors()
        .iter()
        .map(|h| (Polygon::new(h.clone(), vec![]), h.bounding_rect()))
        .collect();
    for (i, inner) in p.interiors().iter().enumerate() {
        for (j, (outer, rect)) in holes.iter().enumerate() {
            let covers = |c: &Coord<f64>| {
                rect.is_some_and(|r| {
                    c.x >= r.min().x && c.x <= r.max().x && c.y >= r.min().y && c.y <= r.max().y
                })
            };
            if i == j || !inner.0.first().is_some_and(covers) {
                continue;
            }
            if let Some((c, CoordPos::Inside)) = first_off_boundary(inner, outer) {
                return Some(at("Holes are nested", c));
            }
        }
    }
    None
}

fn nested_shell(polygons: &[Polygon<f64>], shells: &[Polygon<f64>]) -> Option<String> {
    if polygons.len() < 2 {
        return None;
    }
    for (i, inner) in polygons.iter().enumerate() {
        for (j, outer) in polygons.iter().enumerate() {
            if i == j {
                continue;
            }
            let Some((c, CoordPos::Inside)) = first_off_boundary(inner.exterior(), &shells[j])
            else {
                continue;
            };
            let in_a_hole = outer.interiors().iter().any(|h| {
                Polygon::new(h.clone(), vec![]).coordinate_position(&c) == CoordPos::Inside
            });
            if !in_a_hole {
                return Some(at("Nested shells", c));
            }
        }
    }
    None
}

/// Rings of one polygon that touch in a cycle (two touches between the same rings, or a ring
/// touching the others in a loop) split its interior.
fn disconnected_interior(polygons: &[Polygon<f64>], touches: &[Touch]) -> Option<String> {
    let mut polygon_of: Vec<usize> = Vec::new();
    for (n, p) in polygons.iter().enumerate() {
        polygon_of.extend(polygon_rings(p).map(|_| n));
    }
    let mut parent: Vec<usize> = (0..polygon_of.len()).collect();
    fn root(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for t in touches {
        if polygon_of[t.rings.0] != polygon_of[t.rings.1] {
            continue;
        }
        let (a, b) = (root(&mut parent, t.rings.0), root(&mut parent, t.rings.1));
        if a == b {
            return Some(at("Interior is disconnected", t.at));
        }
        parent[a] = b;
    }
    None
}
