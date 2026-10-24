defmodule GeoPandasCompare do
  @moduledoc """
  Compares the metrics GeoFrame and GeoPandas computed for the same analyses.

  Both sides are maps `%{analysis => %{metric => value}}` where a value is a number, a string
  or a list of those (the shape `examples/geopandas/compare.py` prints as JSON). Tolerances
  are declared once, in `tolerances.json` next to this file:

    * integers and strings must be equal;
    * floats must agree within `default_float_rel`, a relative difference measured against
      `max(|a|, |b|, 1)` (so a value near zero is compared absolutely);
    * `overrides` maps `"analysis.metric"` to a looser `rel`, with the reason in `why`.
  """

  @tolerances_path Path.join(__DIR__, "tolerances.json")

  @doc "Compares two result maps; returns one row per metric, sorted by analysis and metric."
  def rows(geoframe, geopandas) do
    tolerances = @tolerances_path |> File.read!() |> Jason.decode!()

    for analysis <- keys(geoframe, geopandas),
        metric <- keys(geoframe[analysis] || %{}, geopandas[analysis] || %{}) do
      a = get_in(geoframe, [analysis, metric])
      b = get_in(geopandas, [analysis, metric])
      override = get_in(tolerances, ["overrides", "#{analysis}.#{metric}"]) || %{}
      allowed = override["rel"] || if(exact?(b), do: 0, else: tolerances["default_float_rel"])
      diff = difference(a, b)

      %{
        analysis: analysis,
        metric: metric,
        geoframe: a,
        geopandas: b,
        rel_diff: diff,
        tolerance: allowed,
        status: status(diff, allowed),
        note: override["why"]
      }
    end
  end

  @doc "Whether every row passed."
  def all_pass?(rows), do: Enum.all?(rows, &(&1.status == "PASS"))

  @doc "The rows as an `Explorer.DataFrame` (values shortened for display)."
  def dataframe(rows) do
    Explorer.DataFrame.new(
      analysis: Enum.map(rows, & &1.analysis),
      metric: Enum.map(rows, & &1.metric),
      geoframe: Enum.map(rows, &short(&1.geoframe)),
      geopandas: Enum.map(rows, &short(&1.geopandas)),
      rel_diff: Enum.map(rows, &(&1.rel_diff || 1.0)),
      status: Enum.map(rows, & &1.status)
    )
  end

  @doc "The rows as a plain-text table."
  def format(rows) do
    header = ["analysis", "metric", "GeoFrame", "GeoPandas", "rel diff", "tol", "status"]

    body =
      for r <- rows do
        [
          r.analysis,
          r.metric,
          short(r.geoframe),
          short(r.geopandas),
          if(r.rel_diff,
            do: :erlang.float_to_binary(r.rel_diff / 1, [{:scientific, 1}]),
            else: "-"
          ),
          if(r.tolerance == 0,
            do: "exact",
            else: :erlang.float_to_binary(r.tolerance / 1, [{:scientific, 0}])
          ),
          r.status
        ]
      end

    widths =
      [header | body]
      |> Enum.zip_with(fn column -> column |> Enum.map(&String.length/1) |> Enum.max() end)

    [header, Enum.map(widths, &String.duplicate("-", &1)) | body]
    |> Enum.map_join("\n", fn row ->
      row
      |> Enum.zip(widths)
      |> Enum.map_join("  ", fn {cell, w} -> String.pad_trailing(cell, w) end)
    end)
  end

  defp exact?(value) when is_list(value), do: Enum.all?(value, &exact?/1)
  defp exact?(value), do: is_integer(value) or is_binary(value)

  defp keys(a, b), do: (Map.keys(a) ++ Map.keys(b)) |> Enum.uniq() |> Enum.sort()

  # Relative difference, or `nil` when the values are not comparable (different type or length).
  defp difference(a, b) when is_number(a) and is_number(b),
    do: abs(a - b) / Enum.max([abs(a), abs(b), 1.0])

  defp difference(a, b) when is_binary(a) and is_binary(b), do: if(a == b, do: 0.0, else: 1.0)

  defp difference(a, b) when is_list(a) and is_list(b) and length(a) == length(b),
    do:
      a
      |> Enum.zip(b)
      |> Enum.map(fn {x, y} -> difference(x, y) || 1.0 end)
      |> Enum.max(fn -> 0.0 end)

  defp difference(_a, _b), do: nil

  defp status(nil, _allowed), do: "FAIL"
  defp status(diff, allowed), do: if(diff <= allowed, do: "PASS", else: "FAIL")

  defp short(value) when is_float(value), do: :erlang.float_to_binary(value, [{:decimals, 3}])
  defp short(value) when is_binary(value), do: value
  defp short(nil), do: "(missing)"

  defp short(value) when is_list(value) do
    shown = value |> Enum.take(3) |> Enum.map_join(", ", &short/1)
    if length(value) > 3, do: "[#{shown}, ... #{length(value)}]", else: "[#{shown}]"
  end

  defp short(value), do: to_string(value)
end
