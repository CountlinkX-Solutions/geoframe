//! Pure-Rust geometry core for GeoFrame. Holds no Rustler/BEAM dependencies so it can be
//! unit-tested with `cargo test` and reused outside Elixir.

mod attributes;
pub(crate) mod column;
mod flatgeobuf_io;
mod geojson_io;
mod parquet_io;
mod predicates;
mod reproject;
mod set_ops;
mod sindex;
mod unary;

pub use attributes::VectorRead;
pub use column::GeoColumn;
pub use flatgeobuf_io::{read_flatgeobuf, write_flatgeobuf};
pub use geojson_io::{read_geojson, write_geojson};
pub use parquet_io::{read_geoparquet, write_geoparquet, GeoParquetRead};
pub use predicates::Predicate;
pub use reproject::normalize_crs;
pub use set_ops::SetOp;
pub use sindex::{NearestPairs, QueryPredicate};
pub use unary::{BufferOptions, Origin, UnaryOp};

#[cfg(test)]
mod flatgeobuf_tests;
#[cfg(test)]
mod geojson_tests;
#[cfg(test)]
mod parquet_tests;
#[cfg(test)]
mod predicates_tests;
#[cfg(test)]
mod reproject_tests;
#[cfg(test)]
mod set_ops_tests;
#[cfg(test)]
mod sindex_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod unary_tests;
