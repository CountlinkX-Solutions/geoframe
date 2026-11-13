//! GEOS' view of a zero-area polygon ring. `geo` accepts a ring whose vertices are all
//! collinear; GEOS reports it as a self-intersection (or too few points) and `make_valid`
//! collapses it to its linework. These helpers add that rule on top of `geo`'s validation.

use geo::algorithm::kernels::{Kernel, Orientation, RobustKernel};
use geo::{
    Coord, Geometry, GeometryCollection, LineString, MultiLineString, MultiPoint, MultiPolygon,
    Point, Polygon, Validation,
};

/// `geo`'s validity, plus: no polygon ring is collinear (zero area) and no polygon's holes
/// disconnect its interior (GEOS' `Interior is disconnected`, which `geo` accepts).
pub(crate) fn is_valid(geom: &Geometry<f64>) -> bool {
    geom.is_valid()
        && collapsed_reason(geom).is_none()
        && crate::validity_reason::disconnected_reason(geom).is_none()
}

/// GEOS-style reason for the first collapsed ring of `geom`, if any.
pub(crate) fn collapsed_reason(geom: &Geometry<f64>) -> Option<String> {
    match geom {
        Geometry::Polygon(p) => polygon_reason(p),
        Geometry::MultiPolygon(mp) => mp.0.iter().find_map(polygon_reason),
        Geometry::GeometryCollection(gc) => gc.0.iter().find_map(collapsed_reason),
        _ => None,
    }
}

fn polygon_reason(p: &Polygon<f64>) -> Option<String> {
    std::iter::once(p.exterior())
        .chain(p.interiors())
        .find(|ring| is_collapsed(ring))
        .map(|ring| {
            let distinct = distinct_coords(ring);
            if distinct.len() < 3 {
                let c = ring.0[0];
                format!("Too few points in geometry component[{} {}]", c.x, c.y)
            } else {
                let c = ring.0[1];
                format!("Self-intersection[{} {}]", c.x, c.y)
            }
        })
}

/// A non-empty ring whose vertices all lie on one line (or on one point).
fn is_collapsed(ring: &LineString<f64>) -> bool {
    let Some(first) = ring.0.first().copied() else {
        return false;
    };
    let Some(second) = ring.0.iter().copied().find(|c| *c != first) else {
        return true;
    };
    ring.0
        .iter()
        .all(|c| RobustKernel::orient2d(first, second, *c) == Orientation::Collinear)
}

fn distinct_coords(ring: &LineString<f64>) -> Vec<Coord<f64>> {
    let mut out: Vec<Coord<f64>> = Vec::new();
    for c in &ring.0 {
        if !out.contains(c) {
            out.push(*c);
        }
    }
    out
}

/// The unique vertices of a collapsed ring in the direction of its first segment; one point
/// when every vertex is equal.
fn linework(ring: &LineString<f64>) -> Vec<Coord<f64>> {
    let mut pts = distinct_coords(ring);
    if let [a, b, ..] = pts[..] {
        let by_x = (b.x - a.x).abs() >= (b.y - a.y).abs();
        let sign = if by_x { b.x - a.x } else { b.y - a.y };
        let key = |c: &Coord<f64>| if by_x { c.x } else { c.y };
        pts.sort_by(|p, q| key(p).total_cmp(&key(q)));
        if sign < 0.0 {
            pts.reverse();
        }
    }
    pts
}

/// `make_valid` for polygons with a collapsed ring, as GEOS: a collapsed shell becomes its
/// linework, a collapsed hole is dropped from its polygon and kept as linework, the other
/// polygons stay. `None` when no ring is collapsed, or when a collapsed shell has holes
/// (GEOS then builds a polygon from the holes: left to the caller, unchanged).
pub(crate) fn repair_collapsed(
    polygons: &[Polygon<f64>],
    repair: impl Fn(MultiPolygon<f64>) -> Option<Geometry<f64>>,
) -> Option<Geometry<f64>> {
    if !polygons.iter().any(|p| polygon_reason(p).is_some()) {
        return None;
    }
    if polygons
        .iter()
        .any(|p| is_collapsed(p.exterior()) && !p.interiors().is_empty())
    {
        return None;
    }
    let mut kept: Vec<Polygon<f64>> = Vec::new();
    let mut collapsed: Vec<Vec<Coord<f64>>> = Vec::new();
    for p in polygons {
        if is_collapsed(p.exterior()) {
            collapsed.push(linework(p.exterior()));
            continue;
        }
        let (bad, good): (Vec<_>, Vec<_>) = p.interiors().iter().cloned().partition(is_collapsed);
        collapsed.extend(bad.iter().map(linework));
        kept.push(Polygon::new(p.exterior().clone(), good));
    }
    let mut members: Vec<Geometry<f64>> = Vec::new();
    if !kept.is_empty() {
        let multi = MultiPolygon::new(kept);
        members.push(if is_valid(&Geometry::MultiPolygon(multi.clone())) {
            crate::unary::collapse(multi)
        } else {
            repair(multi)?
        });
    }
    let segments: Vec<LineString<f64>> = collapsed
        .iter()
        .flat_map(|pts| pts.windows(2).map(|w| LineString::new(w.to_vec())))
        .collect();
    match segments.len() {
        0 => {}
        1 => members.push(Geometry::LineString(segments[0].clone())),
        _ => members.push(Geometry::MultiLineString(MultiLineString::new(segments))),
    }
    let points: Vec<Point<f64>> = collapsed
        .iter()
        .filter(|pts| pts.len() == 1)
        .map(|pts| Point::from(pts[0]))
        .collect();
    match points.len() {
        0 => {}
        1 => members.push(Geometry::Point(points[0])),
        _ => members.push(Geometry::MultiPoint(MultiPoint::new(points))),
    }
    Some(if members.len() == 1 {
        members.remove(0)
    } else {
        Geometry::GeometryCollection(GeometryCollection::new_from(members))
    })
}
