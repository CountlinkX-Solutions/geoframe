//! `polygonize` and `build_area`: expected answers come from GeoPandas 1.2 / Shapely 2.2
//! (GEOS 3.14), `GeoSeries.polygonize(full=True)` and `GeoSeries.build_area()`.

use crate::{GeoColumn, SetOp};

fn series(rows: &[&str]) -> GeoColumn {
    let rows: Vec<Option<&str>> = rows.iter().map(|r| Some(*r)).collect();
    GeoColumn::from_wkt(&rows, None).expect("valid WKT")
}

/// Rows as canonical (GEOS `normalize`) text, in a fixed order.
fn canonical(column: &GeoColumn) -> Vec<String> {
    let mut rows: Vec<String> = column
        .normalize()
        .unwrap()
        .to_wkt()
        .unwrap()
        .into_iter()
        .map(|r| r.expect("no null rows"))
        .collect();
    rows.sort();
    rows
}

fn expected(rows: &[&str]) -> Vec<String> {
    if rows.is_empty() {
        return vec![];
    }
    canonical(&series(rows))
}

struct Case {
    name: &'static str,
    input: &'static [&'static str],
    node: bool,
    polygons: &'static [&'static str],
    cuts: &'static [&'static str],
    dangles: &'static [&'static str],
    invalid: &'static [&'static str],
}

const SQUARE: &str = "LINESTRING (0 0, 4 0, 4 4, 0 4, 0 0)";

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one ring",
            input: &[SQUARE],
            node: true,
            polygons: &["POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))"],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "diagonals cross a square",
            input: &["LINESTRING (0 0, 4 4)", "LINESTRING (0 4, 4 0)", SQUARE],
            node: true,
            polygons: &[
                "POLYGON ((0 0, 2 2, 4 0, 0 0))",
                "POLYGON ((2 2, 0 0, 0 4, 2 2))",
                "POLYGON ((2 2, 4 4, 4 0, 2 2))",
                "POLYGON ((4 4, 2 2, 0 4, 4 4))",
            ],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "a dangle and a cut edge",
            input: &[
                SQUARE,
                "LINESTRING (4 2, 6 2)",
                "LINESTRING (0 2, 2 2)",
                "LINESTRING (6 2, 8 2, 8 4, 6 4, 6 2)",
            ],
            node: true,
            polygons: &[
                "POLYGON ((4 2, 4 0, 0 0, 0 2, 0 4, 4 4, 4 2))",
                "POLYGON ((6 2, 6 4, 8 4, 8 2, 6 2))",
            ],
            cuts: &["LINESTRING (4 2, 6 2)"],
            dangles: &["LINESTRING (0 2, 2 2)"],
            invalid: &[],
        },
        Case {
            name: "nested rings give a donut and the island",
            input: &[
                "LINESTRING (0 0, 10 0, 10 10, 0 10, 0 0)",
                "LINESTRING (2 2, 4 2, 4 4, 2 4, 2 2)",
            ],
            node: true,
            polygons: &[
                "POLYGON ((0 0, 0 10, 10 10, 10 0, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))",
                "POLYGON ((2 2, 2 4, 4 4, 4 2, 2 2))",
            ],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "a self-crossing ring that is not noded is invalid",
            input: &["LINESTRING (0 0, 4 4, 4 0, 0 4, 0 0)"],
            node: false,
            polygons: &[],
            cuts: &[],
            dangles: &[],
            invalid: &["LINESTRING (0 0, 4 4, 4 0, 0 4, 0 0)"],
        },
        Case {
            name: "the same ring noded gives two triangles",
            input: &["LINESTRING (0 0, 4 4, 4 0, 0 4, 0 0)"],
            node: true,
            polygons: &[
                "POLYGON ((2 2, 0 0, 0 4, 2 2))",
                "POLYGON ((2 2, 4 4, 4 0, 2 2))",
            ],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "crossing lines that are not noded are dangles",
            input: &["LINESTRING (0 0, 4 4)", "LINESTRING (0 4, 4 0)", SQUARE],
            node: false,
            polygons: &["POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))"],
            cuts: &[],
            dangles: &["LINESTRING (0 0, 4 4)", "LINESTRING (0 4, 4 0)"],
            invalid: &[],
        },
        Case {
            name: "polygons are merged before they are noded",
            input: &[
                "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))",
                "POLYGON ((10 0, 20 0, 20 10, 10 10, 10 0))",
            ],
            node: true,
            // GEOS keeps (10 10) and (10 0), where the two polygons met, as vertices of the
            // merged ring.
            polygons: &[
                "POLYGON ((0 0, 0 10, 10 10, 20 10, 20 0, 10 0, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))",
                "POLYGON ((2 2, 2 4, 4 4, 4 2, 2 2))",
            ],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        // The cases below come from GeoPandas 1.2 / Shapely 2.2 (GEOS 3.14):
        //   GeoSeries([wkt.loads(w) for w in input]).polygonize(node=True)  (then `normalize`)
        // The merged ring keeps every input vertex that lies on it, collinear or not, and the
        // points where input edges cross or touch; vertices inside the merged area are gone.
        Case {
            name: "two squares sharing an edge keep the ends of the shared edge",
            input: &[
                "POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))",
                "POLYGON ((1 0, 2 0, 2 1, 1 1, 1 0))",
            ],
            node: true,
            polygons: &["POLYGON ((0 0, 0 1, 1 1, 2 1, 2 0, 1 0, 0 0))"],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "three squares meeting at a T keep the vertices on the outline only",
            input: &[
                "POLYGON ((0 0, 2 0, 2 1, 0 1, 0 0))",
                "POLYGON ((0 1, 1 1, 1 2, 0 2, 0 1))",
                "POLYGON ((1 1, 2 1, 2 2, 1 2, 1 1))",
            ],
            node: true,
            polygons: &["POLYGON ((0 0, 0 1, 0 2, 1 2, 2 2, 2 1, 2 0, 0 0))"],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "a polygon meeting the middle of an edge keeps its own corner there",
            input: &[
                "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))",
                "POLYGON ((2 1, 3 1, 3 3, 2 3, 2 1))",
            ],
            node: true,
            polygons: &["POLYGON ((0 0, 0 2, 2 2, 2 3, 3 3, 3 1, 2 1, 2 0, 0 0))"],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "a line that starts on the outline and runs inside leaves its vertex there",
            input: &[
                "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))",
                "LINESTRING (0 2, 2 2)",
            ],
            node: true,
            polygons: &["POLYGON ((0 0, 0 2, 0 4, 4 4, 4 0, 0 0))"],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "lines that touch the outline from outside split it where they touch",
            input: &[
                "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))",
                "LINESTRING (4 2, 6 2, 6 5)",
                "LINESTRING (2 4, 2 6)",
            ],
            node: true,
            polygons: &["POLYGON ((0 0, 0 4, 2 4, 4 4, 4 2, 4 0, 0 0))"],
            cuts: &[],
            dangles: &["LINESTRING (4 2, 6 2, 6 5)", "LINESTRING (2 4, 2 6)"],
            invalid: &[],
        },
        Case {
            name: "a collinear vertex of a single polygon stays",
            input: &["POLYGON ((0 0, 1 0, 2 0, 2 2, 0 2, 0 0))"],
            node: true,
            polygons: &["POLYGON ((0 0, 0 2, 2 2, 2 0, 1 0, 0 0))"],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "equal edges that are not noded form a ring of no area",
            input: &[SQUARE, "LINESTRING (0 0, 4 0)", "LINESTRING (4 0, 0 0)"],
            node: false,
            polygons: &["POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))"],
            cuts: &[],
            dangles: &[],
            invalid: &["LINESTRING (0 0, 4 0, 0 0)"],
        },
        Case {
            name: "a point has no linework",
            input: &["POINT (1 1)"],
            node: true,
            polygons: &[],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "squares touching at a corner",
            input: &[SQUARE, "LINESTRING (4 4, 8 4, 8 8, 4 8, 4 4)"],
            node: true,
            polygons: &[
                "POLYGON ((4 4, 4 0, 0 0, 0 4, 4 4))",
                "POLYGON ((4 4, 4 8, 8 8, 8 4, 4 4))",
            ],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "duplicate segments are ignored",
            input: &[SQUARE, "LINESTRING (0 0, 4 0)", "LINESTRING (4 0, 0 0)"],
            node: true,
            polygons: &["POLYGON ((4 0, 0 0, 0 4, 4 4, 4 0))"],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "a ring that touches the shell is a hole and an island",
            input: &[
                "LINESTRING (0 0, 10 0, 10 10, 0 10, 0 0)",
                "LINESTRING (0 5, 5 2, 5 8, 0 5)",
            ],
            node: true,
            polygons: &[
                "POLYGON ((0 5, 0 10, 10 10, 10 0, 0 0, 0 5), (0 5, 5 2, 5 8, 0 5))",
                "POLYGON ((0 5, 5 8, 5 2, 0 5))",
            ],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
        Case {
            name: "a ring that touches the shell from inside at the bottom",
            input: &[
                "LINESTRING (0 0, 10 0, 10 10, 0 10, 0 0)",
                "LINESTRING (5 0, 2 4, 8 4, 5 0)",
            ],
            node: true,
            polygons: &[
                "POLYGON ((5 0, 0 0, 0 10, 10 10, 10 0, 5 0), (5 0, 8 4, 2 4, 5 0))",
                "POLYGON ((5 0, 2 4, 8 4, 5 0))",
            ],
            cuts: &[],
            dangles: &[],
            invalid: &[],
        },
    ]
}

#[test]
fn polygonize_matches_geos() {
    let mut failures = Vec::new();
    for case in cases() {
        let out = series(case.input).polygonize(case.node).unwrap();
        for (what, got, want) in [
            ("polygons", &out.polygons, case.polygons),
            ("cuts", &out.cuts, case.cuts),
            ("dangles", &out.dangles, case.dangles),
            ("invalid", &out.invalid, case.invalid),
        ] {
            if canonical(got) != expected(want) {
                failures.push(format!(
                    "{} / {what}:\n got  {:?}\n want {:?}",
                    case.name,
                    canonical(got),
                    expected(want)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The union of all rows of `column`, as a one-row column.
fn area_of(column: &GeoColumn) -> f64 {
    column.area().into_iter().flatten().sum()
}

fn xor_area(a: &GeoColumn, b: &GeoColumn) -> f64 {
    let ua = a.union_all().unwrap();
    let ub = b.union_all().unwrap();
    area_of(&ua.set_op(&ub, SetOp::SymmetricDifference).unwrap())
}

#[test]
fn build_area_matches_geos() {
    let cases: &[(&[&str], bool, &[&str])] = &[
        (
            &[
                "LINESTRING (0 0, 10 0, 10 10, 0 10, 0 0)",
                "LINESTRING (2 2, 4 2, 4 4, 2 4, 2 2)",
            ],
            true,
            &["POLYGON ((0 0, 0 10, 10 10, 10 0, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))"],
        ),
        (
            &[
                "LINESTRING (0 0, 10 0, 10 10, 0 10, 0 0)",
                "LINESTRING (2 2, 4 2, 4 4, 2 4, 2 2)",
                "LINESTRING (3 3, 3.5 3, 3.5 3.5, 3 3.5, 3 3)",
            ],
            true,
            &[
                "POLYGON ((3 3.5, 3.5 3.5, 3.5 3, 3 3, 3 3.5))",
                "POLYGON ((0 10, 10 10, 10 0, 0 0, 0 10), (4 2, 4 4, 2 4, 2 2, 4 2))",
            ],
        ),
        (
            &["LINESTRING (0 0, 4 4)", "LINESTRING (0 4, 4 0)", SQUARE],
            true,
            &["POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))"],
        ),
        (
            &["LINESTRING (0 0, 4 4)", "LINESTRING (0 4, 4 0)", SQUARE],
            false,
            &["POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))"],
        ),
        (
            &[
                "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))",
                "POLYGON ((10 0, 20 0, 20 10, 10 10, 10 0))",
            ],
            true,
            &["POLYGON ((0 0, 0 10, 10 10, 20 10, 20 0, 10 0, 0 0), (4 2, 4 4, 2 4, 2 2, 4 2))"],
        ),
        (&["POINT (0 0)"], true, &[]),
        (
            &[SQUARE, "LINESTRING (4 0, 8 0, 8 4, 4 4)"],
            true,
            &["POLYGON ((0 0, 0 4, 4 4, 8 4, 8 0, 4 0, 0 0))"],
        ),
    ];
    for (input, node, want) in cases {
        let got = series(input).build_area(*node).unwrap();
        assert_eq!(got.len(), want.len(), "{input:?}: number of polygons");
        if want.is_empty() {
            continue;
        }
        let want = series(want);
        assert!(
            xor_area(&got, &want) < 1e-9,
            "{input:?}: {:?} != {:?}",
            got.to_wkt().unwrap(),
            want.to_wkt().unwrap()
        );
    }
}

#[test]
fn polygonize_skips_null_rows_and_keeps_the_crs() {
    let rows = [Some(SQUARE), None];
    let c = GeoColumn::from_wkt(&rows, Some("EPSG:4326")).unwrap();
    let out = c.polygonize(true).unwrap();
    assert_eq!(out.polygons.len(), 1);
    assert_eq!(out.polygons.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(out.cuts.crs().as_deref(), Some("EPSG:4326"));
    let area = c.build_area(true).unwrap();
    assert_eq!(area.crs().as_deref(), Some("EPSG:4326"));
}

#[test]
fn polygonize_of_an_empty_series_is_empty() {
    let c = GeoColumn::from_wkt::<&str>(&[], None).unwrap();
    let out = c.polygonize(true).unwrap();
    assert_eq!(out.polygons.len(), 0);
    assert_eq!(c.build_area(true).unwrap().len(), 0);
}

#[test]
fn polygonize_a_grid_of_lines_makes_a_cell_per_square() {
    // 30 x 30 lines crossing: 29 x 29 cells; a quadratic implementation would crawl.
    let mut rows = Vec::new();
    for i in 0..30 {
        rows.push(format!("LINESTRING ({i} 0, {i} 29)"));
        rows.push(format!("LINESTRING (0 {i}, 29 {i})"));
    }
    let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let out = series(&refs).polygonize(true).unwrap();
    assert_eq!(out.polygons.len(), 29 * 29);
    assert!(out.dangles.is_empty());
}

#[test]
fn polygonize_a_sparse_grid_of_90_000_cells_is_fast() {
    // 600 long lines: 360,000 pairs would be a lot if every pair were examined; the spatial
    // index only finds the crossing ones.
    let mut rows = Vec::new();
    for i in 0..300 {
        rows.push(format!("LINESTRING ({i} 0, {i} 300)"));
        rows.push(format!("LINESTRING (0 {i}, 300 {i})"));
    }
    let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let out = series(&refs).polygonize(true).unwrap();
    assert_eq!(out.polygons.len(), 299 * 299);
}
