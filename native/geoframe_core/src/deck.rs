//! Flat binary buffers for deck.gl, built from a geometry column.
//!
//! # Model
//!
//! A column is split into three layers (points, lines, polygons). Every layer is a set of flat
//! typed buffers that deck.gl reads without any per-feature JavaScript object:
//!
//! * **positions** - interleaved `x, y` (longitude, latitude) as `f64`. Float64 keeps full
//!   precision at street scale; deck.gl splits it into high and low 32-bit halves on the GPU.
//!   Z and M are dropped.
//! * **offsets** - `i32` vertex indices of the first vertex of every part, with one trailing
//!   entry (`parts + 1`), the shape of deck.gl's `startIndices`. A layer with no parts has no
//!   offsets at all.
//! * **rows** - `i32`, the source row of every part. A multi geometry yields one part per member,
//!   all carrying the same row, so null and empty rows leave gaps and tooltips and styles map back
//!   to the table through `rows`.
//! * **triangles** - polygons are triangulated here (ear clipping, holes supported) into `u32`
//!   vertex indices that are global to the polygon positions, so deck.gl skips its own
//!   tessellation. `ring_starts` marks every ring (exterior and holes) for outlines.
//!
//! # Skipping rules
//!
//! Null and empty rows produce nothing. A part with a non-finite coordinate, a line with fewer
//! than 2 vertices, or a polygon whose exterior has fewer than 4 vertices is dropped and counted
//! in `skipped`; a degenerate hole is dropped silently with its polygon kept. Geometry
//! collections are flattened into their members.

use earcut::Earcut;
use geo::{Coord, Geometry, LineString, Polygon};

use crate::GeoColumn;

/// The flat buffers for one column. See the module docs for the layout.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DeckBuffers {
    pub point_positions: Vec<f64>,
    pub point_rows: Vec<i32>,
    pub line_positions: Vec<f64>,
    pub line_starts: Vec<i32>,
    pub line_rows: Vec<i32>,
    pub polygon_positions: Vec<f64>,
    pub polygon_starts: Vec<i32>,
    pub ring_starts: Vec<i32>,
    pub triangles: Vec<u32>,
    pub polygon_rows: Vec<i32>,
    pub skipped: usize,
}

fn finite(coords: &[Coord<f64>]) -> bool {
    coords.iter().all(|c| c.x.is_finite() && c.y.is_finite())
}

fn push_coords(target: &mut Vec<f64>, coords: &[Coord<f64>]) {
    for c in coords {
        target.push(c.x);
        target.push(c.y);
    }
}

/// Appends the end of a part to `starts`, which begins with a leading 0.
fn push_end(starts: &mut Vec<i32>, end: usize) {
    if starts.is_empty() {
        starts.push(0);
    }
    starts.push(end as i32);
}

struct Builder {
    out: DeckBuffers,
    earcut: Earcut<f64>,
    scratch: Vec<u32>,
}

impl Builder {
    fn add(&mut self, row: i32, geometry: &Geometry<f64>) {
        match geometry {
            Geometry::Point(p) => self.point(row, p.0),
            Geometry::MultiPoint(m) => m.0.iter().for_each(|p| self.point(row, p.0)),
            Geometry::Line(l) => self.path(row, &[l.start, l.end]),
            Geometry::LineString(l) => self.path(row, &l.0),
            Geometry::MultiLineString(m) => m.0.iter().for_each(|l| self.path(row, &l.0)),
            Geometry::Polygon(p) => self.polygon(row, p),
            Geometry::MultiPolygon(m) => m.0.iter().for_each(|p| self.polygon(row, p)),
            Geometry::Rect(r) => self.polygon(row, &r.to_polygon()),
            Geometry::Triangle(t) => self.polygon(row, &t.to_polygon()),
            Geometry::GeometryCollection(c) => c.0.iter().for_each(|g| self.add(row, g)),
        }
    }

    fn point(&mut self, row: i32, c: Coord<f64>) {
        if !finite(&[c]) {
            self.out.skipped += 1;
            return;
        }
        push_coords(&mut self.out.point_positions, &[c]);
        self.out.point_rows.push(row);
    }

    fn path(&mut self, row: i32, coords: &[Coord<f64>]) {
        if coords.is_empty() {
            return;
        }
        if coords.len() < 2 || !finite(coords) {
            self.out.skipped += 1;
            return;
        }
        push_coords(&mut self.out.line_positions, coords);
        push_end(&mut self.out.line_starts, self.out.line_positions.len() / 2);
        self.out.line_rows.push(row);
    }

    fn polygon(&mut self, row: i32, polygon: &Polygon<f64>) {
        let exterior = &polygon.exterior().0;
        if exterior.is_empty() {
            return;
        }
        if exterior.len() < 4 || !finite(exterior) {
            self.out.skipped += 1;
            return;
        }
        let holes: Vec<&LineString<f64>> = polygon
            .interiors()
            .iter()
            .filter(|h| h.0.len() >= 4 && finite(&h.0))
            .collect();

        let base = self.out.polygon_positions.len() / 2;
        let mut local = Vec::new();
        let mut hole_starts: Vec<u32> = Vec::new();
        for (i, ring) in std::iter::once(&polygon.exterior())
            .chain(holes.iter())
            .enumerate()
        {
            if i > 0 {
                hole_starts.push((local.len() / 2) as u32);
            }
            push_coords(&mut local, &ring.0);
            push_end(&mut self.out.ring_starts, base + local.len() / 2);
        }
        self.earcut.earcut(
            local.as_chunks::<2>().0.iter().copied(),
            &hole_starts,
            &mut self.scratch,
        );
        self.out
            .triangles
            .extend(self.scratch.iter().map(|i| i + base as u32));
        self.out.polygon_positions.extend_from_slice(&local);
        push_end(
            &mut self.out.polygon_starts,
            self.out.polygon_positions.len() / 2,
        );
        self.out.polygon_rows.push(row);
    }
}

impl DeckBuffers {
    /// Builds the buffers for every row of `column`.
    pub fn from_column(column: &GeoColumn) -> Self {
        let mut builder = Builder {
            out: Self::default(),
            earcut: Earcut::new(),
            scratch: Vec::new(),
        };
        for (row, geometry) in column.geo_rows().iter().enumerate() {
            if let Some(geometry) = geometry {
                builder.add(row as i32, geometry);
            }
        }
        builder.out
    }
}

/// Copies `stride` bytes per entry of `rows` out of `row_values` (one `stride`-byte value per
/// table row). Rows beyond the values yield zero bytes.
pub fn gather_rows(rows: &[i32], row_values: &[u8], stride: usize) -> Vec<u8> {
    let mut out = vec![0u8; rows.len() * stride];
    for (slot, &row) in out.chunks_exact_mut(stride).zip(rows) {
        let start = row as usize * stride;
        if let Some(value) = row_values.get(start..start + stride) {
            slot.copy_from_slice(value);
        }
    }
    out
}

/// Like [`gather_rows`] but one value per *vertex*: part `i` (source row `rows[i]`) covers the
/// vertices `starts[i]..starts[i + 1]`. deck.gl reads per-feature binary attributes of path and
/// polygon layers per vertex.
pub fn gather_vertices(rows: &[i32], starts: &[i32], row_values: &[u8], stride: usize) -> Vec<u8> {
    let vertices = starts.last().map_or(0, |&end| end as usize);
    let mut out = Vec::with_capacity(vertices * stride);
    for (part, &row) in rows.iter().enumerate() {
        let count = (starts[part + 1] - starts[part]) as usize;
        let start = row as usize * stride;
        let value = row_values.get(start..start + stride);
        for _ in 0..count {
            match value {
                Some(v) => out.extend_from_slice(v),
                None => out.resize(out.len() + stride, 0),
            }
        }
    }
    out
}

fn f64_bytes(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn i32_bytes(values: &[i32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn u32_bytes(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// The named little-endian buffers for `column`, plus the number of skipped parts.
///
/// Names are `points.*`, `lines.*` and `polygons.*`; a layer without parts is omitted.
/// `colors` (RGBA, 4 bytes per table row) and `sizes` (`f32`, 4 bytes per table row) are optional
/// (empty = none); they are gathered into `*.colors` and `*.sizes`: one entry per point, but one per **vertex**
/// for lines and polygons, the layout deck.gl expects for binary path and polygon attributes.
/// Polygons take colors only.
pub fn deck_buffers(
    column: &GeoColumn,
    colors: &[u8],
    sizes: &[u8],
) -> (Vec<(&'static str, Vec<u8>)>, usize) {
    let b = DeckBuffers::from_column(column);
    let mut list: Vec<(&'static str, Vec<u8>)> = Vec::new();
    // Points are styled per point; lines and polygons per vertex (`starts` = Some).
    let style = |list: &mut Vec<(&'static str, Vec<u8>)>,
                 rows: &[i32],
                 starts: Option<&[i32]>,
                 color: &'static str,
                 size: Option<&'static str>| {
        let gather = |values: &[u8]| match starts {
            Some(starts) => gather_vertices(rows, starts, values, 4),
            None => gather_rows(rows, values, 4),
        };
        if !colors.is_empty() {
            list.push((color, gather(colors)));
        }
        if let (Some(name), false) = (size, sizes.is_empty()) {
            list.push((name, gather(sizes)));
        }
    };

    if !b.point_rows.is_empty() {
        list.push(("points.positions", f64_bytes(&b.point_positions)));
        list.push(("points.rows", i32_bytes(&b.point_rows)));
        let names = ("points.colors", Some("points.sizes"));
        style(&mut list, &b.point_rows, None, names.0, names.1);
    }
    if !b.line_rows.is_empty() {
        list.push(("lines.positions", f64_bytes(&b.line_positions)));
        list.push(("lines.starts", i32_bytes(&b.line_starts)));
        list.push(("lines.rows", i32_bytes(&b.line_rows)));
        let starts = Some(b.line_starts.as_slice());
        style(
            &mut list,
            &b.line_rows,
            starts,
            "lines.colors",
            Some("lines.sizes"),
        );
    }
    if !b.polygon_rows.is_empty() {
        list.push(("polygons.positions", f64_bytes(&b.polygon_positions)));
        list.push(("polygons.starts", i32_bytes(&b.polygon_starts)));
        list.push(("polygons.ring_starts", i32_bytes(&b.ring_starts)));
        list.push(("polygons.triangles", u32_bytes(&b.triangles)));
        list.push(("polygons.rows", i32_bytes(&b.polygon_rows)));
        let starts = Some(b.polygon_starts.as_slice());
        style(&mut list, &b.polygon_rows, starts, "polygons.colors", None);
    }
    (list, b.skipped)
}
