//! Pure-Rust geometry core for GeoFrame. Holds no Rustler/BEAM dependencies so it can be
//! unit-tested with `cargo test` and reused outside Elixir.

mod affine;
mod arrow_io;
mod atomic;
mod attributes;
mod buffer_single_sided;
mod buffer_zero;
pub(crate) mod column;
mod coverage;
mod coverage_simplify;
mod crs_convert;
mod crs_datum_names;
mod crs_extra;
mod crs_names;
mod crs_wkt;
mod curves;
mod datum;
mod datum_grids;
#[allow(clippy::approx_constant)] // generated data: a bounding box edge reads 3.14
mod datum_helmert;
mod dbf;
mod deck;
mod ewkb;
mod feather_stream;
mod flatgeobuf_io;
mod flatgeobuf_nulls;
mod geo_convert;
mod geojson_io;
mod geojson_stream;
mod gpkg_io;
mod gpkg_stream;
mod group_agg;
mod line_merge;
mod mixed_dims;
mod mixed_ops;
mod noding;
mod offset_curve;
mod parity_a;
mod parity_b;
mod parity_c;
mod parity_e;
mod parity_f;
mod parity_h;
mod parity_i;
mod parity_j;
mod parquet_io;
mod polygonize;
mod predicates;
mod proj_ext;
mod projjson;
mod reproject;
mod set_ops;
mod shapefile_io;
mod shapefile_stream;
mod sindex;
mod stream_io;
mod unary;
mod utm;
mod utm_crs;
mod validity;
mod validity_reason;
mod wkt1_projjson;
mod wkt2_projjson;
mod wkt_read;
mod wkt_write;
mod zm;
mod zm_bridge;
mod zm_io;
mod zm_overlay;

pub use arrow_io::{
    from_arrow_ipc, read_feather, to_arrow_ipc, to_arrow_ipc_multi, to_arrow_ipc_multi_batched,
    write_feather, write_feather_multi, write_feather_multi_batched, FeatherCompression,
    GeometryEncoding, TableRead, DEFAULT_RECORD_BATCH_SIZE,
};
pub use attributes::VectorRead;
pub use column::GeoColumn;
pub use datum::{grid_dirs, register_grids, set_grid_dirs, unregister_grids};
pub use deck::{deck_buffers, DeckBuffers};
pub use ewkb::{ewkb_to_iso, wkb_to_ewkb};
pub use flatgeobuf_io::{read_flatgeobuf, write_flatgeobuf};
pub use geojson_io::{read_geojson, read_geojson_str, write_geojson};
pub use gpkg_io::{
    gpkg_info, gpkg_layers, read_gpkg, write_gpkg, GpkgRead, GpkgWriteOptions, LayerInfo,
    LayerSummary,
};
pub use group_agg::GroupAgg;
pub use offset_curve::{OffsetJoin, OffsetParams, MAX_QUAD_SEGS};
pub use parity_e::{GridFeature, GridType};
pub use parity_i::{FlatVertexParts, PrecisionMode, VertexKind, VertexParts, VertexRing};
pub use parity_j::NO_CLEARANCE;
pub use parquet_io::{
    geoparquet_metadata, read_geoparquet, read_geoparquet_filtered, write_geoparquet,
    write_geoparquet_multi, write_geoparquet_with, GeoParquetRead, ParquetOptions,
};
pub use polygonize::Polygonized;
pub use predicates::Predicate;
pub use reproject::{
    crs_equivalent, crs_is_geographic, grid_status, normalize_crs, register_grids_for,
    transform_xy, unregister_grids_for,
};
pub use set_ops::SetOp;
pub use shapefile_io::{read_shapefile, shapefile_info, write_shapefile};
pub use sindex::{join_rows, JoinHow, JoinRows, NearestPairs, QueryPredicate, NO_ROW};
pub use stream_io::{open_batch_readers, BatchReader, StreamBatch};
pub use unary::{BufferOptions, Origin, UnaryOp};

#[cfg(test)]
mod affine_tests;
#[cfg(test)]
mod arrow_multi_tests;
#[cfg(test)]
mod buffer_zero_tests;
#[cfg(test)]
mod coverage_simplify_tests;
#[cfg(test)]
mod coverage_tests;
#[cfg(test)]
mod crs_convert_tests;
#[cfg(test)]
mod crs_esri_datum_tests;
#[cfg(test)]
mod crs_esri_projection_tests;
#[cfg(test)]
mod crs_null_operation_tests;
#[cfg(test)]
mod crs_parity_tests;
#[cfg(test)]
mod curves_tests;
#[cfg(test)]
mod datum_tests;
#[cfg(test)]
mod deck_tests;
#[cfg(test)]
mod empty_member_tests;
#[cfg(test)]
mod ewkb_tests;
#[cfg(test)]
mod flatgeobuf_tests;
#[cfg(test)]
mod geojson_stream_tests;
#[cfg(test)]
mod geojson_tests;
#[cfg(test)]
mod gpkg_tests;
#[cfg(test)]
mod group_agg_tests;
#[cfg(test)]
mod hardening_tests;
#[cfg(test)]
mod io_g1_tests;
#[cfg(test)]
mod offset_curve_tests;
#[cfg(test)]
mod parity_a_tests;
#[cfg(test)]
mod parity_b_tests;
#[cfg(test)]
mod parity_c_tests;
#[cfg(test)]
mod parity_e_tests;
#[cfg(test)]
mod parity_f_tests;
#[cfg(test)]
mod parity_h_tests;
#[cfg(test)]
mod parity_i_tests;
#[cfg(test)]
mod parity_j_tests;
#[cfg(test)]
mod parquet_multi_tests;
#[cfg(test)]
mod parquet_tests;
#[cfg(test)]
mod perf_bench_tests;
#[cfg(test)]
mod polygonize_tests;
#[cfg(test)]
mod predicates_tests;
#[cfg(test)]
mod projjson_tests;
#[cfg(test)]
mod reproject_tests;
#[cfg(test)]
mod set_ops_tests;
#[cfg(test)]
mod shapefile_tests;
#[cfg(test)]
mod sindex_tests;
#[cfg(test)]
mod stream_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod unary_tests;
#[cfg(test)]
mod validity_tests;
#[cfg(test)]
mod wkt_read_tests;
#[cfg(test)]
mod zm_io_tests;
#[cfg(test)]
mod zm_ops_tests;
#[cfg(test)]
mod zm_overlay_tests;
#[cfg(test)]
mod zm_tests;
