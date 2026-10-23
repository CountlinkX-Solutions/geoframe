//! `concave_hull`: the GEOS/JTS `ConcaveHull` algorithm.
//!
//! The hull is built by eroding the border of the Delaunay triangulation of the input
//! vertices. Triangles on the border (exactly one boundary edge) are removed from the longest
//! boundary edge down while that edge is at least the target length and the removal keeps the
//! hull simple (the vertex opposite the boundary edge must not already lie on the boundary).
//! The target length is `ratio * (longest - shortest) + shortest` over the triangulation's
//! edges, `0` for ratio 0 (erode as much as possible) and twice the longest edge for ratio 1
//! (the convex hull). With `allow_holes`, interior triangles with a long edge are then removed
//! too, together with the long neighbours they expose, as long as they stay away from the
//! boundary.
//!
//! Where the Delaunay triangulation is not unique (cocircular vertices) or sizes tie exactly,
//! the order in which equal candidates are removed can differ from GEOS, and so can the hull.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use geo::{Contains, ConvexHull, Coord, Geometry, LineString, MultiPoint, Point, Polygon};
use geo::{MultiPolygon, TriangulateDelaunayUnconstrained};

use super::{coord_key, empty_collection, unique_coords};

/// A max-heap key: larger sizes first, then larger triangle indices first.
#[derive(Clone, Copy, PartialEq)]
struct Entry {
    size: f64,
    tri: usize,
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.size
            .total_cmp(&other.size)
            .then(self.tri.cmp(&other.tri))
    }
}

/// A boundary half-edge: its end vertex id and its two end coordinates.
type Half = (usize, Coord<f64>, Coord<f64>);

struct Hull {
    corners: Vec<[Coord<f64>; 3]>,
    vertex: Vec<[usize; 3]>,
    /// `adjacent[t][i]`: the triangle across the edge from corner `i` to corner `i + 1`.
    adjacent: Vec<[Option<usize>; 3]>,
    alive: Vec<bool>,
    /// Number of boundary edges (of live triangles) at each vertex.
    boundary_edges: Vec<u32>,
}

fn dist(a: Coord<f64>, b: Coord<f64>) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

impl Hull {
    fn new(triangles: Vec<[Coord<f64>; 3]>) -> Self {
        let mut ids: HashMap<(u64, u64), usize> = HashMap::new();
        let mut corners = Vec::with_capacity(triangles.len());
        let mut vertex = Vec::with_capacity(triangles.len());
        for [a, b, c] in triangles {
            let cross = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            let (b, c) = if cross < 0.0 { (c, b) } else { (b, c) };
            let mut ids_of = [0usize; 3];
            for (slot, p) in ids_of.iter_mut().zip([a, b, c]) {
                let next = ids.len();
                *slot = *ids.entry(coord_key(p)).or_insert(next);
            }
            corners.push([a, b, c]);
            vertex.push(ids_of);
        }
        let mut edges: HashMap<(usize, usize), usize> = HashMap::new();
        for (t, v) in vertex.iter().enumerate() {
            for i in 0..3 {
                edges.insert((v[i], v[(i + 1) % 3]), t);
            }
        }
        let adjacent: Vec<[Option<usize>; 3]> = vertex
            .iter()
            .map(|v| {
                let mut adj = [None; 3];
                for (i, slot) in adj.iter_mut().enumerate() {
                    *slot = edges.get(&(v[(i + 1) % 3], v[i])).copied();
                }
                adj
            })
            .collect();
        let mut boundary_edges = vec![0u32; ids.len()];
        for (t, v) in vertex.iter().enumerate() {
            for i in 0..3 {
                if adjacent[t][i].is_none() {
                    boundary_edges[v[i]] += 1;
                    boundary_edges[v[(i + 1) % 3]] += 1;
                }
            }
        }
        let alive = vec![true; corners.len()];
        Self {
            corners,
            vertex,
            adjacent,
            alive,
            boundary_edges,
        }
    }

    fn edge_length(&self, t: usize, i: usize) -> f64 {
        dist(self.corners[t][i], self.corners[t][(i + 1) % 3])
    }

    fn adjacent_count(&self, t: usize) -> usize {
        self.adjacent[t].iter().flatten().count()
    }

    /// Length of the boundary edges of `t` (summed, as JTS does for an ear).
    fn boundary_size(&self, t: usize) -> f64 {
        (0..3)
            .filter(|&i| self.adjacent[t][i].is_none())
            .map(|i| self.edge_length(t, i))
            .sum()
    }

    fn longest_edge(&self, t: usize) -> f64 {
        (0..3).map(|i| self.edge_length(t, i)).fold(0.0, f64::max)
    }

    /// Whether a vertex of `t` that has no boundary edge of `t` itself lies on the boundary.
    fn touches_boundary(&self, t: usize) -> bool {
        (0..3).any(|i| {
            self.adjacent[t][i].is_some()
                && self.adjacent[t][(i + 2) % 3].is_some()
                && self.boundary_edges[self.vertex[t][i]] > 0
        })
    }

    fn remove(&mut self, t: usize) {
        self.alive[t] = false;
        for i in 0..3 {
            let (a, b) = (self.vertex[t][i], self.vertex[t][(i + 1) % 3]);
            match self.adjacent[t][i].take() {
                Some(other) => {
                    for slot in self.adjacent[other].iter_mut() {
                        if *slot == Some(t) {
                            *slot = None;
                        }
                    }
                    self.boundary_edges[a] += 1;
                    self.boundary_edges[b] += 1;
                }
                None => {
                    self.boundary_edges[a] -= 1;
                    self.boundary_edges[b] -= 1;
                }
            }
        }
    }

    fn erode_border(&mut self, target: f64) {
        let mut queue = BinaryHeap::new();
        let push = |h: &Self, q: &mut BinaryHeap<Entry>, t: usize| {
            if h.alive[t] && h.adjacent_count(t) == 2 {
                q.push(Entry {
                    size: h.boundary_size(t),
                    tri: t,
                });
            }
        };
        for t in 0..self.corners.len() {
            push(self, &mut queue, t);
        }
        while let Some(Entry { size, tri }) = queue.pop() {
            if !self.alive[tri] || self.adjacent_count(tri) != 2 || size != self.boundary_size(tri)
            {
                continue;
            }
            if size < target {
                break;
            }
            if self.touches_boundary(tri) {
                continue;
            }
            let neighbours: Vec<usize> = self.adjacent[tri].iter().flatten().copied().collect();
            self.remove(tri);
            for n in neighbours {
                push(self, &mut queue, n);
            }
        }
    }

    fn carve_holes(&mut self, target: f64) {
        let mut candidates: Vec<usize> = (0..self.corners.len())
            .filter(|&t| {
                self.alive[t]
                    && self.adjacent_count(t) == 3
                    && self.longest_edge(t) >= target
                    && !self.touches_boundary(t)
            })
            .collect();
        candidates.sort_by(|&a, &b| {
            self.longest_edge(b)
                .total_cmp(&self.longest_edge(a))
                .then(b.cmp(&a))
        });
        for start in candidates {
            if !self.alive[start] || self.adjacent_count(start) != 3 || self.touches_boundary(start)
            {
                continue;
            }
            let mut queue = BinaryHeap::new();
            queue.push(Entry {
                size: self.longest_edge(start),
                tri: start,
            });
            while let Some(Entry { tri, .. }) = queue.pop() {
                if !self.alive[tri] {
                    continue;
                }
                if tri != start {
                    if self.longest_edge(tri) < target {
                        break;
                    }
                    if self.touches_boundary(tri) {
                        continue;
                    }
                }
                let neighbours: Vec<usize> = self.adjacent[tri].iter().flatten().copied().collect();
                self.remove(tri);
                for n in neighbours {
                    if self.alive[n] {
                        queue.push(Entry {
                            size: self.longest_edge(n),
                            tri: n,
                        });
                    }
                }
            }
        }
    }

    /// The live triangles as a polygon (several when the hull falls apart, holes included).
    fn to_geometry(&self) -> Geometry<f64> {
        // Boundary half-edges run counter-clockwise around shells and clockwise around holes.
        let mut outgoing: HashMap<usize, Vec<Half>> = HashMap::new();
        for t in (0..self.corners.len()).filter(|&t| self.alive[t]) {
            for i in 0..3 {
                if self.adjacent[t][i].is_none() {
                    outgoing.entry(self.vertex[t][i]).or_default().push((
                        self.vertex[t][(i + 1) % 3],
                        self.corners[t][i],
                        self.corners[t][(i + 1) % 3],
                    ));
                }
            }
        }
        let mut starts: Vec<usize> = outgoing.keys().copied().collect();
        starts.sort_unstable();
        let mut rings: Vec<Vec<Coord<f64>>> = Vec::new();
        for start in starts {
            while let Some((mut next, from, to)) = outgoing.get_mut(&start).and_then(Vec::pop) {
                let mut ring = vec![from, to];
                while next != start {
                    let Some((n, _, to)) = outgoing.get_mut(&next).and_then(Vec::pop) else {
                        break;
                    };
                    ring.push(to);
                    next = n;
                }
                if next == start && ring.len() >= 4 {
                    rings.push(ring);
                }
            }
        }
        assemble(rings)
    }
}

fn signed_area(ring: &[Coord<f64>]) -> f64 {
    ring.windows(2)
        .map(|w| w[0].x * w[1].y - w[1].x * w[0].y)
        .sum::<f64>()
        / 2.0
}

/// Shells (counter-clockwise) and the holes inside them (clockwise) as (multi)polygon.
fn assemble(rings: Vec<Vec<Coord<f64>>>) -> Geometry<f64> {
    let (shells, holes): (Vec<_>, Vec<_>) = rings.into_iter().partition(|r| signed_area(r) > 0.0);
    let mut polygons: Vec<(Polygon<f64>, f64)> = shells
        .into_iter()
        .map(|r| {
            let area = signed_area(&r);
            (Polygon::new(LineString::new(r), vec![]), area)
        })
        .collect();
    for hole in holes {
        let probe = Point::from(hole[0]);
        let owner = polygons
            .iter()
            .enumerate()
            .filter(|(_, (p, _))| p.contains(&probe) || p.exterior().0.contains(&hole[0]))
            .min_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
            .map(|(i, _)| i);
        if let Some(i) = owner {
            polygons[i].0.interiors_push(LineString::new(hole));
        }
    }
    let mut polygons: Vec<Polygon<f64>> = polygons.into_iter().map(|(p, _)| p).collect();
    match polygons.len() {
        0 => empty_collection(),
        1 => Geometry::Polygon(polygons.remove(0)),
        _ => Geometry::MultiPolygon(MultiPolygon::new(polygons)),
    }
}

/// `true` when every point lies on the line through the first two distinct points.
fn collinear(points: &[Coord<f64>]) -> bool {
    let (a, b) = (points[0], points[1]);
    points[2..]
        .iter()
        .all(|c| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x) == 0.0)
}

/// The convex hull of collinear (or fewer than three) points: a point or the segment between
/// the lexicographic extremes.
pub(super) fn degenerate_hull(points: &[Coord<f64>]) -> Geometry<f64> {
    let lex = |a: &&Coord<f64>, b: &&Coord<f64>| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y));
    match points {
        [] => empty_collection(),
        [only] => Geometry::Point(Point::from(*only)),
        _ => {
            let lo = points.iter().min_by(lex).copied().unwrap();
            let hi = points.iter().max_by(lex).copied().unwrap();
            Geometry::LineString(LineString::new(vec![lo, hi]))
        }
    }
}

pub(super) fn concave_hull(
    geom: &Geometry<f64>,
    ratio: f64,
    allow_holes: bool,
) -> Option<Geometry<f64>> {
    let points = unique_coords(geom);
    if points.len() < 3 || collinear(&points) {
        return Some(degenerate_hull(&points));
    }
    let cloud = MultiPoint::from(points.iter().copied().map(Point::from).collect::<Vec<_>>());
    let triangles = cloud.unconstrained_triangulation().ok()?;
    if triangles.is_empty() {
        return Some(Geometry::Polygon(cloud.convex_hull()));
    }
    let mut hull = Hull::new(triangles.iter().map(|t| [t.v1(), t.v2(), t.v3()]).collect());
    let (mut longest, mut shortest) = (f64::MIN, f64::MAX);
    for t in 0..hull.corners.len() {
        for i in 0..3 {
            let len = hull.edge_length(t, i);
            longest = longest.max(len);
            shortest = shortest.min(len);
        }
    }
    let target = if ratio <= 0.0 {
        0.0
    } else if ratio >= 1.0 {
        2.0 * longest
    } else {
        ratio * (longest - shortest) + shortest
    };
    hull.erode_border(target);
    if allow_holes {
        hull.carve_holes(target);
    }
    Some(hull.to_geometry())
}
