//! The Z/M model of the interpolating and overlay operations (GEOS `OverlayNG`).
//!
//! `geo` computes these operations in 2D. GEOS gives the result vertices their ordinates as
//! follows, and [`Model::at`] reproduces it for every vertex of the 2D result:
//!
//! 1. A vertex equal (in XY) to an input vertex takes that vertex's ordinate. When both inputs
//!    have the vertex the first input with a finite value wins (GEOS depends on its noding
//!    order here; see `docs/parity.md`).
//! 2. A vertex on an input segment (a node) interpolates along it. With two segments through
//!    the vertex (one per input, a crossing) the two interpolations are averaged; a NaN end on
//!    a segment hands back the other end (`LineIntersector::zInterpolate`).
//! 3. A Z that is still NaN is filled from the elevation model: the input's extent is cut into
//!    3x3 cells holding the mean Z of the vertices in them, and an empty cell gets the mean of
//!    the non-empty cells. M has no such fill and stays NaN.
//!
//! The result has Z (or M) when any input has it. Rows without Z and M never reach this
//! module, so a 2D column pays nothing.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

use geo::{Coord, Geometry};
use geo_traits::{
    CoordTrait, GeometryCollectionTrait, GeometryTrait, GeometryType, LineStringTrait,
    MultiLineStringTrait, MultiPointTrait, MultiPolygonTrait, PointTrait, PolygonTrait,
};
use geoarrow_array::builder::GeometryBuilder;
use rstar::primitives::GeomWithData;
use rstar::{PointDistance, RTree, RTreeObject, AABB};

use crate::ewkb::MAX_NESTING_DEPTH;
use crate::zm::{dimensions, flags};
use crate::zm_bridge::push_zm;
use crate::GeoColumn;

/// Distance, relative to the extent of the inputs, under which a result vertex counts as an
/// input vertex or as lying on an input segment. `geo`'s overlay snaps its output to a grid and
/// moves vertices by up to about 1e-9 of the extent.
const NEAR: f64 = 1e-7;

#[derive(Clone, Copy, Debug)]
struct Vertex {
    x: f64,
    y: f64,
    z: f64,
    m: f64,
}

impl Vertex {
    fn of<C: CoordTrait<T = f64>>(c: &C) -> Self {
        let (z, m) = match flags(c.dim()) {
            (true, true) => (c.nth_or_panic(2), c.nth_or_panic(3)),
            (true, false) => (c.nth_or_panic(2), f64::NAN),
            (false, true) => (f64::NAN, c.nth_or_panic(2)),
            (false, false) => (f64::NAN, f64::NAN),
        };
        Self {
            x: c.x(),
            y: c.y(),
            z,
            m,
        }
    }

    fn ordinate(&self, which: usize) -> f64 {
        if which == 0 {
            self.z
        } else {
            self.m
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Segment {
    a: Vertex,
    b: Vertex,
}

impl RTreeObject for Segment {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners([self.a.x, self.a.y], [self.b.x, self.b.y])
    }
}

impl PointDistance for Segment {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        let (dx, dy) = (self.b.x - self.a.x, self.b.y - self.a.y);
        let length = dx * dx + dy * dy;
        let t = if length > 0.0 {
            (((point[0] - self.a.x) * dx + (point[1] - self.a.y) * dy) / length).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (px, py) = (self.a.x + t * dx - point[0], self.a.y + t * dy - point[1]);
        px * px + py * py
    }
}

type Key = (u64, u64);

fn key(x: f64, y: f64) -> Key {
    ((x + 0.0).to_bits(), (y + 0.0).to_bits())
}

/// The vertices and segments of one input row.
pub(crate) struct Source {
    /// `(has_z, has_m)` of the input row.
    pub dims: (bool, bool),
    /// `(z, m)` of the first vertex at each XY.
    vertices: HashMap<Key, (f64, f64)>,
    /// The same vertices indexed by position, for those the 2D algorithm moved slightly.
    points: RTree<GeomWithData<[f64; 2], (f64, f64)>>,
    tree: RTree<Segment>,
    /// `(x, y, z)` of every vertex with a finite Z, closing vertices included (elevation model).
    heights: Vec<[f64; 3]>,
    /// `(min_x, min_y, max_x, max_y)` of every vertex.
    extent: Option<[f64; 4]>,
}

#[derive(Default)]
struct Collector {
    vertices: HashMap<Key, (f64, f64)>,
    order: Vec<([f64; 2], (f64, f64))>,
    segments: Vec<Segment>,
    heights: Vec<[f64; 3]>,
    extent: Option<[f64; 4]>,
    measured: bool,
}

impl Collector {
    fn vertex(&mut self, v: Vertex) {
        // A vertex without a position cannot be looked up (and would upset the indexes).
        if !(v.x.is_finite() && v.y.is_finite()) {
            return;
        }
        let e = self.extent.get_or_insert([v.x, v.y, v.x, v.y]);
        *e = [e[0].min(v.x), e[1].min(v.y), e[2].max(v.x), e[3].max(v.y)];
        if !self.measured {
            return;
        }
        if let std::collections::hash_map::Entry::Vacant(slot) = self.vertices.entry(key(v.x, v.y))
        {
            slot.insert((v.z, v.m));
            self.order.push(([v.x, v.y], (v.z, v.m)));
        }
        if v.z.is_finite() {
            self.heights.push([v.x, v.y, v.z]);
        }
    }

    fn line<C: CoordTrait<T = f64>>(&mut self, coords: impl Iterator<Item = C>) {
        let mut previous: Option<Vertex> = None;
        for c in coords {
            let v = Vertex::of(&c);
            self.vertex(v);
            if !(v.x.is_finite() && v.y.is_finite()) {
                previous = None;
                continue;
            }
            if let Some(a) = previous.filter(|a| self.measured && (a.x != v.x || a.y != v.y)) {
                self.segments.push(Segment { a, b: v });
            }
            previous = Some(v);
        }
    }

    fn polygon<P: PolygonTrait<T = f64>>(&mut self, polygon: &P) {
        if let Some(ring) = polygon.exterior() {
            self.line(ring.coords());
        }
        for ring in polygon.interiors() {
            self.line(ring.coords());
        }
    }

    fn walk<G: GeometryTrait<T = f64>>(&mut self, geometry: &G, depth: usize) {
        if depth > MAX_NESTING_DEPTH {
            return;
        }
        match geometry.as_type() {
            GeometryType::Point(p) => {
                if let Some(c) = p.coord() {
                    self.vertex(Vertex::of(&c));
                }
            }
            GeometryType::LineString(l) => self.line(l.coords()),
            GeometryType::Polygon(p) => self.polygon(p),
            GeometryType::MultiPoint(mp) => {
                for p in mp.points() {
                    if let Some(c) = p.coord() {
                        self.vertex(Vertex::of(&c));
                    }
                }
            }
            GeometryType::MultiLineString(ml) => {
                for l in ml.line_strings() {
                    self.line(l.coords());
                }
            }
            GeometryType::MultiPolygon(mp) => {
                for p in mp.polygons() {
                    self.polygon(&p);
                }
            }
            GeometryType::GeometryCollection(gc) => {
                for member in gc.geometries() {
                    self.walk(&member, depth + 1);
                }
            }
            _ => {}
        }
    }
}

impl Source {
    /// Reads a stored geometry. Rows with neither Z nor M only keep their extent.
    pub(crate) fn of<G: GeometryTrait<T = f64>>(geometry: &G) -> Self {
        let dims = dimensions(geometry);
        let mut collector = Collector {
            measured: dims.0 || dims.1,
            ..Collector::default()
        };
        collector.walk(geometry, 0);
        Self {
            dims,
            vertices: collector.vertices,
            points: RTree::bulk_load(
                collector
                    .order
                    .into_iter()
                    .map(|(p, v)| GeomWithData::new(p, v))
                    .collect(),
            ),
            tree: RTree::bulk_load(collector.segments),
            heights: collector.heights,
            extent: collector.extent,
        }
    }

    /// One source holding the vertices and segments of all of `sources` (the first source
    /// wins at a repeated vertex). Used when many rows are united.
    pub(crate) fn merged(sources: &[Rc<Source>]) -> Self {
        let mut merged = Self {
            dims: (false, false),
            vertices: HashMap::new(),
            points: RTree::new(),
            tree: RTree::new(),
            heights: Vec::new(),
            extent: None,
        };
        let mut segments = Vec::new();
        let mut points = Vec::new();
        for s in sources {
            merged.dims = (merged.dims.0 || s.dims.0, merged.dims.1 || s.dims.1);
            for p in s.points.iter() {
                if let std::collections::hash_map::Entry::Vacant(slot) =
                    merged.vertices.entry(key(p.geom()[0], p.geom()[1]))
                {
                    slot.insert(p.data);
                    points.push(*p);
                }
            }
            segments.extend(s.tree.iter().copied());
            merged.heights.extend_from_slice(&s.heights);
            merged.extent = union_extent(merged.extent, s.extent);
        }
        merged.points = RTree::bulk_load(points);
        merged.tree = RTree::bulk_load(segments);
        merged
    }
}

fn union_extent(a: Option<[f64; 4]>, b: Option<[f64; 4]>) -> Option<[f64; 4]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (a, b) => a.or(b),
    }
}

/// GEOS `LineIntersector::zInterpolate` for one ordinate.
fn interpolate(which: usize, x: f64, y: f64, s: &Segment) -> f64 {
    let (z1, z2) = (s.a.ordinate(which), s.b.ordinate(which));
    if z1.is_nan() {
        return z2;
    }
    if z2.is_nan() {
        return z1;
    }
    if x == s.a.x && y == s.a.y {
        return z1;
    }
    if x == s.b.x && y == s.b.y {
        return z2;
    }
    let dz = z2 - z1;
    if dz == 0.0 {
        return z1;
    }
    let (dx, dy) = (s.b.x - s.a.x, s.b.y - s.a.y);
    let (pdx, pdy) = (x - s.a.x, y - s.a.y);
    let fraction = ((pdx * pdx + pdy * pdy) / (dx * dx + dy * dy)).sqrt();
    z1 + dz * fraction
}

const CELLS: usize = 3;

/// GEOS `ElevationModel`: mean Z per cell of a 3x3 grid over the inputs' extent.
struct Elevation {
    origin: [f64; 2],
    size: [f64; 2],
    cells: [[(f64, u32); CELLS]; CELLS],
    mean: f64,
}

impl Elevation {
    fn new(sources: &[Rc<Source>]) -> Option<Self> {
        let extent = sources
            .iter()
            .fold(None, |e, s| union_extent(e, s.extent))?;
        let size = [
            (extent[2] - extent[0]) / CELLS as f64,
            (extent[3] - extent[1]) / CELLS as f64,
        ];
        let mut model = Self {
            origin: [extent[0], extent[1]],
            size,
            cells: [[(0.0, 0); CELLS]; CELLS],
            mean: f64::NAN,
        };
        let mut any = false;
        for [x, y, z] in sources.iter().flat_map(|s| s.heights.iter().copied()) {
            let (i, j) = model.cell(x, y);
            model.cells[i][j].0 += z;
            model.cells[i][j].1 += 1;
            any = true;
        }
        if !any {
            return None;
        }
        let means: Vec<f64> = model
            .cells
            .iter()
            .flatten()
            .filter(|(_, n)| *n > 0)
            .map(|(sum, n)| sum / f64::from(*n))
            .collect();
        model.mean = means.iter().sum::<f64>() / means.len() as f64;
        Some(model)
    }

    fn index(value: f64, origin: f64, size: f64) -> usize {
        if size <= 0.0 {
            return 0;
        }
        let i = ((value - origin) / size) as i64;
        i.clamp(0, CELLS as i64 - 1) as usize
    }

    fn cell(&self, x: f64, y: f64) -> (usize, usize) {
        (
            Self::index(x, self.origin[0], self.size[0]),
            Self::index(y, self.origin[1], self.size[1]),
        )
    }

    fn z(&self, x: f64, y: f64) -> f64 {
        let (i, j) = self.cell(x, y);
        let (sum, n) = self.cells[i][j];
        if n == 0 {
            self.mean
        } else {
            sum / f64::from(n)
        }
    }
}

/// Resolves the ordinates of result vertices for one row.
pub(crate) struct Model {
    sources: Vec<Rc<Source>>,
    tolerance_2: f64,
    fill: bool,
    elevation: std::cell::OnceCell<Option<Elevation>>,
}

impl Model {
    /// `fill` enables the elevation-model fill of NaN Z (overlay and clip, not interpolation).
    pub(crate) fn new(sources: Vec<Rc<Source>>, fill: bool) -> Self {
        let extent = sources.iter().fold(None, |e, s| union_extent(e, s.extent));
        let span = extent.map_or(0.0, |e| (e[2] - e[0]).max(e[3] - e[1]));
        let tolerance = NEAR * span;
        Self {
            sources,
            tolerance_2: tolerance * tolerance,
            fill,
            elevation: std::cell::OnceCell::new(),
        }
    }

    /// `(z, m)` of the result vertex `c`.
    pub(crate) fn at(&self, c: Coord<f64>) -> (f64, f64) {
        if !(c.x.is_finite() && c.y.is_finite()) {
            return (f64::NAN, f64::NAN);
        }
        let mut out = [f64::NAN; 2];
        let k = key(c.x, c.y);
        for s in &self.sources {
            if let Some(&(z, m)) = s.vertices.get(&k) {
                take(&mut out, (z, m));
            }
        }
        if out.iter().any(|v| v.is_nan()) {
            for s in &self.sources {
                let near = s
                    .points
                    .nearest_neighbor_iter_with_distance_2(&[c.x, c.y])
                    .next()
                    .filter(|(_, d)| *d <= self.tolerance_2);
                if let Some((p, _)) = near {
                    take(&mut out, p.data);
                }
            }
        }
        for (which, value) in out.iter_mut().enumerate() {
            if value.is_nan() {
                *value = self.along_segments(which, c);
            }
        }
        if out[0].is_nan() && self.fill {
            let elevation = self.elevation.get_or_init(|| Elevation::new(&self.sources));
            if let Some(model) = elevation {
                out[0] = model.z(c.x, c.y);
            }
        }
        (out[0], out[1])
    }

    /// Interpolates along the segments through `c`: the mean of the first two segments with a
    /// value (one per input at a crossing).
    fn along_segments(&self, which: usize, c: Coord<f64>) -> f64 {
        let mut found = Vec::with_capacity(2);
        'sources: for s in &self.sources {
            let mut hits: Vec<&Segment> = s
                .tree
                .locate_within_distance([c.x, c.y], self.tolerance_2)
                .collect();
            hits.sort_by(|a, b| {
                let key = |s: &Segment| (s.a.x.to_bits(), s.a.y.to_bits(), s.b.x.to_bits());
                key(a).cmp(&key(b))
            });
            for segment in hits {
                let value = interpolate(which, c.x, c.y, segment);
                if !value.is_nan() {
                    found.push(value);
                    if found.len() == 2 {
                        break 'sources;
                    }
                }
            }
        }
        match found.as_slice() {
            [] => f64::NAN,
            [one] => *one,
            [a, b, ..] => (a + b) / 2.0,
        }
    }
}

/// Fills the NaN slots of `out` from `(z, m)`.
fn take(out: &mut [f64; 2], (z, m): (f64, f64)) {
    if out[0].is_nan() {
        out[0] = z;
    }
    if out[1].is_nan() {
        out[1] = m;
    }
}

/// How one output row gets its ordinates.
pub(crate) struct RowPlan {
    pub dims: (bool, bool),
    pub model: Model,
}

impl GeoColumn {
    /// One [`Source`] per row (`None` for null rows).
    pub(crate) fn zm_sources(&self) -> Vec<Option<Rc<Source>>> {
        self.map_scalars(|scalar| Rc::new(Source::of(scalar)))
    }

    /// Builds a column from 2D `rows`; row `i` is written with `plan(i)`'s dimensions and
    /// ordinates, or as plain 2D when the plan is `None` or has neither Z nor M.
    pub(crate) fn emit_zm(
        &self,
        rows: &[Option<Geometry<f64>>],
        plan: impl Fn(usize) -> Option<RowPlan>,
    ) -> Result<Self, String> {
        let mut builder = GeometryBuilder::new(self.array_type());
        for (i, row) in rows.iter().enumerate() {
            match (row, plan(i)) {
                (Some(geometry), Some(plan)) if plan.dims != (false, false) => {
                    let ordinates = |c: Coord<f64>| plan.model.at(c);
                    push_zm(&mut builder, geometry, plan.dims, &ordinates)?;
                }
                _ => builder
                    .push_geometry(row.as_ref())
                    .map_err(|e| e.to_string())?,
            }
        }
        Ok(self.with_array(builder.finish()))
    }

    /// [`rebuild`](Self::rebuild) for rows computed from this column's rows one for one: new
    /// vertices interpolate along the source row, M is dropped (`fill` enables the elevation
    /// fill of NaN Z).
    pub(crate) fn rebuild_interpolated(
        &self,
        rows: &[Option<Geometry<f64>>],
        fill: bool,
    ) -> Result<Self, String> {
        if !self.any_zm() {
            return self.rebuild(rows);
        }
        let sources = self.zm_sources();
        self.emit_zm(rows, |i| {
            let source = sources.get(i)?.as_ref()?;
            Some(RowPlan {
                dims: (source.dims.0, false),
                model: Model::new(vec![source.clone()], fill),
            })
        })
    }

    /// Pairwise overlay rows `rows` (row `i` pairs left row `i` with right row `i`, or right
    /// row 0 when `broadcasts`) with the Z and M of both inputs.
    pub(crate) fn rebuild_overlay(
        &self,
        other: &GeoColumn,
        rows: &[Option<Geometry<f64>>],
        intersection: bool,
    ) -> Result<Self, String> {
        if !(self.any_zm() || other.any_zm()) {
            return self.rebuild(rows);
        }
        let broadcasts = other.len() == 1 && self.len() != 1;
        let (left, right) = (self.zm_sources(), other.zm_sources());
        self.emit_zm(rows, |i| {
            let a = left.get(i)?.as_ref()?;
            let b = right.get(if broadcasts { 0 } else { i })?.as_ref()?;
            // Where both inputs have a vertex with different Z, GEOS picks by noding order. Over
            // random inputs the intersection mostly keeps the first input's value and the other
            // operations the second's.
            let order = if intersection {
                vec![a.clone(), b.clone()]
            } else {
                vec![b.clone(), a.clone()]
            };
            Some(RowPlan {
                dims: (a.dims.0 || b.dims.0, a.dims.1 || b.dims.1),
                model: Model::new(order, true),
            })
        })
    }

    /// A united row for every group of source rows (`groups[k]` lists the rows united into
    /// output row `k`).
    pub(crate) fn rebuild_united(
        &self,
        rows: &[Option<Geometry<f64>>],
        groups: &[Vec<usize>],
    ) -> Result<Self, String> {
        if !self.any_zm() {
            return self.rebuild(rows);
        }
        let sources = self.zm_sources();
        self.emit_zm(rows, |k| {
            let members: Vec<Rc<Source>> = groups
                .get(k)?
                .iter()
                .rev()
                .filter_map(|&i| sources.get(i).cloned().flatten())
                .collect();
            let source = Rc::new(Source::merged(&members));
            Some(RowPlan {
                dims: source.dims,
                model: Model::new(vec![source], true),
            })
        })
    }

    /// [`map_geometry`](Self::map_geometry) for operations whose result vertices are input
    /// vertices or lie on input segments (hulls, merged lines, valid polygons, ...). `carry`
    /// says which ordinates the result keeps, given the input geometry and its `(z, m)`; they
    /// come from the input row by [`Model::at`] (a crossing averages, as GEOS does).
    pub(crate) fn map_geometry_model(
        &self,
        carry: impl Fn(&Geometry<f64>, (bool, bool)) -> (bool, bool),
        f: impl Fn(&Geometry<f64>) -> Option<Geometry<f64>>,
    ) -> Result<Self, String> {
        let inputs = self.geo_rows();
        let rows: Vec<Option<Geometry<f64>>> = inputs
            .iter()
            .map(|row| {
                row.as_ref()
                    .and_then(|g| catch_unwind(AssertUnwindSafe(|| f(g))).ok().flatten())
            })
            .collect();
        if !self.any_zm() {
            return self.rebuild(&rows);
        }
        let sources = self.zm_sources();
        self.emit_zm(&rows, |i| {
            let source = sources.get(i)?.as_ref()?;
            let input = inputs.get(i)?.as_ref()?;
            Some(RowPlan {
                dims: carry(input, source.dims),
                model: Model::new(vec![source.clone()], false),
            })
        })
    }

    /// [`rebuild`](Self::rebuild) for rows built from the vertices of the whole column
    /// (polygonize, Delaunay): every row takes its Z from all the rows together; M is dropped.
    pub(crate) fn rebuild_merged(&self, rows: &[Option<Geometry<f64>>]) -> Result<Self, String> {
        if !self.any_zm() {
            return self.rebuild(rows);
        }
        let all: Vec<Rc<Source>> = self.zm_sources().into_iter().flatten().collect();
        let merged = Rc::new(Source::merged(&all));
        self.emit_zm(rows, |_| {
            Some(RowPlan {
                dims: (merged.dims.0, false),
                model: Model::new(vec![merged.clone()], false),
            })
        })
    }
}

/// Keeps Z and drops M (Shapely's constructive operations on existing vertices).
pub(crate) fn z_only(_: &Geometry<f64>, dims: (bool, bool)) -> (bool, bool) {
    (dims.0, false)
}

/// Keeps Z and M.
pub(crate) fn z_and_m(_: &Geometry<f64>, dims: (bool, bool)) -> (bool, bool) {
    dims
}
