#!/usr/bin/env bash
# Runs the same ten analyses in GeoPandas and in GeoFrame and prints a PASS/FAIL table.
#
#     scripts/compare_geopandas.sh
#
# GeoPandas: examples/geopandas/compare.py, run with the reference venv (override with PYTHON).
# GeoFrame:  scripts/compare_geopandas.exs, which runs the Elixir cells of
#            notebooks/compare.livemd. Tolerances: examples/geopandas/tolerances.json.
# Exits 1 when a metric differs by more than its tolerance, or when the Python blocks of the
# notebook have drifted from examples/geopandas/compare.py.
set -euo pipefail

cd "$(dirname "$0")/.."
PYTHON="${PYTHON:-$HOME/.venvs/geopandas-ref/bin/python}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The notebook's Python cells must contain the code of compare.py verbatim.
"$PYTHON" -I - <<'PY'
import re, sys

notebook = open("notebooks/compare.livemd").read()
script = open("examples/geopandas/compare.py").read()
blocks = re.findall(r"^```python\n(.*?)^```", notebook, re.S | re.M)
chunks = {
    m.group(1): m.group(2).strip("\n")
    for m in re.finditer(r"^# --- (\w+(?:: \w+)?)\n(.*?)(?=^# --- |\Z)", script, re.S | re.M)
    if m.group(1) != "main"
}
missing = [name for name, chunk in chunks.items() if not any(chunk in block for block in blocks)]
if missing:
    sys.exit(f"notebooks/compare.livemd has drifted from examples/geopandas/compare.py: {missing}")
PY

"$PYTHON" -I examples/geopandas/compare.py > "$work/geopandas.json"
elixir scripts/compare_geopandas.exs --compare "$work/geopandas.json"
