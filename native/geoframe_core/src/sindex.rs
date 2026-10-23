//! R-tree spatial index and bulk pair query.
//!
//! # Algorithm
//!
//! An R-tree (bulk-loaded, `rstar`) is built over the bounding rectangles of the *right*
//! geometries. Each non-null left geometry probes it with its own bounding rectangle
//! (expanded by the distance for `dwithin`); the candidates are then refined with the exact
//! predicate, evaluated as `left.predicate(right)` (GeoPandas semantics). The envelope test
//! is a necessary condition for every supported predicate, so the refinement result equals
//! a brute-force evaluation over all pairs.
//!
//! # Contract
//!
//! Output is two parallel index vectors ordered by `(left, right)`. Null and empty rows (and
//! rows `geo` cannot represent) never match. A pair whose evaluation panics inside `geo`
//! does not match. Single-threaded by design; probing is embarrassingly parallel and is a
//! known gap.
//!
//! # Nearest search
//!
//! `nearest_pairs` walks the same tree best-first: a priority queue ordered by the distance
//! between the left envelope and each node envelope. That distance is a lower bound of the
//! exact geometry distance, so once the queue's smallest entry exceeds the best distance
//! found (plus a relative epsilon for rounding) no unseen geometry can tie or win and the
//! search stops. Every right geometry at exactly the minimum distance is returned (ties),
//! ordered by right row. `max_distance` is an inclusive cutoff that also bounds the walk.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use geo::{BoundingRect, Distance, Euclidean, Geometry};
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, RTreeNode, RTreeObject, AABB};

use crate::predicates::{guarded, pair_predicate};
use crate::{GeoColumn, Predicate};

/// A predicate the spatial index can answer: every topological relation except `disjoint`
/// (which cannot be pruned by envelopes), plus distance-within.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueryPredicate {
    Relation(Predicate),
    /// Planar distance at most the limit (inclusive).
    DWithin(f64),
}

impl QueryPredicate {
    /// Parses a predicate name; `distance` is required for (and only used by) `dwithin`.
    pub fn from_name(name: &str, distance: Option<f64>) -> Result<Self, String> {
        if name == "dwithin" {
            return match distance {
                Some(d) if d.is_finite() && d >= 0.0 => Ok(Self::DWithin(d)),
                Some(d) => Err(format!(
                    "dwithin distance must be finite and non-negative, got {d}"
                )),
                None => Err("dwithin requires a distance".to_string()),
            };
        }
        match Predicate::from_name(name) {
            Some(Predicate::Disjoint) => {
                Err("disjoint cannot be answered by a spatial index".to_string())
            }
            Some(p) => Ok(Self::Relation(p)),
            None => Err(format!("unknown predicate: {name}")),
        }
    }

    fn padding(self) -> f64 {
        match self {
            Self::DWithin(d) => d,
            Self::Relation(_) => 0.0,
        }
    }

    fn holds(self, left: &Geometry<f64>, right: &Geometry<f64>) -> bool {
        match self {
            Self::Relation(p) => pair_predicate(left, right, p) == Some(true),
            Self::DWithin(d) => guarded(|| Euclidean.distance(left, right) <= d).unwrap_or(false),
        }
    }
}

/// Parallel `(left, right, distance)` vectors.
pub type NearestPairs = (Vec<u32>, Vec<u32>, Vec<f64>);

type Entry = GeomWithData<Rectangle<[f64; 2]>, u32>;

fn envelope(geometry: &Geometry<f64>, pad: f64) -> Option<AABB<[f64; 2]>> {
    let rect = geometry.bounding_rect()?;
    Some(AABB::from_corners(
        [rect.min().x - pad, rect.min().y - pad],
        [rect.max().x + pad, rect.max().y + pad],
    ))
}

impl GeoColumn {
    /// All `(left, right)` row pairs satisfying `left.predicate(right)`, as parallel index
    /// vectors ordered by left then right row.
    pub fn query_pairs(
        &self,
        right: &GeoColumn,
        predicate: QueryPredicate,
    ) -> Result<(Vec<u32>, Vec<u32>), String> {
        self.check_crs(right)?;
        if self.len() > u32::MAX as usize || right.len() > u32::MAX as usize {
            return Err("spatial index supports at most 2^32 - 1 rows per side".to_string());
        }
        let right_rows = right.geo_rows();
        let tree = build_tree(&right_rows);

        let (mut left_idx, mut right_idx) = (Vec::new(), Vec::new());
        let mut candidates: Vec<u32> = Vec::new();
        for (i, row) in self.geo_rows().iter().enumerate() {
            let Some(geometry) = row else { continue };
            let Some(probe) = envelope(geometry, predicate.padding()) else {
                continue;
            };
            candidates.clear();
            candidates.extend(tree.locate_in_envelope_intersecting(&probe).map(|e| e.data));
            candidates.sort_unstable();
            for &j in &candidates {
                if let Some(other) = &right_rows[j as usize] {
                    if predicate.holds(geometry, other) {
                        left_idx.push(i as u32);
                        right_idx.push(j);
                    }
                }
            }
        }
        Ok((left_idx, right_idx))
    }

    /// For each left row, every right row at the minimum planar distance (ties included),
    /// as parallel `(left, right, distance)` vectors ordered by left then right row.
    ///
    /// `max_distance` drops matches farther than the limit (inclusive). With `exclusive`,
    /// right geometries topologically equal to the left one are ignored. Null and empty
    /// rows never match and never match anything.
    pub fn nearest_pairs(
        &self,
        right: &GeoColumn,
        max_distance: Option<f64>,
        exclusive: bool,
    ) -> Result<NearestPairs, String> {
        self.check_crs(right)?;
        if let Some(d) = max_distance {
            if !(d.is_finite() && d >= 0.0) {
                return Err(format!(
                    "max_distance must be finite and non-negative, got {d}"
                ));
            }
        }
        if self.len() > u32::MAX as usize || right.len() > u32::MAX as usize {
            return Err("spatial index supports at most 2^32 - 1 rows per side".to_string());
        }
        let right_rows = right.geo_rows();
        let tree = build_tree(&right_rows);

        let (mut left_idx, mut right_idx, mut distances) = (Vec::new(), Vec::new(), Vec::new());
        for (i, row) in self.geo_rows().iter().enumerate() {
            let Some(geometry) = row else { continue };
            let Some(probe) = envelope(geometry, 0.0) else {
                continue;
            };
            let mut found = nearest_matches(
                &tree,
                &right_rows,
                geometry,
                &probe,
                max_distance,
                exclusive,
            );
            found.sort_by_key(|&(j, _)| j);
            for (j, d) in found {
                left_idx.push(i as u32);
                right_idx.push(j);
                distances.push(d);
            }
        }
        Ok((left_idx, right_idx, distances))
    }
}

fn build_tree(rows: &[Option<Geometry<f64>>]) -> RTree<Entry> {
    let entries: Vec<Entry> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, row)| {
            let aabb = envelope(row.as_ref()?, 0.0)?;
            Some(GeomWithData::new(
                Rectangle::from_corners(aabb.lower(), aabb.upper()),
                i as u32,
            ))
        })
        .collect();
    RTree::bulk_load(entries)
}

/// Queue item ordered so the `BinaryHeap` (a max-heap) pops the smallest distance first.
struct Pending<'a> {
    distance: f64,
    node: &'a RTreeNode<Entry>,
}

impl PartialEq for Pending<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.distance.total_cmp(&other.distance) == Ordering::Equal
    }
}
impl Eq for Pending<'_> {}
impl PartialOrd for Pending<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Pending<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        other.distance.total_cmp(&self.distance)
    }
}

/// Euclidean distance between two boxes (0 when they overlap).
fn box_distance(a: &AABB<[f64; 2]>, b: &AABB<[f64; 2]>) -> f64 {
    let gap = |axis: usize| {
        (a.lower()[axis] - b.upper()[axis])
            .max(b.lower()[axis] - a.upper()[axis])
            .max(0.0)
    };
    gap(0).hypot(gap(1))
}

/// Relative slack so rounding differences between the envelope bound and the exact
/// distance never prune a geometry that ties the best one.
fn slack(distance: f64) -> f64 {
    distance + distance.abs() * 1e-12 + 1e-12
}

fn nearest_matches(
    tree: &RTree<Entry>,
    rows: &[Option<Geometry<f64>>],
    geometry: &Geometry<f64>,
    probe: &AABB<[f64; 2]>,
    max_distance: Option<f64>,
    exclusive: bool,
) -> Vec<(u32, f64)> {
    let mut best = f64::INFINITY;
    let mut found: Vec<(u32, f64)> = Vec::new();
    let mut queue = BinaryHeap::new();
    for child in tree.root().children() {
        queue.push(Pending {
            distance: box_distance(probe, &child.envelope()),
            node: child,
        });
    }
    while let Some(Pending { distance, node }) = queue.pop() {
        let bound = max_distance.map_or(best, |m| best.min(m));
        if distance > slack(bound) && bound.is_finite() {
            break;
        }
        match node {
            RTreeNode::Parent(parent) => {
                for child in parent.children() {
                    queue.push(Pending {
                        distance: box_distance(probe, &child.envelope()),
                        node: child,
                    });
                }
            }
            RTreeNode::Leaf(entry) => {
                let Some(other) = &rows[entry.data as usize] else {
                    continue;
                };
                if exclusive && pair_predicate(geometry, other, Predicate::Equals) == Some(true) {
                    continue;
                }
                let Some(d) = guarded(|| Euclidean.distance(geometry, other)) else {
                    continue;
                };
                if !d.is_finite() || max_distance.is_some_and(|m| d > m) || d > best {
                    continue;
                }
                if d < best {
                    best = d;
                    found.clear();
                }
                found.push((entry.data, d));
            }
        }
    }
    found.retain(|&(_, d)| d == best);
    found
}
