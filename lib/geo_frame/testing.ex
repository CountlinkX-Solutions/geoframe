defmodule GeoFrame.Testing do
  @moduledoc """
  Assertions for comparing geometry in tests, like `geopandas.testing`.

  Both functions raise `ExUnit.AssertionError` (part of Elixir core, so there is no compile
  time dependency on the test framework) with a readable report of the first differing rows,
  and return `:ok` otherwise.

      import GeoFrame.Testing

      assert_geoseries_equal(result, expected)
      assert_geodataframe_equal(result, expected, check_less_precise: true)

  ## How geometries are compared

  GeoPandas compares with topological `geom_equals` by default and with a tolerance when
  `check_less_precise` is set; GeoFrame makes the same choice:

    * default - `GeoFrame.GeoSeries.geom_equals/2`: the same point set, so vertex order, start
      vertex and ring orientation do not matter;
    * `check_less_precise: true` - `GeoFrame.GeoSeries.geom_equals_exact/3` with a tolerance
      of `1.0e-6`, which does care about vertex order;
    * `normalize: true` - always the topological comparison, which is already independent of
      vertex order (GeoFrame has no `normalize` operation), so it wins over
      `check_less_precise`.

  Two null rows are equal; a null row never equals a geometry. Two rows that both hold the
  same empty geometry are equal.

  ## Options

    * `:check_crs` - also require equal CRS (default `true`).
    * `:check_less_precise` - tolerance comparison (default `false`). For frames it also lets
      float attribute values differ by `1.0e-6`.
    * `:check_geom_type` - also require equal geometry types per row (default `false`).
    * `:normalize` - see above (default `false`).
    * `:check_dtype` - frames only: require equal attribute dtypes (default `true`).
  """

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @tolerance 1.0e-6
  @shown_rows 5
  @series_opts [:check_crs, :check_less_precise, :check_geom_type, :normalize]

  @doc """
  Asserts two `GeoFrame.GeoSeries` are equal. See the module documentation for options.

  ## Examples

      iex> a = GeoFrame.GeoSeries.from_wkt!(["POINT (1 1)", nil])
      iex> b = GeoFrame.GeoSeries.from_wkt!(["POINT (1 1)", nil])
      iex> GeoFrame.Testing.assert_geoseries_equal(a, b)
      :ok
  """
  @spec assert_geoseries_equal(GeoSeries.t(), GeoSeries.t(), keyword()) :: :ok
  def assert_geoseries_equal(%GeoSeries{} = left, %GeoSeries{} = right, opts \\ []) do
    opts = validate!(opts, @series_opts)
    fail_unless(series_problems(left, right, opts), "GeoSeries are different")
  end

  @doc """
  Asserts two `GeoFrame.GeoDataFrame` are equal: same attribute and geometry column names in
  order, equal attributes, equal geometries. Secondary geometry columns
  (`GeoFrame.GeoDataFrame.put_geometry/3`) are compared like the active one, each with its own
  CRS. See the module documentation for options.

  ## Examples

      iex> geometry = GeoFrame.GeoSeries.from_wkt!(["POINT (1 1)"])
      iex> gdf = GeoFrame.GeoDataFrame.new(Explorer.DataFrame.new(id: [1]), geometry)
      iex> GeoFrame.Testing.assert_geodataframe_equal(gdf, gdf)
      :ok
  """
  @spec assert_geodataframe_equal(GeoDataFrame.t(), GeoDataFrame.t(), keyword()) :: :ok
  def assert_geodataframe_equal(%GeoDataFrame{} = left, %GeoDataFrame{} = right, opts \\ []) do
    opts = validate!(opts, @series_opts ++ [:check_dtype])
    fail_unless(frame_problems(left, right, opts), "GeoDataFrames are different")
  end

  # -- series -----------------------------------------------------------------------------

  defp series_problems(left, right, opts) do
    if GeoSeries.size(left) == GeoSeries.size(right) do
      crs_problems(left, right, opts) ++ geometry_problems(left, right, opts)
    else
      [
        "length differs: left has #{GeoSeries.size(left)} rows, right has #{GeoSeries.size(right)}"
      ]
    end
  end

  defp crs_problems(left, right, opts) do
    if opts[:check_crs] and GeoSeries.crs(left) != GeoSeries.crs(right) do
      [
        "CRS differs: left is #{inspect(GeoSeries.crs(left))}, right is #{inspect(GeoSeries.crs(right))}"
      ]
    else
      []
    end
  end

  defp geometry_problems(left, right, opts) do
    # Comparing needs equal CRS or at least one unset; with `check_crs: false` the CRS is
    # dropped so two different ones can still be compared.
    {l, r} = {GeoSeries.set_crs(left, nil), GeoSeries.set_crs(right, nil)}
    {lw, rw} = {GeoSeries.to_wkt(l), GeoSeries.to_wkt(r)}
    mask = equality_mask(l, r, opts)

    differing =
      [lw, rw, mask]
      |> Enum.zip()
      |> Enum.with_index()
      |> Enum.filter(fn {{a, b, equal}, _} -> not row_equal?(a, b, equal) end)

    type_differing =
      if opts[:check_geom_type] do
        types = Enum.zip(types(l), types(r))

        for {{a, b}, index} <- Enum.with_index(types), a != b, do: {index, a, b}
      else
        []
      end

    geometry_report(differing) ++ type_report(type_differing)
  end

  defp equality_mask(left, right, opts) do
    series =
      if opts[:check_less_precise] and not opts[:normalize],
        do: GeoSeries.geom_equals_exact(left, right, @tolerance),
        else: GeoSeries.geom_equals(left, right)

    Series.to_list(series)
  end

  # `mask` is nil when either side is null or empty; then the stored text decides.
  defp row_equal?(_a, _b, true), do: true
  defp row_equal?(a, b, nil), do: a == b
  defp row_equal?(_a, _b, false), do: false

  defp types(series), do: series |> GeoSeries.geom_type() |> Series.to_list()

  defp geometry_report([]), do: []

  defp geometry_report(differing) do
    rows =
      differing
      |> Enum.take(@shown_rows)
      |> Enum.map(fn {{a, b, _}, index} ->
        "  row #{index}: left #{show(a)}, right #{show(b)}"
      end)

    ["geometries differ in #{length(differing)} row(s)" | rows ++ more(differing)]
  end

  defp type_report([]), do: []

  defp type_report(differing) do
    rows =
      differing
      |> Enum.take(@shown_rows)
      |> Enum.map(fn {index, a, b} -> "  row #{index}: left #{show(a)}, right #{show(b)}" end)

    ["geometry type differs in #{length(differing)} row(s)" | rows ++ more(differing)]
  end

  defp more(list) when length(list) > @shown_rows,
    do: ["  ... #{length(list) - @shown_rows} more"]

  defp more(_), do: []

  defp show(nil), do: "nil"
  defp show(text), do: text

  # -- frames -----------------------------------------------------------------------------

  defp frame_problems(left, right, opts) do
    names = {GeoDataFrame.names(left), GeoDataFrame.names(right)}

    cond do
      GeoDataFrame.n_rows(left) != GeoDataFrame.n_rows(right) ->
        [
          "length differs: left has #{GeoDataFrame.n_rows(left)} rows, " <>
            "right has #{GeoDataFrame.n_rows(right)}"
        ]

      elem(names, 0) != elem(names, 1) ->
        ["columns differ: left #{inspect(elem(names, 0))}, right #{inspect(elem(names, 1))}"]

      true ->
        dtype_problems(left, right, opts) ++
          attribute_problems(left, right, opts) ++
          series_problems(left.geometry, right.geometry, opts) ++
          secondary_problems(left, right, opts)
    end
  end

  # Every secondary geometry column, matched by name (the names and their order were compared
  # with the column names): same rows, CRS and geometries, as GeoPandas does for each
  # geometry-dtype column.
  defp secondary_problems(left, right, opts) do
    for {name, series} <- left.secondary_geometries,
        {^name, other} = List.keyfind(right.secondary_geometries, name, 0),
        problem <- series_problems(series, other, opts) do
      "geometry column #{inspect(name)}: #{problem}"
    end
  end

  defp dtype_problems(left, right, opts) do
    if opts[:check_dtype] do
      for {name, dtype} <- DataFrame.dtypes(left.data),
          other = Map.fetch!(DataFrame.dtypes(right.data), name),
          dtype != other do
        "dtype of column #{inspect(name)} differs: left #{inspect(dtype)}, right #{inspect(other)}"
      end
    else
      []
    end
  end

  defp attribute_problems(left, right, opts) do
    for name <- DataFrame.names(left.data),
        report = column_report(name, left.data, right.data, opts),
        report != [],
        line <- report do
      line
    end
  end

  defp column_report(name, left, right, opts) do
    values = Enum.zip(column(left, name), column(right, name)) |> Enum.with_index()
    differing = Enum.reject(values, fn {{a, b}, _} -> value_equal?(a, b, opts) end)

    if differing == [] do
      []
    else
      rows =
        differing
        |> Enum.take(@shown_rows)
        |> Enum.map(fn {{a, b}, index} ->
          "  row #{index}: left #{inspect(a)}, right #{inspect(b)}"
        end)

      ["column #{inspect(name)} differs in #{length(differing)} row(s)" | rows ++ more(differing)]
    end
  end

  defp column(data, name), do: data |> DataFrame.pull(name) |> Series.to_list()

  defp value_equal?(a, b, opts) do
    cond do
      a == b -> true
      opts[:check_less_precise] and is_number(a) and is_number(b) -> abs(a - b) <= @tolerance
      true -> false
    end
  end

  # -- plumbing ---------------------------------------------------------------------------

  defp validate!(opts, allowed) do
    defaults = [
      check_crs: true,
      check_less_precise: false,
      check_geom_type: false,
      normalize: false,
      check_dtype: true
    ]

    opts = Keyword.validate!(opts, Keyword.take(defaults, allowed))
    Keyword.merge(defaults, opts)
  end

  defp fail_unless([], _title), do: :ok

  defp fail_unless(problems, title) do
    raise ExUnit.AssertionError, message: Enum.join([title | problems], "\n")
  end
end
