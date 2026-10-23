#!/usr/bin/env bash
# Dev-only: write sample files with GeoFrame and read them back with GeoPandas / pyarrow.
# Needs the reference venv (see test/reference/README.md); CI does not run this.
set -euo pipefail
cd "$(dirname "$0")/.."
python="${GEOPANDAS_PYTHON:-$HOME/.venvs/geopandas-ref/bin/python}"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
mix run scripts/write_io_samples.exs "$dir"
"$python" -I test/reference/check_geoframe_io.py "$dir"
