# Contributing to GeoFrame

Thank you for your interest in GeoFrame. Bug reports, documentation fixes and code changes are
all welcome. Everyone taking part is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md). Security issues are reported privately, as described in
[SECURITY.md](SECURITY.md), never in a public issue.

## Reporting bugs

Open an issue with the **Bug report** template. The most useful reports include:

- the GeoFrame version (release tag or commit SHA);
- the Elixir and OTP versions (`elixir --version`);
- the operating system and target (for example `x86_64-unknown-linux-gnu`);
- whether the NIF was built from source or downloaded precompiled;
- a minimal reproduction: a few lines of Elixir and, when a file is involved, the smallest input
  that shows the problem;
- when the result differs from GeoPandas, what GeoPandas / Shapely returns for the same input.

Check [docs/parity.md](docs/parity.md) first: known differences from GeoPandas are listed there.

For a new feature, open an issue with the **Feature request** template before starting larger
work, so the API can be discussed first.

## Development setup

You need:

- **Elixir and Erlang/OTP**: the versions pinned in `.tool-versions` (Elixir 1.19 / OTP 28).
  CI also tests Elixir 1.17 / OTP 27, the oldest pair it covers.
- **Rust**: the stable toolchain with the `rustfmt` and `clippy` components
  (`rustup component add rustfmt clippy`). A checkout always builds the NIF from source in
  `:dev` and `:test`, through Rustler, so no precompiled download is involved.
- **Docker** (optional): only to run the PostGIS tests locally.

Then fetch the dependencies and run the tests:

```sh
mix deps.get
mix test
```

## Checks

CI (`.github/workflows/ci.yml`) runs the following with `MIX_ENV=test`. Run them before opening a
pull request.

Elixir:

```sh
mix format --check-formatted
mix compile --warnings-as-errors
mix credo --strict
mix test
mix docs --warnings-as-errors
```

PostGIS tests are excluded unless a database is given:

```sh
docker run -d --name geoframe-postgis -e POSTGRES_PASSWORD=geoframe -p 55432:5432 postgis/postgis:17-3.5
GEOFRAME_POSTGIS_URL=postgres://postgres:geoframe@127.0.0.1:55432/postgres mix test --only postgis
```

Rust (from the repository root, which holds the Cargo workspace):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI also runs the code cells of every notebook, which needs network access:

```sh
MIX_ENV=dev elixir scripts/run_livemd.exs notebooks/tour.livemd
```

CI runs the formatter check, Credo, the docs build and the notebooks on Elixir 1.19 only (older
formatters disagree); compilation and the tests run on every Elixir / OTP pair.

## Project layout

- `lib/`: the Elixir API (`GeoFrame`, `GeoFrame.GeoDataFrame`, `GeoFrame.GeoSeries`, ...).
- `native/geoframe_core`: the Rust crate with all geometry, CRS and I/O logic, and its tests.
- `native/geoframe_native`: the thin Rustler NIF wrapper around `geoframe_core`.
- `test/reference`: the GeoPandas reference suite. Its generators are dev-only Python scripts;
  CI compares against the committed fixtures without Python. See
  [test/reference/README.md](test/reference/README.md).
- `docs/parity.md`: every known difference from GeoPandas, with its tests.

## Pull requests

- **One change per pull request.** Keep unrelated fixes, refactors and formatting in separate
  pull requests, so each can be reviewed and reverted on its own.
- **Tests and docs go with the behaviour.** A change in behaviour comes with tests (Elixir, Rust
  or both) and the matching documentation: function docs, the README and, when the result differs
  from GeoPandas, `docs/parity.md` and `test/reference/known_deviations.exs`.
- **Changelog.** Add an entry under `[Unreleased]` in [CHANGELOG.md](CHANGELOG.md) for any
  user-visible change.
- **Conventional Commits.** Commit messages follow
  [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/), for example
  `fix(wkt): write and read empty members as GEOS` or `docs: correct the buffer example`.
  Common types in this repository are `feat`, `fix`, `docs`, `test`, `refactor`, `ci` and
  `chore`.
- **CI must pass.** All checks above run on every pull request.

## License

GeoFrame is licensed under the [Apache License 2.0](LICENSE). As stated in section 5 of the
license, any contribution you submit for inclusion is licensed under the same terms.
