//! Batch G1: GeoParquet write options, bbox reads, Feather and Arrow IPC interchange.

use std::fs::File;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::{
    Array, ArrayRef, BinaryArray, Float64Array, Int64Array, RecordBatch, StringArray,
};
use arrow_ipc::reader::{FileReader, StreamReader};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use tempfile::TempDir;

use crate::test_support::{column, ipc, names, path, wkt};
use crate::{
    from_arrow_ipc, read_feather, read_geoparquet, read_geoparquet_filtered, to_arrow_ipc,
    write_feather, write_geoparquet, write_geoparquet_with, FeatherCompression, GeoColumn,
    GeometryEncoding, ParquetOptions,
};

fn attributes(n: usize) -> Vec<u8> {
    ipc(vec![
        (
            "id",
            Arc::new(Int64Array::from((0..n as i64).collect::<Vec<_>>())) as ArrayRef,
        ),
        (
            "name",
            Arc::new(StringArray::from(
                (0..n).map(|i| Some(format!("n{i}"))).collect::<Vec<_>>(),
            )),
        ),
        (
            "score",
            Arc::new(Float64Array::from(
                (0..n).map(|i| Some(i as f64 * 0.5)).collect::<Vec<_>>(),
            )),
        ),
    ])
}

/// Points on the diagonal: row `i` sits at `(i, i)`.
fn points(n: usize, crs: Option<&str>) -> GeoColumn {
    let rows: Vec<Option<String>> = (0..n).map(|i| Some(format!("POINT ({i} {i})"))).collect();
    GeoColumn::from_wkt(&rows, crs).unwrap()
}

fn mixed(crs: Option<&str>) -> GeoColumn {
    GeoColumn::from_wkt(
        &[
            Some("POINT (1 2)"),
            None,
            Some("POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))"),
        ],
        crs,
    )
    .unwrap()
}

fn polygons(crs: Option<&str>) -> GeoColumn {
    GeoColumn::from_wkt(
        &[
            Some("POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))"),
            None,
            Some("MULTIPOLYGON (((10 10, 11 10, 11 11, 10 10)), ((20 20, 21 20, 21 21, 20 20)))"),
        ],
        crs,
    )
    .unwrap()
}

fn opts() -> ParquetOptions {
    ParquetOptions::default()
}

fn parquet_builder(file: &str) -> ParquetRecordBatchReaderBuilder<File> {
    ParquetRecordBatchReaderBuilder::try_new(File::open(file).unwrap()).unwrap()
}

fn geo_json(file: &str) -> serde_json::Value {
    let builder = parquet_builder(file);
    let kv = builder
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .unwrap()
        .iter()
        .find(|kv| kv.key == "geo")
        .expect("geo key");
    serde_json::from_str(kv.value.as_ref().unwrap()).unwrap()
}

fn ids(read_ipc: &[u8]) -> Vec<Option<i64>> {
    let (_, array) = column(read_ipc, "id");
    array
        .as_primitive::<arrow_array::types::Int64Type>()
        .iter()
        .collect()
}

// -- GeoParquet write options -----------------------------------------------------------

#[test]
fn every_compression_round_trips_and_is_recorded_in_the_file() {
    let dir = TempDir::new().unwrap();
    type Check = fn(&Compression) -> bool;
    let cases: [(&str, Check); 6] = [
        ("snappy", |c| matches!(c, Compression::SNAPPY)),
        ("zstd", |c| matches!(c, Compression::ZSTD(_))),
        ("gzip", |c| matches!(c, Compression::GZIP(_))),
        ("lz4", |c| matches!(c, Compression::LZ4_RAW)),
        ("brotli", |c| matches!(c, Compression::BROTLI(_))),
        ("none", |c| matches!(c, Compression::UNCOMPRESSED)),
    ];
    for (name, check) in cases {
        let file = path(&dir, &format!("{name}.parquet"));
        let geom = mixed(Some("EPSG:4326"));
        let options = ParquetOptions {
            compression: name.to_string(),
            ..opts()
        };
        write_geoparquet_with(&file, &geom, "geometry", &attributes(3), &options).unwrap();

        let builder = parquet_builder(&file);
        let used = builder.metadata().row_group(0).column(0).compression();
        assert!(check(&used), "{name}: got {used:?}");

        let read = read_geoparquet(&file, None).unwrap();
        assert_eq!(wkt(&read.geometry), wkt(&geom), "{name}");
        assert_eq!(read.geometry.crs().as_deref(), Some("EPSG:4326"));
        assert_eq!(ids(&read.attributes_ipc), [Some(0), Some(1), Some(2)]);
    }
}

#[test]
fn compression_level_is_applied_and_validated() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "level.parquet");
    let options = ParquetOptions {
        compression: "zstd".into(),
        compression_level: Some(7),
        ..opts()
    };
    write_geoparquet_with(
        &file,
        &points(4, None),
        "geometry",
        &attributes(4),
        &options,
    )
    .unwrap();
    let used = parquet_builder(&file)
        .metadata()
        .row_group(0)
        .column(0)
        .compression();
    // The level is a writer setting only; the file records the codec.
    assert!(matches!(used, Compression::ZSTD(_)), "{used:?}");

    let bad = ParquetOptions {
        compression: "gzip".into(),
        compression_level: Some(99),
        ..opts()
    };
    let err = write_geoparquet_with(&file, &points(4, None), "geometry", &attributes(4), &bad)
        .unwrap_err();
    assert!(err.contains("compression_level"), "{err}");

    let unsupported = ParquetOptions {
        compression: "snappy".into(),
        compression_level: Some(3),
        ..opts()
    };
    let err = write_geoparquet_with(
        &file,
        &points(4, None),
        "geometry",
        &attributes(4),
        &unsupported,
    )
    .unwrap_err();
    assert!(err.contains("compression_level"), "{err}");
}

#[test]
fn unknown_compression_is_rejected() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "bad.parquet");
    let options = ParquetOptions {
        compression: "rar".into(),
        ..opts()
    };
    let err = write_geoparquet_with(
        &file,
        &points(2, None),
        "geometry",
        &attributes(2),
        &options,
    )
    .unwrap_err();
    assert!(err.contains("rar"), "{err}");
    assert!(!std::path::Path::new(&file).exists());
}

#[test]
fn row_group_size_splits_the_file() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "groups.parquet");
    let options = ParquetOptions {
        row_group_size: Some(4),
        ..opts()
    };
    write_geoparquet_with(
        &file,
        &points(10, None),
        "geometry",
        &attributes(10),
        &options,
    )
    .unwrap();
    let builder = parquet_builder(&file);
    let sizes: Vec<i64> = builder
        .metadata()
        .row_groups()
        .iter()
        .map(|g| g.num_rows())
        .collect();
    assert_eq!(sizes, [4, 4, 2]);

    let read = read_geoparquet(&file, None).unwrap();
    assert_eq!(read.geometry.len(), 10);

    let zero = ParquetOptions {
        row_group_size: Some(0),
        ..opts()
    };
    assert!(
        write_geoparquet_with(&file, &points(2, None), "geometry", &attributes(2), &zero)
            .unwrap_err()
            .contains("row_group_size")
    );
}

#[test]
fn geoarrow_encoding_writes_native_columns_and_round_trips() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "native.parquet");
    let geom = points(5, Some("EPSG:4326"));
    let options = ParquetOptions {
        geometry_encoding: GeometryEncoding::GeoArrow,
        ..opts()
    };
    write_geoparquet_with(&file, &geom, "geometry", &attributes(5), &options).unwrap();

    let geo = geo_json(&file);
    assert_eq!(geo["columns"]["geometry"]["encoding"], "point");
    assert_eq!(geo["columns"]["geometry"]["geometry_types"][0], "Point");
    let builder = parquet_builder(&file);
    let field = builder
        .schema()
        .field_with_name("geometry")
        .unwrap()
        .clone();
    assert!(
        matches!(field.data_type(), DataType::Struct(_)),
        "{field:?}"
    );

    let read = read_geoparquet(&file, None).unwrap();
    assert_eq!(wkt(&read.geometry), wkt(&geom));
    assert_eq!(read.geometry.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(names(&read.attributes_ipc), ["id", "name", "score"]);
}

#[test]
fn geoarrow_encoding_promotes_single_parts_to_multi_and_keeps_nulls() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "multi.parquet");
    let geom = polygons(Some("EPSG:4326"));
    let options = ParquetOptions {
        geometry_encoding: GeometryEncoding::GeoArrow,
        ..opts()
    };
    write_geoparquet_with(&file, &geom, "geometry", &attributes(3), &options).unwrap();
    assert_eq!(
        geo_json(&file)["columns"]["geometry"]["encoding"],
        "multipolygon"
    );

    let read = read_geoparquet(&file, None).unwrap();
    let rows = wkt(&read.geometry);
    assert!(rows[0].as_deref().unwrap().starts_with("MULTIPOLYGON"));
    assert_eq!(rows[1], None);
    assert_eq!(read.geometry.len(), 3);
}

#[test]
fn geoarrow_encoding_rejects_incompatible_mixed_types() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "mixed.parquet");
    let options = ParquetOptions {
        geometry_encoding: GeometryEncoding::GeoArrow,
        ..opts()
    };
    let err = write_geoparquet_with(&file, &mixed(None), "geometry", &attributes(3), &options)
        .unwrap_err();
    assert!(err.contains("geoarrow"), "{err}");
    assert!(!std::path::Path::new(&file).exists());
}

#[test]
fn covering_bbox_adds_a_struct_column_and_metadata_and_is_hidden_on_read() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "covering.parquet");
    let options = ParquetOptions {
        covering_bbox: true,
        ..opts()
    };
    write_geoparquet_with(
        &file,
        &mixed(Some("EPSG:4326")),
        "geometry",
        &attributes(3),
        &options,
    )
    .unwrap();

    let geo = geo_json(&file);
    let covering = &geo["columns"]["geometry"]["covering"]["bbox"];
    assert_eq!(covering["xmin"], serde_json::json!(["bbox", "xmin"]));
    assert_eq!(covering["ymax"], serde_json::json!(["bbox", "ymax"]));
    assert_eq!(geo["version"], "1.1.0");

    let builder = parquet_builder(&file);
    let field = builder.schema().field_with_name("bbox").unwrap().clone();
    assert!(matches!(field.data_type(), DataType::Struct(f) if f.len() == 4));
    let batch = builder.build().unwrap().next().unwrap().unwrap();
    let bbox = batch.column_by_name("bbox").unwrap().as_struct().clone();
    let xmax = bbox
        .column_by_name("xmax")
        .unwrap()
        .as_primitive::<arrow_array::types::Float64Type>();
    assert_eq!(xmax.value(0), 1.0);
    assert_eq!(xmax.value(2), 4.0);
    assert!(bbox.is_null(1));

    let read = read_geoparquet(&file, None).unwrap();
    assert_eq!(names(&read.attributes_ipc), ["id", "name", "score"]);
    assert_eq!(read.geometry.len(), 3);
}

#[test]
fn covering_bbox_conflicts_with_an_existing_bbox_attribute() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "conflict.parquet");
    let attrs = ipc(vec![(
        "bbox",
        Arc::new(Int64Array::from(vec![1, 2])) as ArrayRef,
    )]);
    let options = ParquetOptions {
        covering_bbox: true,
        ..opts()
    };
    let err =
        write_geoparquet_with(&file, &points(2, None), "geometry", &attrs, &options).unwrap_err();
    assert!(err.contains("bbox"), "{err}");
}

#[test]
fn schema_version_1_0_0_is_written_and_rejects_1_1_features() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "v1.parquet");
    let options = ParquetOptions {
        schema_version: "1.0.0".into(),
        ..opts()
    };
    write_geoparquet_with(&file, &mixed(None), "geometry", &attributes(3), &options).unwrap();
    let geo = geo_json(&file);
    assert_eq!(geo["version"], "1.0.0");
    assert!(geo["columns"]["geometry"].get("covering").is_none());
    assert_eq!(read_geoparquet(&file, None).unwrap().geometry.len(), 3);

    for bad in [
        ParquetOptions {
            schema_version: "1.0.0".into(),
            covering_bbox: true,
            ..opts()
        },
        ParquetOptions {
            schema_version: "1.0.0".into(),
            geometry_encoding: GeometryEncoding::GeoArrow,
            ..opts()
        },
        ParquetOptions {
            schema_version: "2.0.0".into(),
            ..opts()
        },
    ] {
        let err = write_geoparquet_with(&file, &points(2, None), "geometry", &attributes(2), &bad)
            .unwrap_err();
        assert!(err.contains("schema_version"), "{err}");
    }
}

#[test]
fn default_options_keep_the_previous_output() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "default.parquet");
    write_geoparquet(&file, &mixed(None), "geometry", &attributes(3)).unwrap();
    let geo = geo_json(&file);
    assert_eq!(geo["version"], "1.1.0");
    assert_eq!(geo["columns"]["geometry"]["encoding"], "WKB");
    assert!(geo["columns"]["geometry"].get("covering").is_none());
    let used = parquet_builder(&file)
        .metadata()
        .row_group(0)
        .column(0)
        .compression();
    assert_eq!(used, Compression::SNAPPY);
}

// -- GeoParquet bbox reads ----------------------------------------------------------------

fn write_diagonal(dir: &TempDir, name: &str, options: &ParquetOptions) -> String {
    let file = path(dir, name);
    let options = ParquetOptions {
        row_group_size: Some(5),
        ..options.clone()
    };
    write_geoparquet_with(
        &file,
        &points(20, Some("EPSG:4326")),
        "geometry",
        &attributes(20),
        &options,
    )
    .unwrap();
    file
}

#[test]
fn bbox_filter_works_with_covering_native_and_plain_wkb_files() {
    let dir = TempDir::new().unwrap();
    let variants = [
        (
            "covering",
            ParquetOptions {
                covering_bbox: true,
                ..opts()
            },
        ),
        (
            "native",
            ParquetOptions {
                geometry_encoding: GeometryEncoding::GeoArrow,
                ..opts()
            },
        ),
        ("plain", opts()),
    ];
    for (label, options) in variants {
        let file = write_diagonal(&dir, &format!("{label}.parquet"), &options);
        let read = read_geoparquet_filtered(&file, None, Some([6.5, 6.5, 9.5, 9.5])).unwrap();
        assert_eq!(
            ids(&read.attributes_ipc),
            [Some(7), Some(8), Some(9)],
            "{label}"
        );
        assert_eq!(read.geometry.len(), 3, "{label}");
        assert_eq!(read.geometry.crs().as_deref(), Some("EPSG:4326"));
        assert_eq!(wkt(&read.geometry)[0].as_deref(), Some("POINT(7 7)"));
        assert_eq!(
            names(&read.attributes_ipc),
            ["id", "name", "score"],
            "{label}"
        );

        // Boundary touches count as intersecting; a box with no rows gives an empty frame.
        let edge = read_geoparquet_filtered(&file, None, Some([19.0, 19.0, 30.0, 30.0])).unwrap();
        assert_eq!(ids(&edge.attributes_ipc), [Some(19)], "{label}");
        let none =
            read_geoparquet_filtered(&file, None, Some([100.0, 100.0, 101.0, 101.0])).unwrap();
        assert_eq!(none.geometry.len(), 0, "{label}");
        assert_eq!(
            names(&none.attributes_ipc),
            ["id", "name", "score"],
            "{label}"
        );
    }
}

#[test]
fn bbox_filter_with_a_column_selection_and_no_filter_is_identity() {
    let dir = TempDir::new().unwrap();
    let file = write_diagonal(
        &dir,
        "cols.parquet",
        &ParquetOptions {
            covering_bbox: true,
            ..opts()
        },
    );
    let cols = vec!["id".to_string()];
    let read = read_geoparquet_filtered(&file, Some(&cols), Some([0.0, 0.0, 1.0, 1.0])).unwrap();
    assert_eq!(names(&read.attributes_ipc), ["id"]);
    assert_eq!(ids(&read.attributes_ipc), [Some(0), Some(1)]);
    let all = read_geoparquet_filtered(&file, None, None).unwrap();
    assert_eq!(all.geometry.len(), 20);
}

// -- Feather and Arrow IPC ----------------------------------------------------------------

fn ext_name(field: &Field) -> Option<&str> {
    field
        .metadata()
        .get("ARROW:extension:name")
        .map(String::as_str)
}

#[test]
fn feather_round_trips_every_compression_and_encoding() {
    let dir = TempDir::new().unwrap();
    for compression in [
        FeatherCompression::Uncompressed,
        FeatherCompression::Lz4,
        FeatherCompression::Zstd,
    ] {
        for encoding in [GeometryEncoding::Wkb, GeometryEncoding::GeoArrow] {
            let file = path(&dir, "t.feather");
            let geom = polygons(Some("EPSG:3857"));
            write_feather(
                &file,
                &geom,
                "geometry",
                &attributes(3),
                encoding,
                compression,
            )
            .unwrap();
            let read = read_feather(&file, None, None).unwrap();
            assert_eq!(read.geometry_column, "geometry");
            assert_eq!(read.geometry.crs().as_deref(), Some("EPSG:3857"));
            assert_eq!(read.geometry.len(), 3);
            assert_eq!(wkt(&read.geometry)[1], None);
            assert_eq!(ids(&read.attributes_ipc), [Some(0), Some(1), Some(2)]);
            assert_eq!(names(&read.attributes_ipc), ["id", "name", "score"]);
        }
    }
}

#[test]
fn feather_schema_carries_geoarrow_extension_metadata_and_geo_json() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "meta.feather");
    write_feather(
        &file,
        &mixed(Some("EPSG:4326")),
        "geometry",
        &attributes(3),
        GeometryEncoding::Wkb,
        FeatherCompression::Lz4,
    )
    .unwrap();
    let reader = FileReader::try_new(File::open(&file).unwrap(), None).unwrap();
    let schema = reader.schema();
    let field = schema.field_with_name("geometry").unwrap();
    assert_eq!(ext_name(field), Some("geoarrow.wkb"));
    let meta: serde_json::Value =
        serde_json::from_str(&field.metadata()["ARROW:extension:metadata"]).unwrap();
    assert_eq!(meta["crs"]["id"]["code"], 4326);
    let geo: serde_json::Value = serde_json::from_str(&schema.metadata()["geo"]).unwrap();
    assert_eq!(geo["primary_column"], "geometry");
    assert_eq!(geo["columns"]["geometry"]["encoding"], "WKB");
    assert!(geo["columns"]["geometry"].get("covering").is_none());
}

#[test]
fn feather_geoarrow_encoding_uses_native_extension_types() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "native.feather");
    write_feather(
        &file,
        &points(4, None),
        "geometry",
        &attributes(4),
        GeometryEncoding::GeoArrow,
        FeatherCompression::Uncompressed,
    )
    .unwrap();
    let reader = FileReader::try_new(File::open(&file).unwrap(), None).unwrap();
    let schema = reader.schema();
    assert_eq!(
        ext_name(schema.field_with_name("geometry").unwrap()),
        Some("geoarrow.point")
    );
    let read = read_feather(&file, None, None).unwrap();
    assert_eq!(wkt(&read.geometry)[3].as_deref(), Some("POINT(3 3)"));
    assert_eq!(read.geometry.crs(), None);
}

#[test]
fn feather_column_selection_and_missing_geometry_errors() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "cols.feather");
    write_feather(
        &file,
        &points(3, None),
        "geometry",
        &attributes(3),
        GeometryEncoding::Wkb,
        FeatherCompression::Lz4,
    )
    .unwrap();
    let cols = vec!["name".to_string()];
    let read = read_feather(&file, Some(&cols), None).unwrap();
    assert_eq!(names(&read.attributes_ipc), ["name"]);
    let missing = vec!["nope".to_string()];
    assert!(read_feather(&file, Some(&missing), None)
        .unwrap_err()
        .contains("nope"));

    // A table with no geometry column is not a geo table.
    let plain = path(&dir, "plain.feather");
    let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int64, true)]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int64Array::from(vec![1])) as ArrayRef],
    )
    .unwrap();
    let mut writer =
        arrow_ipc::writer::FileWriter::try_new(File::create(&plain).unwrap(), &schema).unwrap();
    writer.write(&batch).unwrap();
    writer.finish().unwrap();
    assert!(read_feather(&plain, None, None)
        .unwrap_err()
        .contains("geometry"));
}

#[test]
fn arrow_stream_round_trip_for_both_encodings() {
    for encoding in [GeometryEncoding::Wkb, GeometryEncoding::GeoArrow] {
        let geom = polygons(Some("EPSG:4326"));
        let bytes = to_arrow_ipc(&geom, "geometry", &attributes(3), encoding).unwrap();

        let reader = StreamReader::try_new(&bytes[..], None).unwrap();
        let schema = reader.schema();
        let field = schema.field_with_name("geometry").unwrap();
        let expected = match encoding {
            GeometryEncoding::Wkb => "geoarrow.wkb",
            GeometryEncoding::GeoArrow => "geoarrow.multipolygon",
        };
        assert_eq!(ext_name(field), Some(expected));
        let meta: serde_json::Value =
            serde_json::from_str(&field.metadata()["ARROW:extension:metadata"]).unwrap();
        assert!(meta["crs"].is_object(), "PROJJSON expected: {meta}");
        assert_eq!(meta["crs"]["id"]["authority"], "EPSG");

        let back = from_arrow_ipc(&bytes, None, None).unwrap();
        assert_eq!(back.geometry.crs().as_deref(), Some("EPSG:4326"));
        assert_eq!(back.geometry.len(), 3);
        assert_eq!(wkt(&back.geometry)[1], None);
        assert_eq!(ids(&back.attributes_ipc), [Some(0), Some(1), Some(2)]);
    }
}

#[test]
fn arrow_stream_of_a_bare_series_has_one_geometry_column() {
    let geom = mixed(Some("OGC:CRS84"));
    let bytes = to_arrow_ipc(&geom, "geometry", &[], GeometryEncoding::Wkb).unwrap();
    let schema = StreamReader::try_new(&bytes[..], None).unwrap().schema();
    assert_eq!(schema.fields().len(), 1);
    let back = from_arrow_ipc(&bytes, None, None).unwrap();
    assert_eq!(back.geometry.crs().as_deref(), Some("OGC:CRS84"));
    assert_eq!(wkt(&back.geometry), wkt(&geom));
    assert!(!back.attributes_ipc.is_empty());
    assert!(names(&back.attributes_ipc).is_empty());
}

fn stream_of(fields: Vec<Field>, arrays: Vec<ArrayRef>) -> Vec<u8> {
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
    crate::attributes::ipc_from_batches(&schema, &[batch]).unwrap()
}

#[test]
fn from_arrow_accepts_plain_wkb_plain_wkt_and_wkt_extension() {
    let geom = mixed(None);
    let wkb: Vec<Option<Vec<u8>>> = geom.to_wkb().unwrap();
    let wkb_array = BinaryArray::from_iter(wkb.iter().map(|b| b.as_deref()));
    let bytes = stream_of(
        vec![Field::new("geometry", DataType::Binary, true)],
        vec![Arc::new(wkb_array)],
    );
    let back = from_arrow_ipc(&bytes, None, None).unwrap();
    assert_eq!(wkt(&back.geometry), wkt(&geom));

    let rows = [Some("POINT (5 6)"), None, Some("LINESTRING (0 0, 1 1)")];
    let wkt_array = StringArray::from(rows.to_vec());
    let bytes = stream_of(
        vec![
            Field::new("geom", DataType::Utf8, true),
            Field::new("id", DataType::Int64, true),
        ],
        vec![
            Arc::new(wkt_array.clone()),
            Arc::new(Int64Array::from(vec![1, 2, 3])),
        ],
    );
    let back = from_arrow_ipc(&bytes, None, Some("geom")).unwrap();
    assert_eq!(back.geometry_column, "geom");
    assert_eq!(
        wkt(&back.geometry)[2].as_deref(),
        Some("LINESTRING(0 0,1 1)")
    );
    assert_eq!(names(&back.attributes_ipc), ["id"]);

    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "ARROW:extension:name".to_string(),
        "geoarrow.wkt".to_string(),
    );
    metadata.insert(
        "ARROW:extension:metadata".to_string(),
        r#"{"crs":"EPSG:4326","crs_type":"authority_code"}"#.to_string(),
    );
    let bytes = stream_of(
        vec![Field::new("geometry", DataType::Utf8, true).with_metadata(metadata)],
        vec![Arc::new(wkt_array)],
    );
    let back = from_arrow_ipc(&bytes, None, None).unwrap();
    assert_eq!(back.geometry.crs().as_deref(), Some("EPSG:4326"));
    assert_eq!(back.geometry.len(), 3);

    let nothing = stream_of(
        vec![Field::new("a", DataType::Int64, true)],
        vec![Arc::new(Int64Array::from(vec![1]))],
    );
    assert!(from_arrow_ipc(&nothing, None, None)
        .unwrap_err()
        .contains("geometry"));
    assert!(from_arrow_ipc(b"not arrow", None, None).is_err());
}

// -- PROJJSON for well-known CRS ------------------------------------------------------------

fn crs_in_geo(file: &str) -> serde_json::Value {
    geo_json(file)["columns"]["geometry"]["crs"].clone()
}

#[test]
fn well_known_projected_crs_are_written_as_complete_projjson() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "crs.parquet");

    write_geoparquet(
        &file,
        &points(2, Some("EPSG:3857")),
        "geometry",
        &attributes(2),
    )
    .unwrap();
    let crs = crs_in_geo(&file);
    assert_eq!(crs["type"], "ProjectedCRS");
    assert_eq!(crs["id"]["code"], 3857);
    assert_eq!(crs["base_crs"]["id"]["code"], 4326);
    assert_eq!(crs["conversion"]["method"]["id"]["code"], 1024);
    assert_eq!(crs["coordinate_system"]["subtype"], "Cartesian");

    write_geoparquet(
        &file,
        &points(2, Some("EPSG:32617")),
        "geometry",
        &attributes(2),
    )
    .unwrap();
    let crs = crs_in_geo(&file);
    assert_eq!(crs["name"], "WGS 84 / UTM zone 17N");
    assert_eq!(crs["id"]["code"], 32617);
    let params = crs["conversion"]["parameters"].as_array().unwrap();
    let value = |name: &str| params.iter().find(|p| p["name"] == name).unwrap()["value"].clone();
    assert_eq!(value("Longitude of natural origin"), -81);
    assert_eq!(value("False northing"), 0);

    write_geoparquet(
        &file,
        &points(2, Some("EPSG:32701")),
        "geometry",
        &attributes(2),
    )
    .unwrap();
    let crs = crs_in_geo(&file);
    assert_eq!(crs["name"], "WGS 84 / UTM zone 1S");
    let params = crs["conversion"]["parameters"].as_array().unwrap();
    let value = |name: &str| params.iter().find(|p| p["name"] == name).unwrap()["value"].clone();
    assert_eq!(value("Longitude of natural origin"), -177);
    assert_eq!(value("False northing"), 10000000);

    // Any other EPSG code in the bundled table is translated from its WKT1 definition; the
    // object has a top-level `type`, which pyproj requires.
    for (code, kind) in [
        (3035, "ProjectedCRS"),
        (2154, "ProjectedCRS"),
        (2236, "ProjectedCRS"),
        (4258, "GeographicCRS"),
    ] {
        write_geoparquet(
            &file,
            &points(2, Some(&format!("EPSG:{code}"))),
            "geometry",
            &attributes(2),
        )
        .unwrap();
        let crs = crs_in_geo(&file);
        assert_eq!(crs["type"], kind, "EPSG:{code}");
        assert_eq!(crs["id"]["code"], code);
        assert_eq!(
            read_geoparquet(&file, None)
                .unwrap()
                .geometry
                .crs()
                .as_deref(),
            Some(format!("EPSG:{code}").as_str())
        );
    }

    // An identifier without a definition keeps the identifier-only stub, which reads back in
    // GeoFrame.
    write_geoparquet(
        &file,
        &points(2, Some("EPSG:99999")),
        "geometry",
        &attributes(2),
    )
    .unwrap();
    assert_eq!(crs_in_geo(&file)["id"]["code"], 99999);
    assert_eq!(
        read_geoparquet(&file, None)
            .unwrap()
            .geometry
            .crs()
            .as_deref(),
        Some("EPSG:99999")
    );
}

#[test]
fn projected_projjson_round_trips_to_its_authority_code() {
    let dir = TempDir::new().unwrap();
    let file = path(&dir, "rt.feather");
    for code in ["EPSG:3857", "EPSG:32633", "EPSG:32760", "EPSG:4269"] {
        write_feather(
            &file,
            &points(2, Some(code)),
            "geometry",
            &attributes(2),
            GeometryEncoding::Wkb,
            FeatherCompression::Lz4,
        )
        .unwrap();
        assert_eq!(
            read_feather(&file, None, None)
                .unwrap()
                .geometry
                .crs()
                .as_deref(),
            Some(code)
        );
    }
}

#[test]
fn written_covering_statistics_let_the_reader_skip_row_groups() {
    use geo::{coord, Rect};
    use geoparquet::reader::GeoParquetReaderBuilder;

    let dir = TempDir::new().unwrap();
    let file = write_diagonal(
        &dir,
        "prune.parquet",
        &ParquetOptions {
            covering_bbox: true,
            ..opts()
        },
    );
    let builder = parquet_builder(&file);
    let geo_meta = builder.geoparquet_metadata().unwrap().unwrap();
    assert_eq!(builder.metadata().num_row_groups(), 4);
    let query = Rect::new(coord! { x: 6.5, y: 6.5 }, coord! { x: 9.5, y: 9.5 });
    // Rows 5..10 live in the second row group; the other three are skipped.
    assert_eq!(
        builder
            .intersecting_row_groups(query, &geo_meta, None)
            .unwrap(),
        vec![1]
    );
}

#[test]
fn geoarrow_encoding_promotes_lines_and_points_too() {
    let dir = TempDir::new().unwrap();
    for (rows, expected) in [
        (
            vec![
                Some("LINESTRING (0 0, 1 1)"),
                None,
                Some("MULTILINESTRING ((2 2, 3 3), (4 4, 5 5))"),
            ],
            "multilinestring",
        ),
        (
            vec![Some("POINT (1 1)"), Some("MULTIPOINT ((2 2), (3 3))"), None],
            "multipoint",
        ),
    ] {
        let file = path(&dir, "p.parquet");
        let geom = GeoColumn::from_wkt(&rows, Some("EPSG:4326")).unwrap();
        let options = ParquetOptions {
            geometry_encoding: GeometryEncoding::GeoArrow,
            ..opts()
        };
        write_geoparquet_with(&file, &geom, "geometry", &attributes(3), &options).unwrap();
        assert_eq!(geo_json(&file)["columns"]["geometry"]["encoding"], expected);
        let read = read_geoparquet(&file, None).unwrap();
        assert_eq!(read.geometry.len(), 3);
        assert_eq!(
            wkt(&read.geometry).iter().filter(|r| r.is_none()).count(),
            1
        );
    }
}
