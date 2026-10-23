//! Pure-Rust geometry core for GeoFrame. Holds no Rustler/BEAM dependencies so it can be
//! unit-tested with `cargo test` and reused outside Elixir.

mod affine;
mod arrow_io;
mod atomic;
mod attributes;
pub(crate) mod column;
mod crs_wkt;
mod dbf;
mod ewkb;
mod flatgeobuf_io;
mod geojson_io;
mod gpkg_io;
mod line_merge;
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
mod projjson;
mod reproject;
mod set_ops;
mod shapefile_io;
mod sindex;
mod unary;

pub use arrow_io::{
    from_arrow_ipc, read_feather, to_arrow_ipc, write_feather, FeatherCompression,
    GeometryEncoding, TableRead,
};
pub use attributes::VectorRead;
pub use column::GeoColumn;
pub use ewkb::{ewkb_to_iso, wkb_to_ewkb};
pub use flatgeobuf_io::{read_flatgeobuf, write_flatgeobuf};
pub use geojson_io::{read_geojson, read_geojson_str, write_geojson};
pub use gpkg_io::{
    gpkg_info, gpkg_layers, read_gpkg, write_gpkg, GpkgRead, GpkgWriteOptions, LayerInfo,
    LayerSummary,
};
pub use offset_curve::{OffsetJoin, OffsetParams, MAX_QUAD_SEGS};
pub use parity_e::{GridFeature, GridType};
pub use parity_i::{PrecisionMode, VertexKind, VertexParts};
pub use parity_j::NO_CLEARANCE;
pub use parquet_io::{
    geoparquet_metadata, read_geoparquet, read_geoparquet_filtered, write_geoparquet,
    write_geoparquet_with, GeoParquetRead, ParquetOptions,
};
pub use polygonize::Polygonized;
pub use predicates::Predicate;
pub use reproject::{crs_equivalent, crs_is_geographic, normalize_crs};
pub use set_ops::SetOp;
pub use shapefile_io::{read_shapefile, shapefile_info, write_shapefile};
pub use sindex::{NearestPairs, QueryPredicate};
pub use unary::{BufferOptions, Origin, UnaryOp};

#[cfg(test)]
mod affine_tests;
#[cfg(test)]
mod ewkb_tests;
#[cfg(test)]
mod flatgeobuf_tests;
#[cfg(test)]
mod geojson_tests;
#[cfg(test)]
mod gpkg_tests;
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
mod parquet_tests;
#[cfg(test)]
mod polygonize_tests;
#[cfg(test)]
mod predicates_tests;
#[cfg(test)]
mod reproject_tests;
#[cfg(test)]
mod set_ops_tests;
#[cfg(test)]
mod shapefile_tests;
#[cfg(test)]
mod sindex_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod unary_tests;
