//! Per-group geometry aggregation: every group must give what filtering its rows and
//! aggregating them with the whole-column method gives. Shapely 2.2 values are in comments:
//!
//! ```python
//! shapely.envelope(shapely.GeometryCollection(g)).wkt  # g0: POLYGON ((0 0, 3 0, 3 3, 0 3, 0 0))
//! shapely.convex_hull(shapely.GeometryCollection(g)).wkt  # g2: LINESTRING (5 5, 7 1)
//! shapely.centroid(shapely.union_all(g)).wkt  # g0: POINT (1.5 1.5), g2: POINT (6 3)
//! ```

use crate::{GeoColumn, GroupAgg, UnaryOp};

const ROWS: [Option<&str>; 8] = [
    Some("POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))"),
    Some("POLYGON ((10 10, 12 10, 12 12, 10 12, 10 10))"),
    Some("POINT (5 5)"),
    Some("POLYGON ((1 1, 3 1, 3 3, 1 3, 1 1))"),
    None,
    Some("POINT (7 1)"),
    None,
    Some("POINT (5 5)"),
];
// group 0: rows 0, 3 (polygons); group 1: row 1; group 2: rows 2, 4, 5, 7 (points and a
// null); group 3: row 6 (only a null).
const IDS: [u32; 8] = [0, 1, 2, 0, 2, 2, 3, 2];

fn column() -> GeoColumn {
    GeoColumn::from_wkt(&ROWS, Some("EPSG:3857")).expect("valid WKT")
}

fn members(group: u32) -> Vec<usize> {
    (0..IDS.len()).filter(|&i| IDS[i] == group).collect()
}

fn per_group(agg: impl Fn(&GeoColumn) -> GeoColumn) -> Vec<Option<String>> {
    let col = column();
    (0..4)
        .flat_map(|g| agg(&col.take(&members(g)).unwrap()).to_wkt().unwrap())
        .collect()
}

fn grouped(agg: GroupAgg) -> GeoColumn {
    column().aggregate_groups(&IDS, 4, agg).expect("aggregate")
}

#[test]
fn every_aggregation_equals_filtering_each_group() {
    let cases: Vec<(GroupAgg, Vec<Option<String>>)> = vec![
        (GroupAgg::UnionAll, per_group(|c| c.union_all().unwrap())),
        (
            GroupAgg::IntersectionAll,
            per_group(|c| c.intersection_all().unwrap()),
        ),
        (GroupAgg::Collect, per_group(|c| c.collect(false).unwrap())),
        (
            GroupAgg::Envelope,
            per_group(|c| c.collect(false).unwrap().unary(UnaryOp::Envelope).unwrap()),
        ),
        (
            GroupAgg::ConvexHull,
            per_group(|c| {
                c.collect(false)
                    .unwrap()
                    .unary(UnaryOp::ConvexHull)
                    .unwrap()
            }),
        ),
        (
            GroupAgg::Centroid,
            per_group(|c| c.union_all().unwrap().centroid().unwrap()),
        ),
    ];
    for (agg, expected) in cases {
        let out = grouped(agg);
        assert_eq!(out.len(), 4, "{agg:?}");
        assert_eq!(out.crs().as_deref(), Some("EPSG:3857"), "{agg:?}");
        assert_eq!(out.to_wkt().unwrap(), expected, "{agg:?}");
    }
}

#[test]
fn shapely_values_for_the_reference_groups() {
    let wkt = |agg| grouped(agg).to_wkt().unwrap();
    // Vertex order of a ring is not part of the contract, so compare extent and area.
    let envelope = grouped(GroupAgg::Envelope);
    let boxes = envelope.bounds();
    assert_eq!(boxes[0], Some([0.0, 0.0, 3.0, 3.0]));
    assert_eq!(boxes[2], Some([5.0, 1.0, 7.0, 5.0]));
    assert_eq!(envelope.area()[0], Some(9.0));
    assert_eq!(
        wkt(GroupAgg::ConvexHull)[2].as_deref(),
        Some("LINESTRING(5 5,7 1)")
    );
    let centroid = wkt(GroupAgg::Centroid);
    assert_eq!(centroid[0].as_deref(), Some("POINT(1.5 1.5)"));
    assert_eq!(centroid[2].as_deref(), Some("POINT(6 3)"));
    // GeoFrame's collect keeps duplicates and builds Multi* for one family.
    let collect = wkt(GroupAgg::Collect);
    assert_eq!(collect[2].as_deref(), Some("MULTIPOINT((5 5),(7 1),(5 5))"));
    assert_eq!(
        collect[1].as_deref(),
        Some("POLYGON((10 10,12 10,12 12,10 12,10 10))")
    );
}

#[test]
fn first_and_last_skip_nulls_and_all_null_groups_stay_null() {
    let first = grouped(GroupAgg::First).to_wkt().unwrap();
    let last = grouped(GroupAgg::Last).to_wkt().unwrap();
    assert_eq!(first[0].as_deref(), Some("POLYGON((0 0,2 0,2 2,0 2,0 0))"));
    assert_eq!(last[0].as_deref(), Some("POLYGON((1 1,3 1,3 3,1 3,1 1))"));
    assert_eq!(first[2].as_deref(), Some("POINT(5 5)"));
    assert_eq!(last[2].as_deref(), Some("POINT(5 5)"));
    assert_eq!(first[3], None);
    assert_eq!(last[3], None);
}

#[test]
fn union_matches_union_groups() {
    let a = grouped(GroupAgg::UnionAll).to_wkt().unwrap();
    let b = column().union_groups(&IDS, 4).unwrap().to_wkt().unwrap();
    assert_eq!(a, b);
}

#[test]
fn bad_inputs_are_errors_and_zero_groups_is_empty() {
    let col = column();
    assert!(col
        .aggregate_groups(&IDS[..3], 4, GroupAgg::UnionAll)
        .is_err());
    assert!(col.aggregate_groups(&IDS, 3, GroupAgg::Envelope).is_err());
    assert!(col.aggregate_groups(&IDS, 99, GroupAgg::Envelope).is_err());
    let none = col.take(&[]).unwrap();
    let out = none.aggregate_groups(&[], 0, GroupAgg::Collect).unwrap();
    assert_eq!(out.len(), 0);
    assert_eq!(out.crs().as_deref(), Some("EPSG:3857"));
    assert_eq!(
        GroupAgg::from_name("convex_hull"),
        Some(GroupAgg::ConvexHull)
    );
    assert_eq!(GroupAgg::from_name("nope"), None);
}
