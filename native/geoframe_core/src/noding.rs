//! Planar noding of two linework sets.
//!
//! [`node`] splits every segment of `a` and `b` at the points where the two sets meet and
//! reports, for each resulting piece, whether it lies on the other set (a collinear overlap).
//! It is the building block of the line overlays, of the lower-dimensional parts of polygon
//! overlays and of `clip_by_rect`.
//!
//! Segment pairs come from an R-tree over the segments of `b`; each pair goes through `geo`'s
//! robust `line_intersection`. A crossing point is computed once and used by both segments, so
//! the two sides always share bit-identical node coordinates. Consecutive pieces of one input
//! line are joined again unless a node separates them (or their `shared` flag differs), which
//! keeps plain vertices out of the output.

use std::collections::HashSet;

use geo::algorithm::line_intersection::{line_intersection, LineIntersection};
use geo::{Coord, Line, LineString};
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, AABB};

/// A maximal run of an input line between two nodes.
#[derive(Debug, Clone)]
pub(crate) struct Piece {
    pub coords: Vec<Coord<f64>>,
    /// The piece lies on the other input (collinear overlap).
    pub shared: bool,
}

/// The result of [`node`].
#[derive(Debug, Default)]
pub(crate) struct Noded {
    pub a: Vec<Piece>,
    pub b: Vec<Piece>,
    /// Every point where the two inputs meet (crossings, touches and overlap ends), deduplicated.
    pub nodes: Vec<Coord<f64>>,
}

/// Exact coordinate key (negative zero folded into zero).
pub(crate) fn key(c: Coord<f64>) -> (u64, u64) {
    ((c.x + 0.0).to_bits(), (c.y + 0.0).to_bits())
}

struct Seg {
    line: Line<f64>,
    owner: usize,
}

#[derive(Default)]
struct Cuts {
    points: Vec<Coord<f64>>,
    spans: Vec<(f64, f64)>,
}

/// Collinear-overlap spans of one segment, ready for "is this interval covered" queries.
struct Covered {
    /// `(lo, highest hi among the spans up to and including this one)`, sorted by `lo`.
    by_lo: Vec<(f64, f64)>,
}

impl Covered {
    fn new(spans: &[(f64, f64)]) -> Self {
        let mut sorted = spans.to_vec();
        sorted.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut reach = f64::NEG_INFINITY;
        let by_lo = sorted
            .into_iter()
            .map(|(lo, hi)| {
                reach = reach.max(hi);
                (lo, reach)
            })
            .collect();
        Self { by_lo }
    }

    /// Some span has `lo <= t0 && t1 <= hi`: among the spans starting at or before `t0`, the
    /// furthest-reaching one decides, so a binary search replaces the scan over every span.
    fn contains(&self, t0: f64, t1: f64) -> bool {
        let upto = self.by_lo.partition_point(|&(lo, _)| lo <= t0);
        upto > 0 && t1 <= self.by_lo[upto - 1].1
    }
}

/// Lines without repeated consecutive vertices; lines left with fewer than two are dropped.
fn clean(lines: &[LineString<f64>]) -> Vec<Vec<Coord<f64>>> {
    lines
        .iter()
        .filter_map(|ls| {
            let mut coords: Vec<Coord<f64>> = Vec::with_capacity(ls.0.len());
            for &c in &ls.0 {
                if coords.last() != Some(&c) {
                    coords.push(c);
                }
            }
            (coords.len() >= 2).then_some(coords)
        })
        .collect()
}

fn segments(lines: &[Vec<Coord<f64>>]) -> Vec<Seg> {
    lines
        .iter()
        .enumerate()
        .flat_map(|(owner, coords)| {
            coords.windows(2).map(move |w| Seg {
                line: Line::new(w[0], w[1]),
                owner,
            })
        })
        .collect()
}

/// Position of `c` along `line` as a fraction of its length (0 at the start, 1 at the end).
fn param(line: &Line<f64>, c: Coord<f64>) -> f64 {
    let d = line.end - line.start;
    let v = c - line.start;
    (v.x * d.x + v.y * d.y) / (d.x * d.x + d.y * d.y)
}

/// Splits `line` at the cut points; returns `(from, to, shared)` per resulting piece.
fn split(line: &Line<f64>, cuts: &Cuts) -> Vec<(Coord<f64>, Coord<f64>, bool)> {
    let mut breaks: Vec<(f64, Coord<f64>)> = cuts
        .points
        .iter()
        .map(|&c| (param(line, c), c))
        .filter(|(t, c)| *t > 0.0 && *t < 1.0 && *c != line.start && *c != line.end)
        .collect();
    breaks.sort_by(|x, y| x.0.total_cmp(&y.0));
    breaks.dedup_by(|x, y| x.1 == y.1);
    let mut all = vec![(0.0, line.start)];
    all.extend(breaks);
    all.push((1.0, line.end));
    let covered = Covered::new(&cuts.spans);
    all.windows(2)
        .filter(|w| w[0].1 != w[1].1)
        .map(|w| {
            let (t0, from) = w[0];
            let (t1, to) = w[1];
            let shared = covered.contains(t0, t1);
            (from, to, shared)
        })
        .collect()
}

/// Joins the pieces of consecutive segments of one input line, cutting at nodes.
fn assemble(segs: &[Seg], cuts: &[Cuts], nodes: &HashSet<(u64, u64)>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let mut owner_of_last: Option<usize> = None;
    for (seg, cut) in segs.iter().zip(cuts) {
        for (from, to, shared) in split(&seg.line, cut) {
            let joins = owner_of_last == Some(seg.owner)
                && out.last().is_some_and(|last| {
                    last.shared == shared
                        && last.coords.last() == Some(&from)
                        && !nodes.contains(&key(from))
                });
            if joins {
                out.last_mut().expect("a previous piece").coords.push(to);
            } else {
                out.push(Piece {
                    coords: vec![from, to],
                    shared,
                });
            }
            owner_of_last = Some(seg.owner);
        }
    }
    out
}

/// Nodes the linework `a` against `b`.
pub(crate) fn node(a: &[LineString<f64>], b: &[LineString<f64>]) -> Noded {
    let a = segments(&clean(a));
    let b = segments(&clean(b));
    let mut cuts_a: Vec<Cuts> = a.iter().map(|_| Cuts::default()).collect();
    let mut cuts_b: Vec<Cuts> = b.iter().map(|_| Cuts::default()).collect();
    let mut node_list: Vec<Coord<f64>> = Vec::new();

    let tree = RTree::bulk_load(
        b.iter()
            .enumerate()
            .map(|(i, s)| GeomWithData::new(rect(&s.line), i))
            .collect(),
    );
    for (i, sa) in a.iter().enumerate() {
        let query = envelope(&sa.line);
        for hit in tree.locate_in_envelope_intersecting(&query) {
            let j = hit.data;
            match line_intersection(sa.line, b[j].line) {
                None => {}
                Some(LineIntersection::SinglePoint { intersection, .. }) => {
                    cuts_a[i].points.push(intersection);
                    cuts_b[j].points.push(intersection);
                    node_list.push(intersection);
                }
                Some(LineIntersection::Collinear { intersection }) => {
                    for c in [intersection.start, intersection.end] {
                        cuts_a[i].points.push(c);
                        cuts_b[j].points.push(c);
                        node_list.push(c);
                    }
                    cuts_a[i].spans.push(span(&sa.line, &intersection));
                    cuts_b[j].spans.push(span(&b[j].line, &intersection));
                }
            }
        }
    }

    let mut seen = HashSet::new();
    node_list.retain(|c| seen.insert(key(*c)));
    Noded {
        a: assemble(&a, &cuts_a, &seen),
        b: assemble(&b, &cuts_b, &seen),
        nodes: node_list,
    }
}

/// Nodes a set of lines against itself in one pass and returns the union linework: every
/// line is cut where any other segment crosses, touches or overlaps it, and a stretch covered
/// by several lines appears once (in the first line that has it). Pieces come in input order.
///
/// This replaces folding [`node`] over the lines one at a time, which re-noded the whole
/// accumulated linework per line (quadratic) and cut pieces by sub-segments of earlier cuts.
/// One R-tree over all segments finds every candidate pair once, and each crossing is computed
/// from the original segments.
pub(crate) fn node_union(lines: &[LineString<f64>]) -> Vec<Piece> {
    let segs = segments(&clean(lines));
    let mut cuts: Vec<Cuts> = segs.iter().map(|_| Cuts::default()).collect();
    let mut node_list: Vec<Coord<f64>> = Vec::new();
    let tree = RTree::bulk_load(
        segs.iter()
            .enumerate()
            .map(|(i, s)| GeomWithData::new(rect(&s.line), i))
            .collect(),
    );
    for (i, si) in segs.iter().enumerate() {
        let query = envelope(&si.line);
        for hit in tree.locate_in_envelope_intersecting(&query) {
            let j = hit.data;
            if j <= i {
                continue;
            }
            // Consecutive segments of one line meet at a plain vertex, not at a node.
            let consecutive = si.owner == segs[j].owner && j == i + 1;
            match line_intersection(si.line, segs[j].line) {
                None => {}
                Some(LineIntersection::SinglePoint { intersection, .. }) => {
                    if !consecutive {
                        cuts[i].points.push(intersection);
                        cuts[j].points.push(intersection);
                        node_list.push(intersection);
                    }
                }
                Some(LineIntersection::Collinear { intersection }) => {
                    for c in [intersection.start, intersection.end] {
                        cuts[i].points.push(c);
                        cuts[j].points.push(c);
                        node_list.push(c);
                    }
                    // Only the later segment is "covered": the first one keeps the stretch.
                    cuts[j].spans.push(span(&segs[j].line, &intersection));
                }
            }
        }
    }
    let mut seen = HashSet::new();
    node_list.retain(|c| seen.insert(key(*c)));
    assemble(&segs, &cuts, &seen)
        .into_iter()
        .filter(|p| !p.shared)
        .collect()
}

fn span(line: &Line<f64>, overlap: &Line<f64>) -> (f64, f64) {
    let (t0, t1) = (param(line, overlap.start), param(line, overlap.end));
    (t0.min(t1), t0.max(t1))
}

fn rect(line: &Line<f64>) -> Rectangle<[f64; 2]> {
    Rectangle::from_corners(
        [line.start.x.min(line.end.x), line.start.y.min(line.end.y)],
        [line.start.x.max(line.end.x), line.start.y.max(line.end.y)],
    )
}

fn envelope(line: &Line<f64>) -> AABB<[f64; 2]> {
    AABB::from_corners(
        [line.start.x.min(line.end.x), line.start.y.min(line.end.y)],
        [line.start.x.max(line.end.x), line.start.y.max(line.end.y)],
    )
}

/// The midpoint of the first segment of a piece, used to classify the whole piece.
pub(crate) fn probe(piece: &Piece) -> Coord<f64> {
    let (p, q) = (piece.coords[0], piece.coords[1]);
    Coord {
        x: (p.x + q.x) / 2.0,
        y: (p.y + q.y) / 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covered_matches_the_scan_over_every_span() {
        let spans = vec![
            (0.1, 0.3),
            (0.25, 0.6),
            (0.7, 0.8),
            (0.0, 0.05),
            (0.75, 0.9),
        ];
        let covered = Covered::new(&spans);
        let grid: Vec<f64> = (0..=40).map(|i| f64::from(i) / 40.0).collect();
        for &t0 in &grid {
            for &t1 in grid.iter().filter(|&&t| t > t0) {
                let scan = spans.iter().any(|&(lo, hi)| lo <= t0 && t1 <= hi);
                assert_eq!(covered.contains(t0, t1), scan, "[{t0}, {t1}]");
            }
        }
        assert!(!Covered::new(&[]).contains(0.0, 1.0));
    }

    #[test]
    fn node_union_keeps_input_order_and_drops_repeated_stretches() {
        let line = |pts: &[(f64, f64)]| {
            LineString::new(pts.iter().map(|&(x, y)| Coord { x, y }).collect())
        };
        let pieces = node_union(&[
            line(&[(0.0, 0.0), (4.0, 0.0)]),
            line(&[(1.0, 0.0), (3.0, 0.0)]),
            line(&[(2.0, -1.0), (2.0, 1.0)]),
        ]);
        let ends: Vec<_> = pieces
            .iter()
            .map(|p| (p.coords[0], *p.coords.last().unwrap()))
            .collect();
        let at = |x: f64, y: f64| Coord { x, y };
        assert_eq!(
            ends,
            vec![
                (at(0.0, 0.0), at(1.0, 0.0)),
                (at(1.0, 0.0), at(2.0, 0.0)),
                (at(2.0, 0.0), at(3.0, 0.0)),
                (at(3.0, 0.0), at(4.0, 0.0)),
                (at(2.0, -1.0), at(2.0, 0.0)),
                (at(2.0, 0.0), at(2.0, 1.0)),
            ]
        );
    }
}
