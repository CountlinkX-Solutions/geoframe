//! Pure-Rust geometry core for GeoFrame. Holds no Rustler/BEAM dependencies so it can be
//! unit-tested with `cargo test` and reused outside Elixir.

mod column;
mod parquet_io;

pub use column::GeoColumn;
pub use parquet_io::{read_geoparquet, write_geoparquet, GeoParquetRead};

#[cfg(test)]
mod parquet_tests;
#[cfg(test)]
mod tests;
