# geoframe_native

The Rustler NIF crate behind `GeoFrame.Native`. It is a thin wrapper around
[`geoframe_core`](../geoframe_core): it converts between BEAM terms and core types, holds
geometry columns and streaming batch readers as Rustler resources, and registers its functions
under `Elixir.GeoFrame.Native`. All geometry, CRS and I/O logic, and its tests, live in
`geoframe_core`.

- Built as a `cdylib` (`libgeoframe_native`); not published to crates.io (`publish = false`).
- Long-running calls run on dirty CPU or dirty I/O schedulers.
- It has no Rust tests: NIF symbols (`enif_*`) only resolve inside the BEAM, so the crate is
  exercised by the Elixir test suite (`mix test`).
- A checkout or a git or path dependency builds it from source through Rustler. Releases attach
  precompiled builds of it for `rustler_precompiled`; see the main [README](../../README.md).
