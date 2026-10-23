//! Affine completeness: `skew`, `affine_transform`, per-row parameters for the affine family,
//! and `wrap_longitude`.
//!
//! Per-row parameters are slices with one entry per row; a `None` entry (or a `None` origin)
//! nulls that row, as does a null geometry. Slice lengths must equal the column length.
//! Scalar `translate`/`scale`/`rotate` stay in `unary.rs` as the fast path.

use geo::{AffineTransform, Geometry, MapCoords};

use crate::reproject::crs_is_geographic;
use crate::zm_bridge::Affine3;
use crate::{GeoColumn, Origin};

fn check_len(name: &str, got: usize, want: usize) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!(
            "{name} has {got} entries but the series has {want} rows"
        ))
    }
}

impl GeoColumn {
    /// Builds each row's transform from its index with `make`; `None` nulls the row.
    fn affine_rows(
        &self,
        make: impl Fn(usize, &Geometry<f64>) -> Option<AffineTransform<f64>>,
    ) -> Result<Self, String> {
        self.affine3_rows(|i, g| make(i, g).map(Affine3::flat))
    }

    /// Per-row [`GeoColumn::translate`].
    pub fn translate_rows(
        &self,
        xoff: &[Option<f64>],
        yoff: &[Option<f64>],
    ) -> Result<Self, String> {
        check_len("xoff", xoff.len(), self.len())?;
        check_len("yoff", yoff.len(), self.len())?;
        self.affine_rows(|i, _| Some(AffineTransform::translate(xoff[i]?, yoff[i]?)))
    }

    /// Per-row [`GeoColumn::scale`].
    pub fn scale_rows(
        &self,
        xfact: &[Option<f64>],
        yfact: &[Option<f64>],
        origins: &[Option<Origin>],
    ) -> Result<Self, String> {
        check_len("xfact", xfact.len(), self.len())?;
        check_len("yfact", yfact.len(), self.len())?;
        check_len("origin", origins.len(), self.len())?;
        self.affine_rows(|i, g| {
            let at = origins[i]?.resolve(g)?;
            Some(AffineTransform::scale(xfact[i]?, yfact[i]?, at))
        })
    }

    /// Per-row [`GeoColumn::rotate`] (counter-clockwise degrees).
    pub fn rotate_rows(
        &self,
        degrees: &[Option<f64>],
        origins: &[Option<Origin>],
    ) -> Result<Self, String> {
        check_len("angle", degrees.len(), self.len())?;
        check_len("origin", origins.len(), self.len())?;
        self.affine_rows(|i, g| {
            let at = origins[i]?.resolve(g)?;
            Some(AffineTransform::rotate(degrees[i]?, at))
        })
    }

    /// Shears by `xs` along x and `ys` along y about each row's origin (Shapely `skew`).
    /// Angles are degrees unless `use_radians`.
    pub fn skew_rows(
        &self,
        xs: &[Option<f64>],
        ys: &[Option<f64>],
        origins: &[Option<Origin>],
        use_radians: bool,
    ) -> Result<Self, String> {
        check_len("xs", xs.len(), self.len())?;
        check_len("ys", ys.len(), self.len())?;
        check_len("origin", origins.len(), self.len())?;
        let deg = |v: f64| if use_radians { v.to_degrees() } else { v };
        self.affine_rows(|i, g| {
            let at = origins[i]?.resolve(g)?;
            Some(AffineTransform::skew(deg(xs[i]?), deg(ys[i]?), at))
        })
    }

    /// Applies the 2D matrix `[a, b, d, e, xoff, yoff]` (Shapely order):
    /// `x' = a x + b y + xoff`, `y' = d x + e y + yoff`.
    pub fn affine_transform(&self, matrix: &[f64; 6]) -> Result<Self, String> {
        let [a, b, d, e, xoff, yoff] = *matrix;
        let transform = AffineTransform::new(a, b, xoff, d, e, yoff);
        self.affine_rows(|_, _| Some(transform))
    }

    /// Applies Shapely's 3D matrix `[a, b, c, d, e, f, g, h, i, xoff, yoff, zoff]`:
    /// `x' = a x + b y + c z + xoff`, `y' = d x + e y + f z + yoff`,
    /// `z' = g x + h y + i z + zoff`. Geometries without Z use `a, b, d, e, xoff, yoff` only;
    /// M is dropped.
    pub fn affine_transform3(&self, matrix: &[f64; 12]) -> Result<Self, String> {
        let transform = Affine3::from_matrix(matrix);
        self.affine3_rows(|_, _| Some(transform))
    }

    /// Wraps every x into `[-180, 180)` (so `180` becomes `-180`). Geometries are not split at
    /// the antimeridian, only their coordinates move. Fails for projected CRSs; a column
    /// without a CRS is assumed to be in degrees.
    pub fn wrap_longitude(&self) -> Result<Self, String> {
        if self.is_geographic() == Some(false) {
            return Err(
                "wrap_longitude needs a geographic CRS (or none); this CRS is projected"
                    .to_string(),
            );
        }
        self.map_geometry(|g| {
            Some(g.map_coords(|c| geo::Coord {
                x: wrap_degrees(c.x),
                y: c.y,
            }))
        })
    }

    /// Whether the CRS is geographic (lon/lat); `None` without a CRS or when it is unknown.
    pub fn is_geographic(&self) -> Option<bool> {
        self.crs().and_then(|c| crs_is_geographic(&c))
    }
}

fn wrap_degrees(x: f64) -> f64 {
    if x.is_finite() && !(-180.0..180.0).contains(&x) {
        (x + 180.0).rem_euclid(360.0) - 180.0
    } else {
        x
    }
}
