//! The Z/M-carrying bridge between the GeoArrow column and `geo`.
//!
//! `geo` geometries are 2D, so the generic constructive path ([`GeoColumn::map_geometry`])
//! returns 2D. Operations that Shapely/GEOS defines to keep Z (and sometimes M) use one of two
//! helpers instead:
//!
//! * **Vertex maps** ([`GeoColumn::map_vertices`], [`GeoColumn::reverse_vertices`]) rewrite the
//!   ISO WKB of each row in place: `(x, y)` goes through a function, Z and M pass through or
//!   are transformed. Used by the affine family and `to_crs`.
//! * **Restoring** ([`GeoColumn::map_geometry_zm`], [`GeoColumn::rebuild_zm`]) runs the 2D
//!   algorithm and then gives every surviving vertex the Z (and M) of the input vertex with
//!   exactly the same XY. Used by operations that keep a subset of vertices (`simplify`,
//!   `normalize`, `boundary`, parts, `set_precision`, `snap`). A vertex with no counterpart
//!   gets NaN, as GEOS does. When one XY occurs with several Z values the first one wins.
//!
//! Rows without Z or M are never touched, so a 2D column pays nothing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use geo::{AffineOps, AffineTransform, Coord, Geometry, LineString, Polygon};
use geoarrow_array::builder::GeometryBuilder;

use crate::zm::{dimensions, rewrite, Edit, VertexFn};
use crate::GeoColumn;

/// Which ordinates survive an operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Keep {
    /// Z and M, as the input had them.
    Both,
    /// Z only; M is dropped.
    Z,
}

impl Keep {
    fn apply(self, (z, m): (bool, bool)) -> (bool, bool) {
        match self {
            Self::Both => (z, m),
            Self::Z => (z, false),
        }
    }
}

/// `(z, m)` per XY, keyed by the bit patterns of x and y.
pub(crate) type Lookup = HashMap<(u64, u64), (f64, f64)>;

fn key(c: Coord<f64>) -> (u64, u64) {
    (c.x.to_bits(), c.y.to_bits())
}

/// The first `(z, m)` of every vertex of an ISO WKB geometry, indexed by `map(xy)`.
pub(crate) fn lookup_of(bytes: &[u8], map: &dyn Fn(Coord<f64>) -> Coord<f64>) -> Lookup {
    let seen = RefCell::new(Lookup::new());
    let visit = |x: f64, y: f64, z: f64, m: f64| {
        seen.borrow_mut()
            .entry(key(map(Coord { x, y })))
            .or_insert((z, m));
        Ok((x, y, z))
    };
    // The rewritten bytes are discarded: the walk only visits the vertices.
    let _ = rewrite(bytes, Edit::Map(&visit));
    seen.into_inner()
}

/// How [`GeoColumn::rebuild_zm`] restores ordinates.
pub(crate) struct Restore<'a> {
    pub keep: Keep,
    /// Applied to the input vertices before they are indexed (for example a grid snap).
    pub index: &'a dyn Fn(Coord<f64>) -> Coord<f64>,
    /// Per output row: vertices whose ordinates win over the input's (the snap reference).
    pub reference: Option<&'a dyn Fn(usize) -> Option<Lookup>>,
}

pub(crate) fn identity(c: Coord<f64>) -> Coord<f64> {
    c
}

impl<'a> Restore<'a> {
    pub(crate) fn keep(keep: Keep) -> Self {
        Self {
            keep,
            index: &identity,
            reference: None,
        }
    }
}

// -- ISO WKB writer for geo geometries ---------------------------------------------------------

struct Writer<'a> {
    out: Vec<u8>,
    z: bool,
    m: bool,
    ordinates: &'a dyn Fn(Coord<f64>) -> (f64, f64),
}

impl Writer<'_> {
    fn u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    fn header(&mut self, base: u32) {
        self.out.push(1);
        self.u32(base + 1000 * u32::from(self.z) + 2000 * u32::from(self.m));
    }

    fn coord(&mut self, c: Coord<f64>) {
        let (z, m) = (self.ordinates)(c);
        for value in [
            Some(c.x),
            Some(c.y),
            self.z.then_some(z),
            self.m.then_some(m),
        ]
        .into_iter()
        .flatten()
        {
            self.out.extend_from_slice(&value.to_le_bytes());
        }
    }

    fn coords(&mut self, line: &LineString<f64>) {
        self.u32(line.0.len() as u32);
        for c in &line.0 {
            self.coord(*c);
        }
    }

    fn polygon_body(&mut self, polygon: &Polygon<f64>) {
        if polygon.exterior().0.is_empty() && polygon.interiors().is_empty() {
            self.u32(0);
            return;
        }
        self.u32(1 + polygon.interiors().len() as u32);
        self.coords(polygon.exterior());
        for ring in polygon.interiors() {
            self.coords(ring);
        }
    }

    fn geometry(&mut self, geometry: &Geometry<f64>) {
        match geometry {
            Geometry::Point(p) => {
                self.header(1);
                self.coord(p.0);
            }
            Geometry::Line(l) => {
                self.header(2);
                self.coords(&LineString::from(*l));
            }
            Geometry::LineString(l) => {
                self.header(2);
                self.coords(l);
            }
            Geometry::Polygon(p) => {
                self.header(3);
                self.polygon_body(p);
            }
            Geometry::Rect(r) => self.geometry(&Geometry::Polygon(r.to_polygon())),
            Geometry::Triangle(t) => self.geometry(&Geometry::Polygon(t.to_polygon())),
            Geometry::MultiPoint(mp) => {
                self.header(4);
                self.u32(mp.0.len() as u32);
                for p in &mp.0 {
                    self.geometry(&Geometry::Point(*p));
                }
            }
            Geometry::MultiLineString(ml) => {
                self.header(5);
                self.u32(ml.0.len() as u32);
                for l in &ml.0 {
                    self.header(2);
                    self.coords(l);
                }
            }
            Geometry::MultiPolygon(mp) => {
                self.header(6);
                self.u32(mp.0.len() as u32);
                for p in &mp.0 {
                    self.header(3);
                    self.polygon_body(p);
                }
            }
            Geometry::GeometryCollection(c) => {
                self.header(7);
                self.u32(c.0.len() as u32);
                for g in &c.0 {
                    self.geometry(g);
                }
            }
        }
    }
}

// -- affine transforms with Z ---------------------------------------------------------------------

/// A 3D affine transform: the 2D part acts on `(x, y)`; the rest is Shapely's 12-number form
/// `x' = a x + b y + c z + xoff`, `y' = d x + e y + f z + yoff`, `z' = g x + h y + i z + zoff`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Affine3 {
    plane: AffineTransform<f64>,
    c: f64,
    f: f64,
    g: f64,
    h: f64,
    i: f64,
    zoff: f64,
}

impl Affine3 {
    /// A 2D transform that leaves Z alone.
    pub(crate) fn flat(plane: AffineTransform<f64>) -> Self {
        Self {
            plane,
            c: 0.0,
            f: 0.0,
            g: 0.0,
            h: 0.0,
            i: 1.0,
            zoff: 0.0,
        }
    }

    /// A 2D transform that also scales Z by `zfact` and shifts it by `zoff`.
    pub(crate) fn with_z(plane: AffineTransform<f64>, zfact: f64, zoff: f64) -> Self {
        Self {
            i: zfact,
            zoff,
            ..Self::flat(plane)
        }
    }

    /// Shapely's `[a, b, c, d, e, f, g, h, i, xoff, yoff, zoff]`.
    pub(crate) fn from_matrix(m: &[f64; 12]) -> Self {
        let [a, b, c, d, e, f, g, h, i, xoff, yoff, zoff] = *m;
        Self {
            plane: AffineTransform::new(a, b, xoff, d, e, yoff),
            c,
            f,
            g,
            h,
            i,
            zoff,
        }
    }

    fn apply(&self, x: f64, y: f64, z: f64) -> (f64, f64, f64) {
        let p = self.plane.apply(Coord { x, y });
        if z.is_nan() {
            return (p.x, p.y, f64::NAN);
        }
        let mut nz = self.i * z + self.zoff;
        if self.g != 0.0 {
            nz += self.g * x;
        }
        if self.h != 0.0 {
            nz += self.h * y;
        }
        (p.x + self.c * z, p.y + self.f * z, nz)
    }
}

impl GeoColumn {
    /// Whether any row has a Z or M ordinate.
    pub(crate) fn any_zm(&self) -> bool {
        self.map_scalars(|s| dimensions(s))
            .into_iter()
            .flatten()
            .any(|(z, m)| z || m)
    }

    fn push_wkb(builder: &mut GeometryBuilder, bytes: Option<Vec<u8>>) -> Result<(), String> {
        let parsed = bytes
            .as_deref()
            .map(wkb::reader::read_wkb)
            .transpose()
            .map_err(|e| e.to_string())?;
        builder
            .push_geometry(parsed.as_ref())
            .map_err(|e| e.to_string())
    }

    /// Rewrites the WKB of every row that `edit` accepts (`None` makes the row null, as does a
    /// row `geo` cannot represent or a failing edit).
    fn rewrite_rows<'a>(
        &self,
        edit: impl Fn(
            usize,
            &Geometry<f64>,
        ) -> Option<Box<dyn Fn(&[u8]) -> Result<Vec<u8>, String> + 'a>>,
    ) -> Result<Self, String> {
        let wkb = self.to_wkb()?;
        let mut builder = GeometryBuilder::new(self.array_type());
        for (i, (geometry, bytes)) in self.geo_rows().iter().zip(&wkb).enumerate() {
            let rewritten = match (geometry, bytes) {
                (Some(g), Some(bytes)) => catch_unwind(AssertUnwindSafe(|| {
                    edit(i, g).and_then(|run| run(bytes).ok())
                }))
                .ok()
                .flatten(),
                _ => None,
            };
            Self::push_wkb(&mut builder, rewritten)?;
        }
        Ok(self.with_array(builder.finish()))
    }

    /// Maps every vertex through `make(row, geometry)`'s function; Z is kept and transformed,
    /// M is dropped (Shapely's `transform` and affine behaviour). Rows without Z or M behave
    /// exactly like the 2D path.
    pub(crate) fn map_vertices<'a>(
        &self,
        make: impl Fn(usize, &Geometry<f64>) -> Option<Box<VertexFn<'a>>>,
    ) -> Result<Self, String> {
        self.rewrite_rows(|i, g| {
            let f = make(i, g)?;
            Some(Box::new(move |bytes: &[u8]| rewrite(bytes, Edit::Map(&*f))))
        })
    }

    /// Reverses every line and ring, keeping Z and M exactly.
    pub(crate) fn reverse_vertices(&self) -> Result<Self, String> {
        self.rewrite_rows(|_, _| Some(Box::new(|bytes: &[u8]| rewrite(bytes, Edit::Reverse))))
    }

    /// Applies one 3D affine transform per row (`None` nulls the row). A column without Z or
    /// M takes the plain `geo` path.
    pub(crate) fn affine3_rows(
        &self,
        make: impl Fn(usize, &Geometry<f64>) -> Option<Affine3>,
    ) -> Result<Self, String> {
        if !self.any_zm() {
            let rows: Vec<Option<Geometry<f64>>> = self
                .geo_rows()
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    row.as_ref().and_then(|g| {
                        catch_unwind(AssertUnwindSafe(|| {
                            make(i, g).map(|t| g.affine_transform(&t.plane))
                        }))
                        .ok()
                        .flatten()
                    })
                })
                .collect();
            return self.rebuild(&rows);
        }
        self.map_vertices(|i, g| {
            let t = make(i, g)?;
            Some(Box::new(move |x, y, z, _m| Ok(t.apply(x, y, z))))
        })
    }

    /// [`map_geometry`](Self::map_geometry) that gives the surviving vertices back their Z (and
    /// M, per `keep`) by exact XY lookup in the input row.
    pub(crate) fn map_geometry_zm(
        &self,
        keep: Keep,
        f: impl Fn(&Geometry<f64>) -> Option<Geometry<f64>>,
    ) -> Result<Self, String> {
        self.map_geometry_restoring(&Restore::keep(keep), f)
    }

    /// [`map_geometry_zm`](Self::map_geometry_zm) with full control of the restoration.
    pub(crate) fn map_geometry_restoring(
        &self,
        restore: &Restore<'_>,
        f: impl Fn(&Geometry<f64>) -> Option<Geometry<f64>>,
    ) -> Result<Self, String> {
        let rows: Vec<Option<Geometry<f64>>> = self
            .geo_rows()
            .iter()
            .map(|row| {
                row.as_ref()
                    .and_then(|g| catch_unwind(AssertUnwindSafe(|| f(g))).ok().flatten())
            })
            .collect();
        let source: Vec<usize> = (0..rows.len()).collect();
        self.rebuild_zm(&rows, &source, restore)
    }

    /// Builds a column from `rows` (output row `k` derives from input row `source[k]`) and
    /// restores Z/M on rows whose source has them; other rows are written as 2D `geo`.
    pub(crate) fn rebuild_zm(
        &self,
        rows: &[Option<Geometry<f64>>],
        source: &[usize],
        restore: &Restore<'_>,
    ) -> Result<Self, String> {
        let dims = self.map_scalars(|s| dimensions(s));
        let wkb = self.to_wkb()?;
        let mut builder = GeometryBuilder::new(self.array_type());
        let mut cache: HashMap<usize, Lookup> = HashMap::new();
        for (k, row) in rows.iter().enumerate() {
            let src = source[k];
            let in_dims = dims.get(src).copied().flatten().unwrap_or((false, false));
            let (z, m) = restore.keep.apply(in_dims);
            let (Some(geometry), true) = (row, z || m) else {
                builder
                    .push_geometry(row.as_ref())
                    .map_err(|e| e.to_string())?;
                continue;
            };
            let own = cache.entry(src).or_insert_with(|| {
                wkb[src]
                    .as_deref()
                    .map(|bytes| lookup_of(bytes, restore.index))
                    .unwrap_or_default()
            });
            let reference = restore.reference.and_then(|r| r(k));
            let ordinates = |c: Coord<f64>| {
                reference
                    .as_ref()
                    .and_then(|r| r.get(&key(c)))
                    .or_else(|| own.get(&key(c)))
                    .copied()
                    .unwrap_or((f64::NAN, f64::NAN))
            };
            let mut writer = Writer {
                out: Vec::new(),
                z,
                m,
                ordinates: &ordinates,
            };
            writer.geometry(geometry);
            Self::push_wkb(&mut builder, Some(writer.out))?;
        }
        Ok(self.with_array(builder.finish()))
    }
}
