#!/usr/bin/env bash
# Dev-only: GeoFrame <-> GeoPandas through a real PostGIS database.
# Needs GEOFRAME_POSTGIS_URL and the reference venv with sqlalchemy and psycopg.
set -euo pipefail
cd "$(dirname "$0")/.."
python="${GEOPANDAS_PYTHON:-$HOME/.venvs/geopandas-ref/bin/python}"
: "${GEOFRAME_POSTGIS_URL:?set GEOFRAME_POSTGIS_URL}"
MIX_ENV=test mix run scripts/crosscheck_postgis.exs write
"$python" -I scripts/check_postgis.py "$GEOFRAME_POSTGIS_URL" read
"$python" -I scripts/check_postgis.py "$GEOFRAME_POSTGIS_URL" write
MIX_ENV=test mix run scripts/crosscheck_postgis.exs read
