//! `buffer(..., single_sided=True)`: the band on one side of a line (Shapely 2 / GEOS).
//!
//! A positive distance buffers the left of the line's direction, a negative one the right.
//! Line ends are flat (the cap style is ignored); corners on the outside of the turn use the
//! join style (round fillet, limited mitre or bevel) and corners on the inside are cut by the
//! shifted segments. The band is built as the union of one quad per segment (the segment and
//! its shifted copy) and one wedge per outside corner, which is the region GEOS keeps between
//! the line and its offset curve.
//!
//! Other geometries, as GEOS treats them:
//!
//! * a point or multipoint gives its ordinary buffer for a positive distance and an empty
//!   result otherwise;
//! * a polygon gives only the band around its rings: the buffer without the polygon for a
//!   positive distance, the polygon without its eroded part for a negative one;
//! * a collection is the union of its members' bands; a distance of 0 and an empty input give
//!   an empty polygon.
//!
//! Deviations from GEOS: where GEOS' own output is inconsistent it is not copied. A closed line
//! gets the band between the ring and its offset (GEOS returns the interior of the inner ring),
//! a polygon with a hole or a multipolygon with parts closer than the distance can differ, and
//! a band that overlaps itself (a line doubling back closer than the distance) keeps every
//! covered point where GEOS can drop the overlap.

use geo::{unary_union, BooleanOps, Buffer, Coord, Geometry, LineString, MultiPolygon, Polygon};

use crate::unary::{collapse, BufferOptions};

type C = Coord<f64>;

/// Which join closes an outside corner.
#[derive(Clone, Copy)]
pub(crate) enum Join {
    Round,
    Mitre,
    Bevel,
}

/// Parameters of the band, already validated.
pub(crate) struct Sided {
    pub join: Join,
    /// Radians per arc step.
    pub step: f64,
    pub mitre_limit: f64,
}

impl Sided {
    pub(crate) fn new(options: &BufferOptions) -> Result<Self, String> {
        let join = match options.join.as_str() {
            "round" => Join::Round,
            "mitre" => Join::Mitre,
            "bevel" => Join::Bevel,
            other => return Err(format!("unknown join_style: {other}")),
        };
        Ok(Self {
            join,
            step: std::f64::consts::FRAC_PI_2 / f64::from(options.quad_segs.max(1)),
            mitre_limit: options.mitre_limit,
        })
    }
}

fn sub(a: C, b: C) -> C {
    C {
        x: a.x - b.x,
        y: a.y - b.y,
    }
}

fn add(a: C, b: C) -> C {
    C {
        x: a.x + b.x,
        y: a.y + b.y,
    }
}

fn scale(a: C, f: f64) -> C {
    C {
        x: a.x * f,
        y: a.y * f,
    }
}

fn cross(a: C, b: C) -> f64 {
    a.x * b.y - a.y * b.x
}

fn dot(a: C, b: C) -> f64 {
    a.x * b.x + a.y * b.y
}

fn unit(a: C, b: C) -> C {
    let v = sub(b, a);
    scale(v, 1.0 / v.x.hypot(v.y))
}

/// Left normal of a unit direction, scaled by the signed distance.
fn shifted(u: C, d: f64) -> C {
    C {
        x: -u.y * d,
        y: u.x * d,
    }
}

fn polygon(mut ring: Vec<C>) -> Polygon<f64> {
    let area: f64 = ring
        .iter()
        .zip(ring.iter().cycle().skip(1))
        .map(|(a, b)| cross(*a, *b))
        .sum();
    if area < 0.0 {
        ring.reverse();
    }
    Polygon::new(LineString::new(ring), vec![])
}

/// The points of the wedge that closes the outside corner at `v` between the shifted ends
/// `o0` and `o1` (after `o0`, before `o1`).
fn corner(sided: &Sided, v: C, (u0, u1): (C, C), (n0, n1): (C, C), d: f64) -> Vec<C> {
    let (o0, o1) = (add(v, n0), add(v, n1));
    match sided.join {
        Join::Bevel => vec![o0, o1],
        Join::Round => {
            let mut sweep = cross(n0, n1).atan2(dot(n0, n1));
            if sweep.abs() > std::f64::consts::PI - 1e-9 {
                // A hairpin: go round the end of the segment, whichever way that is.
                let quarter = C { x: -n0.y, y: n0.x };
                let sign = if dot(quarter, u0) >= 0.0 { 1.0 } else { -1.0 };
                sweep = sign * std::f64::consts::PI;
            }
            let steps = (sweep.abs() / sided.step).ceil().max(1.0) as usize;
            let radius = d.abs();
            let start = n0.y.atan2(n0.x);
            let mut pts = vec![o0];
            for k in 1..steps {
                let a = start + sweep * (k as f64 / steps as f64);
                pts.push(add(
                    v,
                    C {
                        x: radius * a.cos(),
                        y: radius * a.sin(),
                    },
                ));
            }
            pts.push(o1);
            pts
        }
        Join::Mitre => {
            let bis = add(n0, n1);
            let len = bis.x.hypot(bis.y);
            let limit = sided.mitre_limit * d.abs();
            let cos_half = len / (2.0 * d.abs());
            if cos_half < 1e-9 {
                return vec![o0, o1];
            }
            let reach = d.abs() / cos_half;
            let dir = scale(bis, 1.0 / len);
            if reach <= limit {
                return vec![o0, add(v, scale(dir, reach)), o1];
            }
            // Beyond the limit the mitre is cut square at `limit` from the vertex.
            let cut = add(v, scale(dir, limit));
            let p0 = add(o0, scale(u0, dot(sub(cut, o0), dir) / dot(u0, dir)));
            let p1 = add(o1, scale(u1, dot(sub(cut, o1), dir) / dot(u1, dir)));
            vec![o0, p0, p1, o1]
        }
    }
}

/// Where the shifted copies of two consecutive segments meet at an inside corner `v`, when
/// that point lies on both copies (short segments: it does not, and the quads stay square).
fn inside_meet(v: C, (u0, u1): (C, C), (len0, len1): (f64, f64), d: f64) -> Option<C> {
    let cos = dot(u0, u1);
    if cos < -1.0 + 1e-9 {
        return None;
    }
    let (n0, n1) = (shifted(u0, d), shifted(u1, d));
    let meet = add(v, scale(add(n0, n1), 1.0 / (1.0 + cos)));
    let back = -dot(sub(meet, add(v, n0)), u0);
    let ahead = dot(sub(meet, add(v, n1)), u1);
    (back >= -1e-12 && back <= len0 && ahead >= -1e-12 && ahead <= len1).then_some(meet)
}

/// What closes a line.
#[derive(Clone, Copy, PartialEq)]
enum Ring {
    Open,
    /// A closed line: inside corners are met at the start too, but an outside corner there stays
    /// flat as in GEOS.
    ClosedLine,
    /// A polygon ring: every corner is joined.
    Polygon,
}

/// The band polygons of one line or ring.
///
/// Each segment contributes the quad between it and its shifted copy; at an inside corner the
/// quads end on the line through the corner and the meeting point of the shifted copies, as the
/// offset curve does, so they do not reach past the neighbouring segment. Outside corners add
/// a wedge.
fn band(coords: &[C], ring: Ring, d: f64, sided: &Sided, out: &mut Vec<Polygon<f64>>) {
    let flat_start = ring == Ring::ClosedLine;
    let ring = ring != Ring::Open;
    let n = coords.len();
    let segments = if ring { n } else { n - 1 };
    let at = |i: usize| (coords[i % n], coords[(i + 1) % n]);
    let dirs: Vec<C> = (0..segments)
        .map(|i| {
            let (a, b) = at(i);
            unit(a, b)
        })
        .collect();
    let lens: Vec<f64> = (0..segments)
        .map(|i| {
            let (a, b) = at(i);
            (b.x - a.x).hypot(b.y - a.y)
        })
        .collect();
    // Corner `i` joins segment `i - 1` and segment `i`.
    let has_corner = |i: usize| if ring { true } else { i >= 1 && i < n - 1 };
    let pair = |i: usize| {
        let prev = (i + segments - 1) % segments;
        (
            (dirs[prev], dirs[i % segments]),
            (lens[prev], lens[i % segments]),
        )
    };
    let is_inside = |i: usize| {
        let ((u0, u1), _) = pair(i);
        cross(u0, u1) * d > 0.0
    };
    let meet = |i: usize| {
        if !has_corner(i % n) || !is_inside(i % n) {
            return None;
        }
        let (dirs, lens) = pair(i % n);
        inside_meet(coords[i % n], dirs, lens, d)
    };
    for (i, &u) in dirs.iter().enumerate() {
        let (a, b) = at(i);
        let s = shifted(u, d);
        let start = meet(i).unwrap_or_else(|| add(a, s));
        let end = meet(i + 1).unwrap_or_else(|| add(b, s));
        out.push(polygon(vec![a, b, end, start]));
    }
    for i in (0..n).filter(|&i| has_corner(i) && !(flat_start && i == 0)) {
        let ((u0, u1), _) = pair(i);
        let turn = cross(u0, u1);
        // The shifted side is outside the turn when the line turns away from it.
        if turn * d >= 0.0 || (turn.abs() < 1e-12 && dot(u0, u1) > 0.0) {
            continue;
        }
        let v = coords[i];
        let mut wedge = vec![v];
        wedge.extend(corner(
            sided,
            v,
            (u0, u1),
            (shifted(u0, d), shifted(u1, d)),
            d,
        ));
        out.push(polygon(wedge));
    }
}

/// A ring's distinct vertices without the closing repeat; `None` below three.
fn clean_ring(ring: &LineString<f64>) -> Option<Vec<C>> {
    let mut coords: Vec<C> = Vec::with_capacity(ring.0.len());
    for &c in &ring.0 {
        if coords.last() != Some(&c) {
            coords.push(c);
        }
    }
    if coords.len() > 1 && coords.first() == coords.last() {
        coords.pop();
    }
    (coords.len() >= 3).then_some(coords)
}

fn line_bands(line: &LineString<f64>, d: f64, sided: &Sided, out: &mut Vec<Polygon<f64>>) {
    let mut coords: Vec<C> = Vec::with_capacity(line.0.len());
    for &c in &line.0 {
        if coords.last() != Some(&c) {
            coords.push(c);
        }
    }
    if coords.len() >= 4 && coords.first() == coords.last() {
        coords.pop();
        band(&coords, Ring::ClosedLine, d, sided, out);
    } else if coords.len() >= 2 {
        band(&coords, Ring::Open, d, sided, out);
    }
}

fn clockwise(coords: &[C]) -> bool {
    let n = coords.len();
    (0..n)
        .map(|i| (coords[(i + 1) % n].x - coords[i].x) * (coords[(i + 1) % n].y + coords[i].y))
        .sum::<f64>()
        > 0.0
}

/// Bands of the rings of `p`, oriented so a positive distance grows the polygon.
fn polygon_bands(p: &Polygon<f64>, d: f64, sided: &Sided, out: &mut Vec<Polygon<f64>>) {
    let rings =
        std::iter::once((p.exterior(), true)).chain(p.interiors().iter().map(|r| (r, false)));
    for (ring, shell) in rings {
        if let Some(mut coords) = clean_ring(ring) {
            if clockwise(&coords) != shell {
                coords.reverse();
            }
            band(&coords, Ring::Polygon, d, sided, out);
        }
    }
}

fn union(polys: Vec<Polygon<f64>>) -> MultiPolygon<f64> {
    unary_union(&polys)
}

fn members(
    g: &Geometry<f64>,
    d: f64,
    sided: &Sided,
    options: &BufferOptions,
    acc: &mut Vec<Polygon<f64>>,
) {
    match g {
        Geometry::Point(_) | Geometry::MultiPoint(_) => {
            if d > 0.0 {
                if let Ok(style) = options.style(d) {
                    acc.extend(g.buffer_with_style(style).0);
                }
            }
        }
        Geometry::Line(l) => line_bands(&LineString::from(*l), d, sided, acc),
        Geometry::LineString(l) => line_bands(l, d, sided, acc),
        Geometry::MultiLineString(m) => m.0.iter().for_each(|l| line_bands(l, d, sided, acc)),
        Geometry::Polygon(p) => acc.extend(polygon_band(p, d, sided, options).0),
        Geometry::MultiPolygon(m) => {
            m.0.iter()
                .for_each(|p| acc.extend(polygon_band(p, d, sided, options).0))
        }
        Geometry::Rect(r) => acc.extend(polygon_band(&r.to_polygon(), d, sided, options).0),
        Geometry::Triangle(t) => acc.extend(polygon_band(&t.to_polygon(), d, sided, options).0),
        Geometry::GeometryCollection(c) => {
            c.0.iter().for_each(|m| members(m, d, sided, options, acc))
        }
    }
}

fn polygon_band(
    p: &Polygon<f64>,
    d: f64,
    sided: &Sided,
    options: &BufferOptions,
) -> MultiPolygon<f64> {
    // GEOS leaves out a polygon that a negative distance erodes completely.
    if d < 0.0 {
        let eroded = options
            .style(d)
            .map_or(true, |style| p.buffer_with_style(style).0.is_empty());
        if eroded {
            return MultiPolygon::new(vec![]);
        }
    }
    let mut bands = Vec::new();
    polygon_bands(p, d, sided, &mut bands);
    let bands = union(bands);
    let body = MultiPolygon::new(vec![p.clone()]);
    if d > 0.0 {
        bands.difference(&body)
    } else {
        bands.intersection(&body)
    }
}

/// The single-sided buffer of `g` at the signed distance `d`.
pub(crate) fn single_sided_geometry(
    g: &Geometry<f64>,
    d: f64,
    sided: &Sided,
    options: &BufferOptions,
) -> Option<Geometry<f64>> {
    if !d.is_finite() {
        return None;
    }
    let mut acc = Vec::new();
    if d != 0.0 {
        members(g, d, sided, options, &mut acc);
    }
    Some(collapse(union(acc)))
}
