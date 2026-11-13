//! `polygonize` and `build_area`: polygons from linework (GeoPandas `GeoSeries.polygonize` and
//! `GeoSeries.build_area`, GEOS `Polygonizer` and `BuildArea`).
//!
//! All non-null rows are used together, as GeoPandas does. With `node = true` the polygons of
//! the input are merged first, the lines inside them dropped (a union), and all linework is
//! noded: split wherever lines cross or touch, duplicates removed. Lines split at the ends of
//! the input lines too, as in a GEOS union. With `node = false` every line is taken as one
//! edge between its first and last vertex, so crossings that are not vertices of both lines
//! are not nodes.
//!
//! The edges form a planar graph, which is processed like the GEOS polygonizer:
//!
//! 1. *dangles*: edges ending in a node of degree one are removed, repeatedly;
//! 2. *cut edges*: edges with the same face on both sides (they join two parts of the graph)
//!    are removed;
//! 3. the faces of what is left are walked (each directed edge continues with the next edge
//!    clockwise from its reverse), a walk that passes a node twice is split there, and every
//!    resulting ring that is not simple is an *invalid ring*;
//! 4. counter-clockwise rings are shells and clockwise rings are holes of the smallest shell
//!    that contains them (holes inside no shell are the outside of their part and are
//!    dropped). Every shell gives a polygon, also when it lies in another polygon's hole.
//!
//! Shells are output clockwise and holes counter-clockwise, as GEOS does. Which ring vertex
//! comes first and the order of the polygons differ from GEOS; the polygons themselves agree.
//! As in GEOS, a ring of the merged area is one edge (a node only at its start) and the points
//! where a line crosses it are vertices of it, so a line piece that ends at an inner vertex of a
//! ring is a dangle.
//! Where input polygons are merged first (`node = true`) every input vertex that lies on the
//! merged outline stays, collinear ones included, as in a GEOS union (see
//! `keep_input_vertices`).
//!
//! `build_area` makes the faces even-odd: the symmetric difference of the filled shells, so
//! a ring inside a ring is a hole, a ring inside that an island again, and polygons that share
//! an edge merge.
//!
//! The noding looks at pairs of segments found with an R-tree and gives up (an error) after
//! 50,000,000 candidate pairs.

use std::collections::{HashMap, HashSet};

use geo::algorithm::coordinate_position::{CoordPos, CoordinatePosition};
use geo::line_intersection::{line_intersection, LineIntersection};
use geo::orient::{Direction, Orient};
use geo::{
    unary_union, BooleanOps, Coord, Geometry, Line, LineString, MultiLineString, MultiPolygon,
    Polygon, Validation,
};
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{Envelope, RTree, AABB};

use crate::mixed_ops::line_outside_polygon;
use crate::noding::key;
use crate::predicates::guarded;
use crate::GeoColumn;

/// Most candidate segment pairs the noding examines.
const PAIR_BUDGET: u64 = 50_000_000;

type C = Coord<f64>;
type Key = (u64, u64);

/// The four results of [`GeoColumn::polygonize`], one geometry per row.
pub struct Polygonized {
    pub polygons: GeoColumn,
    pub cuts: GeoColumn,
    pub dangles: GeoColumn,
    pub invalid: GeoColumn,
}

// -- linework -----------------------------------------------------------------------------

#[derive(Default)]
struct Linework {
    polygons: Vec<Polygon<f64>>,
    lines: Vec<LineString<f64>>,
}

fn gather(g: &Geometry<f64>, out: &mut Linework) {
    match g {
        Geometry::LineString(l) => out.lines.push(l.clone()),
        Geometry::Line(l) => out.lines.push(LineString::from(*l)),
        Geometry::MultiLineString(m) => out.lines.extend(m.0.iter().cloned()),
        Geometry::Polygon(p) => out.polygons.push(p.clone()),
        Geometry::MultiPolygon(m) => out.polygons.extend(m.0.iter().cloned()),
        Geometry::Rect(r) => out.polygons.push(r.to_polygon()),
        Geometry::Triangle(t) => out.polygons.push(t.to_polygon()),
        Geometry::GeometryCollection(c) => c.0.iter().for_each(|m| gather(m, out)),
        Geometry::Point(_) | Geometry::MultiPoint(_) => {}
    }
}

fn rings(p: &Polygon<f64>) -> impl Iterator<Item = &LineString<f64>> {
    std::iter::once(p.exterior()).chain(p.interiors())
}

/// Consecutive repeated vertices removed.
fn dedup(coords: &[C]) -> Vec<C> {
    let mut out: Vec<C> = Vec::with_capacity(coords.len());
    for &c in coords {
        if out.last() != Some(&c) {
            out.push(c);
        }
    }
    out
}

fn finite(c: &C) -> bool {
    c.x.is_finite() && c.y.is_finite()
}

// -- noding -------------------------------------------------------------------------------

fn envelope(a: C, b: C) -> AABB<[f64; 2]> {
    AABB::from_corners([a.x.min(b.x), a.y.min(b.y)], [a.x.max(b.x), a.y.max(b.y)])
}

/// Position of `c` along the segment `a`-`b` as a fraction of its length.
fn fraction(a: C, b: C, c: C) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    ((c.x - a.x) * dx + (c.y - a.y) * dy) / (dx * dx + dy * dy)
}

/// Splits `lines` at every point where they cross or touch (each other or themselves) and at
/// their own ends; identical pieces appear once. Returns the edges (chains of vertices between
/// nodes).
pub(crate) fn node_lines(lines: &[LineString<f64>]) -> Result<Vec<Vec<C>>, String> {
    struct Seg {
        a: C,
        b: C,
        line: usize,
        index: usize,
    }
    let cleaned: Vec<Vec<C>> = lines
        .iter()
        .map(|l| dedup(&l.0))
        .filter(|c| c.len() >= 2)
        .collect();
    let mut segs: Vec<Seg> = Vec::new();
    for (line, coords) in cleaned.iter().enumerate() {
        for (index, w) in coords.windows(2).enumerate() {
            segs.push(Seg {
                a: w[0],
                b: w[1],
                line,
                index,
            });
        }
    }
    let tree = RTree::bulk_load(
        segs.iter()
            .enumerate()
            .map(|(i, s)| GeomWithData::new(Rectangle::from_aabb(envelope(s.a, s.b)), i))
            .collect(),
    );
    let mut cuts: Vec<Vec<C>> = vec![Vec::new(); segs.len()];
    let mut nodes: HashSet<Key> = HashSet::new();
    let mut pairs = 0_u64;
    for (i, s) in segs.iter().enumerate() {
        for hit in tree.locate_in_envelope_intersecting(&envelope(s.a, s.b)) {
            let j = hit.data;
            if j <= i {
                continue;
            }
            pairs += 1;
            if pairs > PAIR_BUDGET {
                return Err(format!(
                    "polygonize: noding needs more than {PAIR_BUDGET} segment pairs"
                ));
            }
            let t = &segs[j];
            let Some(hit) = line_intersection(Line::new(s.a, s.b), Line::new(t.a, t.b)) else {
                continue;
            };
            let consecutive = s.line == t.line && t.index == s.index + 1;
            match hit {
                LineIntersection::SinglePoint { intersection, .. } => {
                    // The shared vertex of consecutive segments is not a node.
                    if consecutive && intersection == s.b {
                        continue;
                    }
                    cuts[i].push(intersection);
                    cuts[j].push(intersection);
                    nodes.insert(key(intersection));
                }
                LineIntersection::Collinear { intersection } => {
                    for c in [intersection.start, intersection.end] {
                        cuts[i].push(c);
                        cuts[j].push(c);
                        nodes.insert(key(c));
                    }
                }
            }
        }
    }
    // Rebuild every line with its cut points inserted, then break it at nodes.
    let mut edges: Vec<Vec<C>> = Vec::new();
    let mut seen: HashSet<Vec<Key>> = HashSet::new();
    let mut at = 0;
    for coords in &cleaned {
        let mut full: Vec<C> = vec![coords[0]];
        for w in coords.windows(2) {
            let (a, b) = (w[0], w[1]);
            let mut on: Vec<C> = std::mem::take(&mut cuts[at])
                .into_iter()
                .filter(|&c| c != a && c != b)
                .collect();
            on.sort_by(|p, q| fraction(a, b, *p).total_cmp(&fraction(a, b, *q)));
            on.dedup();
            full.extend(on);
            full.push(b);
            at += 1;
        }
        let ends = (key(full[0]), key(full[full.len() - 1]));
        let mut chain: Vec<C> = vec![full[0]];
        for (k, &c) in full.iter().enumerate().skip(1) {
            chain.push(c);
            let is_end = k == full.len() - 1;
            if is_end || nodes.contains(&key(c)) {
                push_edge(&mut edges, &mut seen, std::mem::take(&mut chain));
                chain = vec![c];
            }
        }
        let _ = ends;
    }
    Ok(edges)
}

/// Adds an edge unless the same chain (either way round) is already there.
fn push_edge(edges: &mut Vec<Vec<C>>, seen: &mut HashSet<Vec<Key>>, chain: Vec<C>) {
    if chain.len() < 2 {
        return;
    }
    let forward: Vec<Key> = chain.iter().map(|c| key(*c)).collect();
    let backward: Vec<Key> = forward.iter().rev().copied().collect();
    if seen.insert(forward.clone().min(backward)) {
        edges.push(chain);
    }
}

// -- graph --------------------------------------------------------------------------------

struct Edge {
    coords: Vec<C>,
    from: usize,
    to: usize,
}

struct Graph {
    edges: Vec<Edge>,
    /// Coordinates of the nodes.
    nodes: Vec<C>,
}

impl Graph {
    fn new(chains: Vec<Vec<C>>) -> Self {
        let mut index: HashMap<Key, usize> = HashMap::new();
        let mut nodes: Vec<C> = Vec::new();
        let mut node_of = |c: C, nodes: &mut Vec<C>| {
            *index.entry(key(c)).or_insert_with(|| {
                nodes.push(c);
                nodes.len() - 1
            })
        };
        let mut edges = Vec::new();
        for coords in chains {
            let from = node_of(coords[0], &mut nodes);
            let to = node_of(coords[coords.len() - 1], &mut nodes);
            edges.push(Edge { coords, from, to });
        }
        Self { edges, nodes }
    }

    /// The directed edge `d` (edge `d / 2`, forward when `d` is even) as (start, end) nodes.
    fn ends(&self, d: usize) -> (usize, usize) {
        let e = &self.edges[d / 2];
        if d.is_multiple_of(2) {
            (e.from, e.to)
        } else {
            (e.to, e.from)
        }
    }

    /// The first step of directed edge `d` away from its start.
    fn direction(&self, d: usize) -> C {
        let c = &self.edges[d / 2].coords;
        if d.is_multiple_of(2) {
            Coord {
                x: c[1].x - c[0].x,
                y: c[1].y - c[0].y,
            }
        } else {
            let n = c.len();
            Coord {
                x: c[n - 2].x - c[n - 1].x,
                y: c[n - 2].y - c[n - 1].y,
            }
        }
    }

    fn coords(&self, d: usize) -> Vec<C> {
        let c = &self.edges[d / 2].coords;
        if d.is_multiple_of(2) {
            c.clone()
        } else {
            c.iter().rev().copied().collect()
        }
    }
}

/// Counter-clockwise order of directions by angle from the positive x axis.
fn cmp_angle(a: C, b: C) -> std::cmp::Ordering {
    let half = |v: C| u8::from(!(v.y > 0.0 || (v.y == 0.0 && v.x > 0.0)));
    half(a)
        .cmp(&half(b))
        .then_with(|| (a.x * b.y - a.y * b.x).total_cmp(&0.0).reverse())
}

/// The walks around the faces of the graph restricted to `alive` edges: each directed edge
/// belongs to exactly one walk (a list of directed edges).
fn face_walks(graph: &Graph, alive: &[bool]) -> Vec<Vec<usize>> {
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); graph.nodes.len()];
    for (e, _) in graph.edges.iter().enumerate().filter(|(e, _)| alive[*e]) {
        for d in [2 * e, 2 * e + 1] {
            outgoing[graph.ends(d).0].push(d);
        }
    }
    for list in &mut outgoing {
        list.sort_by(|&p, &q| cmp_angle(graph.direction(p), graph.direction(q)));
    }
    let mut position: HashMap<usize, usize> = HashMap::new();
    for list in &outgoing {
        for (i, &d) in list.iter().enumerate() {
            position.insert(d, i);
        }
    }
    let next = |d: usize| -> usize {
        let at = graph.ends(d).1;
        let list = &outgoing[at];
        let reverse = d ^ 1;
        list[(position[&reverse] + list.len() - 1) % list.len()]
    };
    let mut done: HashSet<usize> = HashSet::new();
    let mut walks = Vec::new();
    for list in &outgoing {
        for &start in list {
            if done.contains(&start) {
                continue;
            }
            let mut walk = Vec::new();
            let mut d = start;
            while done.insert(d) {
                walk.push(d);
                d = next(d);
            }
            walks.push(walk);
        }
    }
    walks
}

/// Splits a face walk where it passes a node twice into closed walks.
fn simple_cycles(graph: &Graph, walk: &[usize]) -> Vec<Vec<usize>> {
    let mut cycles = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut at: HashMap<usize, usize> = HashMap::new();
    for &d in walk {
        let start = graph.ends(d).0;
        if let Some(&p) = at.get(&start) {
            let ring: Vec<usize> = stack.drain(p..).collect();
            for &r in &ring {
                at.remove(&graph.ends(r).0);
            }
            cycles.push(ring);
        }
        at.insert(start, stack.len());
        stack.push(d);
    }
    if !stack.is_empty() {
        cycles.push(stack);
    }
    cycles
}

fn ring_coords(graph: &Graph, cycle: &[usize]) -> Vec<C> {
    let mut out: Vec<C> = Vec::new();
    for &d in cycle {
        let c = graph.coords(d);
        if out.is_empty() {
            out.extend(c);
        } else {
            out.extend(c.into_iter().skip(1));
        }
    }
    out
}

/// Twice the signed area (positive counter-clockwise).
fn signed_area(ring: &[C]) -> f64 {
    ring.windows(2)
        .map(|w| w[0].x * w[1].y - w[1].x * w[0].y)
        .sum()
}

#[derive(Default)]
struct Outcome {
    shells: Vec<Vec<C>>,
    holes: Vec<Vec<C>>,
    cuts: Vec<Vec<C>>,
    dangles: Vec<Vec<C>>,
    invalid: Vec<Vec<C>>,
}

fn polygonize_chains(chains: Vec<Vec<C>>) -> Outcome {
    let graph = Graph::new(chains);
    let n = graph.edges.len();
    let mut alive = vec![true; n];
    let mut out = Outcome::default();

    // 1. Dangles: edges ending in a node of degree one, repeatedly.
    let mut degree = vec![0_usize; graph.nodes.len()];
    let mut incident: Vec<Vec<usize>> = vec![Vec::new(); graph.nodes.len()];
    for (e, edge) in graph.edges.iter().enumerate() {
        degree[edge.from] += 1;
        degree[edge.to] += 1;
        incident[edge.from].push(e);
        incident[edge.to].push(e);
    }
    let mut queue: Vec<usize> = (0..degree.len()).filter(|&v| degree[v] == 1).collect();
    while let Some(v) = queue.pop() {
        if degree[v] != 1 {
            continue;
        }
        let Some(&e) = incident[v].iter().find(|&&e| alive[e]) else {
            continue;
        };
        alive[e] = false;
        out.dangles.push(graph.edges[e].coords.clone());
        for end in [graph.edges[e].from, graph.edges[e].to] {
            degree[end] -= 1;
            if degree[end] == 1 {
                queue.push(end);
            }
        }
    }

    // 2. Cut edges: the same face walk passes an edge in both directions.
    let walks = face_walks(&graph, &alive);
    let mut walk_of: HashMap<usize, usize> = HashMap::new();
    for (w, walk) in walks.iter().enumerate() {
        for &d in walk {
            walk_of.insert(d, w);
        }
    }
    for e in 0..n {
        if alive[e] && walk_of[&(2 * e)] == walk_of[&(2 * e + 1)] {
            alive[e] = false;
            out.cuts.push(graph.edges[e].coords.clone());
        }
    }

    // 3. Rings.
    let mut seen_invalid: HashSet<Vec<Key>> = HashSet::new();
    for walk in face_walks(&graph, &alive) {
        for cycle in simple_cycles(&graph, &walk) {
            let ring = ring_coords(&graph, &cycle);
            let area = signed_area(&ring);
            let valid = ring.len() >= 4
                && area != 0.0
                && Polygon::new(LineString::new(ring.clone()), vec![]).is_valid();
            if !valid {
                // A ring that crosses itself is found from both of its sides: report it once.
                let forward: Vec<Key> = ring.iter().map(|c| key(*c)).collect();
                let backward: Vec<Key> = forward.iter().rev().copied().collect();
                if seen_invalid.insert(forward.min(backward)) {
                    out.invalid.push(ring);
                }
            } else if area > 0.0 {
                // Counter-clockwise: a shell; GEOS reports shells clockwise.
                let mut ring = ring;
                ring.reverse();
                out.shells.push(ring);
            } else {
                let mut ring = ring;
                ring.reverse();
                out.holes.push(ring);
            }
        }
    }
    out
}

fn ring_envelope(ring: &[C]) -> AABB<[f64; 2]> {
    let mut env = AABB::from_corners([ring[0].x, ring[0].y], [ring[0].x, ring[0].y]);
    for c in ring {
        env.merge(&AABB::from_corners([c.x, c.y], [c.x, c.y]));
    }
    env
}

fn inside(shell: &[C], point: C) -> bool {
    Polygon::new(LineString::new(shell.to_vec()), vec![]).coordinate_position(&point)
        != CoordPos::Outside
}

/// Polygons from shells and the holes assigned to the smallest shell containing them.
fn assemble(shells: &[Vec<C>], holes: &[Vec<C>]) -> Vec<Polygon<f64>> {
    let envelopes: Vec<AABB<[f64; 2]>> = shells.iter().map(|s| ring_envelope(s)).collect();
    let tree = RTree::bulk_load(
        envelopes
            .iter()
            .enumerate()
            .map(|(i, e)| GeomWithData::new(Rectangle::from_aabb(*e), i))
            .collect(),
    );
    let mut assigned: Vec<Vec<usize>> = vec![Vec::new(); shells.len()];
    for (h, hole) in holes.iter().enumerate() {
        let env = ring_envelope(hole);
        let mut best: Option<usize> = None;
        for hit in tree.locate_in_envelope_intersecting(&env) {
            let s = hit.data;
            let shell_env = envelopes[s];
            if shell_env == env || !shell_env.contains_envelope(&env) {
                continue;
            }
            // A vertex of the hole that is not a vertex of the shell decides containment.
            let shell_keys: HashSet<Key> = shells[s].iter().map(|c| key(*c)).collect();
            let probe = hole
                .iter()
                .find(|c| !shell_keys.contains(&key(**c)))
                .copied()
                .unwrap_or(hole[0]);
            if !inside(&shells[s], probe) {
                continue;
            }
            if best.is_none_or(|b| envelopes[b].contains_envelope(&shell_env)) {
                best = Some(s);
            }
        }
        if let Some(s) = best {
            assigned[s].push(h);
        }
    }
    shells
        .iter()
        .zip(&assigned)
        .map(|(shell, hs)| {
            Polygon::new(
                LineString::new(shell.clone()),
                hs.iter()
                    .map(|&h| LineString::new(holes[h].clone()))
                    .collect(),
            )
        })
        .collect()
}

// -- column level -------------------------------------------------------------------------

/// The edges of the input: merged and noded (`node`) or the lines as they are.
fn edges_of(rows: &[Geometry<f64>], node: bool) -> Result<Vec<Vec<C>>, String> {
    let mut work = Linework::default();
    rows.iter().for_each(|g| gather(g, &mut work));
    let all_finite = work
        .lines
        .iter()
        .chain(work.polygons.iter().flat_map(rings))
        .all(|l| l.0.iter().all(finite));
    if !all_finite {
        return Err("polygonize: coordinates must be finite".to_string());
    }
    if !node {
        // Every line is an edge, duplicates included: two equal edges form a ring of no area.
        return Ok(work
            .lines
            .iter()
            .chain(work.polygons.iter().flat_map(rings))
            .map(|line| dedup(&line.0))
            .filter(|coords| coords.len() >= 2)
            .collect());
    }
    // A union: polygons merged, lines inside them dropped.
    let area: MultiPolygon<f64> = if work.polygons.is_empty() {
        MultiPolygon::new(vec![])
    } else {
        let oriented: Vec<Polygon<f64>> = work
            .polygons
            .iter()
            .map(|p| p.orient(Direction::Default))
            .collect();
        keep_input_vertices(unary_union(&oriented), &oriented, &work.lines)
    };
    let mut lines: Vec<LineString<f64>> = work.lines;
    let mut rings_out: Vec<Vec<C>> = Vec::new();
    if !area.0.is_empty() {
        lines = line_outside_polygon(&MultiLineString::new(lines), &area);
        let area = add_crossings(area, &lines);
        rings_out = area
            .0
            .iter()
            .flat_map(rings)
            .map(|r| dedup(&r.0))
            .filter(|c| c.len() >= 2)
            .collect();
    }
    // As in the GEOS polygonizer, a ring of the merged area is one edge: its only node is its
    // start, so a line piece that ends at another vertex of the ring is a dangle.
    let mut edges = node_lines(&lines)?;
    edges.extend(rings_out);
    Ok(edges)
}

/// Adds the points where a line piece ends on the outline of the area (where the line crossed
/// or touched it) to the outline. The union has them as vertices, but they are computed, so
/// they are on the outline only up to rounding.
fn add_crossings(area: MultiPolygon<f64>, pieces: &[LineString<f64>]) -> MultiPolygon<f64> {
    let ends = pieces
        .iter()
        .flat_map(|l| [l.0.first(), l.0.last()])
        .flatten()
        .copied();
    insert_on_outline(area, ends, 2e-9, |a, b, c| {
        let tolerance = 1e-9 * (1.0 + a.x.abs().max(a.y.abs()).max(b.x.abs()).max(b.y.abs()));
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let length2 = dx * dx + dy * dy;
        let t = (((c.x - a.x) * dx + (c.y - a.y) * dy) / length2).clamp(0.0, 1.0);
        let (px, py) = (a.x + t * dx - c.x, a.y + t * dy - c.y);
        px.hypot(py) <= tolerance
    })
}

/// Puts back the vertices the union dropped. GEOS keeps every input vertex that lies on the
/// outline of the merged area, collinear ones included (where polygons met, where a ring had a
/// vertex in the middle of a straight run, where a line touches it); the union used here keeps only the corners.
fn keep_input_vertices(
    area: MultiPolygon<f64>,
    inputs: &[Polygon<f64>],
    lines: &[LineString<f64>],
) -> MultiPolygon<f64> {
    use geo::algorithm::kernels::{Kernel, Orientation, RobustKernel};

    let ring_vertices = inputs.iter().flat_map(rings).flat_map(|r| r.0.iter());
    let points = ring_vertices
        .chain(lines.iter().flat_map(|l| l.0.iter()))
        .copied();
    insert_on_outline(area, points, 0.0, |a, b, c| {
        RobustKernel::orient2d(a, b, c) == Orientation::Collinear
    })
}

/// Inserts every point that `on_segment(a, b, point)` finds on a segment `a`-`b` of the outline
/// of `area`, strictly inside it, into that segment.
fn insert_on_outline(
    area: MultiPolygon<f64>,
    points: impl Iterator<Item = C>,
    slack: f64,
    on_segment: impl Fn(C, C, C) -> bool,
) -> MultiPolygon<f64> {
    let outline: Vec<&LineString<f64>> = area.0.iter().flat_map(rings).collect();
    let mut segments: Vec<(C, C, usize)> = Vec::new();
    for (ring, line) in outline.iter().enumerate() {
        segments.extend(line.0.windows(2).map(|w| (w[0], w[1], ring)));
    }
    let tree = RTree::bulk_load(
        segments
            .iter()
            .enumerate()
            .map(|(i, (a, b, _))| GeomWithData::new(Rectangle::from_aabb(envelope(*a, *b)), i))
            .collect(),
    );
    // For every segment of the outline, the input vertices that lie strictly inside it.
    let mut inserts: HashMap<usize, Vec<C>> = HashMap::new();
    let mut done: HashSet<Key> = HashSet::new();
    for c in points {
        if !done.insert(key(c)) {
            continue;
        }
        let e = slack * (1.0 + c.x.abs().max(c.y.abs()));
        let point = AABB::from_corners([c.x - e, c.y - e], [c.x + e, c.y + e]);
        for hit in tree.locate_in_envelope_intersecting(&point) {
            let (a, b, _) = segments[hit.data];
            if c != a && c != b && on_segment(a, b, c) {
                inserts.entry(hit.data).or_default().push(c);
            }
        }
    }
    if inserts.is_empty() {
        return area;
    }
    let mut at = 0;
    let mut rebuilt = |line: &LineString<f64>| -> LineString<f64> {
        let mut out: Vec<C> = Vec::with_capacity(line.0.len());
        for w in line.0.windows(2) {
            out.push(w[0]);
            if let Some(mut on) = inserts.remove(&at) {
                on.sort_by(|p, q| fraction(w[0], w[1], *p).total_cmp(&fraction(w[0], w[1], *q)));
                out.extend(on);
            }
            at += 1;
        }
        out.extend(line.0.last().copied());
        LineString::new(out)
    };
    MultiPolygon::new(
        area.0
            .iter()
            .map(|p| {
                let exterior = rebuilt(p.exterior());
                let holes = p.interiors().iter().map(&mut rebuilt).collect();
                Polygon::new(exterior, holes)
            })
            .collect(),
    )
}

fn column_of(template: &GeoColumn, rows: Vec<Geometry<f64>>) -> Result<GeoColumn, String> {
    let rows: Vec<Option<Geometry<f64>>> = rows.into_iter().map(Some).collect();
    template.rebuild_merged(&rows)
}

fn lines_column(template: &GeoColumn, lines: Vec<Vec<C>>) -> Result<GeoColumn, String> {
    column_of(
        template,
        lines
            .into_iter()
            .map(|l| Geometry::LineString(LineString::new(l)))
            .collect(),
    )
}

impl GeoColumn {
    fn polygonize_outcome(&self, node: bool) -> Result<(Outcome, Vec<Polygon<f64>>), String> {
        let rows: Vec<Geometry<f64>> = self.geo_rows().into_iter().flatten().collect();
        // `geo` can panic on degenerate input; that must not take the VM down.
        guarded(|| {
            let chains = edges_of(&rows, node)?;
            let outcome = polygonize_chains(chains);
            let polygons = assemble(&outcome.shells, &outcome.holes);
            Ok((outcome, polygons))
        })
        .unwrap_or_else(|| Err("polygonize: the linework could not be processed".to_string()))
    }

    /// Polygons formed by the linework of all rows together (see the module documentation),
    /// with the cut edges, dangles and invalid rings. Every result column has one geometry
    /// per polygon or line and the CRS of this column; null rows are ignored.
    pub fn polygonize(&self, node: bool) -> Result<Polygonized, String> {
        let (outcome, polygons) = self.polygonize_outcome(node)?;
        Ok(Polygonized {
            polygons: column_of(self, polygons.into_iter().map(Geometry::Polygon).collect())?,
            cuts: lines_column(self, outcome.cuts)?,
            dangles: lines_column(self, outcome.dangles)?,
            invalid: lines_column(self, outcome.invalid)?,
        })
    }

    /// The area enclosed by the linework of all rows together, even-odd: the symmetric
    /// difference of the filled polygons that [`polygonize`](Self::polygonize) finds. One
    /// polygon per row of the result.
    pub fn build_area(&self, node: bool) -> Result<GeoColumn, String> {
        let (outcome, _) = self.polygonize_outcome(node)?;
        let mut area = MultiPolygon::new(vec![]);
        for shell in &outcome.shells {
            let filled =
                MultiPolygon::new(vec![Polygon::new(LineString::new(shell.clone()), vec![])]);
            area = if area.0.is_empty() {
                filled
            } else {
                area.xor(&filled)
            };
        }
        column_of(self, area.0.into_iter().map(Geometry::Polygon).collect())
    }
}
