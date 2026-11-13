//! Curved geometries are read linearized as GDAL does (B10c).
//!
//! `test/reference/fixtures/curves/curves.json` is produced by
//! `test/reference/generate_curves.py` (GeoPackage rows holding curve geometries, read back
//! with pyogrio 0.13 / GDAL 3.12). Each case holds the ISO WKB input and the linear WKB GDAL
//! returned; vertices must agree to 1e-9, and the vertex count exactly.

use crate::ewkb::ewkb_to_iso;
use crate::GeoColumn;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Everything a linear WKB holds, flattened: type words and counts, then every ordinate.
#[derive(Debug, Default, PartialEq)]
struct Flat {
    words: Vec<u32>,
    ordinates: Vec<f64>,
}

fn walk(b: &[u8], pos: &mut usize, out: &mut Flat) {
    let little = b[*pos] == 1;
    *pos += 1;
    let word = |pos: &mut usize| -> u32 {
        let raw: [u8; 4] = b[*pos..*pos + 4].try_into().unwrap();
        *pos += 4;
        if little {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        }
    };
    let code = word(pos);
    out.words.push(code);
    let dims =
        2 + usize::from(matches!(code / 1000, 1 | 3)) + usize::from(matches!(code / 1000, 2 | 3));
    let coords = |pos: &mut usize, n: usize, out: &mut Flat| {
        for _ in 0..n * dims {
            let raw: [u8; 8] = b[*pos..*pos + 8].try_into().unwrap();
            *pos += 8;
            out.ordinates.push(if little {
                f64::from_le_bytes(raw)
            } else {
                f64::from_be_bytes(raw)
            });
        }
    };
    match code % 1000 {
        1 => coords(pos, 1, out),
        2 => {
            let n = word(pos);
            out.words.push(n);
            coords(pos, n as usize, out);
        }
        3 => {
            let rings = word(pos);
            out.words.push(rings);
            for _ in 0..rings {
                let n = word(pos);
                out.words.push(n);
                coords(pos, n as usize, out);
            }
        }
        4..=7 => {
            let n = word(pos);
            out.words.push(n);
            for _ in 0..n {
                walk(b, pos, out);
            }
        }
        other => panic!("curve type {other} left in the output"),
    }
}

fn flat(b: &[u8]) -> Flat {
    let mut out = Flat::default();
    let mut pos = 0;
    walk(b, &mut pos, &mut out);
    assert_eq!(pos, b.len(), "trailing bytes");
    out
}

fn assert_close(case: &str, got: &Flat, want: &Flat) {
    assert_eq!(
        got.words, want.words,
        "{case}: type words and vertex counts"
    );
    assert_eq!(
        got.ordinates.len(),
        want.ordinates.len(),
        "{case}: ordinates"
    );
    for (i, (g, w)) in got.ordinates.iter().zip(&want.ordinates).enumerate() {
        assert!((g - w).abs() <= 1e-9, "{case}: ordinate {i}: {g} vs {w}");
    }
}

#[test]
fn every_curve_case_matches_gdal() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../test/reference/fixtures/curves/curves.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() >= 30);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let (iso, _) = ewkb_to_iso(&hex(case["input"].as_str().unwrap())).unwrap();
        let want = flat(&hex(case["expected"].as_str().unwrap()));
        assert_close(name, &flat(&iso), &want);
    }
}

#[test]
fn curve_rows_no_longer_abort_a_read() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../test/reference/fixtures/curves/curves.json"
    ))
    .unwrap();
    let rows: Vec<Option<Vec<u8>>> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Some(hex(c["input"].as_str().unwrap())))
        .collect();
    let column = GeoColumn::from_wkb(&rows, None).unwrap();
    let types = column.geom_type();
    assert!(types.iter().all(|t| t.is_some()));
    assert_eq!(types.len(), rows.len());
}

fn le_header(code: u32) -> Vec<u8> {
    let mut v = vec![1u8];
    v.extend_from_slice(&code.to_le_bytes());
    v
}

fn push_points(v: &mut Vec<u8>, pts: &[&[f64]]) {
    v.extend_from_slice(&(pts.len() as u32).to_le_bytes());
    for p in pts {
        for c in *p {
            v.extend_from_slice(&c.to_le_bytes());
        }
    }
}

#[test]
fn m_is_interpolated_along_the_arc() {
    // GDAL drops M when it linearizes an arc; GeoFrame keeps it, linear in the angle.
    let mut b = le_header(2008);
    push_points(
        &mut b,
        &[&[0.0, 0.0, 0.0], &[1.0, 1.0, 50.0], &[2.0, 0.0, 100.0]],
    );
    let (iso, _) = ewkb_to_iso(&b).unwrap();
    let f = flat(&iso);
    assert_eq!(f.words, vec![2002, 46]);
    for (k, v) in f.ordinates.chunks(3).enumerate() {
        let want = 100.0 * k as f64 / 45.0;
        assert!((v[2] - want).abs() < 1e-9, "vertex {k}: {} vs {want}", v[2]);
    }
}

#[test]
fn zm_compound_curve_keeps_both_ordinates() {
    let mut line = le_header(3002);
    push_points(
        &mut line,
        &[&[-2.0, 0.0, 1.0, 10.0], &[0.0, 0.0, 2.0, 20.0]],
    );
    let mut arc = le_header(3008);
    push_points(
        &mut arc,
        &[
            &[0.0, 0.0, 2.0, 20.0],
            &[1.0, 1.0, 3.0, 30.0],
            &[2.0, 0.0, 4.0, 40.0],
        ],
    );
    let mut b = le_header(3009);
    b.extend_from_slice(&2u32.to_le_bytes());
    b.extend_from_slice(&line);
    b.extend_from_slice(&arc);
    let (iso, _) = ewkb_to_iso(&b).unwrap();
    let f = flat(&iso);
    assert_eq!(f.words, vec![3002, 47]);
    let last = &f.ordinates[f.ordinates.len() - 4..];
    assert_eq!(last, &[2.0, 0.0, 4.0, 40.0]);
}

#[test]
fn an_arc_with_fewer_than_three_points_is_empty_like_gdal() {
    let mut b = le_header(8);
    push_points(&mut b, &[&[0.0, 0.0], &[1.0, 1.0]]);
    let (iso, _) = ewkb_to_iso(&b).unwrap();
    assert_eq!(flat(&iso).words, vec![2, 0]);
}

#[test]
fn a_smaller_step_adds_vertices() {
    use crate::curves::{linearize_arc, Pt};
    let p = |x, y| Pt {
        x,
        y,
        z: 0.0,
        m: 0.0,
    };
    let four = linearize_arc(p(0.0, 0.0), p(1.0, 1.0), p(2.0, 0.0), 4.0);
    let one = linearize_arc(p(0.0, 0.0), p(1.0, 1.0), p(2.0, 0.0), 1.0);
    assert_eq!(four.len(), 46);
    assert!(one.len() > 4 * four.len() - 8, "{}", one.len());
}

#[test]
fn a_truncated_curve_is_still_an_error() {
    let mut b = le_header(8);
    b.extend_from_slice(&3u32.to_le_bytes());
    b.extend_from_slice(&[0u8; 20]);
    assert!(ewkb_to_iso(&b).is_err());
    // A curve member of the wrong family is rejected instead of guessed.
    let mut poly = le_header(2);
    push_points(&mut poly, &[&[0.0, 0.0], &[1.0, 1.0]]);
    let mut multi_surface = le_header(12);
    multi_surface.extend_from_slice(&1u32.to_le_bytes());
    multi_surface.extend_from_slice(&poly);
    let column = GeoColumn::from_wkb(&[Some(multi_surface)], None);
    assert!(column.is_err());
}

#[test]
fn declared_curve_layer_types_report_the_linear_type_like_pyogrio() {
    // pyogrio 0.13 / GDAL 3.12 `read_info` on a GeoPackage layer declared with a curve type.
    use crate::gpkg_io::display_geometry_type as shown;
    assert_eq!(shown("CIRCULARSTRING", 0, 0), "LineString");
    assert_eq!(shown("COMPOUNDCURVE", 0, 0), "LineString");
    assert_eq!(shown("CURVE", 0, 0), "LineString");
    assert_eq!(shown("CURVEPOLYGON", 0, 0), "Polygon");
    assert_eq!(shown("SURFACE", 0, 0), "Polygon");
    assert_eq!(shown("MULTICURVE", 0, 0), "MultiLineString");
    assert_eq!(shown("MULTISURFACE", 1, 0), "MultiPolygon Z");
    assert_eq!(shown("CIRCULARSTRING", 1, 1), "LineString ZM");
}

#[test]
fn wkt_curves_are_not_parsed() {
    // The `wkt` crate has no curve keywords, so WKT input stays linear-only; the error names
    // the row. WKB / EWKB / GeoPackage / PostGIS input is where curves occur in practice.
    let err = GeoColumn::from_wkt(&[Some("CIRCULARSTRING(0 0, 1 1, 2 0)")], None).unwrap_err();
    assert!(err.contains("invalid WKT at row 0"), "{err}");
}

/// Fuzz against GDAL: `GEOFRAME_CURVE_FUZZ=<cases.json> cargo test -- --ignored curve_fuzz`, where
/// the file has the layout of `curves.json` (random arcs of every sweep, near-collinear, Z,
/// compound members, curve polygons, written to a GeoPackage and read back by pyogrio).
#[test]
#[ignore]
fn curve_fuzz_against_gdal() {
    let Ok(path) = std::env::var("GEOFRAME_CURVE_FUZZ") else {
        return;
    };
    let fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let (mut agree, mut differ, mut exact, mut z_quirk) = (0, 0, 0, 0);
    let mut worst = 0.0f64;
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let (iso, _) = ewkb_to_iso(&hex(case["input"].as_str().unwrap())).unwrap();
        let (got, want) = (flat(&iso), flat(&hex(case["expected"].as_str().unwrap())));
        // GDAL 3.12 returns Z = 0 for a CurvePolygon inside a MultiSurface Z (a GDAL defect,
        // see docs/parity.md); those rows are compared on X and Y only.
        let skip_z = got.words.first() == Some(&1006);
        z_quirk += usize::from(skip_z);
        let stride = if skip_z { 3 } else { 1 };
        let keep = |i: usize| !skip_z || i % stride != 2;
        let same_shape = got.words == want.words && got.ordinates.len() == want.ordinates.len();
        let max = got
            .ordinates
            .iter()
            .zip(&want.ordinates)
            .enumerate()
            .filter(|(i, _)| keep(*i))
            .map(|(_, (g, w))| (g - w).abs() / w.abs().max(1.0))
            .fold(0.0, f64::max);
        if same_shape && max <= 1e-9 {
            agree += 1;
            exact += usize::from(got.ordinates == want.ordinates);
            worst = worst.max(max);
        } else {
            differ += 1;
            eprintln!("{name}: shape {same_shape} max relative diff {max}");
        }
    }
    eprintln!("fuzz: {agree} agree ({exact} bit-exact, worst relative {worst:e}; {z_quirk} MultiSurface Z compared on X/Y), {differ} differ");
    assert_eq!(differ, 0);
}
