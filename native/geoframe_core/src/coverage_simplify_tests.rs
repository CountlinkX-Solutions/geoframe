//! Batch I3 (part 2): `simplify_coverage`. Expected answers come from Shapely 2.2 / GEOS 3.14
//! (`coverage_simplify`).

use crate::coverage_tests::{canon, canon_all, col, some, text};
use crate::GeoColumn;

const A: &str = "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))";
const B: &str = "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))";

// -- simplification -----------------------------------------------------------------------

const DOC: &[&str] = &[
    "POLYGON ((0 0, 1 1.1, 2 0, 1.5 1, 2 2, 0 2, 0 0))",
    "POLYGON ((2 0, 4 0, 4 2, 2 2, 1.5 1, 2 0))",
];

/// (rows, tolerance, simplify_boundary, expected rows)
type Simplify = (&'static [&'static str], f64, bool, &'static [&'static str]);

const SIMPLIFY: &[Simplify] = &[
    (
        DOC,
        1.0,
        true,
        &[
            "POLYGON ((2 0, 2 2, 0 2, 2 0))",
            "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))",
        ],
    ),
    (
        DOC,
        1.0,
        false,
        &[
            "POLYGON ((2 0, 2 2, 0 2, 0 0, 1 1.1, 2 0))",
            "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))",
        ],
    ),
    (
        DOC,
        0.2,
        true,
        &[
            "POLYGON ((2 0, 1.5 1, 2 2, 0 2, 0 0, 1 1.1, 2 0))",
            "POLYGON ((2 0, 4 0, 4 2, 2 2, 1.5 1, 2 0))",
        ],
    ),
    (
        DOC,
        -1.0,
        true,
        &[
            "POLYGON ((2 0, 2 2, 0 2, 2 0))",
            "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))",
        ],
    ),
    (
        &[A, B],
        100.0,
        true,
        &[
            "POLYGON ((2 0, 2 2, 0 0, 2 0))",
            "POLYGON ((2 0, 4 2, 2 2, 2 0))",
        ],
    ),
    (
        &[A, B],
        100.0,
        false,
        &[
            "POLYGON ((2 0, 2 2, 0 2, 0 0, 2 0))",
            "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))",
        ],
    ),
    // a collinear vertex goes at tolerance 0, unless an edge of another polygon runs between
    // the same two vertices
    (
        &[
            "POLYGON ((0 0, 1 0, 2 0, 2 2, 0 2, 0 0))",
            "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 1, 2 0))",
        ],
        0.0,
        true,
        &[
            "POLYGON ((2 0, 2 2, 0 2, 0 0, 2 0))",
            "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 1, 2 0))",
        ],
    ),
    // a ring with no node: the vertex the ring starts at can go too
    (
        &["POLYGON ((0 0, 5 0, 5 1, 4 1.2, 3 1, 2 1.2, 1 1, 0 1, 0 0))"],
        1.0,
        true,
        &["POLYGON ((0 0, 5 0, 5 1, 0 1, 0 0))"],
    ),
    (
        &["POLYGON ((0 0, 5 0, 5 1, 4 1.2, 3 1, 2 1.2, 1 1, 0 1, 0 0))"],
        5.0,
        true,
        &["POLYGON ((5 0, 5 1, 0 1, 5 0))"],
    ),
    (
        &["POLYGON ((0 0, 5 0, 5 1, 4 1.2, 3 1, 2 1.2, 1 1, 0 1, 0 0))"],
        1.0,
        false,
        &["POLYGON ((0 0, 5 0, 5 1, 4 1.2, 3 1, 2 1.2, 1 1, 0 1, 0 0))"],
    ),
    // a hole filled by another polygon: one shared ring
    (
        &[
            "POLYGON ((0 0, 6 0, 6 6, 0 6, 0 0), (2 2, 2 4, 3 4.3, 4 4, 4 2, 3 1.7, 2 2))",
            "POLYGON ((2 2, 3 1.7, 4 2, 4 4, 3 4.3, 2 4, 2 2))",
        ],
        2.0,
        true,
        &[
            "POLYGON ((0 0, 6 0, 6 6, 0 6, 0 0), (2 4, 4 4, 4 2, 2 4))",
            "POLYGON ((2 4, 4 4, 4 2, 2 4))",
        ],
    ),
    // three polygons meet at a node that stays
    (
        &[
            "POLYGON ((0 0, 2 0, 2 1, 1 1.2, 0 1, 0 0))",
            "POLYGON ((2 0, 4 0, 4 1, 3 1.2, 2 1, 2 0))",
            "POLYGON ((0 1, 1 1.2, 2 1, 3 1.2, 4 1, 4 3, 0 3, 0 1))",
        ],
        0.5,
        true,
        &[
            "POLYGON ((2 0, 2 1, 0 1, 0 0, 2 0))",
            "POLYGON ((2 0, 4 0, 4 1, 2 1, 2 0))",
            "POLYGON ((0 1, 2 1, 4 1, 4 3, 0 3, 0 1))",
        ],
    ),
    (
        &[
            "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((5 0, 7 0, 7 2, 5 2, 5 0)))",
            "POLYGON ((2 0, 5 0, 5 2, 4 2.3, 3 1.8, 2 2, 2 0))",
        ],
        0.5,
        true,
        &[
            "MULTIPOLYGON (((2 0, 2 2, 0 2, 0 0, 2 0)), ((5 0, 7 0, 7 2, 5 2, 5 0)))",
            "POLYGON ((2 0, 5 0, 5 2, 4 2.3, 3 1.8, 2 2, 2 0))",
        ],
    ),
];

#[test]
fn simplify_matches_geos() {
    for (rows, tol, boundary, expected) in SIMPLIFY {
        let got = some(rows).simplify_coverage(*tol, *boundary).unwrap();
        assert_eq!(
            text(&got),
            canon_all(expected),
            "simplify_coverage({rows:?}, {tol}, {boundary})"
        );
    }
}

#[test]
fn simplify_leaves_nulls_empties_and_non_polygons() {
    let column = col(&[
        Some(DOC[0]),
        None,
        Some("POINT (1 1)"),
        Some("POLYGON EMPTY"),
        Some(DOC[1]),
    ]);
    let got = text(&column.simplify_coverage(1.0, true).unwrap());
    assert_eq!(
        got[0].as_deref(),
        Some(canon("POLYGON ((2 0, 2 2, 0 2, 2 0))").as_str())
    );
    assert_eq!(got[1], None);
    assert_eq!(got[2], None);
    assert_eq!(got[3], Some(canon("POLYGON EMPTY")));
    assert_eq!(
        got[4].as_deref(),
        Some(canon("POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))").as_str())
    );
}

#[test]
fn simplify_rejects_non_finite_tolerance_and_keeps_crs() {
    let column = GeoColumn::from_wkt(&[Some(A), Some(B)], Some("EPSG:4326")).unwrap();
    assert!(column.simplify_coverage(f64::NAN, true).is_err());
    assert!(column.simplify_coverage(f64::INFINITY, true).is_err());
    let out = column.simplify_coverage(0.5, true).unwrap();
    assert_eq!(out.crs().as_deref(), Some("EPSG:4326"));
    assert!(some(&[]).simplify_coverage(1.0, true).unwrap().is_empty());
}

#[test]
fn simplified_coverage_stays_valid() {
    let column = some(&[
        "POLYGON ((0 0, 2 0, 2 1, 1 1.2, 0 1, 0 0))",
        "POLYGON ((2 0, 4 0, 4 1, 3 1.2, 2 1, 2 0))",
        "POLYGON ((0 1, 1 1.2, 2 1, 3 1.2, 4 1, 4 3, 0 3, 0 1))",
    ]);
    for tol in [0.1, 0.5, 1.0, 3.0] {
        let out = column.simplify_coverage(tol, true).unwrap();
        assert!(out.is_valid_coverage(0.0).unwrap(), "tolerance {tol}");
    }
}

// -- fuzzing against GeoPandas --------------------------------------------------------------

/// Compares random coverages with GeoPandas: `COVERAGE_FUZZ=<json from
/// test/reference/fuzz_coverage.py simplify> cargo test -p geoframe_core fuzz_simplification --
/// --ignored --nocapture`.
#[test]
#[ignore = "needs a case file from test/reference/fuzz_coverage.py"]
fn fuzz_simplification() {
    let path = std::env::var("COVERAGE_FUZZ").expect("COVERAGE_FUZZ");
    let cases: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let (mut bad, mut total) = (0, 0);
    for c in cases.as_array().unwrap() {
        let rows: Vec<&str> = c["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let tol = c["tol"].as_f64().unwrap();
        let boundary = c["boundary"].as_bool().unwrap();
        let want: Vec<Option<String>> = c["expected"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| Some(canon(v.as_str().unwrap())))
            .collect();
        let got = text(&some(&rows).simplify_coverage(tol, boundary).unwrap());
        total += 1;
        if got != want {
            bad += 1;
            if bad <= 6 {
                println!("MISMATCH tol={tol} boundary={boundary} rows={}", rows.len());
                for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                    if g != w {
                        println!(
                            "  row {i}\n    in   {}\n    got  {g:?}\n    want {w:?}",
                            rows[i]
                        );
                    }
                }
            }
        }
    }
    println!("{bad} mismatches of {total}");
}

// -- hardening ----------------------------------------------------------------------------

#[test]
fn a_long_shared_edge_simplifies_in_reasonable_time() {
    // two polygons sharing a wavy edge of 40,000 vertices
    let n = 40_000;
    let wave: Vec<String> = (0..=n)
        .map(|i| {
            let x = i as f64 * 0.001;
            format!("{x} {}", 1.0 + 0.0005 * ((i * 7919) % 13) as f64)
        })
        .collect();
    let top = wave.join(", ");
    let bottom = wave.iter().rev().cloned().collect::<Vec<_>>().join(", ");
    let last_x = n as f64 * 0.001;
    let a = format!("POLYGON (({top}, {last_x} 2, 0 2, {}))", wave[0]);
    let b = format!(
        "POLYGON (({bottom}, 0 0, {last_x} 0, {}))",
        wave[wave.len() - 1]
    );
    let start = std::time::Instant::now();
    let out = some(&[&a, &b]).simplify_coverage(0.02, true).unwrap();
    assert_eq!(out.len(), 2);
    assert!(start.elapsed().as_secs() < 60, "{:?}", start.elapsed());
}

#[test]
fn extreme_and_degenerate_input_does_not_panic() {
    let rows = [
        "POLYGON ((0 0, 1e308 0, 1e308 1e308, 0 1e308, 0 0))",
        "POLYGON ((1e308 0, 1.7e308 0, 1.7e308 1e308, 1e308 1e308, 1e308 0))",
        "POLYGON ((0 0, 1e-320 0, 1e-320 1e-320, 0 0))",
        "POLYGON ((0 0, 0 0, 0 0, 0 0))",
        "POLYGON ((0 0, 1 1, 2 2, 0 0))",
        "POLYGON ((0 0, 2 0, 1 0, 0 0))",
        "POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))",
        "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((0 0, 1 0, 1 1, 0 0)))",
    ];
    let column = some(&rows);
    for tol in [0.0, 1e-300, 1.0, 1e154, 1e308] {
        for boundary in [true, false] {
            let out = column.simplify_coverage(tol, boundary).unwrap();
            assert_eq!(out.len(), rows.len());
        }
    }
}

#[test]
fn work_is_capped() {
    let column = some(DOC);
    assert!(column
        .simplify_coverage_capped(1.0, true, 5, u64::MAX)
        .is_err());
    assert!(column
        .simplify_coverage_capped(1.0, true, 1_000, 2)
        .is_err());
    assert!(column
        .simplify_coverage_capped(1.0, true, 1_000, 1_000)
        .is_ok());
}

#[test]
fn a_collapsed_ring_is_left_as_it_is() {
    // GEOS drops the repeated vertex but does not simplify a ring of three coordinates.
    let out = some(&["POLYGON ((0.625 2.53, 0.625 2.53, 0.62 2.535, 0.625 2.53))"])
        .simplify_coverage(3.0, true)
        .unwrap();
    assert_eq!(
        text(&out),
        canon_all(&["POLYGON ((0.625 2.53, 0.62 2.535, 0.625 2.53))"])
    );
}
