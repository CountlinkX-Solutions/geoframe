//! Batch E: constructive geometry with GeoPandas/Shapely names.
//!
//! * [`concave`]: `concave_hull`, a port of the GEOS/JTS border-erosion algorithm.
//! * [`bounding`]: `minimum_rotated_rectangle`, `minimum_bounding_circle`,
//!   `minimum_bounding_radius` and `maximum_inscribed_circle`.
//! * [`triangulate`]: `delaunay_triangles`, `constrained_delaunay_triangles` and
//!   `voronoi_polygons`.
//! * [`sampling`]: `sample_points` with a seeded generator (no extra dependency).
//! * [`grid`]: `make_grid` (GeoPandas 1.2).
//!
//! All of it is planar and 2D, and uses the null policy of [`GeoColumn::map_geometry`]: null
//! rows stay null, and so do rows whose geometry type the operation does not define. Where
//! GEOS returns an empty geometry we return an empty geometry too.

mod bounding;
mod concave;
mod grid;
mod sampling;
mod triangulate;

use std::collections::HashSet;

use geo::{Coord, CoordsIter, Geometry, GeometryCollection, Rect};

use crate::predicates::guarded;
use crate::GeoColumn;

pub use grid::{GridFeature, GridType};

/// The distinct vertices of a geometry in first-seen order (NaN vertices are dropped).
pub(super) fn unique_coords(geom: &Geometry<f64>) -> Vec<Coord<f64>> {
    let mut seen = HashSet::new();
    geom.coords_iter()
        .filter(|c| c.x.is_finite() && c.y.is_finite())
        .filter(|c| seen.insert(coord_key(*c)))
        .collect()
}

/// A hashable key for a coordinate (`-0.0` and `0.0` are the same vertex).
pub(super) fn coord_key(c: Coord<f64>) -> (u64, u64) {
    ((c.x + 0.0).to_bits(), (c.y + 0.0).to_bits())
}

pub(super) fn empty_collection() -> Geometry<f64> {
    Geometry::GeometryCollection(GeometryCollection::new_from(vec![]))
}

impl GeoColumn {
    /// The concave hull of each geometry's vertices (GEOS `ConcaveHull`): `ratio` in `[0, 1]`
    /// is the share of the Delaunay edge-length range above which border triangles are eroded
    /// (0 erodes the most, 1 gives the convex hull); `allow_holes` also removes long interior
    /// triangles. Fewer than three or collinear vertices give a point or a line.
    pub fn concave_hull(&self, ratio: f64, allow_holes: bool) -> Result<Self, String> {
        if !(0.0..=1.0).contains(&ratio) {
            return Err(format!("ratio must be between 0 and 1, got {ratio}"));
        }
        self.map_geometry(|g| concave::concave_hull(g, ratio, allow_holes))
    }

    /// The minimum-area rotated rectangle around each geometry; a point or a line when the
    /// vertices are fewer than three or collinear.
    pub fn minimum_rotated_rectangle(&self) -> Result<Self, String> {
        self.map_geometry(bounding::minimum_rotated_rectangle)
    }

    /// The minimum bounding circle as a 32-gon laid out like Shapely's buffered center (the
    /// point itself for a zero radius).
    pub fn minimum_bounding_circle(&self) -> Result<Self, String> {
        self.map_geometry(|g| bounding::bounding_circle(g).map(bounding::circle_polygon))
    }

    /// Radius of the minimum bounding circle of each geometry; empty geometries give `None`.
    pub fn minimum_bounding_radius(&self) -> Vec<Option<f64>> {
        self.geo_rows()
            .iter()
            .map(|row| {
                row.as_ref()
                    .and_then(|g| guarded(|| bounding::bounding_circle(g)).flatten())
                    .map(|c| c.radius)
            })
            .collect()
    }

    /// The line from the center of the largest circle inside each polygon to the nearest
    /// boundary point. `tolerance` (default: the larger bounding-box side over 1000) bounds
    /// how far the center may be from the optimum. Non-polygons are null.
    pub fn maximum_inscribed_circle(&self, tolerance: Option<f64>) -> Result<Self, String> {
        if tolerance.is_some_and(|t| t.is_nan() || t <= 0.0) {
            return Err("tolerance must be positive".to_string());
        }
        self.map_geometry(|g| bounding::maximum_inscribed_circle(g, tolerance))
    }

    /// Every non-null geometry as one collection, the point cloud GeoPandas triangulates.
    fn point_cloud(&self) -> Geometry<f64> {
        Geometry::GeometryCollection(GeometryCollection::new_from(
            self.geo_rows().into_iter().flatten().collect(),
        ))
    }

    /// The Delaunay triangulation of the vertices of the whole column, one triangle per row
    /// (GeoPandas `delaunay_triangles`), or with `only_edges` one `LineString` per unique
    /// edge. Vertices closer than `tolerance` are merged first. Fewer than three or collinear
    /// vertices give no triangles.
    pub fn delaunay_triangles(&self, tolerance: f64, only_edges: bool) -> Result<Self, String> {
        if tolerance.is_nan() || tolerance < 0.0 {
            return Err("tolerance must be a non-negative number".to_string());
        }
        let parts =
            guarded(|| triangulate::delaunay_parts(&self.point_cloud(), tolerance, only_edges))
                .flatten()
                .ok_or_else(|| "the Delaunay triangulation failed".to_string())?;
        self.rebuild(&parts.into_iter().map(Some).collect::<Vec<_>>())
    }

    /// Triangles that fill each polygon, respecting holes, using only its vertices. Other
    /// types give an empty collection.
    pub fn constrained_delaunay_triangles(&self) -> Result<Self, String> {
        self.map_geometry(triangulate::constrained_delaunay_triangles)
    }

    /// The Voronoi diagram of the vertices of the whole column, one polygon per row and per
    /// distinct vertex, in vertex order (GeoPandas `voronoi_polygons`). Cells are clipped to the
    /// envelope of the vertices grown by its larger side and extended to cover `extend_to` (a
    /// one-row column); with `only_edges` the rows are the interior cell sides as lines. Fewer
    /// than two vertices give no rows. `ordered` is accepted for Shapely compatibility: cells
    /// always come in vertex order.
    pub fn voronoi_polygons(
        &self,
        tolerance: f64,
        extend_to: Option<&GeoColumn>,
        only_edges: bool,
        _ordered: bool,
    ) -> Result<Self, String> {
        if tolerance.is_nan() || tolerance < 0.0 {
            return Err("tolerance must be a non-negative number".to_string());
        }
        let extent = match extend_to {
            None => None,
            Some(column) => {
                if column.len() != 1 {
                    return Err(format!(
                        "extend_to must be a single geometry, got {} rows",
                        column.len()
                    ));
                }
                self.check_crs(column)?;
                let bounds = column.bounds()[0]
                    .ok_or_else(|| "extend_to must be a non-empty geometry".to_string())?;
                Some(Rect::new(
                    Coord {
                        x: bounds[0],
                        y: bounds[1],
                    },
                    Coord {
                        x: bounds[2],
                        y: bounds[3],
                    },
                ))
            }
        };
        let parts = guarded(|| {
            triangulate::voronoi_parts(&self.point_cloud(), tolerance, extent, only_edges)
        })
        .flatten()
        .ok_or_else(|| "the Voronoi diagram failed".to_string())?;
        self.rebuild(&parts.into_iter().map(Some).collect::<Vec<_>>())
    }

    /// `sizes[i]` uniform random points inside (polygons) or along (lines) row `i`, as a
    /// `MultiPoint`; one size broadcasts. Other types and zero-area polygons give an empty
    /// `MultiPoint`. The same `seed` always gives the same points.
    pub fn sample_points(&self, sizes: &[usize], seed: Option<u64>) -> Result<Self, String> {
        if sizes.len() != 1 && sizes.len() != self.len() {
            return Err(format!(
                "size must be one integer or have the same length as the series ({}), got {}",
                self.len(),
                sizes.len()
            ));
        }
        let total = sizes.iter().try_fold(0usize, |acc, &n| acc.checked_add(n));
        // A broadcast size applies to every row.
        let total = total.and_then(|t| {
            if sizes.len() == 1 {
                t.checked_mul(self.len().max(1))
            } else {
                Some(t)
            }
        });
        if total.is_none_or(|t| t > sampling::MAX_SAMPLES) {
            return Err(format!(
                "sample_points: total number of samples exceeds the maximum of {}",
                sampling::MAX_SAMPLES
            ));
        }
        let mut rng = sampling::Rng::new(seed);
        let rows: Vec<Option<Geometry<f64>>> = self
            .geo_rows()
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let size = if sizes.len() == 1 { sizes[0] } else { sizes[i] };
                row.as_ref()
                    .and_then(|g| guarded(|| sampling::sample(g, size, &mut rng)).flatten())
            })
            .collect();
        self.rebuild(&rows)
    }

    /// A square or hexagonal grid over the total bounds of this column (GeoPandas
    /// `make_grid`): `feature` picks centers, corners or cell polygons; `offset` moves the
    /// grid origin (default: the lower left corner of the bounds); `intersect` keeps only the
    /// elements that intersect a geometry; `flat_topped` orients hexagons. The CRS is kept.
    pub fn make_grid(
        &self,
        cell_size: f64,
        grid_type: GridType,
        feature: GridFeature,
        offset: Option<(f64, f64)>,
        intersect: bool,
        flat_topped: bool,
    ) -> Result<Self, String> {
        let bounds = self
            .total_bounds()
            .filter(|_| !self.is_empty())
            .ok_or_else(|| "input geometry is empty".to_string())?;
        let rect = Rect::new(
            Coord {
                x: bounds[0],
                y: bounds[1],
            },
            Coord {
                x: bounds[2],
                y: bounds[3],
            },
        );
        let inputs: Vec<Geometry<f64>> = self.geo_rows().into_iter().flatten().collect();
        let spec = grid::GridSpec {
            cell_size,
            grid: grid_type,
            feature,
            offset,
            intersect,
            flat_topped,
        };
        let cells = grid::make_grid(rect, &inputs, &spec)?;
        self.rebuild(&cells.into_iter().map(Some).collect::<Vec<_>>())
    }
}
