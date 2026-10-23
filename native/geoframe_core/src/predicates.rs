//! Element-wise binary predicates, distance and DE-9IM between two geometry columns.
//!
//! # Pairing
//!
//! Columns pair positionally. The right-hand column may have length 1, in which case it
//! broadcasts against every row of the left one; any other length mismatch is an error.
//! Two columns whose CRS are both set and different are rejected (GeoPandas only warns; we
//! fail loudly because a silent answer in mixed coordinate systems is meaningless).
//!
//! # Nulls
//!
//! A row is null when either side is null or cannot be expressed as a `geo` geometry (such as
//! an empty point), the same policy as the measures in [`GeoColumn`].
//!
//! # Evaluation
//!
//! Topological predicates compute one DE-9IM [`IntersectionMatrix`] per pair and read the
//! requested predicate from it. When one side is a single geometry it is prepared once
//! ([`PreparedGeometry`]) so its topology graph is not rebuilt per row. `intersects` skips the
//! matrix and uses the cheaper [`geo::Intersects`]; `distance` is planar Euclidean.
//!
//! Each pair runs under `catch_unwind`: `geo`'s relate can panic on degenerate input, and a
//! bad row must yield null rather than unwind into the BEAM.

use std::panic::{catch_unwind, AssertUnwindSafe};

use geo::coordinate_position::CoordPos;
use geo::dimensions::Dimensions;
use geo::relate::{IntersectionMatrix, Relate};
use geo::{Euclidean, Geometry, Intersects, PreparedGeometry};

use crate::GeoColumn;

/// A boolean binary predicate between two geometries (GeoPandas semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Predicate {
    Intersects,
    Contains,
    Within,
    Touches,
    Crosses,
    Overlaps,
    Disjoint,
    Covers,
    CoveredBy,
    /// Topological equality: same point set regardless of vertex order or start vertex.
    Equals,
}

impl Predicate {
    /// Parses the GeoPandas-style predicate name used by the NIF layer.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "intersects" => Self::Intersects,
            "contains" => Self::Contains,
            "within" => Self::Within,
            "touches" => Self::Touches,
            "crosses" => Self::Crosses,
            "overlaps" => Self::Overlaps,
            "disjoint" => Self::Disjoint,
            "covers" => Self::Covers,
            "covered_by" => Self::CoveredBy,
            "geom_equals" => Self::Equals,
            _ => return None,
        })
    }
}

/// A DE-9IM matrix read in the order (left row geometry, right geometry).
///
/// When the right-hand geometry is prepared, `geo` relates it as the left argument, so the
/// matrix comes back transposed; `transposed` records that and undoes it on read.
struct De9im {
    matrix: IntersectionMatrix,
    transposed: bool,
}

impl De9im {
    fn eval(&self, predicate: Predicate) -> bool {
        let m = &self.matrix;
        let t = self.transposed;
        match predicate {
            Predicate::Intersects => m.is_intersects(),
            Predicate::Disjoint => m.is_disjoint(),
            Predicate::Touches => m.is_touches(),
            Predicate::Crosses => m.is_crosses(),
            Predicate::Overlaps => m.is_overlaps(),
            Predicate::Equals => m.is_equal_topo(),
            Predicate::Contains if t => m.is_within(),
            Predicate::Contains => m.is_contains(),
            Predicate::Within if t => m.is_contains(),
            Predicate::Within => m.is_within(),
            Predicate::Covers if t => m.is_coveredby(),
            Predicate::Covers => m.is_covers(),
            Predicate::CoveredBy if t => m.is_covers(),
            Predicate::CoveredBy => m.is_coveredby(),
        }
    }

    /// The nine-character DE-9IM code, e.g. `212101212`.
    fn code(&self) -> String {
        const POSITIONS: [CoordPos; 3] =
            [CoordPos::Inside, CoordPos::OnBoundary, CoordPos::Outside];
        POSITIONS
            .iter()
            .flat_map(|&a| POSITIONS.iter().map(move |&b| (a, b)))
            .map(|(a, b)| {
                match if self.transposed {
                    self.matrix.get(b, a)
                } else {
                    self.matrix.get(a, b)
                } {
                    Dimensions::Empty => 'F',
                    Dimensions::ZeroDimensional => '0',
                    Dimensions::OneDimensional => '1',
                    Dimensions::TwoDimensional => '2',
                }
            })
            .collect()
    }
}

/// Runs `f`, turning a panic inside `geo` into `None`.
fn guarded<T>(f: impl FnOnce() -> T) -> Option<T> {
    catch_unwind(AssertUnwindSafe(f)).ok()
}

impl GeoColumn {
    /// Resolves the CRS and length rules shared by every binary operation. Returns whether
    /// `other` broadcasts (length 1 against a column of a different length).
    fn check_pairing(&self, other: &GeoColumn) -> Result<bool, String> {
        if let (Some(a), Some(b)) = (self.crs(), other.crs()) {
            if a != b {
                return Err(format!(
                    "CRS mismatch: left is {a}, right is {b}; reproject one side first"
                ));
            }
        }
        if self.len() == other.len() {
            Ok(false)
        } else if other.len() == 1 {
            Ok(true)
        } else {
            Err(format!(
                "length mismatch: left has {} rows, right has {} (a single geometry broadcasts)",
                self.len(),
                other.len()
            ))
        }
    }

    /// Applies `pair` to each row pair. When the right side is a single geometry it is turned
    /// into `P` once by `prepare` and each row is evaluated with `against`.
    fn zip_pairs<T, P>(
        &self,
        other: &GeoColumn,
        pair: impl Fn(&Geometry<f64>, &Geometry<f64>) -> Option<T>,
        prepare: impl Fn(&Geometry<f64>) -> P,
        against: impl Fn(&P, &Geometry<f64>) -> Option<T>,
    ) -> Result<Vec<Option<T>>, String> {
        let broadcasts = self.check_pairing(other)?;
        let left = self.geo_rows();
        let right = other.geo_rows();
        if broadcasts {
            return Ok(match &right[0] {
                None => left.iter().map(|_| None).collect(),
                Some(scalar) => {
                    let prepared = prepare(scalar);
                    left.iter()
                        .map(|row| row.as_ref().and_then(|g| against(&prepared, g)))
                        .collect()
                }
            });
        }
        Ok(left
            .iter()
            .zip(&right)
            .map(|(a, b)| match (a, b) {
                (Some(a), Some(b)) => pair(a, b),
                _ => None,
            })
            .collect())
    }

    /// Relates each pair once. With a broadcast right side the scalar is prepared a single
    /// time and the (transposed) matrix is read back in (row, scalar) order.
    fn relate_pairs<T>(
        &self,
        other: &GeoColumn,
        read: impl Fn(&De9im) -> T,
    ) -> Result<Vec<Option<T>>, String> {
        self.zip_pairs(
            other,
            |a, b| {
                guarded(|| De9im {
                    matrix: a.relate(b),
                    transposed: false,
                })
                .map(|m| read(&m))
            },
            |scalar| PreparedGeometry::from(scalar.clone()),
            |prepared, row| {
                guarded(|| De9im {
                    matrix: prepared.relate(row),
                    transposed: true,
                })
                .map(|m| read(&m))
            },
        )
    }

    /// Element-wise boolean predicate between this column and `other`.
    pub fn predicate(
        &self,
        other: &GeoColumn,
        predicate: Predicate,
    ) -> Result<Vec<Option<bool>>, String> {
        if predicate == Predicate::Intersects {
            return self.zip_pairs(
                other,
                |a, b| guarded(|| a.intersects(b)),
                |scalar| scalar.clone(),
                |scalar, row| guarded(|| row.intersects(scalar)),
            );
        }
        self.relate_pairs(other, |m| m.eval(predicate))
    }

    /// Planar Euclidean distance; zero when the geometries intersect.
    pub fn distance(&self, other: &GeoColumn) -> Result<Vec<Option<f64>>, String> {
        use geo::Distance;
        self.zip_pairs(
            other,
            |a, b| guarded(|| Euclidean.distance(a, b)),
            |scalar| scalar.clone(),
            |scalar, row| guarded(|| Euclidean.distance(row, scalar)),
        )
    }

    /// Whether the distance between each pair is at most `limit` (inclusive).
    pub fn dwithin(&self, other: &GeoColumn, limit: f64) -> Result<Vec<Option<bool>>, String> {
        Ok(self
            .distance(other)?
            .into_iter()
            .map(|d| d.map(|d| d <= limit))
            .collect())
    }

    /// The DE-9IM string (`"212101212"`) relating each pair, left geometry first.
    pub fn relate(&self, other: &GeoColumn) -> Result<Vec<Option<String>>, String> {
        self.relate_pairs(other, De9im::code)
    }
}
