//! Batch I3 (part 2): `simplify_coverage`.
//!
//! The GEOS `CoverageSimplifier` (JTS `CoverageSimplifier`, `CoverageRingEdges` and the
//! topology-preserving Visvalingam-Whyatt `TPVWSimplifier`) as observed through Shapely 2.2
//! (GEOS 3.14):
//!
//! 1. every ring of every polygon is cut into *edges* at the *nodes* of the coverage: vertices
//!    where three or more rings meet, vertices where the outer boundary of the coverage
//!    pinches (more than two boundary segments), and vertices where a ring switches between
//!    the outer boundary and a shared edge. A ring without nodes is a *free ring*. An edge
//!    that two rings share is simplified once, and both rings get the result;
//! 2. each edge is simplified on its own with Visvalingam-Whyatt: the vertex whose triangle
//!    with its neighbours is smallest goes first, while that triangle's area is at most the
//!    squared `tolerance`. The ends of an edge never move (only a free ring may lose any
//!    vertex), a line keeps at least two vertices and a ring at least three. A vertex is only
//!    removed when no vertex of any edge (the not yet simplified ones and, when only the inner
//!    edges are simplified, the outer boundary) lies inside or on the triangle it would cut
//!    off, and when no other edge already runs between the same two neighbours. Together
//!    these keep the coverage valid;
//! 3. edges are simplified in the order their rings were met, and the polygons are rebuilt
//!    from them, each ring starting at its first node.
//!
//! Rows that are not polygons or multipolygons, null rows and rows with a non-finite
//! coordinate are null; empty polygons stay empty.
//!
//! # Work caps
//!
//! A call over 10,000,000 vertices, or whose vertex-in-triangle checks visit over 50,000,000
//! vertices, is an error.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use geo::algorithm::kernels::{Kernel, Orientation, RobustKernel};
use geo::{Coord, Geometry, LineString, MultiPolygon, Polygon};
use rstar::primitives::GeomWithData;
use rstar::RTree;

use crate::coverage::{finite, polygons_of, WORK_CAP};
use crate::noding::key;
use crate::predicates::guarded;
use crate::GeoColumn;

type C = Coord<f64>;
type PointKey = (u64, u64);
type SegKey = (PointKey, PointKey);

/// Most vertices one call accepts.
pub(crate) const MAX_VERTICES: usize = 10_000_000;

const NONE: u32 = u32::MAX;

// -- coverage edges -----------------------------------------------------------------------

/// A stretch of ring between two nodes (or a whole free ring), shared by one or two rings.
struct CoverageEdge {
    pts: Vec<C>,
    rings: u32,
    free: bool,
}

/// How one ring is made of edges: the edge ids in ring order.
type RingEdges = Vec<usize>;

fn seg_key(a: C, b: C) -> SegKey {
    let (ka, kb) = (key(a), key(b));
    if ka <= kb {
        (ka, kb)
    } else {
        (kb, ka)
    }
}

/// A ring (closed, no repeated points, at least four coordinates); `None` when it is degenerate.
fn clean_ring(ring: &LineString<f64>) -> Option<Vec<C>> {
    let mut pts: Vec<C> = Vec::with_capacity(ring.0.len());
    for &c in &ring.0 {
        if pts.last() != Some(&c) {
            pts.push(c);
        }
    }
    if pts.len() >= 2 && pts.first() != pts.last() {
        pts.push(pts[0]);
    }
    (pts.len() >= 4).then_some(pts)
}

/// The rings of a polygon: `Some(vec![])` for an empty polygon, `None` when a ring is degenerate.
fn polygon_rings(polygon: &Polygon<f64>) -> Option<Vec<Vec<C>>> {
    if polygon.exterior().0.is_empty() {
        return Some(Vec::new());
    }
    let mut rings = vec![clean_ring(polygon.exterior())?];
    for hole in polygon.interiors() {
        rings.push(clean_ring(hole)?);
    }
    Some(rings)
}

/// The nodes of the coverage: where the rings must be cut into edges.
fn find_nodes(rings: &[&Vec<C>]) -> HashSet<PointKey> {
    let mut nodes = HashSet::new();
    // vertices of three or more rings
    let mut counts: HashMap<PointKey, u32> = HashMap::new();
    for ring in rings {
        for &c in &ring[..ring.len() - 1] {
            *counts.entry(key(c)).or_insert(0) += 1;
        }
    }
    nodes.extend(counts.into_iter().filter(|&(_, n)| n >= 3).map(|(k, _)| k));
    // vertices where the outer boundary pinches, and where a ring leaves it
    let boundary = boundary_segments(rings);
    let mut ends: HashMap<PointKey, u32> = HashMap::new();
    for &(a, b) in &boundary {
        *ends.entry(a).or_insert(0) += 1;
        *ends.entry(b).or_insert(0) += 1;
    }
    nodes.extend(ends.into_iter().filter(|&(_, n)| n > 2).map(|(k, _)| k));
    for ring in rings {
        let m = ring.len() - 1;
        let on_boundary = |i: usize| boundary.contains(&seg_key(ring[i], ring[i + 1]));
        let mut previous = on_boundary(m - 1);
        for (i, &c) in ring[..m].iter().enumerate() {
            let flag = on_boundary(i);
            if flag != previous {
                nodes.insert(key(c));
            }
            previous = flag;
        }
    }
    nodes
}

/// Segments that occur an odd number of times in the coverage: its outer boundary.
fn boundary_segments(rings: &[&Vec<C>]) -> HashSet<SegKey> {
    let mut set = HashSet::new();
    for ring in rings {
        for w in ring.windows(2) {
            let k = seg_key(w[0], w[1]);
            if !set.remove(&k) {
                set.insert(k);
            }
        }
    }
    set
}

/// Identity of an edge independent of its direction.
fn edge_key(pts: &[C], free: bool) -> (SegKey, usize) {
    let first = seg_key(pts[0], pts[1]);
    let last = seg_key(pts[pts.len() - 2], pts[pts.len() - 1]);
    let k = if free {
        pts.windows(2)
            .map(|w| seg_key(w[0], w[1]))
            .min()
            .unwrap_or(first)
    } else {
        first.min(last)
    };
    (k, pts.len())
}

/// Cuts all rings into edges. Returns the edges (in the order they were met) and, per ring,
/// the edges it is made of.
fn build_edges(rings: &[&Vec<C>]) -> (Vec<CoverageEdge>, Vec<RingEdges>) {
    let nodes = find_nodes(rings);
    let mut edges: Vec<CoverageEdge> = Vec::new();
    let mut by_key: HashMap<(SegKey, usize), usize> = HashMap::new();
    let mut ring_edges = Vec::with_capacity(rings.len());
    for ring in rings {
        let m = ring.len() - 1;
        let at_nodes: Vec<usize> = (0..m).filter(|&i| nodes.contains(&key(ring[i]))).collect();
        let mut pieces: Vec<(Vec<C>, bool)> = Vec::new();
        if at_nodes.is_empty() {
            pieces.push((ring.to_vec(), true));
        } else {
            for (n, &a) in at_nodes.iter().enumerate() {
                let b = at_nodes[(n + 1) % at_nodes.len()];
                let len = (b + m - a) % m;
                let len = if len == 0 { m } else { len };
                let pts: Vec<C> = (0..=len).map(|k| ring[(a + k) % m]).collect();
                pieces.push((pts, false));
            }
        }
        let mut used = RingEdges::new();
        for (pts, free) in pieces {
            let k = edge_key(&pts, free);
            match by_key.get(&k) {
                Some(&id) => {
                    edges[id].rings += 1;
                    used.push(id);
                }
                None => {
                    by_key.insert(k, edges.len());
                    used.push(edges.len());
                    edges.push(CoverageEdge {
                        pts,
                        rings: 1,
                        free,
                    });
                }
            }
        }
        ring_edges.push(used);
    }
    (edges, ring_edges)
}

/// A ring made of its edges again. The first edge is taken as stored (so a ring may come out
/// reversed, and starting at another node, when a neighbour met the edge first); every next
/// edge is turned to continue from where the ring has got to.
fn rebuild_ring(used: &RingEdges, edges: &[CoverageEdge]) -> LineString<f64> {
    let mut pts: Vec<C> = Vec::new();
    for &id in used {
        let edge = &edges[id].pts;
        let forward = match pts.last() {
            Some(last) => edge[0] == *last,
            // The first edge runs as stored unless that leads away from the next one.
            None => used.get(1).is_none_or(|&next| {
                let end = edge[edge.len() - 1];
                let next = &edges[next].pts;
                end == next[0] || end == next[next.len() - 1]
            }),
        };
        let mut part: Vec<C> = edge.clone();
        if !forward {
            part.reverse();
        }
        let skip = usize::from(!pts.is_empty());
        pts.extend(part.into_iter().skip(skip));
    }
    LineString::new(pts)
}

// -- topology-preserving Visvalingam-Whyatt -----------------------------------------------

/// A line being simplified: a doubly linked list over its vertices.
struct Line {
    pts: Vec<C>,
    prev: Vec<u32>,
    next: Vec<u32>,
    removed: Vec<bool>,
    size: usize,
    /// Closed: the last vertex repeats the first and is not stored in the list.
    ring: bool,
    /// A free ring may lose any vertex; the ends of every other line stay.
    free: bool,
}

impl Line {
    fn new(pts: &[C], free: bool) -> Self {
        let ring = pts.len() >= 4 && pts.first() == pts.last();
        let n = if ring { pts.len() - 1 } else { pts.len() };
        let prev = (0..n)
            .map(|i| match (i, ring) {
                (0, true) => (n - 1) as u32,
                (0, false) => NONE,
                _ => (i - 1) as u32,
            })
            .collect();
        let next = (0..n)
            .map(|i| {
                if i + 1 < n {
                    (i + 1) as u32
                } else if ring {
                    0
                } else {
                    NONE
                }
            })
            .collect();
        Self {
            pts: pts[..n].to_vec(),
            prev,
            next,
            removed: vec![false; n],
            size: n,
            ring,
            free: ring && free,
        }
    }

    fn min_size(&self) -> usize {
        if self.ring {
            3
        } else {
            2
        }
    }

    /// May the vertex `i` ever be removed?
    fn removable_vertex(&self, i: usize) -> bool {
        self.free || (i != 0 && (self.ring || i != self.pts.len() - 1))
    }

    fn coordinates(&self) -> Vec<C> {
        let mut out: Vec<C> = (0..self.pts.len())
            .filter(|&i| !self.removed[i])
            .map(|i| self.pts[i])
            .collect();
        if self.ring {
            out.push(out[0]);
        }
        out
    }
}

#[derive(Clone, Copy)]
struct Corner {
    area: f64,
    index: u32,
    prev: u32,
    next: u32,
}

impl PartialEq for Corner {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Corner {}
impl PartialOrd for Corner {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Corner {
    /// The smallest area first (a max-heap, so reversed), the lowest index on ties.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .area
            .total_cmp(&self.area)
            .then(other.index.cmp(&self.index))
    }
}

/// `Triangle.area` of JTS.
fn triangle_area(a: C, b: C, c: C) -> f64 {
    (((c.x - a.x) * (b.y - a.y) - (b.x - a.x) * (c.y - a.y)) / 2.0).abs()
}

/// `Triangle.intersects` of JTS: `p` is inside the triangle or on its boundary.
fn triangle_intersects(a: C, b: C, c: C, p: C) -> bool {
    let exterior = if RobustKernel::orient2d(a, b, c) == Orientation::CounterClockwise {
        Orientation::Clockwise
    } else {
        Orientation::CounterClockwise
    };
    ![(a, b), (b, c), (c, a)]
        .iter()
        .any(|&(u, v)| RobustKernel::orient2d(u, v, p) == exterior)
}

type VertexEntry = GeomWithData<[f64; 2], (u32, u32)>;

struct Tpvw {
    lines: Vec<Line>,
    index: RTree<VertexEntry>,
    area_tolerance: f64,
    work: u64,
    cap: u64,
}

impl Tpvw {
    fn corner(&self, li: usize, i: u32) -> Corner {
        let line = &self.lines[li];
        let (p, n) = (line.prev[i as usize], line.next[i as usize]);
        Corner {
            area: triangle_area(
                line.pts[p as usize],
                line.pts[i as usize],
                line.pts[n as usize],
            ),
            index: i,
            prev: p,
            next: n,
        }
    }

    /// Whether the vertex of `corner` can go without breaking the coverage.
    fn removable(&mut self, li: usize, corner: &Corner) -> Result<bool, String> {
        let line = &self.lines[li];
        let p0 = line.pts[corner.prev as usize];
        let p1 = line.pts[corner.index as usize];
        let p2 = line.pts[corner.next as usize];
        let env = rstar::AABB::from_corners(
            [p0.x.min(p1.x).min(p2.x), p0.y.min(p1.y).min(p2.y)],
            [p0.x.max(p1.x).max(p2.x), p0.y.max(p1.y).max(p2.y)],
        );
        let mut visited = 0u64;
        let mut blocked = false;
        for entry in self.index.locate_in_envelope_intersecting(&env) {
            visited += 1;
            let q = Coord {
                x: entry.geom()[0],
                y: entry.geom()[1],
            };
            if q == p0 || q == p1 || q == p2 {
                // Another edge that already runs between the two neighbours would be doubled.
                let (other, _) = entry.data;
                if other as usize != li && (q == p0 || q == p2) {
                    let o = &self.lines[other as usize];
                    if o.size == 2 && !o.ring {
                        let ends = (o.pts[0], o.pts[o.pts.len() - 1]);
                        if ends == (p0, p2) || ends == (p2, p0) {
                            blocked = true;
                            break;
                        }
                    }
                }
                continue;
            }
            if triangle_intersects(p0, p1, p2, q) {
                blocked = true;
                break;
            }
        }
        self.work += visited;
        if self.work > self.cap {
            return Err(format!(
                "the coverage simplification needs more than {} vertex checks",
                self.cap
            ));
        }
        Ok(!blocked)
    }

    fn simplify(&mut self, li: usize) -> Result<(), String> {
        let n = self.lines[li].pts.len();
        let mut queue: BinaryHeap<Corner> = BinaryHeap::new();
        for i in 0..n {
            if self.lines[li].removable_vertex(i) {
                queue.push(self.corner(li, i as u32));
            }
        }
        while let Some(corner) = queue.pop() {
            let line = &self.lines[li];
            if line.size <= line.min_size() || corner.area > self.area_tolerance {
                break;
            }
            let i = corner.index as usize;
            if line.removed[i] || line.prev[i] != corner.prev || line.next[i] != corner.next {
                continue;
            }
            if !self.removable(li, &corner)? {
                continue;
            }
            let at = self.lines[li].pts[i];
            let line = &mut self.lines[li];
            line.removed[i] = true;
            line.size -= 1;
            line.next[corner.prev as usize] = corner.next;
            line.prev[corner.next as usize] = corner.prev;
            self.index
                .remove(&VertexEntry::new([at.x, at.y], (li as u32, corner.index)));
            for neighbour in [corner.prev, corner.next] {
                if self.lines[li].removable_vertex(neighbour as usize) {
                    queue.push(self.corner(li, neighbour));
                }
            }
        }
        Ok(())
    }
}

/// Simplifies `lines` (`(coordinates, is_free_ring)`), with `constraints` as fixed obstacles.
fn simplify_lines(
    lines: &[(&[C], bool)],
    constraints: &[&[C]],
    tolerance: f64,
    cap: u64,
) -> Result<Vec<Vec<C>>, String> {
    let mut all: Vec<Line> = lines
        .iter()
        .map(|&(pts, free)| Line::new(pts, free))
        .collect();
    all.extend(constraints.iter().map(|&pts| Line::new(pts, false)));
    let mut entries = Vec::new();
    for (li, line) in all.iter().enumerate() {
        for (i, c) in line.pts.iter().enumerate() {
            entries.push(VertexEntry::new([c.x, c.y], (li as u32, i as u32)));
        }
    }
    let mut tpvw = Tpvw {
        lines: all,
        index: RTree::bulk_load(entries),
        area_tolerance: tolerance * tolerance,
        work: 0,
        cap,
    };
    for li in 0..lines.len() {
        tpvw.simplify(li)?;
    }
    Ok(tpvw.lines[..lines.len()]
        .iter()
        .map(Line::coordinates)
        .collect())
}

// -- the column operation -----------------------------------------------------------------

/// How a row takes part.
enum Row {
    Null,
    /// Kept as it is (an empty polygon, or one with a degenerate ring).
    Keep(Geometry<f64>),
    /// Polygons rebuilt from rings `first..first + count` of the coverage, in `parts` (one per polygon).
    Rebuilt {
        multi: bool,
        parts: Vec<Vec<usize>>,
    },
}

fn has_non_finite(g: &Geometry<f64>) -> bool {
    geo::CoordsIter::coords_iter(g).any(|c| !c.x.is_finite() || !c.y.is_finite())
}

impl GeoColumn {
    /// Simplifies the shared edges of the polygons of the column, keeping them a coverage.
    ///
    /// `tolerance` is roughly the square root of the area of the triangles removed. With
    /// `simplify_boundary` false only the edges two polygons share change; the outer boundary
    /// of the coverage stays. Rows that are not polygons, null rows and rows with non-finite
    /// coordinates are null.
    pub fn simplify_coverage(
        &self,
        tolerance: f64,
        simplify_boundary: bool,
    ) -> Result<GeoColumn, String> {
        self.simplify_coverage_capped(tolerance, simplify_boundary, MAX_VERTICES, WORK_CAP)
    }

    /// [`simplify_coverage`](Self::simplify_coverage) with explicit caps on the vertices of the
    /// coverage and on the vertices its triangle checks visit.
    pub(crate) fn simplify_coverage_capped(
        &self,
        tolerance: f64,
        simplify_boundary: bool,
        max_vertices: usize,
        work_cap: u64,
    ) -> Result<GeoColumn, String> {
        let tolerance = finite(tolerance, "tolerance")?;
        let stored = self.map_scalars(|_| ());
        let geo = self.geo_rows();
        let outcome = guarded(|| -> Result<Vec<Option<Geometry<f64>>>, String> {
            let mut rings: Vec<Vec<C>> = Vec::new();
            let mut rows: Vec<Row> = Vec::new();
            let mut vertices = 0usize;
            for (s, g) in stored.iter().zip(&geo) {
                let Some(g) = g.as_ref().filter(|g| s.is_some() && !has_non_finite(g)) else {
                    rows.push(Row::Null);
                    continue;
                };
                let Some(polys) = polygons_of(g) else {
                    rows.push(Row::Null);
                    continue;
                };
                let cleaned: Option<Vec<Vec<Vec<C>>>> =
                    polys.iter().map(|p| polygon_rings(p)).collect();
                match cleaned {
                    Some(cleaned) if cleaned.iter().all(|r| !r.is_empty()) => {
                        let mut parts = Vec::new();
                        for poly_rings in cleaned {
                            let mut ids = Vec::new();
                            for ring in poly_rings {
                                vertices += ring.len();
                                ids.push(rings.len());
                                rings.push(ring);
                            }
                            parts.push(ids);
                        }
                        if vertices > max_vertices {
                            return Err(format!(
                                "the coverage has more than {max_vertices} vertices"
                            ));
                        }
                        rows.push(Row::Rebuilt {
                            multi: matches!(g, Geometry::MultiPolygon(_)),
                            parts,
                        });
                    }
                    _ => rows.push(Row::Keep(g.clone())),
                }
            }

            let ring_refs: Vec<&Vec<C>> = rings.iter().collect();
            let (mut edges, ring_edges) = build_edges(&ring_refs);
            let chosen: Vec<usize> = (0..edges.len())
                .filter(|&i| simplify_boundary || edges[i].rings == 2)
                .collect();
            let constraints: Vec<usize> = if simplify_boundary {
                Vec::new()
            } else {
                (0..edges.len()).filter(|&i| edges[i].rings == 1).collect()
            };
            let simplified = simplify_lines(
                &chosen
                    .iter()
                    .map(|&i| (edges[i].pts.as_slice(), edges[i].free))
                    .collect::<Vec<_>>(),
                &constraints
                    .iter()
                    .map(|&i| edges[i].pts.as_slice())
                    .collect::<Vec<_>>(),
                tolerance,
                work_cap,
            )?;
            for (&i, pts) in chosen.iter().zip(simplified) {
                edges[i].pts = pts;
            }

            let polygon = |ids: &[usize]| {
                let mut built = ids.iter().map(|&r| rebuild_ring(&ring_edges[r], &edges));
                let shell = built.next().expect("a shell");
                Polygon::new(shell, built.collect())
            };
            Ok(rows
                .into_iter()
                .map(|row| match row {
                    Row::Null => None,
                    Row::Keep(g) => Some(g),
                    Row::Rebuilt { multi, parts } => {
                        let mut polys: Vec<Polygon<f64>> =
                            parts.iter().map(|ids| polygon(ids)).collect();
                        Some(if multi {
                            Geometry::MultiPolygon(MultiPolygon::new(polys))
                        } else {
                            Geometry::Polygon(polys.remove(0))
                        })
                    }
                })
                .collect())
        })
        .unwrap_or_else(|| {
            Err("the coverage simplification failed on degenerate input".to_string())
        })?;
        self.rebuild(&outcome)
    }
}
