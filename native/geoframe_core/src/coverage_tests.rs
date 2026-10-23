//! Batch I3 (part 1): `is_valid_coverage`, `invalid_coverage_edges`.
//! Expected answers come from Shapely 2.2 / GEOS 3.14 (`coverage_is_valid`,
//! `coverage_invalid_edges`).

use crate::GeoColumn;

pub(crate) fn col(rows: &[Option<&str>]) -> GeoColumn {
    GeoColumn::from_wkt(rows, None).expect("valid WKT")
}

pub(crate) fn some(rows: &[&str]) -> GeoColumn {
    col(&rows.iter().map(|r| Some(*r)).collect::<Vec<_>>())
}

/// `wkt` as this crate prints it, so expected values compare textually.
pub(crate) fn canon(wkt: &str) -> String {
    some(&[wkt]).to_wkt().unwrap().remove(0).unwrap()
}

pub(crate) fn text(column: &GeoColumn) -> Vec<Option<String>> {
    column.to_wkt().unwrap()
}

pub(crate) fn canon_all(rows: &[&str]) -> Vec<Option<String>> {
    rows.iter().map(|r| Some(canon(r))).collect()
}

const A: &str = "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))";
const B: &str = "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))";
const EMPTY: &str = "LINESTRING EMPTY";

// -- validation ---------------------------------------------------------------------------

/// (rows, gap width, valid, invalid edges per row)
type Case = (&'static [&'static str], f64, bool, &'static [&'static str]);

const VALIDATION: &[Case] = &[
    (&[A, B], 0.0, true, &[EMPTY, EMPTY]),
    (
        &[A, "POLYGON ((1 0, 3 0, 3 2, 1 2, 1 0))"],
        0.0,
        false,
        &[
            "LINESTRING (0 0, 2 0, 2 2, 0 2)",
            "LINESTRING (3 2, 1 2, 1 0, 3 0)",
        ],
    ),
    (
        &[
            "POLYGON ((0 0, 1 1, 1 0, 0 0))",
            "POLYGON ((0 0, 0.5 0.5, 1 1, 0 1, 0 0))",
        ],
        0.0,
        false,
        &["LINESTRING (0 0, 1 1)", "LINESTRING (0 0, 0.5 0.5, 1 1)"],
    ),
    (
        &[
            "POLYGON ((0 0, 1 1, 1 0, 0 0))",
            "POLYGON ((0 0, 1 1, 0 1, 0 0))",
        ],
        0.0,
        true,
        &[EMPTY, EMPTY],
    ),
    (
        &[A, "POLYGON ((2.1 0, 4 0, 4 2, 2.1 2, 2.1 0))"],
        0.2,
        false,
        &["LINESTRING (2 0, 2 2)", "LINESTRING (2.1 2, 2.1 0)"],
    ),
    (
        &[A, "POLYGON ((2.1 0, 4 0, 4 2, 2.1 2, 2.1 0))"],
        0.05,
        true,
        &[EMPTY, EMPTY],
    ),
    (
        &[A, "POLYGON ((2.1 0, 4 0, 4 2, 2.1 2, 2.1 0))"],
        0.0,
        true,
        &[EMPTY, EMPTY],
    ),
    (
        &[
            "POLYGON ((0 0, 6 0, 6 6, 0 6, 0 0), (2 2, 2 4, 4 4, 4 2, 2 2))",
            "POLYGON ((2 2, 4 2, 4 4, 2 4, 2 2))",
        ],
        0.0,
        true,
        &[EMPTY, EMPTY],
    ),
    (
        &[
            "POLYGON ((0 0, 6 0, 6 6, 0 6, 0 0), (2 2, 2 4, 4 4, 4 2, 2 2))",
            "POLYGON ((2.1 2.1, 3.9 2.1, 3.9 3.9, 2.1 3.9, 2.1 2.1))",
        ],
        0.3,
        false,
        &[
            "LINESTRING (2 2, 2 4, 4 4, 4 2, 2 2)",
            "LINESTRING (2.1 2.1, 3.9 2.1, 3.9 3.9, 2.1 3.9, 2.1 2.1)",
        ],
    ),
    (
        &[
            A,
            "POLYGON ((2 0, 4 0, 4 1, 2 1, 2 0))",
            "POLYGON ((2 1, 4 1, 4 2, 2 2, 2 1))",
        ],
        0.0,
        false,
        &[
            "LINESTRING (2 0, 2 2)",
            "LINESTRING (2 1, 2 0)",
            "LINESTRING (2 2, 2 1)",
        ],
    ),
    (
        &[
            "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))",
            "POLYGON ((1 1, 2 1, 2 2, 1 2, 1 1))",
        ],
        0.0,
        false,
        &[EMPTY, "LINESTRING (1 1, 2 1, 2 2, 1 2, 1 1)"],
    ),
    (
        &[A, "POLYGON ((2 2, 4 2, 4 4, 2 4, 2 2))"],
        0.0,
        true,
        &[EMPTY, EMPTY],
    ),
    (
        &[
            "POLYGON ((0 0, 3 0, 3 3, 0 3, 0 0))",
            "POLYGON ((2 2, 5 2, 5 5, 2 5, 2 2))",
        ],
        0.0,
        false,
        &["LINESTRING (3 0, 3 3, 0 3)", "LINESTRING (2 5, 2 2, 5 2)"],
    ),
    (
        &[A, A],
        0.0,
        false,
        &[
            "LINESTRING (0 0, 2 0, 2 2, 0 2, 0 0)",
            "LINESTRING (0 0, 2 0, 2 2, 0 2, 0 0)",
        ],
    ),
    (
        &[
            "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((5 5, 6 5, 6 6, 5 5)))",
            "POLYGON ((1 0, 3 0, 3 2, 1 2, 1 0))",
            "POLYGON ((6 5, 7 5, 7 6, 6 5))",
        ],
        0.0,
        false,
        &[
            "LINESTRING (0 0, 2 0, 2 2, 0 2)",
            "LINESTRING (3 2, 1 2, 1 0, 3 0)",
            EMPTY,
        ],
    ),
    (
        &["POLYGON ((0 0, 0 2, 2 2, 2 0, 0 0))", B],
        0.0,
        true,
        &[EMPTY, EMPTY],
    ),
    (
        &[A, "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 1, 2 0))"],
        0.0,
        false,
        &["LINESTRING (2 0, 2 2)", "LINESTRING (2 2, 2 1, 2 0)"],
    ),
];

#[test]
fn validation_matches_geos() {
    for (rows, gap, valid, edges) in VALIDATION {
        let column = some(rows);
        assert_eq!(
            column.is_valid_coverage(*gap).unwrap(),
            *valid,
            "is_valid_coverage({rows:?}, {gap})"
        );
        let got = text(&column.invalid_coverage_edges(*gap).unwrap());
        assert_eq!(
            got,
            canon_all(edges),
            "invalid_coverage_edges({rows:?}, {gap})"
        );
    }
}

#[test]
fn validation_ignores_nulls_and_non_polygons() {
    let column = col(&[
        Some(A),
        None,
        Some("POINT (1 1)"),
        Some("POLYGON EMPTY"),
        Some(B),
    ]);
    assert!(column.is_valid_coverage(0.0).unwrap());
    assert_eq!(
        text(&column.invalid_coverage_edges(0.0).unwrap()),
        vec![
            Some(canon(EMPTY)),
            None,
            Some(canon(EMPTY)),
            Some(canon(EMPTY)),
            Some(canon(EMPTY)),
        ]
    );
    assert!(some(&[]).is_valid_coverage(0.0).unwrap());
    assert!(col(&[None, None]).is_valid_coverage(0.0).unwrap());
}

#[test]
fn validation_gap_width_edge_cases() {
    let column = some(&[A, B]);
    // GEOS treats a negative width as no width.
    assert!(column.is_valid_coverage(-1.0).unwrap());
    assert!(column.is_valid_coverage(f64::NAN).is_err());
    assert!(column.is_valid_coverage(f64::INFINITY).is_err());
    assert!(column.invalid_coverage_edges(f64::NAN).is_err());
}

#[test]
fn validation_keeps_crs() {
    let column = GeoColumn::from_wkt(&[Some(A), Some(B)], Some("EPSG:4326")).unwrap();
    let out = column.invalid_coverage_edges(0.0).unwrap();
    assert_eq!(out.crs().as_deref(), Some("EPSG:4326"));
}

// -- fuzzing against GeoPandas --------------------------------------------------------------

/// Compares random coverages with GeoPandas: `COVERAGE_FUZZ=<json from
/// test/reference/fuzz_coverage.py validate> [GAP=<only this gap width>] cargo test -p
/// geoframe_core fuzz_validation -- --ignored --nocapture`.
#[test]
#[ignore = "needs a case file from test/reference/fuzz_coverage.py"]
fn fuzz_validation() {
    let path = std::env::var("COVERAGE_FUZZ").expect("COVERAGE_FUZZ");
    let cases: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut bad = 0;
    for c in cases.as_array().unwrap() {
        let rows: Vec<&str> = c["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let gap = c["gap"].as_f64().unwrap();
        if let Ok(only) = std::env::var("GAP") {
            if only.parse::<f64>().unwrap() != gap {
                continue;
            }
        }
        let column = some(&rows);
        let want: Vec<Option<String>> = c["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| Some(canon(v.as_str().unwrap())))
            .collect();
        let got = text(&column.invalid_coverage_edges(gap).unwrap());
        let valid = column.is_valid_coverage(gap).unwrap();
        if got != want || valid != c["valid"].as_bool().unwrap() {
            bad += 1;
            if bad <= 4 {
                println!("MISMATCH gap={gap} rows={rows:?}\n  got  {got:?}\n  want {want:?}");
            }
        }
    }
    println!("{} mismatches of {}", bad, cases.as_array().unwrap().len());
}

// -- hardening ----------------------------------------------------------------------------

#[test]
fn validation_work_is_capped() {
    let column = some(&[A, "POLYGON ((1 0, 3 0, 3 2, 1 2, 1 0))"]);
    assert!(column.is_valid_coverage_capped(0.0, 2).is_err());
    assert!(!column.is_valid_coverage_capped(0.0, 1_000).unwrap());
}

#[test]
fn validation_survives_extreme_and_non_finite_input() {
    let huge = "POLYGON ((0 0, 1e308 0, 1e308 1e308, 0 1e308, 0 0))";
    let tiny = "POLYGON ((0 0, 1e-320 0, 1e-320 1e-320, 0 1e-320, 0 0))";
    let column = some(&[huge, tiny, A, B]);
    for gap in [0.0, 1e308, 1e-320] {
        let _ = column.is_valid_coverage(gap).unwrap();
        let out = column.invalid_coverage_edges(gap).unwrap();
        assert_eq!(out.len(), 4);
    }
    let nan = col(&[Some(A), Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"), Some(B)]);
    assert!(nan.is_valid_coverage(f64::MAX).is_ok());
}

/// A little-endian WKB polygon with one ring (which may hold NaN or infinity).
fn wkb_polygon(ring: &[(f64, f64)]) -> Vec<u8> {
    let mut out = vec![1u8];
    out.extend(3u32.to_le_bytes());
    out.extend(1u32.to_le_bytes());
    out.extend((ring.len() as u32 + 1).to_le_bytes());
    for &(x, y) in ring.iter().chain(ring.first()) {
        out.extend(x.to_le_bytes());
        out.extend(y.to_le_bytes());
    }
    out
}

#[test]
fn validation_makes_non_finite_rows_null() {
    let rows = [
        wkb_polygon(&[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]),
        wkb_polygon(&[(2.0, 0.0), (4.0, 0.0), (4.0, f64::NAN), (2.0, 2.0)]),
        wkb_polygon(&[(2.0, 0.0), (4.0, 0.0), (4.0, 2.0), (2.0, 2.0)]),
        wkb_polygon(&[(f64::INFINITY, 0.0), (4.0, 0.0), (4.0, 2.0)]),
    ];
    let refs: Vec<Option<&[u8]>> = rows.iter().map(|r| Some(r.as_slice())).collect();
    let column = GeoColumn::from_wkb(&refs, None).unwrap();
    assert!(column.is_valid_coverage(0.0).unwrap());
    let out = text(&column.invalid_coverage_edges(0.0).unwrap());
    assert_eq!(out[0], Some(canon(EMPTY)));
    assert_eq!(out[1], None);
    assert_eq!(out[2], Some(canon(EMPTY)));
    assert_eq!(out[3], None);
}
