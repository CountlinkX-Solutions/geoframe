//! Parity batch A: coordinate extraction, part access and small predicates.
//!
//! Conventions match the rest of the core: null rows give null results, rows `geo` cannot
//! represent (the empty point) are treated as empty geometries where the answer is defined
//! for them, results are 2D, and a panic inside `geo` makes the affected row null or false.

use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::geo_convert::{to_geo as geometry_to_geo, to_geo_keeping_empty_points};
use geo::line_intersection::{line_intersection, LineIntersection};
use geo::{
    Coord, CoordsIter, Geometry, GeometryCollection, Line, LineString, MultiLineString, MultiPoint,
    MultiPolygon, Point, Polygon, Validation,
};

use crate::zm_bridge::{Keep, Restore};
use crate::zm_overlay::z_only;
use crate::GeoColumn;

/// The text Shapely and GeoPandas report for a valid geometry.
const VALID: &str = "Valid Geometry";

/// Converts the variants that are not stored natively (`Line`, `Rect`, `Triangle`).
pub(crate) fn normalize(geom: &Geometry<f64>) -> Geometry<f64> {
    match geom {
        Geometry::Line(l) => Geometry::LineString(LineString::new(vec![l.start, l.end])),
        Geometry::Rect(r) => Geometry::Polygon(r.to_polygon()),
        Geometry::Triangle(t) => Geometry::Polygon(t.to_polygon()),
        other => other.clone(),
    }
}

/// The member geometries of a multi-part geometry or collection; a single geometry is its
/// own only part.
pub(crate) fn parts(geom: &Geometry<f64>) -> Vec<Geometry<f64>> {
    match normalize(geom) {
        Geometry::MultiPoint(m) => m.0.into_iter().map(Geometry::Point).collect(),
        Geometry::MultiLineString(m) => m.0.into_iter().map(Geometry::LineString).collect(),
        Geometry::MultiPolygon(m) => m.0.into_iter().map(Geometry::Polygon).collect(),
        Geometry::GeometryCollection(c) => c.0,
        single => vec![single],
    }
}

/// Part `n` (negative from the end) of `parts`; `None` when out of range.
fn nth_part(parts: Vec<Geometry<f64>>, n: i64) -> Option<Geometry<f64>> {
    let index = if n < 0 {
        parts.len().checked_sub(n.unsigned_abs() as usize)?
    } else {
        n as usize
    };
    parts.into_iter().nth(index)
}

/// Vertices for [`GeoColumn::mean_coordinate`]: every coordinate except the closing vertex
/// of polygon rings.
fn vertices(geom: &Geometry<f64>, out: &mut Vec<Coord<f64>>) {
    match normalize(geom) {
        Geometry::Point(p) => out.push(p.0),
        Geometry::MultiPoint(m) => out.extend(m.0.iter().map(|p| p.0)),
        Geometry::LineString(l) => out.extend(l.0.iter().copied()),
        Geometry::MultiLineString(m) => m.0.iter().for_each(|l| out.extend(l.0.iter().copied())),
        Geometry::Polygon(p) => polygon_vertices(&p, out),
        Geometry::MultiPolygon(m) => m.0.iter().for_each(|p| polygon_vertices(p, out)),
        Geometry::GeometryCollection(c) => c.0.iter().for_each(|g| vertices(g, out)),
        Geometry::Line(_) | Geometry::Rect(_) | Geometry::Triangle(_) => {}
    }
}

fn polygon_vertices(polygon: &Polygon<f64>, out: &mut Vec<Coord<f64>>) {
    for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
        let keep = ring.0.len().saturating_sub(usize::from(ring.is_closed()));
        out.extend(ring.0.iter().take(keep).copied());
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Family {
    Points,
    Lines,
    Areas,
    Mixed,
}

fn family(geom: &Geometry<f64>) -> Family {
    match normalize(geom) {
        Geometry::Point(_) | Geometry::MultiPoint(_) => Family::Points,
        Geometry::LineString(_) | Geometry::MultiLineString(_) => Family::Lines,
        Geometry::Polygon(_) | Geometry::MultiPolygon(_) => Family::Areas,
        _ => Family::Mixed,
    }
}

/// Whether the closed line has no self-intersection other than at its closing vertex.
fn is_simple_ring(line: &LineString<f64>) -> bool {
    let mut coords = line.0.clone();
    coords.dedup();
    if coords.len() < 4 || coords.first() != coords.last() {
        return false;
    }
    let segments: Vec<Line<f64>> = coords.windows(2).map(|w| Line::new(w[0], w[1])).collect();
    let last = segments.len() - 1;
    for i in 0..segments.len() {
        for j in (i + 1)..segments.len() {
            let Some(hit) = line_intersection(segments[i], segments[j]) else {
                continue;
            };
            let shared = if j == i + 1 {
                Some(segments[j].start)
            } else if i == 0 && j == last {
                Some(segments[0].start)
            } else {
                None
            };
            match (hit, shared) {
                (LineIntersection::SinglePoint { intersection, .. }, Some(at))
                    if intersection == at => {}
                _ => return false,
            }
        }
    }
    true
}

fn closed_line(line: &LineString<f64>) -> bool {
    !line.0.is_empty() && line.is_closed()
}

impl GeoColumn {
    /// `(row, x, y)` columns listing every vertex of every non-null, non-empty geometry in
    /// row order. A polygon ring counts its closing vertex, so the length per row equals
    /// [`GeoColumn::count_coordinates`].
    pub fn get_coordinates(&self) -> (Vec<u32>, Vec<f64>, Vec<f64>) {
        let (mut rows, mut xs, mut ys) = (Vec::new(), Vec::new(), Vec::new());
        for (i, geom) in self.geo_rows().iter().enumerate() {
            for c in geom.iter().flat_map(|g| g.coords_iter()) {
                rows.push(i as u32);
                xs.push(c.x);
                ys.push(c.y);
            }
        }
        (rows, xs, ys)
    }

    /// Number of parts per geometry: members of a multi-part geometry or collection, 1 for a
    /// single geometry and 0 for empty ones. Null rows are null (Shapely counts them as 0).
    pub fn count_geometries(&self) -> Vec<Option<usize>> {
        self.map_scalars(|scalar| match to_geo_keeping_empty_points(scalar) {
            Ok(g) if g.coords_count() > 0 => parts(&g).len(),
            _ => 0,
        })
    }

    /// Number of holes of each polygon; 0 for every other geometry type (as Shapely).
    pub fn count_interior_rings(&self) -> Vec<Option<usize>> {
        self.map_scalars(|scalar| match geometry_to_geo(scalar) {
            Ok(Geometry::Polygon(p)) => p.interiors().len(),
            _ => 0,
        })
    }

    /// The `n`th part of each geometry; negative `n` counts from the end. A single geometry
    /// is its own part 0. Out-of-range indices give null.
    pub fn get_geometry(&self, n: i64) -> Result<Self, String> {
        let rows: Vec<Option<Geometry<f64>>> = self
            .geo_rows_keeping_empty_points()
            .iter()
            .map(|row| {
                let g = row.as_ref()?;
                catch_unwind(AssertUnwindSafe(|| nth_part(parts(g), n)))
                    .ok()
                    .flatten()
            })
            .collect();
        let source: Vec<usize> = (0..rows.len()).collect();
        self.rebuild_zm(&rows, &source, &Restore::keep(Keep::Both))
    }

    /// [`get_geometry`](Self::get_geometry) with one index per row (`None` gives a null row).
    pub fn get_geometry_rows(&self, indices: &[Option<i64>]) -> Result<Self, String> {
        if indices.len() != self.len() {
            return Err(format!(
                "index has {} entries but the series has {} rows",
                indices.len(),
                self.len()
            ));
        }
        let rows: Vec<Option<Geometry<f64>>> = self
            .geo_rows_keeping_empty_points()
            .iter()
            .zip(indices)
            .map(|(row, n)| {
                let (g, n) = (row.as_ref()?, (*n)?);
                catch_unwind(AssertUnwindSafe(|| nth_part(parts(g), n)))
                    .ok()
                    .flatten()
            })
            .collect();
        let source: Vec<usize> = (0..rows.len()).collect();
        self.rebuild_zm(&rows, &source, &Restore::keep(Keep::Both))
    }

    /// The holes of each polygon as a `MultiLineString` (empty for a polygon without holes);
    /// null for every other geometry type.
    pub fn interiors(&self) -> Result<Self, String> {
        self.map_geometry_zm(Keep::Both, |g| match g {
            Geometry::Polygon(p) => Some(Geometry::MultiLineString(MultiLineString::new(
                p.interiors().to_vec(),
            ))),
            _ => None,
        })
    }

    /// The distinct vertices of each geometry as a `MultiPoint`, in first-seen order.
    pub fn extract_unique_points(&self) -> Result<Self, String> {
        self.map_geometry_model(z_only, |g| {
            let mut seen = HashSet::new();
            let points = g
                .coords_iter()
                .filter(|c| seen.insert((c.x.to_bits(), c.y.to_bits())))
                .map(Point::from)
                .collect();
            Some(Geometry::MultiPoint(MultiPoint::new(points)))
        })
    }

    /// The mean of each geometry's vertices as a point (polygon rings do not count their
    /// closing vertex twice); null for empty geometries.
    pub fn mean_coordinate(&self) -> Result<Self, String> {
        self.map_geometry(|g| {
            let mut all = Vec::new();
            vertices(g, &mut all);
            if all.is_empty() {
                return None;
            }
            let n = all.len() as f64;
            let (sx, sy) = all.iter().fold((0.0, 0.0), |(x, y), c| (x + c.x, y + c.y));
            Some(Geometry::Point(Point::new(sx / n, sy / n)))
        })
    }

    /// Combines all non-null geometries into one row: a `Multi*` when they share a family
    /// (points, lines or polygons; multi-part inputs are flattened), a `GeometryCollection`
    /// otherwise. One input geometry is returned as it is unless `multi` forces the `Multi*`
    /// form. No geometries give `GEOMETRYCOLLECTION EMPTY`.
    pub fn collect(&self, multi: bool) -> Result<Self, String> {
        let geoms: Vec<Geometry<f64>> = self.geo_rows().into_iter().flatten().collect();
        let first = geoms.first().map(family);
        let uniform =
            first.is_some_and(|f| f != Family::Mixed && geoms.iter().all(|g| family(g) == f));
        let combined = match (geoms.len(), uniform) {
            (0, _) => Geometry::GeometryCollection(GeometryCollection::default()),
            (1, _) if !multi => normalize(&geoms[0]),
            (_, true) => {
                let flat = geoms.iter().flat_map(parts);
                match first {
                    Some(Family::Points) => Geometry::MultiPoint(MultiPoint::new(
                        flat.filter_map(|g| Point::try_from(g).ok()).collect(),
                    )),
                    Some(Family::Lines) => Geometry::MultiLineString(MultiLineString::new(
                        flat.filter_map(|g| LineString::try_from(g).ok()).collect(),
                    )),
                    _ => Geometry::MultiPolygon(MultiPolygon::new(
                        flat.filter_map(|g| Polygon::try_from(g).ok()).collect(),
                    )),
                }
            }
            _ => Geometry::GeometryCollection(GeometryCollection::new_from(
                geoms.iter().map(normalize).collect(),
            )),
        };
        self.rebuild(&[Some(combined)])
    }

    /// Whether each row is an actual null (not merely empty).
    pub fn null_rows(&self) -> Vec<bool> {
        self.map_scalars(|_| ())
            .into_iter()
            .map(|row| row.is_none())
            .collect()
    }

    /// Whether each geometry is closed, as Shapely's `is_closed`: a non-empty line whose ends
    /// meet, or a multi-line all of whose members do. Every other type is `false`.
    pub fn is_closed(&self) -> Vec<Option<bool>> {
        self.map_scalars(|scalar| match geometry_to_geo(scalar) {
            Ok(Geometry::LineString(l)) => closed_line(&l),
            Ok(Geometry::MultiLineString(m)) => !m.0.is_empty() && m.0.iter().all(closed_line),
            _ => false,
        })
    }

    /// Whether each geometry is a ring: a closed line with at least four vertices that does
    /// not cross or touch itself except at the closing vertex. Every other type is `false`.
    pub fn is_ring(&self) -> Vec<Option<bool>> {
        self.map_scalars(|scalar| match geometry_to_geo(scalar) {
            Ok(Geometry::LineString(l)) => {
                catch_unwind(AssertUnwindSafe(|| is_simple_ring(&l))).unwrap_or(false)
            }
            _ => false,
        })
    }

    /// `"Valid Geometry"` or GEOS' first reason, `<text>[<x> <y>]`; null for null rows. A
    /// geometry `geo` rejects that the GEOS-order analysis passes keeps `geo`'s wording.
    pub fn is_valid_reason(&self) -> Vec<Option<String>> {
        self.map_scalars(|scalar| match geometry_to_geo(scalar) {
            Ok(g) => catch_unwind(AssertUnwindSafe(|| {
                let geo_reason = g.check_validation().err().map(|e| e.to_string());
                match geo_reason {
                    None if crate::validity::is_valid(&g) => VALID.to_string(),
                    geo_reason => crate::validity_reason::geos_reason(&g)
                        .or(geo_reason)
                        .unwrap_or_else(|| "Invalid Geometry".to_string()),
                }
            }))
            .unwrap_or_else(|_| "validation failed".to_string()),
            Err(_) => VALID.to_string(),
        })
    }

    /// Keeps valid geometries and nulls the invalid ones.
    pub fn null_out_invalid(&self) -> Result<Self, String> {
        self.map_geometry(|g| crate::validity::is_valid(g).then(|| g.clone()))
    }
}
