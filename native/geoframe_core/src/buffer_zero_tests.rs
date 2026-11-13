//! `buffer(0)` of an invalid polygon and the repair of a Douglas-Peucker result as GEOS does it.
//! Expected answers come from Shapely 2.2 / GEOS 3.14 (`shapely.buffer(g, 0)`,
//! `shapely.simplify(g, t, preserve_topology=False)`), compared vertex for vertex: GEOS's ring
//! starts, ring directions and part order are reproduced, so nothing is normalized.

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
     "MULTIPOLYGON (((2.9411764705882355 7.0588235294117645, 0 10, 5 12, 2.9411764705882355 7.0588235294117645)), ((5 5, 10 10, 10 0, 5 5)))"),
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
     "POLYGON ((5 5, 0 10, 10 10, 5 5))"),
    // s_cross
    ("POLYGON ((0 0, 10 0, 10 10, 5 -3, 0 10, 0 0))",
     "MULTIPOLYGON (((0 0, 0 10, 3.8461538461538463 0, 0 0)), ((6.153846153846154 0, 10 10, 10 0, 6.153846153846154 0)))"),
    // nested_parts
    ("MULTIPOLYGON (((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 8 2, 8 8, 2 8, 2 2)), ((4 4, 6 4, 6 6, 4 6, 4 4)))",
     "MULTIPOLYGON (((0 0, 0 10, 10 10, 10 0, 0 0), (2 2, 8 2, 8 8, 2 8, 2 2)), ((4 4, 4 6, 6 6, 6 4, 4 4)))"),
    // hole_overlaps_part
    ("MULTIPOLYGON (((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 8 2, 8 8, 2 8, 2 2)), ((5 5, 12 5, 12 12, 5 12, 5 5)))",
     "POLYGON ((0 0, 0 10, 5 10, 5 12, 12 12, 12 5, 10 5, 10 0, 0 0), (5 8, 2 8, 2 2, 8 2, 8 5, 5 5, 5 8))"),
    // ccw_bowtie_unequal
    ("POLYGON ((0 0, 4 0, 0 4, 6 6, 0 0))",
     "POLYGON ((2 2, 0 4, 6 6, 2 2))"),
];

const SIMPLIFY: &[(&str, f64, &str)] = &[
    // s_bowtie_ring
    ("POLYGON ((0 0, 10 0, 0 10, 10 10, 0 0))", 0.0,
     "POLYGON ((5 5, 0 10, 10 10, 5 5))"),
    // s_bowtie_ring
    ("POLYGON ((0 0, 10 0, 0 10, 10 10, 0 0))", 0.05,
     "POLYGON ((5 5, 0 10, 10 10, 5 5))"),
    // s_bowtie_ring
    ("POLYGON ((0 0, 10 0, 0 10, 10 10, 0 0))", 1.0,
     "POLYGON ((5 5, 0 10, 10 10, 5 5))"),
    // s_cross
    ("POLYGON ((0 0, 10 0, 10 10, 5 -3, 0 10, 0 0))", 0.0,
     "MULTIPOLYGON (((0 0, 0 10, 3.8461538461538463 0, 0 0)), ((6.153846153846154 0, 10 10, 10 0, 6.153846153846154 0)))"),
    // s_cross
    ("POLYGON ((0 0, 10 0, 10 10, 5 -3, 0 10, 0 0))", 0.05,
     "MULTIPOLYGON (((0 0, 0 10, 3.8461538461538463 0, 0 0)), ((6.153846153846154 0, 10 10, 10 0, 6.153846153846154 0)))"),
    // s_cross
    ("POLYGON ((0 0, 10 0, 10 10, 5 -3, 0 10, 0 0))", 1.0,
     "MULTIPOLYGON (((0 0, 0 10, 3.8461538461538463 0, 0 0)), ((6.153846153846154 0, 10 10, 10 0, 6.153846153846154 0)))"),
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

/// The text of `wkt` as GeoFrame writes it (the reference values use Shapely's spacing).
fn canonical(wkt: &str) -> String {
    let column = GeoColumn::from_wkt(&[Some(wkt)], None).expect("wkt");
    column.to_wkt().unwrap()[0].clone().unwrap()
}

#[test]
fn buffer_zero_matches_geos() {
    for (input, expected) in BUFFER_ZERO {
        let geom: Geometry<f64> = wkt::TryFromWkt::try_from_wkt_str(input).expect("wkt");
        let got = buffer_zero(&geom).unwrap().wkt_string();
        assert_eq!(canonical(&got), canonical(expected), "buffer(0) of {input}");
    }
}

#[test]
fn simplify_repairs_invalid_results_as_geos() {
    for (input, tolerance, expected) in SIMPLIFY {
        let column = GeoColumn::from_wkt(&[Some(*input)], None).expect("wkt");
        let out = column.simplify(*tolerance, false).unwrap();
        let got = out.to_wkt().unwrap()[0].clone().unwrap();
        assert_eq!(got, canonical(expected), "simplify({tolerance}) of {input}");
    }
}

#[test]
fn simplify_leaves_a_valid_result_alone() {
    let input = "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))";
    let column = GeoColumn::from_wkt(&[Some(input)], None).unwrap();
    let out = column.simplify(1.0, false).unwrap().to_wkt().unwrap();
    // Valid rings keep Douglas-Peucker's start and direction (no repair, no reordering).
    assert_eq!(out[0].clone().unwrap(), canonical(input));
}

/// The repair of a polygon with Z. GEOS (`LineIntersector::computeIntersect`, `Interpolate`)
/// gives a new crossing node the mean of the Z interpolated along its two segments (one NaN
/// end: the other end's Z; a NaN segment: the other segment's Z), a vertex lying on another
/// segment its own Z (interpolated along that segment when NaN), and drops M. Inputs with a
/// NaN Z are ISO WKB in hex (`shapely.to_wkb(g, hex=True, flavor="iso")`); the expected text
/// is Shapely 2.2 / GEOS 3.14 (except where the last case says) written the way GeoFrame writes
/// WKT:
/// `shapely.to_wkt(shapely.simplify(g, t, preserve_topology=False), rounding_precision=-1,
/// output_dimension=4).replace(", ", ",").replace(" (", "(")`.
const SIMPLIFY_Z: &[(&str, f64, &str)] = &[
    // bowtie: (2 2) is halfway along 0 0 1 -> 4 4 2 (1.5) and 4 0 3 -> 0 4 4 (3.5)
    ("POLYGON Z ((0 0 1, 4 4 2, 4 0 3, 0 4 4, 0 0 1))", 0.0,
     "POLYGON Z((2 2 2.5,4 4 2,4 0 3,2 2 2.5))"),
    // lobes_unequal2: two new nodes, unequal fractions
    ("POLYGON Z ((0 0 0, 10 10 10, 10 0 0, 0 10 20, 5 12 0, 0 0 0))", 0.0,
     "MULTIPOLYGON Z(((2.9411764705882355 7.0588235294117645 7.0588235294117645,0 10 20,5 12 0,2.9411764705882355 7.0588235294117645 7.0588235294117645)),((5 5 7.5,10 10 10,10 0 0,5 5 7.5)))"),
    // s_cross: the nodes cut the edge 0 0 0 -> 10 0 0 (Z 0) and the dented edges
    ("POLYGON Z ((0 0 0, 10 0 0, 10 10 10, 5 -3 3, 0 10 1, 0 0 0))", 1.0,
     "MULTIPOLYGON Z(((0 0 0,0 10 1,3.8461538461538463 0 1.2692307692307692,0 0 0)),((6.153846153846154 0 2.307692307692308,10 10 10,10 0 0,6.153846153846154 0 2.307692307692308)))"),
    // s_bowtie_ring with unequal Z
    ("POLYGON Z ((0 0 0, 10 0 5, 0 10 7, 10 10 9, 0 0 0))", 0.0,
     "POLYGON Z((5 5 5.25,0 10 7,10 10 9,5 5 5.25))"),
    // overlap_parts: (4 2) crosses 4 0 4 -> 4 4 4 (4) and 6 2 20 -> 2 2 10 (15); (4 0) and
    // (2 0) are vertices on the other part's edge and keep their Z
    ("MULTIPOLYGON Z (((0 0 0, 4 0 4, 4 4 4, 0 4 0, 0 0 0)), ((2 0 10, 6 0 20, 6 2 20, 2 2 10, 2 0 10)))", 0.0,
     "POLYGON Z((0 0 0,0 4 0,4 4 4,4 2 9.5,6 2 20,6 0 20,4 0 4,2 0 10,0 0 0))"),
    // three segments through (2 2): the node takes the pair GEOS meets first (the first two)
    ("POLYGON Z ((0 0 0, 4 4 8, 4 0 1, 0 4 2, 2 0 3, 2 4 7, 0 0 0))", 0.0,
     "POLYGON Z((2 2 2.75,4 4 8,4 0 1,2 2 2.75))"),
    // a vertex on another part's edge (repaired because the small square overlaps)
    ("MULTIPOLYGON Z (((0 0 1, 4 0 3, 4 4 5, 0 4 1, 0 0 1)), ((4 2 7, 8 0 9, 8 4 9, 4 2 7)), ((1 1 0, 2 1 0, 2 2 0, 1 2 0, 1 1 0)))", 0.0,
     "MULTIPOLYGON Z(((0 0 1,0 4 1,4 4 5,4 2 7,4 0 3,0 0 1)),((4 2 7,8 4 9,8 0 9,4 2 7)))"),
    // XYZM: Z interpolated, M dropped; XYM: 2D
    ("POLYGON ZM ((0 0 1 10, 4 4 2 20, 4 0 3 30, 0 4 4 40, 0 0 1 10))", 0.0,
     "POLYGON Z((2 2 2.5,4 4 2,4 0 3,2 2 2.5))"),
    ("POLYGON M ((0 0 1, 4 4 2, 4 0 3, 0 4 4, 0 0 1))", 0.0,
     "POLYGON((2 2,4 4,4 0,2 2))"),
    // POLYGON Z ((0 0 1, 4 4 NaN, 4 0 3, 0 4 4, 0 0 1)): 0 0 1 -> 4 4 NaN gives 1
    ("01EB030000010000000500000000000000000000000000000000000000000000000000F03F00000000000010400000000000001040000000000000F87F00000000000010400000000000000000000000000000084000000000000000000000000000001040000000000000104000000000000000000000000000000000000000000000F03F", 0.0,
     "POLYGON Z((2 2 2.25,4 4 NaN,4 0 3,2 2 2.25))"),
    // POLYGON Z ((0 0 1, 4 4 2, 4 0 NaN, 0 4 NaN, 0 0 1)): the NaN segment gives nothing
    ("01EB030000010000000500000000000000000000000000000000000000000000000000F03F00000000000010400000000000001040000000000000004000000000000010400000000000000000000000000000F87F00000000000000000000000000001040000000000000F87F00000000000000000000000000000000000000000000F03F", 0.0,
     "POLYGON Z((2 2 1.5,4 4 2,4 0 NaN,2 2 1.5))"),
    // MULTIPOLYGON Z (((0 0 1, 4 0 3, 4 4 5, 0 4 1, 0 0 1)), ((4 2 NaN, 8 0 9, 8 4 9,
    // 4 2 NaN)), ((1 1 0, 2 1 0, 2 2 0, 1 2 0, 1 1 0))): the NaN vertex on 4 0 3 -> 4 4 5
    // becomes a node of Z 4. GEOS gives the triangle `((4 2 NaN, 8 4 9, 8 0 9, 4 2 4))`: a
    // split edge starts with the node's Z but ends with the original vertex
    // (`SegmentNodeList::createSplitEdgePts`), and Z here is looked up by position, so the
    // node has Z 4 at both ends (documented deviation)
    ("01EE0300000300000001EB030000010000000500000000000000000000000000000000000000000000000000F03F00000000000010400000000000000000000000000000084000000000000010400000000000001040000000000000144000000000000000000000000000001040000000000000F03F00000000000000000000000000000000000000000000F03F01EB030000010000000400000000000000000010400000000000000040000000000000F87F00000000000020400000000000000000000000000000224000000000000020400000000000001040000000000000224000000000000010400000000000000040000000000000F87F01EB0300000100000005000000000000000000F03F000000000000F03F00000000000000000000000000000040000000000000F03F0000000000000000000000000000004000000000000000400000000000000000000000000000F03F00000000000000400000000000000000000000000000F03F000000000000F03F0000000000000000", 0.0,
     "MULTIPOLYGON Z(((0 0 1,0 4 1,4 4 5,4 2 4,4 0 3,0 0 1)),((4 2 4,8 4 9,8 0 9,4 2 4)))"),
];

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// A one-row column of `input`: WKT, or ISO WKB in hex (which can carry a NaN Z).
fn column_of(input: &str) -> GeoColumn {
    if input.starts_with("01") {
        GeoColumn::from_wkb(&[Some(hex(input))], None).expect("wkb")
    } else {
        GeoColumn::from_wkt(&[Some(input)], None).expect("wkt")
    }
}

#[test]
fn simplify_repair_interpolates_z_at_new_nodes_as_geos() {
    let mut wrong = Vec::new();
    for (input, tolerance, expected) in SIMPLIFY_Z {
        let out = column_of(input).simplify(*tolerance, false).unwrap();
        let got = out.to_wkt().unwrap()[0].clone().unwrap();
        if got != *expected {
            wrong.push(format!(
                "simplify({tolerance}):\n  got  {got}\n  want {expected}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Fuzz harness, run by hand against Shapely (see `test/reference/fuzz_simplify_repair.py`):
/// reads `GF_FUZZ_IN` (one `wkt<TAB>tolerance` per line; the geometry may be hex ISO WKB) and
/// writes `GF_FUZZ_OUT` (one `repaired<TAB>wkt` per line, `repaired` being 1 when `buffer(0)`
/// was needed).
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
        let column = column_of(wkt);
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
