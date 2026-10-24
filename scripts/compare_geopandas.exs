# The GeoFrame side of the GeoPandas comparison, without Livebook or Python.
#
# It runs the Elixir cells of notebooks/compare.livemd, so this is the same code as the
# notebook, not a copy that could drift. The cells marked `# notebook-only` (the comparison
# tables and the maps) are skipped. Each analysis leaves its metrics in a variable named
# `gf_<analysis>`; the keys and the shape are those examples/geopandas/compare.py prints.
#
#     elixir scripts/compare_geopandas.exs                      # JSON on stdout
#     elixir scripts/compare_geopandas.exs --out gf.json        # JSON in a file
#     elixir scripts/compare_geopandas.exs --compare py.json    # table; exit 1 on any FAIL
#
# scripts/compare_geopandas.sh runs GeoPandas and this script and compares them.

{opts, _, _} = OptionParser.parse(System.argv(), strict: [out: :string, compare: :string])

notebook = Path.expand("../notebooks/compare.livemd", __DIR__)

cells =
  ~r/^```elixir[^\n]*\n(.*?)^```\s*$/ms
  |> Regex.scan(File.read!(notebook), capture: :all_but_first)
  |> Enum.map(fn [code] -> code end)
  |> Enum.reject(&String.starts_with?(&1, "# notebook-only"))

env = Code.env_for_eval(file: notebook)

binding =
  Enum.reduce(cells, {[], env}, fn code, {binding, env} ->
    {_value, binding, env} =
      code
      |> Code.string_to_quoted!(file: notebook)
      |> Code.eval_quoted_with_env(binding, env)

    {binding, env}
  end)
  |> elem(0)

analyses =
  ~r/^# --- analysis: (\w+)/m
  |> Regex.scan(File.read!(Path.expand("../examples/geopandas/compare.py", __DIR__)),
    capture: :all_but_first
  )
  |> List.flatten()

geoframe =
  Map.new(analyses, fn key -> {key, Keyword.fetch!(binding, String.to_atom("gf_" <> key))} end)

if opts[:out], do: File.write!(opts[:out], Jason.encode!(geoframe, pretty: true))

case opts[:compare] do
  nil ->
    unless opts[:out], do: IO.puts(Jason.encode!(geoframe, pretty: true))

  path ->
    geopandas = path |> File.read!() |> Jason.decode!()
    rows = GeoPandasCompare.rows(Map.take(geoframe, Map.keys(geopandas)), geopandas)
    IO.puts(GeoPandasCompare.format(rows))
    failed = Enum.count(rows, &(&1.status == "FAIL"))
    IO.puts("\n#{length(rows) - failed} PASS, #{failed} FAIL")
    if failed > 0, do: System.halt(1)
end
