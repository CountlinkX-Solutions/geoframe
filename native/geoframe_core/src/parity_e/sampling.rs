//! `sample_points`: uniform random points inside polygons or along lines.
//!
//! The generator is SplitMix64, seeded explicitly (or from the clock), so a seed reproduces a
//! sample bit for bit on every platform. It does not reproduce NumPy's stream: a seed gives a
//! deterministic sample, not GeoPandas' sample.

use std::time::{SystemTime, UNIX_EPOCH};

use geo::{
    Area, BoundingRect, Contains, Coord, Euclidean, Geometry, Length, LineString, MultiPoint, Point,
};

/// SplitMix64 (Steele, Lea and Flood; the seeding generator of the xoshiro family).
pub(super) struct Rng(u64);

impl Rng {
    pub fn new(seed: Option<u64>) -> Self {
        let seed = seed.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0x5EED, |d| d.as_nanos() as u64)
        });
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)` with 53 random bits.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn multipoint(points: Vec<Point<f64>>) -> Geometry<f64> {
    Geometry::MultiPoint(MultiPoint::new(points))
}

fn line_parts(geom: &Geometry<f64>) -> Option<Vec<LineString<f64>>> {
    match geom {
        Geometry::LineString(l) => Some(vec![l.clone()]),
        Geometry::Line(l) => Some(vec![LineString::new(vec![l.start, l.end])]),
        Geometry::MultiLineString(m) => Some(m.0.clone()),
        _ => None,
    }
}

/// The point `distance` along the parts laid end to end.
fn locate(parts: &[(LineString<f64>, f64)], mut distance: f64) -> Coord<f64> {
    for (line, length) in parts {
        if distance <= *length {
            for segment in line.lines() {
                let len = Euclidean.length(&segment);
                if distance <= len && len > 0.0 {
                    let t = distance / len;
                    return Coord {
                        x: segment.start.x + t * (segment.end.x - segment.start.x),
                        y: segment.start.y + t * (segment.end.y - segment.start.y),
                    };
                }
                distance -= len;
            }
        }
        distance -= length;
    }
    parts
        .last()
        .and_then(|(l, _)| l.0.last().copied())
        .unwrap_or(Coord { x: 0.0, y: 0.0 })
}

/// Most points one `sample_points` call may draw in total, across all rows.
pub const MAX_SAMPLES: usize = 10_000_000;

/// `size` points, or `None` when a very thin polygon cannot be hit within the attempt budget.
/// Types other than (multi)polygons and (multi)lines, and zero-area shapes, give no points.
pub(super) fn sample(geom: &Geometry<f64>, size: usize, rng: &mut Rng) -> Option<Geometry<f64>> {
    if size == 0 {
        return Some(multipoint(Vec::new()));
    }
    if let Some(parts) = line_parts(geom) {
        let measured: Vec<(LineString<f64>, f64)> = parts
            .into_iter()
            .map(|l| {
                let length = Euclidean.length(&l);
                (l, length)
            })
            .collect();
        let total: f64 = measured.iter().map(|(_, l)| l).sum();
        if total.is_nan() || total <= 0.0 {
            return Some(multipoint(Vec::new()));
        }
        let points = (0..size)
            .map(|_| Point::from(locate(&measured, rng.unit() * total)))
            .collect();
        return Some(multipoint(points));
    }
    if !matches!(geom, Geometry::Polygon(_) | Geometry::MultiPolygon(_)) {
        return Some(multipoint(Vec::new()));
    }
    let Some(rect) = geom.bounding_rect() else {
        return Some(multipoint(Vec::new()));
    };
    if rect.width() <= 0.0 || rect.height() <= 0.0 || geom.unsigned_area() <= 0.0 {
        return Some(multipoint(Vec::new()));
    }
    let mut points = Vec::with_capacity(size);
    let mut attempts = 0usize;
    let budget = size.saturating_mul(1000).saturating_add(100_000);
    while points.len() < size {
        if attempts >= budget {
            return None;
        }
        attempts += 1;
        let p = Point::new(
            rect.min().x + rng.unit() * rect.width(),
            rect.min().y + rng.unit() * rect.height(),
        );
        if geom.contains(&p) {
            points.push(p);
        }
    }
    Some(multipoint(points))
}
