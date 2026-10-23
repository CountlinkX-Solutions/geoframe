//! Pure-Rust geometry core for GeoFrame. Holds no Rustler/BEAM dependencies so it can be
//! unit-tested with `cargo test` and reused outside Elixir.

mod column;

pub use column::GeoColumn;

#[cfg(test)]
mod tests;
