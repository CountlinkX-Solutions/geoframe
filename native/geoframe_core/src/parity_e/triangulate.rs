//! Delaunay triangulation, constrained Delaunay triangulation and Voronoi diagrams.
//!
//! Like GeoPandas, `delaunay_triangles` and `voronoi_polygons` treat a whole series as one
//! point cloud and return one row per triangle, cell or edge; `constrained_delaunay_triangles`
//! works row by row.
//!
//! The triangulation comes from `geo` (spade). Voronoi cells are built here instead of through
//! `geo::Voronoi`: each cell is the clip rectangle cut by the bisector against every Delaunay
//! neighbour of the site. That is exact in floating point (no boolean-operation snapping), keeps
//! one cell per site in site order, and uses the clip envelope of GEOS: the sites' envelope
//! grown by its larger side on every side, then extended to cover `extend_to`.

use std::collections::{HashMap, HashSet};

use geo::triangulate_delaunay::DelaunayTriangulationConfig;
use geo::{
    Coord, Geometry, GeometryCollection, LineString, MultiPoint, Point, Polygon, Rect, Triangle,
    TriangulateDelaunay, TriangulateDelaunayUnconstrained,
};

use super::{coord_key, empty_collection, unique_coords};

/// The distinct vertices of a geometry in first-seen order; with a `tolerance`, vertices closer
/// than that are snapped together, visiting them in lexicographic order as GEOS does (the
/// first one visited wins).
pub(super) fn sites(geom: &Geometry<f64>, tolerance: f64) -> Vec<Coord<f64>> {
    let mut unique = unique_coords(geom);
    if tolerance <= 0.0 {
        return unique;
    }
    unique.sort_by(lex);
    snap_sorted(unique, tolerance)
}

/// Largest `|coordinate| / cell` the hashed snap handles; beyond it the quotient loses the
/// resolution a cell index needs, and the plain scan is used instead.
const MAX_CELLS: f64 = 1e15;

/// Keeps each point (visited in the given order) unless a kept one is closer than
/// `tolerance`. A hash grid with cells of `2 * tolerance` limits every test to the nine
/// cells around the point, so the pass is linear for spread-out points where scanning the
/// kept list was quadratic. The distance test is the same as the scan's, so the result is
/// identical.
fn snap_sorted(points: Vec<Coord<f64>>, tolerance: f64) -> Vec<Coord<f64>> {
    let close = |k: &Coord<f64>, c: &Coord<f64>| (k.x - c.x).hypot(k.y - c.y) < tolerance;
    let cell = 2.0 * tolerance;
    let extent = points
        .iter()
        .map(|c| c.x.abs().max(c.y.abs()))
        .fold(0.0, f64::max);
    if !cell.is_finite() || extent / cell > MAX_CELLS {
        let mut kept: Vec<Coord<f64>> = Vec::new();
        for c in points {
            if !kept.iter().any(|k| close(k, &c)) {
                kept.push(c);
            }
        }
        return kept;
    }
    let index = |c: &Coord<f64>| ((c.x / cell).floor() as i64, (c.y / cell).floor() as i64);
    let mut kept: Vec<Coord<f64>> = Vec::new();
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for c in points {
        let (cx, cy) = index(&c);
        let taken = (cx - 1..=cx + 1).any(|gx| {
            (cy - 1..=cy + 1).any(|gy| {
                grid.get(&(gx, gy))
                    .is_some_and(|ids| ids.iter().any(|&i| close(&kept[i], &c)))
            })
        });
        if !taken {
            grid.entry((cx, cy)).or_default().push(kept.len());
            kept.push(c);
        }
    }
    kept
}

fn collinear(points: &[Coord<f64>]) -> bool {
    if points.len() < 3 {
        return true;
    }
    let (a, b) = (points[0], points[1]);
    points[2..]
        .iter()
        .all(|c| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x) == 0.0)
}

/// Delaunay triangles of the points; none for fewer than three or collinear points.
fn triangulate(points: &[Coord<f64>]) -> Option<Vec<Triangle<f64>>> {
    if collinear(points) {
        return Some(Vec::new());
    }
    let cloud = MultiPoint::from(points.iter().copied().map(Point::from).collect::<Vec<_>>());
    cloud.unconstrained_triangulation().ok()
}

fn triangle_polygon(t: &Triangle<f64>) -> Geometry<f64> {
    Geometry::Polygon(Polygon::new(
        LineString::new(vec![t.v1(), t.v2(), t.v3(), t.v1()]),
        vec![],
    ))
}

fn collection(triangles: &[Triangle<f64>]) -> Geometry<f64> {
    Geometry::GeometryCollection(GeometryCollection::new_from(
        triangles.iter().map(triangle_polygon).collect(),
    ))
}

fn segment(a: Coord<f64>, b: Coord<f64>) -> Geometry<f64> {
    Geometry::LineString(LineString::new(vec![a, b]))
}

fn lex(a: &Coord<f64>, b: &Coord<f64>) -> std::cmp::Ordering {
    a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y))
}

/// Unique undirected edges of the triangles; for collinear sites the chain between neighbours.
fn delaunay_edges(points: &[Coord<f64>], triangles: &[Triangle<f64>]) -> Vec<Geometry<f64>> {
    if triangles.is_empty() {
        let mut sorted = points.to_vec();
        sorted.sort_by(lex);
        return sorted.windows(2).map(|w| segment(w[0], w[1])).collect();
    }
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for t in triangles {
        for line in t.to_lines() {
            let (a, b) = (line.start, line.end);
            let key = {
                let (ka, kb) = (coord_key(a), coord_key(b));
                if ka <= kb {
                    (ka, kb)
                } else {
                    (kb, ka)
                }
            };
            if seen.insert(key) {
                edges.push(segment(a, b));
            }
        }
    }
    edges
}

/// The triangles (or, with `only_edges`, the unique edges as lines) of the Delaunay
/// triangulation of the vertices of `cloud`.
pub(super) fn delaunay_parts(
    cloud: &Geometry<f64>,
    tolerance: f64,
    only_edges: bool,
) -> Option<Vec<Geometry<f64>>> {
    let points = sites(cloud, tolerance);
    let triangles = triangulate(&points)?;
    Some(if only_edges {
        delaunay_edges(&points, &triangles)
    } else {
        triangles.iter().map(triangle_polygon).collect()
    })
}

fn polygons_in<'a>(geom: &'a Geometry<f64>, out: &mut Vec<&'a Polygon<f64>>) {
    match geom {
        Geometry::Polygon(p) => out.push(p),
        Geometry::MultiPolygon(m) => out.extend(m.0.iter()),
        Geometry::GeometryCollection(c) => c.0.iter().for_each(|g| polygons_in(g, out)),
        _ => {}
    }
}

/// Triangles that fill the polygons (holes respected), without extra vertices.
pub(super) fn constrained_delaunay_triangles(geom: &Geometry<f64>) -> Option<Geometry<f64>> {
    let mut polygons = Vec::new();
    polygons_in(geom, &mut polygons);
    let mut triangles = Vec::new();
    for polygon in polygons.into_iter().filter(|p| p.exterior().0.len() >= 4) {
        let exact = DelaunayTriangulationConfig { snap_radius: 0.0 };
        let found = polygon
            .constrained_triangulation(exact)
            .or_else(|_| polygon.constrained_triangulation(Default::default()))
            .ok()?;
        triangles.extend(found);
    }
    Some(if triangles.is_empty() {
        empty_collection()
    } else {
        collection(&triangles)
    })
}

// -- Voronoi ---------------------------------------------------------------------------

/// Keeps the part of the convex ring closer to `a` than to `b`.
fn clip_closer_to(ring: &[Coord<f64>], a: Coord<f64>, b: Coord<f64>) -> Vec<Coord<f64>> {
    let mid = Coord {
        x: (a.x + b.x) / 2.0,
        y: (a.y + b.y) / 2.0,
    };
    let toward_b = Coord {
        x: b.x - a.x,
        y: b.y - a.y,
    };
    let side = |p: Coord<f64>| (mid.x - p.x) * toward_b.x + (mid.y - p.y) * toward_b.y;
    let mut out: Vec<Coord<f64>> = Vec::with_capacity(ring.len() + 1);
    for (i, &cur) in ring.iter().enumerate() {
        let next = ring[(i + 1) % ring.len()];
        let (fc, fn_) = (side(cur), side(next));
        if fc >= 0.0 {
            out.push(cur);
        }
        if (fc > 0.0 && fn_ < 0.0) || (fc < 0.0 && fn_ > 0.0) {
            let t = fc / (fc - fn_);
            out.push(Coord {
                x: cur.x + t * (next.x - cur.x),
                y: cur.y + t * (next.y - cur.y),
            });
        }
    }
    out.dedup();
    if out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// Delaunay neighbours of each site (indices into `points`).
fn neighbours(points: &[Coord<f64>], triangles: &[Triangle<f64>]) -> Vec<Vec<usize>> {
    let index: HashMap<(u64, u64), usize> = points
        .iter()
        .enumerate()
        .map(|(i, &c)| (coord_key(c), i))
        .collect();
    let mut adjacent = vec![HashSet::new(); points.len()];
    if triangles.is_empty() {
        // Collinear sites: the neighbours along the line.
        let mut order: Vec<usize> = (0..points.len()).collect();
        order.sort_by(|&i, &j| lex(&points[i], &points[j]));
        for w in order.windows(2) {
            adjacent[w[0]].insert(w[1]);
            adjacent[w[1]].insert(w[0]);
        }
    }
    for t in triangles {
        let ids: Vec<usize> = [t.v1(), t.v2(), t.v3()]
            .iter()
            .filter_map(|&c| index.get(&coord_key(c)).copied())
            .collect();
        for &i in &ids {
            for &j in &ids {
                if i != j {
                    adjacent[i].insert(j);
                }
            }
        }
    }
    adjacent
        .into_iter()
        .map(|set| {
            let mut v: Vec<usize> = set.into_iter().collect();
            v.sort_unstable();
            v
        })
        .collect()
}

fn quantize(c: Coord<f64>) -> (i64, i64) {
    ((c.x * 1e9).round() as i64, (c.y * 1e9).round() as i64)
}

/// The cells of the Voronoi diagram of the vertices of `cloud` (or, with `only_edges`, the
/// interior sides as lines).
pub(super) fn voronoi_parts(
    cloud: &Geometry<f64>,
    tolerance: f64,
    extend_to: Option<Rect<f64>>,
    only_edges: bool,
) -> Option<Vec<Geometry<f64>>> {
    let points = sites(cloud, tolerance);
    if points.len() < 2 && (points.is_empty() || only_edges || extend_to.is_none()) {
        return Some(Vec::new());
    }
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for c in &points {
        min_x = min_x.min(c.x);
        min_y = min_y.min(c.y);
        max_x = max_x.max(c.x);
        max_y = max_y.max(c.y);
    }
    let margin = (max_x - min_x).max(max_y - min_y);
    let (mut lo_x, mut lo_y, mut hi_x, mut hi_y) = (
        min_x - margin,
        min_y - margin,
        max_x + margin,
        max_y + margin,
    );
    if let Some(r) = extend_to {
        lo_x = lo_x.min(r.min().x);
        lo_y = lo_y.min(r.min().y);
        hi_x = hi_x.max(r.max().x);
        hi_y = hi_y.max(r.max().y);
    }
    let window = [
        Coord { x: lo_x, y: lo_y },
        Coord { x: hi_x, y: lo_y },
        Coord { x: hi_x, y: hi_y },
        Coord { x: lo_x, y: hi_y },
    ];

    let triangles = triangulate(&points)?;
    let near = neighbours(&points, &triangles);
    let rings: Vec<Vec<Coord<f64>>> = points
        .iter()
        .enumerate()
        .map(|(i, &site)| {
            near[i].iter().fold(window.to_vec(), |ring, &j| {
                if ring.len() < 3 {
                    ring
                } else {
                    clip_closer_to(&ring, site, points[j])
                }
            })
        })
        .filter(|ring| ring.len() >= 3)
        .collect();

    if !only_edges {
        return Some(
            rings
                .into_iter()
                .map(|mut ring| {
                    ring.push(ring[0]);
                    Geometry::Polygon(Polygon::new(LineString::new(ring), vec![]))
                })
                .collect(),
        );
    }

    // Interior edges only: the cell sides that are not on the clip window, once each.
    let eps = 1e-9 * margin.max(1.0);
    let on_window = |a: Coord<f64>, b: Coord<f64>| {
        let same = |p: f64, q: f64, side: f64| (p - side).abs() <= eps && (q - side).abs() <= eps;
        same(a.x, b.x, lo_x) || same(a.x, b.x, hi_x) || same(a.y, b.y, lo_y) || same(a.y, b.y, hi_y)
    };
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for ring in &rings {
        for (i, &a) in ring.iter().enumerate() {
            let b = ring[(i + 1) % ring.len()];
            // Cocircular sites give sides of (numerically) zero length: drop them.
            if on_window(a, b) || (a.x - b.x).hypot(a.y - b.y) <= eps {
                continue;
            }
            let (qa, qb) = (quantize(a), quantize(b));
            let key = if qa <= qb { (qa, qb) } else { (qb, qa) };
            if seen.insert(key) {
                edges.push(segment(a, b));
            }
        }
    }
    Some(edges)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scan the hashed snap replaced: test every kept point.
    fn snap_scan(points: Vec<Coord<f64>>, tolerance: f64) -> Vec<Coord<f64>> {
        let mut kept: Vec<Coord<f64>> = Vec::new();
        for c in points {
            if !kept
                .iter()
                .any(|k| (k.x - c.x).hypot(k.y - c.y) < tolerance)
            {
                kept.push(c);
            }
        }
        kept
    }

    fn cloud(n: usize, spread: f64, seed: u64) -> Vec<Coord<f64>> {
        let mut state = seed;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut points: Vec<Coord<f64>> = (0..n)
            .map(|_| Coord {
                x: next() * spread,
                y: next() * spread,
            })
            .collect();
        points.sort_by(lex);
        points
    }

    #[test]
    fn hashed_snap_keeps_exactly_the_points_the_scan_keeps() {
        // Dense clouds (many snaps), sparse ones, negative coordinates, and a tolerance larger
        // than the whole cloud.
        for (n, spread, tolerance) in [
            (3000, 10.0, 0.05),
            (3000, 1000.0, 0.05),
            (2000, 10.0, 0.5),
            (500, 1.0, 5.0),
            (2000, 1e-3, 1e-5),
        ] {
            for shift in [0.0, -500.0, 1e6] {
                let points: Vec<Coord<f64>> = cloud(n, spread, 99)
                    .into_iter()
                    .map(|c| Coord {
                        x: c.x + shift,
                        y: c.y - shift,
                    })
                    .collect();
                assert_eq!(
                    snap_sorted(points.clone(), tolerance),
                    snap_scan(points, tolerance),
                    "n={n} spread={spread} tolerance={tolerance} shift={shift}"
                );
            }
        }
    }

    #[test]
    fn hashed_snap_matches_the_scan_at_the_tolerance_boundary() {
        // Points exactly `tolerance` apart are both kept (the test is strict), across cell edges.
        let points: Vec<Coord<f64>> = (0..50)
            .map(|i| Coord {
                x: i as f64 * 0.25,
                y: 0.0,
            })
            .collect();
        for tolerance in [0.25, 0.2500001, 0.24999, 0.5, 1.0] {
            assert_eq!(
                snap_sorted(points.clone(), tolerance),
                snap_scan(points.clone(), tolerance),
                "tolerance={tolerance}"
            );
        }
    }

    #[test]
    fn extreme_ratios_fall_back_to_the_scan() {
        let points = vec![
            Coord { x: 1e300, y: 0.0 },
            Coord { x: 1e300, y: 1.0 },
            Coord { x: 2e300, y: 0.0 },
        ];
        assert_eq!(snap_sorted(points.clone(), 0.5), snap_scan(points, 0.5));
    }
}
