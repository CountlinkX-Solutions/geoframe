//! Parity batch B: `explode`, `clip_by_rect` and `intersection_all`.

use geo::{
    BooleanOps, Coord, Geometry, GeometryCollection, MultiLineString, MultiPolygon, Point, Rect,
};

use crate::mixed_ops::{classify, Rel};
use crate::parity_a::parts;
use crate::predicates::guarded;
use crate::set_ops::{lines, overlay_pair, points, polygons};
use crate::{GeoColumn, SetOp};

fn empty_collection() -> Geometry<f64> {
    Geometry::GeometryCollection(GeometryCollection::new_from(vec![]))
}

/// `clip_by_rect` of one geometry: the part strictly inside the rectangle's interior for
/// lines and points, the area for polygons. `GeometryCollection` input gives `None`.
fn clip_rect(geom: &Geometry<f64>, rect: Rect<f64>) -> Option<Geometry<f64>> {
    let window = MultiPolygon::new(vec![rect.to_polygon()]);
    let inside = |p: &Point<f64>| {
        p.x() > rect.min().x && p.x() < rect.max().x && p.y() > rect.min().y && p.y() < rect.max().y
    };
    let clipped = match geom {
        Geometry::Point(p) => inside(p).then_some(Geometry::Point(*p)),
        Geometry::MultiPoint(m) => {
            let kept: Vec<Point<f64>> = m.0.iter().filter(|p| inside(p)).copied().collect();
            (!kept.is_empty()).then(|| points(kept))
        }
        Geometry::Polygon(_)
        | Geometry::MultiPolygon(_)
        | Geometry::Rect(_)
        | Geometry::Triangle(_) => {
            let subject = match geom {
                Geometry::Polygon(p) => MultiPolygon::new(vec![p.clone()]),
                Geometry::MultiPolygon(m) => m.clone(),
                Geometry::Rect(r) => MultiPolygon::new(vec![r.to_polygon()]),
                Geometry::Triangle(t) => MultiPolygon::new(vec![t.to_polygon()]),
                _ => unreachable!("matched above"),
            };
            let area = subject.intersection(&window);
            (!area.0.is_empty()).then(|| polygons(area))
        }
        Geometry::Line(_) | Geometry::LineString(_) | Geometry::MultiLineString(_) => {
            let ls = match geom {
                Geometry::Line(l) => MultiLineString::new(vec![(*l).into()]),
                Geometry::LineString(l) => MultiLineString::new(vec![l.clone()]),
                Geometry::MultiLineString(m) => m.clone(),
                _ => unreachable!("matched above"),
            };
            let kept: Vec<_> = classify(&ls, &window)
                .pieces
                .into_iter()
                .filter(|(_, rel)| *rel == Rel::Inside)
                .map(|(piece, _)| geo::LineString::new(piece.coords))
                .collect();
            (!kept.is_empty()).then(|| lines(MultiLineString::new(kept)))
        }
        Geometry::GeometryCollection(_) => return None,
    };
    Some(clipped.unwrap_or_else(empty_collection))
}

impl GeoColumn {
    /// One row per part of every geometry, and the source row of each part.
    ///
    /// Multi-part geometries give their members and collections their direct members (one
    /// level, as `shapely.get_parts`); an atomic geometry, even an empty polygon or line, is its
    /// own single part. Null rows and empty multi-geometries or collections have no parts and
    /// disappear. The result keeps the CRS.
    pub fn explode(&self) -> Result<(GeoColumn, Vec<u32>), String> {
        let mut rows: Vec<Option<Geometry<f64>>> = Vec::new();
        let mut source: Vec<u32> = Vec::new();
        for (i, geom) in self.geo_rows().into_iter().enumerate() {
            let Some(geom) = geom else { continue };
            for part in parts(&geom) {
                rows.push(Some(part));
                source.push(u32::try_from(i).map_err(|_| "too many rows".to_string())?);
            }
        }
        Ok((self.rebuild(&rows)?, source))
    }

    /// Shapely's `clip_by_rect`: each row clipped to the rectangle, row by row, empties kept.
    ///
    /// Only the part reaching into the rectangle's interior survives: boundary-only contact
    /// (a point on the edge, a line along it, a polygon touching it) gives
    /// `GEOMETRYCOLLECTION EMPTY`. Like GEOS the result of a polygon can be invalid.
    /// `GeometryCollection` and null rows give null.
    pub fn clip_by_rect(
        &self,
        xmin: f64,
        ymin: f64,
        xmax: f64,
        ymax: f64,
    ) -> Result<GeoColumn, String> {
        if !(xmin <= xmax && ymin <= ymax) {
            return Err(format!(
                "invalid rectangle: expected xmin <= xmax and ymin <= ymax, got ({xmin}, {ymin}, {xmax}, {ymax})"
            ));
        }
        let rect = Rect::new(Coord { x: xmin, y: ymin }, Coord { x: xmax, y: ymax });
        let rows: Vec<Option<Geometry<f64>>> = self
            .geo_rows()
            .iter()
            .map(|row| {
                row.as_ref()
                    .and_then(|g| guarded(|| clip_rect(g, rect)).flatten())
            })
            .collect();
        self.rebuild(&rows)
    }

    /// The intersection of every non-null row as a one-row column (the CRS is kept).
    ///
    /// Rows are folded left to right with the pairwise `intersection`; no non-null rows give
    /// `GEOMETRYCOLLECTION EMPTY`. A pairwise result that is a `GeometryCollection` (mixed
    /// dimensions) cannot be intersected further and gives null.
    pub fn intersection_all(&self) -> Result<GeoColumn, String> {
        let rows: Vec<Geometry<f64>> = self.geo_rows().into_iter().flatten().collect();
        let result = guarded(|| {
            let mut iter = rows.into_iter();
            let Some(mut acc) = iter.next() else {
                return Some(empty_collection());
            };
            for geom in iter {
                acc = overlay_pair(&acc, &geom, SetOp::Intersection)?;
            }
            Some(acc)
        })
        .flatten();
        self.rebuild(&[result])
    }
}
