//! `buffer(0)` of an invalid polygon and the repair of a Douglas-Peucker result as GEOS does it.
//! Expected answers come from Shapely 2.2 / GEOS 3.14 (`shapely.buffer(g, 0)`,
//! `shapely.simplify(g, t, preserve_topology=False)`), compared after `normalize`.

use crate::buffer_zero::buffer_zero;
use crate::GeoColumn;
use geo::Geometry;
use wkt::ToWkt;

const BUFFER_ZERO: &[(&str, &str)] = &[
    // bowtie
    ("POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))",
     "POLYGON ((2 2, 4 4, 4 0, 2 2))"),
    // lobes_unequal
    ("POLYGON ((0 0, 6 6, 6 0, 3 3, 0 3, 0 0))",
     "POLYGON ((3 3, 6 6, 6 0, 3 3))"),
    // lobes_unequal2
    ("POLYGON ((0 0, 10 10, 10 0, 0 10, 5 12, 0 0))",
     "MULTIPOLYGON (((5 5, 10 10, 10 0, 5 5)), ((0 10, 5 12, 2.9411764705882355 7.0588235294117645, 0 10)))"),
    // figure8_hole
    ("POLYGON ((0 0, 20 20, 20 0, 0 20, 0 0), (14 8, 18 10, 14 12, 14 8))",
     "POLYGON ((10 10, 20 20, 20 0, 10 10), (14 8, 18 10, 14 12, 14 8))"),
    // overlap_parts
    ("MULTIPOLYGON (((0 0, 6 0, 6 6, 0 6, 0 0)), ((3 3, 9 3, 9 9, 3 9, 3 3)))",
     "POLYGON ((0 0, 0 6, 3 6, 3 9, 9 9, 9 3, 6 3, 6 0, 0 0))"),
    // touching_parts
    ("MULTIPOLYGON (((0 0, 4 0, 4 4, 0 4, 0 0)), ((4 0, 8 0, 8 4, 4 4, 4 0)))",
     "POLYGON ((0 0, 0 4, 4 4, 8 4, 8 0, 4 0, 0 0))"),
    // s_bowtie_ring
    ("POLYGON ((0 0, 10 0, 0 10, 10 10, 0 0))",
     "POLYGON ((0 10, 10 10, 5 5, 0 10))"),
    // s_cross
    ("POLYGON ((0 0, 10 0, 10 10, 5 -3, 0 10, 0 0))",
     "MULTIPOLYGON (((6.153846153846154 0, 10 10, 10 0, 6.153846153846154 0)), ((0 0, 0 10, 3.8461538461538463 0, 0 0)))"),
    // nested_parts
    ("MULTIPOLYGON (((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 8 2, 8 8, 2 8, 2 2)), ((4 4, 6 4, 6 6, 4 6, 4 4)))",
     "MULTIPOLYGON (((4 4, 4 6, 6 6, 6 4, 4 4)), ((0 0, 0 10, 10 10, 10 0, 0 0), (2 2, 8 2, 8 8, 2 8, 2 2)))"),
    // hole_overlaps_part
    ("MULTIPOLYGON (((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 8 2, 8 8, 2 8, 2 2)), ((5 5, 12 5, 12 12, 5 12, 5 5)))",
     "POLYGON ((0 0, 0 10, 5 10, 5 12, 12 12, 12 5, 10 5, 10 0, 0 0), (2 2, 8 2, 8 5, 5 5, 5 8, 2 8, 2 2))"),
    // ccw_bowtie_unequal
    ("POLYGON ((0 0, 4 0, 0 4, 6 6, 0 0))",
     "POLYGON ((0 4, 6 6, 2 2, 0 4))"),
];

const SIMPLIFY: &[(&str, f64, &str)] = &[
    // s_bowtie_ring
    ("POLYGON ((0 0, 10 0, 0 10, 10 10, 0 0))", 0.0,
     "POLYGON ((0 10, 10 10, 5 5, 0 10))"),
    // s_bowtie_ring
    ("POLYGON ((0 0, 10 0, 0 10, 10 10, 0 0))", 0.05,
     "POLYGON ((0 10, 10 10, 5 5, 0 10))"),
    // s_bowtie_ring
    ("POLYGON ((0 0, 10 0, 0 10, 10 10, 0 0))", 1.0,
     "POLYGON ((0 10, 10 10, 5 5, 0 10))"),
    // s_cross
    ("POLYGON ((0 0, 10 0, 10 10, 5 -3, 0 10, 0 0))", 0.0,
     "MULTIPOLYGON (((6.153846153846154 0, 10 10, 10 0, 6.153846153846154 0)), ((0 0, 0 10, 3.8461538461538463 0, 0 0)))"),
    // s_cross
    ("POLYGON ((0 0, 10 0, 10 10, 5 -3, 0 10, 0 0))", 0.05,
     "MULTIPOLYGON (((6.153846153846154 0, 10 10, 10 0, 6.153846153846154 0)), ((0 0, 0 10, 3.8461538461538463 0, 0 0)))"),
    // s_cross
    ("POLYGON ((0 0, 10 0, 10 10, 5 -3, 0 10, 0 0))", 1.0,
     "MULTIPOLYGON (((6.153846153846154 0, 10 10, 10 0, 6.153846153846154 0)), ((0 0, 0 10, 3.8461538461538463 0, 0 0)))"),
    // s_touching
    ("MULTIPOLYGON (((0 0, 4 0, 4 4, 0 4, 0 0)), ((4 0, 8 0, 8 4, 4 4, 4 0)))", 0.0,
     "POLYGON ((0 0, 0 4, 4 4, 8 4, 8 0, 4 0, 0 0))"),
    // s_touching
    ("MULTIPOLYGON (((0 0, 4 0, 4 4, 0 4, 0 0)), ((4 0, 8 0, 8 4, 4 4, 4 0)))", 0.05,
     "POLYGON ((0 0, 0 4, 4 4, 8 4, 8 0, 4 0, 0 0))"),
    // s_touching
    ("MULTIPOLYGON (((0 0, 4 0, 4 4, 0 4, 0 0)), ((4 0, 8 0, 8 4, 4 4, 4 0)))", 1.0,
     "POLYGON ((0 0, 0 4, 4 4, 8 4, 8 0, 4 0, 0 0))"),
];

fn normalized(wkt: &str) -> String {
    let column = GeoColumn::from_wkt(&[Some(wkt)], None).expect("wkt");
    column.normalize().unwrap().to_wkt().unwrap()[0]
        .clone()
        .unwrap()
}

#[test]
fn buffer_zero_matches_geos() {
    for (input, expected) in BUFFER_ZERO {
        let geom: Geometry<f64> = wkt::TryFromWkt::try_from_wkt_str(input).expect("wkt");
        let got = buffer_zero(&geom).unwrap().wkt_string();
        assert_eq!(
            normalized(&got),
            normalized(expected),
            "buffer(0) of {input}"
        );
    }
}

#[test]
fn simplify_repairs_invalid_results_as_geos() {
    for (input, tolerance, expected) in SIMPLIFY {
        let column = GeoColumn::from_wkt(&[Some(*input)], None).expect("wkt");
        let out = column.simplify(*tolerance, false).unwrap();
        let got = out.normalize().unwrap().to_wkt().unwrap()[0]
            .clone()
            .unwrap();
        assert_eq!(
            got,
            normalized(expected),
            "simplify({tolerance}) of {input}"
        );
    }
}

#[test]
fn simplify_leaves_a_valid_result_alone() {
    let input = "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))";
    let column = GeoColumn::from_wkt(&[Some(input)], None).unwrap();
    let out = column
        .simplify(1.0, false)
        .unwrap()
        .normalize()
        .unwrap()
        .to_wkt()
        .unwrap();
    assert_eq!(out[0].clone().unwrap(), normalized(input));
}

/// Fuzz harness, run by hand against Shapely (see `test/reference/fuzz_simplify_repair.py`):
/// reads `GF_FUZZ_IN` (one `wkt<TAB>tolerance` per line) and writes `GF_FUZZ_OUT` (one
/// `repaired<TAB>wkt` per line, `repaired` being 1 when `buffer(0)` was needed).
/// `cargo test -p geoframe_core buffer_zero_tests::fuzz_simplify_repair -- --ignored --exact`
#[test]
#[ignore = "run by hand against Shapely"]
fn fuzz_simplify_repair() {
    use std::sync::atomic::Ordering;
    let (Ok(input), Ok(output)) = (std::env::var("GF_FUZZ_IN"), std::env::var("GF_FUZZ_OUT"))
    else {
        return;
    };
    let mut out = String::new();
    for line in std::fs::read_to_string(input).unwrap().lines() {
        let (wkt, tolerance) = line.split_once('\t').unwrap();
        let column = GeoColumn::from_wkt(&[Some(wkt)], None).unwrap();
        let before = crate::buffer_zero::CALLS.load(Ordering::SeqCst);
        let got = column
            .simplify(tolerance.parse().unwrap(), false)
            .unwrap()
            .to_wkt()
            .unwrap()[0]
            .clone()
            .unwrap_or_default();
        let repaired = crate::buffer_zero::CALLS.load(Ordering::SeqCst) > before;
        out.push_str(&format!("{}\t{got}\n", u8::from(repaired)));
    }
    std::fs::write(output, out).unwrap();
}
