//! `offset_curve`: the line parallel to a line at a signed distance (Shapely 2 `offset_curve`).
//!
//! Positive distances offset to the left of the line's direction, negative to the right.
//! The result follows GEOS (3.11 and later):
//!
//! 1. The *raw* one-sided curve is built like GEOS' `OffsetSegmentGenerator`: every segment is
//!    shifted sideways, outside corners are joined with a round fillet (quarter circles of
//!    `quad_segs` steps, never fewer than 8 as GEOS), a mitre (limited by `mitre_limit`) or a
//!    bevel, and inside corners with the intersection of the two shifted segments (or a
//!    closing connector when they do not meet).
//! 2. The raw curve is cut wherever its distance from the input crosses the offset distance
//!    (found exactly: a shifted segment meets the parallels and end circles of every nearby
//!    input segment), and the pieces that lie closer to the input than the distance are
//!    dropped. GEOS keeps the same part of the curve by walking the buffer outline.
//! 3. Pieces that meet end to end at exactly two ends are joined, so a loop that was cut out
//!    leaves one line. The result is a `LineString` or, when the curve is broken, a
//!    `MultiLineString`; `LINESTRING EMPTY` when nothing remains.
//!
//! Lines and multi-lines are offset part by part (a closed line is just a line: no join at its
//! start). A polygon has every ring offset as a ring (shell turned clockwise, holes
//! counter-clockwise, so a positive distance grows the polygon) and the rings are cut against
//! each other. Collections use their lines and polygons; points give an empty line. A
//! distance of 0 returns the (oriented) input lines.
//!
//! Deviations from GEOS: its input simplification (`BufferInputLineSimplifier`) is not
//! applied, so vertices that are almost collinear are kept; where the offset ring of a polygon
//! collapses to its medial axis the degenerate line is kept; the start of a closed result and the
//! order of the parts can differ.
//!
//! Limits: the estimated number of vertices of the raw curve (vertices times `2 * quad_segs`)
//! must stay under 10,000,000 or the call fails; a geometry whose cutting needs more than
//! 50,000,000 segment pairs is null.

use geo::algorithm::kernels::{Kernel, Orientation, RobustKernel};
use geo::line_intersection::{line_intersection, LineIntersection};
use geo::{Coord, CoordsIter, Geometry, Line, LineString, MultiLineString, Polygon};
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, AABB};

use crate::parity_j::{dist, point_segment_distance};
use crate::predicates::guarded;
use crate::GeoColumn;

/// How outside corners are joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsetJoin {
    Round,
    Mitre,
    Bevel,
}

impl OffsetJoin {
    /// Parses the Shapely `join_style` names.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "round" => Ok(Self::Round),
            "mitre" => Ok(Self::Mitre),
            "bevel" => Ok(Self::Bevel),
            other => Err(format!("unknown join_style: {other}")),
        }
    }
}

/// Parameters of [`GeoColumn::offset_curve`].
#[derive(Debug, Clone, Copy)]
pub struct OffsetParams {
    pub quad_segs: u32,
    pub join: OffsetJoin,
    pub mitre_limit: f64,
}

/// Largest `quad_segs` accepted.
pub const MAX_QUAD_SEGS: u32 = 10_000;
/// Most vertices the raw curves of one call may have (estimated before any work).
const MAX_RAW_VERTICES: u64 = 10_000_000;
/// Most (raw segment, input segment) pairs examined for one geometry.
const PAIR_BUDGET: u64 = 50_000_000;

const SNAP_FACTOR: f64 = 1.0e-6;
const SEPARATION_FACTOR: f64 = 1.0e-3;
const INSIDE_SNAP_FACTOR: f64 = 1.0e-3;
const MAX_CLOSING_FACTOR: f64 = 80.0;

type C = Coord<f64>;

// -- raw curve ----------------------------------------------------------------------------

/// What produced a segment of the raw curve.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Tag {
    /// The shifted copy of input segment `n`.
    Offset(usize),
    /// A join at input vertex `n` (fillet, mitre or bevel).
    Join(usize),
    /// A connector at an inside corner: never part of the offset curve.
    Inside,
}

struct Gen {
    join: OffsetJoin,
    mitre_limit: f64,
    d: f64,
    quantum: f64,
    closing_factor: f64,
    min_vertex_dist: f64,
    pts: Vec<C>,
    tags: Vec<Tag>,
    s0: C,
    s1: C,
    s2: C,
    off0: (C, C),
    off1: (C, C),
    /// Index of the segment `s1`-`s2`.
    seg1: usize,
}

fn angle(p0: C, p1: C) -> f64 {
    (p1.y - p0.y).atan2(p1.x - p0.x)
}

fn normalize_angle(mut a: f64) -> f64 {
    while a > std::f64::consts::PI {
        a -= 2.0 * std::f64::consts::PI;
    }
    while a <= -std::f64::consts::PI {
        a += 2.0 * std::f64::consts::PI;
    }
    a
}

impl Gen {
    fn new(params: &OffsetParams, d: f64) -> Self {
        let quad = f64::from(params.quad_segs.max(8));
        let zero = Coord { x: 0.0, y: 0.0 };
        Self {
            join: params.join,
            mitre_limit: params.mitre_limit,
            d,
            quantum: std::f64::consts::FRAC_PI_2 / quad,
            closing_factor: if params.join == OffsetJoin::Round {
                MAX_CLOSING_FACTOR
            } else {
                1.0
            },
            min_vertex_dist: d * SNAP_FACTOR,
            pts: Vec::new(),
            tags: Vec::new(),
            s0: zero,
            s1: zero,
            s2: zero,
            off0: (zero, zero),
            off1: (zero, zero),
            seg1: 0,
        }
    }

    fn add(&mut self, c: C, tag: Tag) {
        if let Some(&last) = self.pts.last() {
            if dist(c, last) < self.min_vertex_dist {
                return;
            }
        }
        self.pts.push(c);
        self.tags.push(tag);
    }

    /// The segment `a`-`b` shifted to its left by the distance.
    fn shifted(&self, a: C, b: C) -> (C, C) {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len = (dx * dx + dy * dy).sqrt();
        let (ux, uy) = (self.d * dx / len, self.d * dy / len);
        (
            Coord {
                x: a.x - uy,
                y: a.y + ux,
            },
            Coord {
                x: b.x - uy,
                y: b.y + ux,
            },
        )
    }

    fn init(&mut self, s1: C, s2: C, seg: usize) {
        self.s1 = s1;
        self.s2 = s2;
        self.seg1 = seg;
        self.off1 = self.shifted(s1, s2);
    }

    fn first_segment(&mut self) {
        let p = self.off1.0;
        self.add(p, Tag::Offset(self.seg1));
    }

    fn last_segment(&mut self) {
        let p = self.off1.1;
        self.add(p, Tag::Offset(self.seg1));
    }

    /// Advances to the next input vertex `p`; `vertex` is the index of the corner (`s1`) the
    /// step joins and `next_seg` the index of the new segment.
    fn next(&mut self, p: C, add_start: bool, vertex: usize, next_seg: usize) {
        let prev_seg = self.seg1;
        self.s0 = self.s1;
        self.s1 = self.s2;
        self.s2 = p;
        self.off0 = self.off1;
        self.off1 = self.shifted(self.s1, self.s2);
        self.seg1 = next_seg;
        if self.s1.x == self.s2.x && self.s1.y == self.s2.y {
            return;
        }
        let orientation = RobustKernel::orient2d(self.s0, self.s1, self.s2);
        match orientation {
            Orientation::Collinear => self.collinear(add_start, prev_seg, vertex),
            // Offsetting to the left: a clockwise turn is an outside corner.
            Orientation::Clockwise => self.outside(orientation, add_start, prev_seg, vertex),
            Orientation::CounterClockwise => self.inside(prev_seg),
        }
    }

    fn collinear(&mut self, add_start: bool, prev_seg: usize, vertex: usize) {
        let reversing = matches!(
            line_intersection(Line::new(self.s0, self.s1), Line::new(self.s1, self.s2)),
            Some(LineIntersection::Collinear { .. })
        );
        if !reversing {
            return;
        }
        if self.join == OffsetJoin::Round {
            self.fillet(self.off0.1, self.off1.0, Orientation::Clockwise, vertex);
        } else {
            if add_start {
                self.add(self.off0.1, Tag::Offset(prev_seg));
            }
            self.add(self.off1.0, Tag::Join(vertex));
        }
    }

    fn outside(
        &mut self,
        orientation: Orientation,
        add_start: bool,
        prev_seg: usize,
        vertex: usize,
    ) {
        if dist(self.off0.1, self.off1.0) < self.d * SEPARATION_FACTOR {
            self.add(self.off0.1, Tag::Offset(prev_seg));
            return;
        }
        match self.join {
            OffsetJoin::Mitre => self.mitre(prev_seg, vertex),
            OffsetJoin::Bevel => {
                self.add(self.off0.1, Tag::Offset(prev_seg));
                self.add(self.off1.0, Tag::Join(vertex));
            }
            OffsetJoin::Round => {
                if add_start {
                    self.add(self.off0.1, Tag::Offset(prev_seg));
                }
                self.fillet(self.off0.1, self.off1.0, orientation, vertex);
                self.add(self.off1.0, Tag::Join(vertex));
            }
        }
    }

    /// `addDirectedFillet`: the arc around `s1` from `p0` to `p1`, both ends included.
    fn fillet(&mut self, p0: C, p1: C, direction: Orientation, vertex: usize) {
        let p = self.s1;
        let mut start = angle(p, p0);
        let end = angle(p, p1);
        let clockwise = direction == Orientation::Clockwise;
        if clockwise {
            if start <= end {
                start += 2.0 * std::f64::consts::PI;
            }
        } else if start >= end {
            start -= 2.0 * std::f64::consts::PI;
        }
        self.add(p0, Tag::Join(vertex));
        let factor = if clockwise { -1.0 } else { 1.0 };
        let total = (start - end).abs();
        let n = (total / self.quantum + 0.5) as usize;
        if n >= 1 {
            let inc = total / n as f64;
            for i in 0..n {
                let a = start + factor * (i as f64) * inc;
                self.add(
                    Coord {
                        x: p.x + self.d * a.cos(),
                        y: p.y + self.d * a.sin(),
                    },
                    Tag::Join(vertex),
                );
            }
        }
        self.add(p1, Tag::Join(vertex));
    }

    fn mitre(&mut self, prev_seg: usize, vertex: usize) {
        let (a, b) = (self.off0, self.off1);
        if let Some(tip) = line_line(a.0, a.1, b.0, b.1) {
            if dist(tip, self.s1) / self.d <= self.mitre_limit {
                // The tip lies on the extension of the previous offset segment.
                self.add(tip, Tag::Offset(prev_seg));
                return;
            }
        }
        // Limited mitre: a bevel cut off at the mitre limit, built on the input corner.
        let base = self.s1;
        let ang0 = angle(base, self.s0);
        let mut diff = angle(base, self.s2) - ang0;
        if diff <= -std::f64::consts::PI {
            diff += 2.0 * std::f64::consts::PI;
        } else if diff > std::f64::consts::PI {
            diff -= 2.0 * std::f64::consts::PI;
        }
        let half = diff / 2.0;
        let mid = normalize_angle(ang0 + half);
        let mitre_mid = normalize_angle(mid + std::f64::consts::PI);
        let mitre_dist = self.mitre_limit * self.d;
        let bevel_half = self.d - mitre_dist * half.sin().abs();
        let bevel_mid = Coord {
            x: base.x + mitre_dist * mitre_mid.cos(),
            y: base.y + mitre_dist * mitre_mid.sin(),
        };
        let along = |offset: f64| {
            let (dx, dy) = (bevel_mid.x - base.x, bevel_mid.y - base.y);
            let len = (dx * dx + dy * dy).sqrt();
            let (ux, uy) = (offset * dx / len, offset * dy / len);
            Coord {
                x: bevel_mid.x - uy,
                y: bevel_mid.y + ux,
            }
        };
        let (left, right) = (along(bevel_half), along(-bevel_half));
        self.add(left, Tag::Offset(prev_seg));
        self.add(right, Tag::Join(vertex));
    }

    fn inside(&mut self, prev_seg: usize) {
        let (a, b) = (self.off0, self.off1);
        match line_intersection(Line::new(a.0, a.1), Line::new(b.0, b.1)) {
            Some(LineIntersection::SinglePoint { intersection, .. }) => {
                self.add(intersection, Tag::Offset(prev_seg));
            }
            Some(LineIntersection::Collinear { intersection }) => {
                self.add(intersection.start, Tag::Offset(prev_seg));
            }
            None => {
                if dist(a.1, b.0) < self.d * INSIDE_SNAP_FACTOR {
                    self.add(a.1, Tag::Offset(prev_seg));
                } else {
                    self.add(a.1, Tag::Offset(prev_seg));
                    let (f, corner) = (self.closing_factor, self.s1);
                    let mid = |p: C| Coord {
                        x: (f * p.x + corner.x) / (f + 1.0),
                        y: (f * p.y + corner.y) / (f + 1.0),
                    };
                    self.add(mid(a.1), Tag::Inside);
                    self.add(mid(b.0), Tag::Inside);
                    self.add(b.0, Tag::Inside);
                }
            }
        }
    }
}

/// Intersection of the infinite lines `a0`-`a1` and `b0`-`b1`.
fn line_line(a0: C, a1: C, b0: C, b1: C) -> Option<C> {
    let (rx, ry) = (a1.x - a0.x, a1.y - a0.y);
    let (sx, sy) = (b1.x - b0.x, b1.y - b0.y);
    let denom = rx * sy - ry * sx;
    if denom == 0.0 {
        return None;
    }
    let t = ((b0.x - a0.x) * sy - (b0.y - a0.y) * sx) / denom;
    Some(Coord {
        x: a0.x + t * rx,
        y: a0.y + t * ry,
    })
}

/// A ring or line to offset: its distinct vertices (a ring without the closing repeat).
struct Part {
    coords: Vec<C>,
    ring: bool,
}

/// The raw one-sided curve of `part` at the positive distance `d` to its left.
fn raw_curve(part: &Part, params: &OffsetParams, d: f64) -> (Vec<C>, Vec<Tag>) {
    let mut gen = Gen::new(params, d);
    let c = &part.coords;
    let n = c.len();
    if part.ring {
        // Segment k runs from vertex k to vertex k + 1 (wrapping).
        gen.init(c[n - 1], c[0], n - 1);
        for i in 1..=n {
            gen.next(c[i % n], i != 1, i - 1, i - 1);
        }
        if let Some(&first) = gen.pts.first() {
            gen.pts.push(first);
            gen.tags.push(Tag::Join(0));
        }
    } else {
        gen.init(c[0], c[1], 0);
        gen.first_segment();
        for (i, &p) in c.iter().enumerate().skip(2) {
            gen.next(p, true, i - 1, i - 1);
        }
        gen.last_segment();
    }
    (gen.pts, gen.tags)
}

// -- cutting and classifying --------------------------------------------------------------

struct InputSeg {
    a: C,
    b: C,
}

type SegTree = RTree<GeomWithData<Rectangle<[f64; 2]>, usize>>;

fn envelope(a: C, b: C, pad: f64) -> AABB<[f64; 2]> {
    AABB::from_corners(
        [a.x.min(b.x) - pad, a.y.min(b.y) - pad],
        [a.x.max(b.x) + pad, a.y.max(b.y) + pad],
    )
}

/// Parameters `t` in (0, 1) at which `a + t (b - a)` is exactly `d` away from the segment
/// `p`-`q`: crossings of its two parallels and of the circles around its ends.
fn locus_crossings(a: C, b: C, p: C, q: C, d: f64, out: &mut Vec<f64>) {
    let (rx, ry) = (b.x - a.x, b.y - a.y);
    let (ux, uy) = (q.x - p.x, q.y - p.y);
    let len = (ux * ux + uy * uy).sqrt();
    let eps = 1.0e-9;
    let a2 = rx * rx + ry * ry;
    if len > 0.0 {
        let (nx, ny) = (-uy / len, ux / len);
        let denom = rx * uy - ry * ux;
        if denom != 0.0 {
            for side in [d, -d] {
                let (ox, oy) = (p.x + side * nx, p.y + side * ny);
                let t = ((ox - a.x) * uy - (oy - a.y) * ux) / denom;
                if t > eps && t < 1.0 - eps {
                    let x = Coord {
                        x: a.x + t * rx,
                        y: a.y + t * ry,
                    };
                    let along = ((x.x - p.x) * ux + (x.y - p.y) * uy) / (len * len);
                    if (-eps..=1.0 + eps).contains(&along) {
                        out.push(t);
                    }
                }
            }
        }
    }
    for c in [p, q] {
        let (fx, fy) = (a.x - c.x, a.y - c.y);
        let bq = rx * fx + ry * fy;
        let cq = fx * fx + fy * fy - d * d;
        let disc = bq * bq - a2 * cq;
        if a2 > 0.0 && disc >= 0.0 {
            let root = disc.sqrt();
            for t in [(-bq - root) / a2, (-bq + root) / a2] {
                if t > eps && t < 1.0 - eps {
                    out.push(t);
                }
            }
        }
    }
}

/// The segments the raw curves are cut and tested against.
struct Input {
    segs: Vec<InputSeg>,
    tree: SegTree,
    /// Index of the first segment of each part, then the total.
    first: Vec<usize>,
    d: f64,
    tol: f64,
    scale: f64,
}

impl Input {
    fn new(parts: &[Part], d: f64) -> Self {
        let mut segs = Vec::new();
        let mut first = Vec::new();
        for p in parts {
            first.push(segs.len());
            let n = p.coords.len();
            let count = if p.ring { n } else { n - 1 };
            for k in 0..count {
                segs.push(InputSeg {
                    a: p.coords[k],
                    b: p.coords[(k + 1) % n],
                });
            }
        }
        first.push(segs.len());
        let tree = RTree::bulk_load(
            segs.iter()
                .enumerate()
                .map(|(i, s)| {
                    GeomWithData::new(
                        Rectangle::from_corners(
                            [s.a.x.min(s.b.x), s.a.y.min(s.b.y)],
                            [s.a.x.max(s.b.x), s.a.y.max(s.b.y)],
                        ),
                        i,
                    )
                })
                .collect(),
        );
        let scale = segs
            .iter()
            .flat_map(|s| [s.a.x.abs(), s.a.y.abs(), s.b.x.abs(), s.b.y.abs()])
            .fold(0.0, f64::max);
        Self {
            segs,
            tree,
            first,
            d,
            tol: 1.0e-9 * d + 1.0e-12 * scale,
            scale,
        }
    }

    /// The unit direction pointing away from the input at `at`, a point of a raw segment with
    /// the given tag: the left normal of the segment an offset came from, or the direction from
    /// the vertex of a join.
    fn outward(&self, tag: Tag, base: usize, count: usize, at: C) -> Option<C> {
        let (dx, dy) = match tag {
            Tag::Offset(n) => {
                let s = &self.segs[base + n];
                (-(s.b.y - s.a.y), s.b.x - s.a.x)
            }
            Tag::Join(v) => {
                let p = self.segs[base + v % count].a;
                (at.x - p.x, at.y - p.y)
            }
            Tag::Inside => return None,
        };
        let len = (dx * dx + dy * dy).sqrt();
        (len > 0.0).then_some(Coord {
            x: dx / len,
            y: dy / len,
        })
    }

    /// The runs of the raw curve of part `part` that belong to the offset curve.
    fn valid_runs(
        &self,
        part: usize,
        raw: &[C],
        tags: &[Tag],
        pairs: &mut u64,
    ) -> Option<Vec<Vec<C>>> {
        let (d, tol) = (self.d, self.tol);
        let base = self.first[part];
        let count = self.first[part + 1] - base;
        let mut runs: Vec<Vec<C>> = Vec::new();
        let mut current: Vec<C> = Vec::new();
        let mut cuts: Vec<f64> = Vec::new();
        for k in 0..raw.len().saturating_sub(1) {
            let (a, b) = (raw[k], raw[k + 1]);
            let tag = tags[k + 1];
            let candidates: Vec<usize> = self
                .tree
                .locate_in_envelope_intersecting(&envelope(a, b, d + tol))
                .map(|e| e.data)
                .collect();
            *pairs += candidates.len() as u64;
            if *pairs > PAIR_BUDGET {
                return None;
            }
            // Segments left out of this raw segment's cuts and distance test: the two meeting
            // at the vertex of a join, everything for an inside connector.
            let skipped = |id: usize| match tag {
                Tag::Offset(_) => false,
                Tag::Join(v) => id == base + v % count || id == base + (v + count - 1) % count,
                Tag::Inside => true,
            };
            cuts.clear();
            for &id in &candidates {
                // The segment an offset segment came from is at distance d all along.
                let is_source = matches!(tag, Tag::Offset(n) if id == base + n);
                if skipped(id) || is_source {
                    continue;
                }
                locus_crossings(a, b, self.segs[id].a, self.segs[id].b, d, &mut cuts);
            }
            cuts.sort_by(f64::total_cmp);
            cuts.dedup_by(|x, y| (*x - *y).abs() < 1.0e-12);
            let mut points = vec![a];
            points.extend(cuts.iter().map(|&t| Coord {
                x: a.x + t * (b.x - a.x),
                y: a.y + t * (b.y - a.y),
            }));
            points.push(b);
            for (index, w) in points.windows(2).enumerate() {
                if w[0] == w[1] {
                    continue;
                }
                let mid = Coord {
                    x: (w[0].x + w[1].x) / 2.0,
                    y: (w[0].y + w[1].y) / 2.0,
                };
                let clear = |p: C| {
                    candidates.iter().filter(|&&id| !skipped(id)).all(|&id| {
                        point_segment_distance(p, self.segs[id].a, self.segs[id].b) >= d - tol
                    })
                };
                // Lying at distance d from the input is not enough: a point that is d from two
                // segments with their neighbourhoods on both sides of it is inside the buffer,
                // so a step away from the input must stay clear too.
                let valid = tag != Tag::Inside
                    && clear(mid)
                    && self.outward(tag, base, count, mid).is_none_or(|n| {
                        clear(Coord {
                            x: mid.x + 10.0 * tol * n.x,
                            y: mid.y + 10.0 * tol * n.y,
                        })
                    });
                if valid {
                    if current.last().is_none_or(|&last| last != w[0]) {
                        if current.len() > 1 {
                            runs.push(std::mem::take(&mut current));
                        }
                        current = vec![w[0]];
                        current.push(w[1]);
                    } else if index > 0 {
                        // w[0] is a cut inside one raw segment: not a vertex of the curve.
                        *current.last_mut().expect("a point") = w[1];
                    } else {
                        current.push(w[1]);
                    }
                } else if current.len() > 1 {
                    runs.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
            }
        }
        if current.len() > 1 {
            runs.push(current);
        }
        Some(runs)
    }
}

/// Joins runs whose ends meet at exactly two ends; returns the resulting lines in order.
fn join_runs(runs: Vec<Vec<C>>, tol: f64) -> Vec<Vec<C>> {
    let n = runs.len();
    if n == 0 {
        return runs;
    }
    #[derive(Clone, Copy)]
    struct End {
        run: usize,
        start: bool,
    }
    let ends: Vec<(C, End)> = runs
        .iter()
        .enumerate()
        .flat_map(|(i, r)| {
            [
                (
                    r[0],
                    End {
                        run: i,
                        start: true,
                    },
                ),
                (
                    r[r.len() - 1],
                    End {
                        run: i,
                        start: false,
                    },
                ),
            ]
        })
        .collect();
    let tree = RTree::bulk_load(
        ends.iter()
            .map(|(c, e)| GeomWithData::new([c.x, c.y], *e))
            .collect::<Vec<_>>(),
    );
    // next[i] = run joined after run i; prev[j] = run joined before run j.
    let mut next: Vec<Option<usize>> = vec![None; n];
    let mut prev: Vec<Option<usize>> = vec![None; n];
    for (i, run) in runs.iter().enumerate() {
        let tail = run[run.len() - 1];
        let near: Vec<End> = tree
            .locate_within_distance([tail.x, tail.y], tol * tol)
            .map(|e| e.data)
            .collect();
        // Exactly this tail and one head: a node of degree two.
        if near.len() == 2 {
            if let Some(head) = near.iter().find(|e| e.start) {
                next[i] = Some(head.run);
                prev[head.run] = Some(i);
            }
        }
    }
    let mut used = vec![false; n];
    let mut lines = Vec::new();
    let order: Vec<usize> = (0..n)
        .filter(|&i| prev[i].is_none())
        .chain((0..n).filter(|&i| prev[i].is_some()))
        .collect();
    for start in order {
        if used[start] {
            continue;
        }
        let mut line: Vec<C> = Vec::new();
        let mut at = Some(start);
        while let Some(i) = at {
            if used[i] {
                break;
            }
            used[i] = true;
            if line.is_empty() {
                line.extend(runs[i].iter().copied());
            } else {
                line.extend(runs[i].iter().skip(1).copied());
            }
            at = next[i];
        }
        if line.len() > 2 && dist(line[0], line[line.len() - 1]) <= tol {
            let first = line[0];
            let last = line.len() - 1;
            line[last] = first;
        }
        lines.push(line);
    }
    lines
}

/// Offsets a set of parts (the rings of one polygon, or one line) against each other.
fn offset_parts(parts: &[Part], d: f64, params: &OffsetParams) -> Option<Vec<LineString<f64>>> {
    let distance = d.abs();
    // Work on the left side: a negative distance is a positive one on the reversed parts.
    let oriented: Vec<Part> = parts
        .iter()
        .map(|p| Part {
            coords: if d < 0.0 {
                p.coords.iter().rev().copied().collect()
            } else {
                p.coords.clone()
            },
            ring: p.ring,
        })
        .collect();
    let input = Input::new(&oriented, distance);
    let mut pairs = 0_u64;
    let mut runs: Vec<Vec<C>> = Vec::new();
    for (index, p) in oriented.iter().enumerate() {
        let (raw, tags) = raw_curve(p, params, distance);
        if raw.len() < 2 {
            continue;
        }
        let mut found = input.valid_runs(index, &raw, &tags, &mut pairs)?;
        // A ring's curve is closed: its last run continues into its first.
        if p.ring && found.len() > 1 && raw.first() == raw.last() {
            let joins = found[0][0] == raw[0] && found[found.len() - 1].last() == raw.last();
            if joins {
                let tail = found.pop().expect("a last run");
                let head = std::mem::take(&mut found[0]);
                let mut merged = tail;
                merged.extend(head.into_iter().skip(1));
                found[0] = merged;
            }
        }
        runs.extend(found);
    }
    let tol = 1.0e-7 * distance + 1.0e-12 * input.scale;
    let mut lines = join_runs(runs, tol);
    if d < 0.0 {
        lines.iter_mut().for_each(|l| l.reverse());
    }
    Some(lines.into_iter().map(LineString::new).collect())
}

// -- geometries ---------------------------------------------------------------------------

fn finite(c: C) -> bool {
    c.x.is_finite() && c.y.is_finite()
}

/// A line without repeated vertices; `None` when fewer than two distinct ones remain.
fn line_part(l: &LineString<f64>) -> Option<Part> {
    let mut coords: Vec<C> = Vec::with_capacity(l.0.len());
    for &c in &l.0 {
        if coords.last() != Some(&c) {
            coords.push(c);
        }
    }
    (coords.len() >= 2).then_some(Part {
        coords,
        ring: false,
    })
}

fn is_clockwise(coords: &[C]) -> bool {
    let n = coords.len();
    let sum: f64 = (0..n)
        .map(|i| {
            let (a, b) = (coords[i], coords[(i + 1) % n]);
            (b.x - a.x) * (b.y + a.y)
        })
        .sum();
    sum > 0.0
}

fn ring_part(r: &LineString<f64>, clockwise: bool) -> Option<Part> {
    let mut coords: Vec<C> = Vec::with_capacity(r.0.len());
    for &c in &r.0 {
        if coords.last() != Some(&c) {
            coords.push(c);
        }
    }
    if coords.len() > 1 && coords.first() == coords.last() {
        coords.pop();
    }
    if coords.len() < 3 {
        return None;
    }
    if is_clockwise(&coords) != clockwise {
        coords.reverse();
    }
    Some(Part { coords, ring: true })
}

fn polygon_parts(p: &Polygon<f64>) -> Vec<Part> {
    let mut parts: Vec<Part> = ring_part(p.exterior(), true).into_iter().collect();
    parts.extend(p.interiors().iter().filter_map(|r| ring_part(r, false)));
    parts
}

fn collect_lines(
    g: &Geometry<f64>,
    d: f64,
    params: &OffsetParams,
    out: &mut Vec<LineString<f64>>,
) -> Option<()> {
    match g {
        Geometry::LineString(l) => {
            if let Some(part) = line_part(l) {
                out.extend(offset_parts(&[part], d, params)?);
            }
        }
        Geometry::Line(l) => {
            collect_lines(&Geometry::LineString(LineString::from(*l)), d, params, out)?;
        }
        Geometry::MultiLineString(m) => {
            for l in &m.0 {
                collect_lines(&Geometry::LineString(l.clone()), d, params, out)?;
            }
        }
        Geometry::Polygon(p) => {
            let parts = polygon_parts(p);
            if !parts.is_empty() {
                out.extend(offset_parts(&parts, d, params)?);
            }
        }
        Geometry::MultiPolygon(m) => {
            for p in &m.0 {
                collect_lines(&Geometry::Polygon(p.clone()), d, params, out)?;
            }
        }
        Geometry::Rect(r) => collect_lines(&Geometry::Polygon(r.to_polygon()), d, params, out)?,
        Geometry::Triangle(t) => {
            collect_lines(&Geometry::Polygon(t.to_polygon()), d, params, out)?;
        }
        Geometry::GeometryCollection(c) => {
            for m in &c.0 {
                collect_lines(m, d, params, out)?;
            }
        }
        Geometry::Point(_) | Geometry::MultiPoint(_) => {}
    }
    Some(())
}

fn offset_geometry(g: &Geometry<f64>, d: f64, params: &OffsetParams) -> Option<Geometry<f64>> {
    if !d.is_finite() || !g.coords_iter().all(finite) {
        return None;
    }
    let mut lines = Vec::new();
    if d == 0.0 {
        // A zero offset is the input line (rings oriented as for any other distance).
        let mut parts = Vec::new();
        gather_parts(g, &mut parts);
        lines.extend(parts.into_iter().map(|p| {
            let mut coords = p.coords;
            if p.ring {
                coords.push(coords[0]);
            }
            LineString::new(coords)
        }));
    } else {
        collect_lines(g, d, params, &mut lines)?;
    }
    Some(match lines.len() {
        0 => Geometry::LineString(LineString::new(vec![])),
        1 => Geometry::LineString(lines.remove(0)),
        _ => Geometry::MultiLineString(MultiLineString::new(lines)),
    })
}

fn gather_parts(g: &Geometry<f64>, out: &mut Vec<Part>) {
    match g {
        Geometry::LineString(l) => out.extend(line_part(l)),
        Geometry::MultiLineString(m) => out.extend(m.0.iter().filter_map(line_part)),
        Geometry::Polygon(p) => out.extend(polygon_parts(p)),
        Geometry::MultiPolygon(m) => m.0.iter().for_each(|p| out.extend(polygon_parts(p))),
        Geometry::GeometryCollection(c) => c.0.iter().for_each(|m| gather_parts(m, out)),
        Geometry::Line(l) => out.extend(line_part(&LineString::from(*l))),
        _ => {}
    }
}

impl GeoColumn {
    /// The offset curve of every geometry at the signed distance of its row (`None` nulls the
    /// row); see the module documentation. Fails when the parameters are invalid or the raw
    /// curves would exceed the vertex limit.
    pub fn offset_curve(
        &self,
        distances: &[Option<f64>],
        params: &OffsetParams,
    ) -> Result<Self, String> {
        if distances.len() != self.len() {
            return Err(format!(
                "distance has {} entries but the column has {} rows",
                distances.len(),
                self.len()
            ));
        }
        if params.quad_segs == 0 || params.quad_segs > MAX_QUAD_SEGS {
            return Err(format!("quad_segs must be between 1 and {MAX_QUAD_SEGS}"));
        }
        if !(params.mitre_limit.is_finite() && params.mitre_limit > 0.0) {
            return Err("mitre_limit must be a positive number".to_string());
        }
        let geoms = self.geo_rows();
        let corner_cost = 2 * u64::from(params.quad_segs.max(8)) + 4;
        let vertices: u64 = geoms
            .iter()
            .flatten()
            .map(|g| g.coords_count() as u64)
            .sum();
        if vertices.saturating_mul(corner_cost) > MAX_RAW_VERTICES {
            return Err(format!(
                "offset_curve would build more than {MAX_RAW_VERTICES} vertices \
                 (reduce quad_segs or split the series)"
            ));
        }
        let rows: Vec<Option<Geometry<f64>>> = geoms
            .iter()
            .zip(distances)
            .map(|(g, d)| match (g, d) {
                (Some(g), Some(d)) => guarded(|| offset_geometry(g, *d, params)).flatten(),
                _ => None,
            })
            .collect();
        self.rebuild(&rows)
    }
}
