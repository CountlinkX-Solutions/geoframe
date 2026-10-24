//! Secondary geometry columns in Feather and Arrow IPC (issue #93): every geometry column is
//! written with its own `geo` entry and extension metadata, and read back with its own CRS.

use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch};
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema};
use tempfile::TempDir;

use crate::{
    from_arrow_ipc, read_feather, to_arrow_ipc_multi, write_feather_multi, FeatherCompression,
    GeoColumn, GeometryEncoding,
};

fn ids() -> Vec<u8> {
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
    let columns: Vec<ArrayRef> = vec![Arc::new(Int64Array::from(vec![1, 2, 3]))];
    let batch = RecordBatch::try_new(schema, columns).unwrap();
    let mut out = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut out, &batch.schema()).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
    }
    out
}

fn points(crs: Option<&str>) -> GeoColumn {
    GeoColumn::from_wkt(&[Some("POINT (0 0)"), None, Some("POINT (2 2)")], crs).unwrap()
}

fn polygons(crs: Option<&str>) -> GeoColumn {
    GeoColumn::from_wkt(
        &[
            Some("POLYGON ((0 0, 1 0, 1 1, 0 0))"),
            Some("POLYGON ((5 5, 6 5, 6 6, 5 5))"),
            None,
        ],
        crs,
    )
    .unwrap()
}

fn wkt(column: &GeoColumn) -> Vec<Option<String>> {
    column.to_wkt().unwrap()
}

#[test]
fn feather_round_trips_every_geometry_column() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("m.feather");
    let path = path.to_str().unwrap();
    let (a, b, c) = (
        points(Some("EPSG:4326")),
        polygons(Some("EPSG:3857")),
        points(None),
    );

    for encoding in [GeometryEncoding::Wkb, GeometryEncoding::GeoArrow] {
        write_feather_multi(
            path,
            &[("geometry", &a), ("footprint", &b), ("centroid", &c)],
            &ids(),
            encoding,
            FeatherCompression::Lz4,
        )
        .unwrap();

        let read = read_feather(path, None, None).unwrap();
        assert_eq!(read.geometry_column, "geometry");
        assert_eq!(read.geometry.crs().as_deref(), Some("EPSG:4326"));
        assert_eq!(wkt(&read.geometry), wkt(&a));
        let names: Vec<&str> = read
            .secondary_geometries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["footprint", "centroid"]);
        assert_eq!(
            read.secondary_geometries[0].1.crs().as_deref(),
            Some("EPSG:3857")
        );
        assert_eq!(wkt(&read.secondary_geometries[0].1), wkt(&b));
        assert_eq!(read.secondary_geometries[1].1.crs(), None);
        assert_eq!(wkt(&read.secondary_geometries[1].1), wkt(&c));
    }
}

#[test]
fn arrow_stream_round_trips_and_selection_applies_to_secondary_columns() {
    let (a, b) = (points(Some("EPSG:4326")), polygons(Some("EPSG:3857")));
    let bytes = to_arrow_ipc_multi(
        &[("geometry", &a), ("footprint", &b)],
        &ids(),
        GeometryEncoding::Wkb,
    )
    .unwrap();

    let read = from_arrow_ipc(&bytes, None, None).unwrap();
    assert_eq!(read.secondary_geometries.len(), 1);
    assert_eq!(wkt(&read.secondary_geometries[0].1), wkt(&b));

    // `columns` selects among the secondary geometry columns like among attributes.
    let only_id = vec!["id".to_string()];
    let read = from_arrow_ipc(&bytes, Some(&only_id), None).unwrap();
    assert!(read.secondary_geometries.is_empty());
    let wanted = vec!["id".to_string(), "footprint".to_string()];
    let read = from_arrow_ipc(&bytes, Some(&wanted), None).unwrap();
    assert_eq!(read.secondary_geometries.len(), 1);
}

#[test]
fn clashing_or_misaligned_geometry_columns_are_refused() {
    let a = points(None);
    let short = GeoColumn::from_wkt(&[Some("POINT (0 0)")], None).unwrap();
    let err = to_arrow_ipc_multi(
        &[("geometry", &a), ("other", &short)],
        &ids(),
        GeometryEncoding::Wkb,
    )
    .unwrap_err();
    assert!(err.contains("other"), "{err}");

    let err = to_arrow_ipc_multi(
        &[("geometry", &a), ("geometry", &a)],
        &ids(),
        GeometryEncoding::Wkb,
    )
    .unwrap_err();
    assert!(err.contains("two geometry columns"), "{err}");
}
