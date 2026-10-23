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

use geo::{BoundingRect, Distance, Euclidean, Geometry};
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, AABB};

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
        let entries: Vec<Entry> = right_rows
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
        let tree = RTree::bulk_load(entries);

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
}
