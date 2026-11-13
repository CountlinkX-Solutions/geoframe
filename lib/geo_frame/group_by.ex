defmodule GeoFrame.GroupBy do
  @moduledoc """
  `group_by/2`, `ungroup/2`, `groups/1`, `summarise/3` and `group_map/4` for
  `GeoFrame.GeoDataFrame`: grouped aggregation of attributes *and* geometry, and a function per group.

  ## Grouping state

  `group_by/2` is `Explorer.DataFrame.group_by/2` on the attribute frame: the groups live in
  `gdf.data` and `groups/1` reads them back. Grouped `head/2`, `tail/2`, `slice/3` and
  `sample/2` act per group as Explorer's do (`head(grouped, 1)` is the first row of *each*
  group; `head`, `tail` and `slice` return them in the original row order, as pandas'
  `groupby.head` does, while `sample` lists group after group in an unspecified order) and the geometry always follows the rows Explorer picked. Geometry columns cannot be grouping keys.

  ## `summarise/3`

  One row per group, in this order: the key columns, the aggregated attributes in the order
  given, then the geometry (the active geometry column, aggregated by the `:geometry`
  option) and any secondary geometry columns that were aggregated. The CRS of each geometry
  column is kept. Rows are aligned by dense group ids computed once on the Elixir side, so
  attribute and geometry row `i` always describe group `i`; every geometry aggregation runs
  in a single NIF call over all groups.

  `aggregations` is a keyword list (or map) `output_name: {function, source_column}`.

  Attribute columns: `:sum`, `:mean`, `:median`, `:min`, `:max`, `:count` (non-null values),
  `:first`, `:last` (the first / last *non-null* value, as pandas does; a group with only nulls
  gives null), `:n_distinct` (distinct non-null values, as pandas `nunique`: unlike Explorer's `n_distinct`, a
  null is not counted).

  `:first` and `:last` skip nulls too (pandas semantics), which differs from
  `Explorer.Series.first/1` and `Explorer.Series.last/1`: they return the first / last value even if it is
  null.

  Geometry columns (the active one or any secondary one): `:union_all`, `:intersection_all`,
  `:collect` (`GeoFrame.GeoSeries.collect/2` of the group), `:envelope` (the box of the group's
  total bounds), `:convex_hull` (of all the group's geometries), `:centroid` (the centroid of
  the group's union: pandas has no grouped centroid, so it is defined this way; null when the
  group mixes polygons, lines and points beyond what `union_all` supports), `:first`, `:last` (first / last non-null geometry)
  produce a secondary geometry column named `output_name`; `:count` (non-null geometries)
  produces an integer attribute. A group without geometries gives what the whole-series
  method gives for none of them (`GEOMETRYCOLLECTION EMPTY` for the unions, intersection and
  collect; null for the others).

  Options:

    * `:geometry` - the aggregation of the active geometry column: any geometry function above
      except `:count`. Defaults to `:union_all`, so `summarise(group_by(gdf, by), [])` has the
      keys and geometry of `GeoFrame.Dissolve.dissolve/2`.
    * `:dropna` - `true` (default) drops rows with a null key; `false` keeps them as a group.
    * `:sort` - `true` (default) orders groups by key (nulls last), `false` by first appearance.

  An ungrouped frame is one group. Raises `ArgumentError` for unknown columns or
  aggregations, a geometry function on an attribute (or the reverse), or an output name that
  repeats or clashes with a key or the geometry column.

  ## `group_map/4`

  See `group_map/4`: the equivalent of pandas `groupby(by).apply(fun)`.

  ## Example

      gdf
      |> GeoFrame.GeoDataFrame.group_by("region")
      |> GeoFrame.GeoDataFrame.summarise(
        population: {:sum, "pop"},
        n: {:count, "pop"},
        hull: {:convex_hull, "geometry"}
      )
  """

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.Dissolve
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @attribute_funcs [:sum, :mean, :median, :min, :max, :count, :first, :last, :n_distinct]
  @geometry_funcs [
    :union_all,
    :intersection_all,
    :collect,
    :envelope,
    :convex_hull,
    :centroid,
    :first,
    :last
  ]

  @doc "Marks the groups of `gdf`; see the module docs and `Explorer.DataFrame.group_by/2`."
  @spec group_by(GeoDataFrame.t(), term()) :: GeoDataFrame.t()
  def group_by(%GeoDataFrame{data: data} = gdf, columns) do
    geometry_columns = GeoDataFrame.geometry_columns(gdf)

    case Enum.find(named(columns), &(&1 in geometry_columns)) do
      nil -> :ok
      name -> raise ArgumentError, "cannot group by the geometry column #{inspect(name)}"
    end

    %{gdf | data: DataFrame.group_by(data, columns)}
  end

  @doc "Removes the groups (all, or the named ones); see `Explorer.DataFrame.ungroup/2`."
  @spec ungroup(GeoDataFrame.t(), term()) :: GeoDataFrame.t()
  def ungroup(%GeoDataFrame{data: data} = gdf, columns \\ ..) do
    %{gdf | data: DataFrame.ungroup(data, columns)}
  end

  @doc "The names of the grouping columns, in grouping order."
  @spec groups(GeoDataFrame.t()) :: [String.t()]
  def groups(%GeoDataFrame{data: data}), do: DataFrame.groups(data)

  @doc "Aggregates every group into one row; see the module docs."
  @spec summarise(GeoDataFrame.t(), keyword() | map(), keyword()) :: GeoDataFrame.t()
  def summarise(%GeoDataFrame{} = gdf, aggregations \\ [], opts \\ []) do
    GeoFrame.Options.validate!(opts, [:geometry, :dropna, :sort])
    by = groups(gdf)
    gdf = %{gdf | data: DataFrame.ungroup(gdf.data)}
    geometry_func = geometry_option!(Keyword.get(opts, :geometry, :union_all))
    specs = parse!(aggregations, gdf, by)

    gdf = if Keyword.get(opts, :dropna, true), do: Dissolve.drop_null_keys(gdf, by), else: gdf
    {gids, groups} = Dissolve.group_ids(gdf, by, Keyword.get(opts, :sort, true))
    n_groups = length(groups)

    geometry = GeoSeries.aggregate_groups(gdf.geometry, gids, n_groups, geometry_func)
    {attribute, geometric} = Enum.split_with(specs, &(&1.kind == :attribute))
    {counts, derived} = Enum.split_with(geometric, &(&1.func == :count))

    triples = for %{out: out, func: func, column: column} <- attribute, do: {out, func, column}
    data = Dissolve.aggregate(gdf.data, by, triples, gids)

    data =
      Enum.reduce(counts, data, fn %{out: out, column: column}, data ->
        put_column(data, out, count_non_null(geometry_series(gdf, column), gids, n_groups))
      end)

    secondary =
      for %{out: out, func: func, column: column} <- derived do
        {out, GeoSeries.aggregate_groups(geometry_series(gdf, column), gids, n_groups, func)}
      end

    ordered =
      by ++
        for %{kind: kind, func: func, out: out} <- specs,
            kind == :attribute or func == :count,
            do: out

    data = if ordered == [], do: data, else: DataFrame.select(data, ordered)

    %{gdf | data: data, geometry: geometry, secondary_geometries: secondary}
  end

  @doc """
  Calls `fun` once per group with the group's `GeoFrame.GeoDataFrame` and concatenates the
  results, like pandas `groupby(by).apply(fun)`.

  `by` is a column name or a list of names (as `group_by/2`); groups come in key order, with
  the options of `summarise/3`:

    * `:sort` - `true` (default) orders groups by key (nulls last), `false` by first appearance.
    * `:dropna` - `true` (default) skips rows with a null key; `false` makes them a group.

  The frame handed to `fun` is ungrouped, holds the rows of the group in their original order
  and **keeps the key columns** (unlike pandas 3, which removes them and puts the keys in the
  index). The result equals concatenating `fun` over `GeoFrame.GeoDataFrame.filter_with/2` per key.

  `fun` returns a `GeoFrame.GeoDataFrame` or an `Explorer.DataFrame`, and every group must
  return the same kind (a mix raises `ArgumentError`, as does any other value). GeoDataFrames must
  share the first group's CRS and geometry column name (and the same secondary geometry
  columns); a mismatch raises `ArgumentError` naming the group. The result is the
  concatenation (`GeoFrame.GeoDataFrame.concat_rows/1`, or `Explorer.DataFrame.concat_rows/1`),
  without groups.

  There is no index, so pandas' `group_keys=True` (the keys prepended as an index) becomes:
  key columns that `fun` dropped are **prepended as columns**, repeated for every row of
  the group; key columns it kept are left where they are. Groups for which `fun` returns no
  rows contribute nothing.

      GeoFrame.GeoDataFrame.group_map(gdf, "region", fn group ->
        GeoFrame.GeoDataFrame.head(group, 2)
      end)
  """
  @spec group_map(
          GeoDataFrame.t(),
          term(),
          (GeoDataFrame.t() -> GeoDataFrame.t() | DataFrame.t()),
          keyword()
        ) ::
          GeoDataFrame.t() | DataFrame.t()
  def group_map(%GeoDataFrame{} = gdf, by, fun, opts \\ []) when is_function(fun, 1) do
    GeoFrame.Options.validate!(opts, [:dropna, :sort])
    gdf = GeoDataFrame.ungroup(gdf)
    by = gdf |> group_by(by) |> groups()
    gdf = if Keyword.get(opts, :dropna, true), do: Dissolve.drop_null_keys(gdf, by), else: gdf
    {gids, keys} = Dissolve.group_ids(gdf, by, Keyword.get(opts, :sort, true))

    positions =
      gids
      |> Enum.with_index()
      |> Enum.group_by(&elem(&1, 0), &elem(&1, 1))

    results =
      for {key, id} <- Enum.with_index(keys) do
        group = GeoDataFrame.take(gdf, Map.fetch!(positions, id))
        {key, group |> fun.() |> add_keys(group, by)}
      end

    concat_results(results)
  end

  defp add_keys(%GeoDataFrame{data: data} = result, group, by),
    do: %{result | data: prepend_missing(data, group, by, GeoDataFrame.n_rows(result))}

  defp add_keys(%DataFrame{} = result, group, by),
    do: prepend_missing(result, group, by, DataFrame.n_rows(result))

  defp add_keys(other, _group, _by) do
    raise ArgumentError,
          "group_map function must return a GeoDataFrame or an Explorer.DataFrame, " <>
            "got: #{inspect(other)}"
  end

  defp prepend_missing(data, group, by, rows) do
    names = DataFrame.names(data)

    missing =
      for key <- by, key not in names do
        series = DataFrame.pull(group.data, key)
        value = series |> Series.to_list() |> hd()
        {key, Series.from_list(List.duplicate(value, rows), dtype: Series.dtype(series))}
      end

    cond do
      missing == [] ->
        data

      names == [] ->
        DataFrame.new(missing)

      true ->
        data
        |> then(&Enum.reduce(missing, &1, fn {k, s}, d -> DataFrame.put(d, k, s) end))
        |> DataFrame.select(Enum.map(missing, &elem(&1, 0)) ++ names)
    end
  end

  defp concat_results([]), do: DataFrame.new([])

  defp concat_results([{_, %GeoDataFrame{} = first} | _] = results) do
    crs = GeoDataFrame.crs(first)

    frames =
      for {key, result} <- results do
        case result do
          %GeoDataFrame{} ->
            if GeoDataFrame.crs(result) != crs do
              raise ArgumentError,
                    "group #{inspect(key)} returned a GeoDataFrame with a different CRS " <>
                      "(#{inspect(GeoDataFrame.crs(result))}) than the first group (#{inspect(crs)})"
            end

            if result.geometry_column != first.geometry_column do
              raise ArgumentError,
                    "group #{inspect(key)} returned a different geometry column " <>
                      "(#{inspect(result.geometry_column)}) than the first group " <>
                      "(#{inspect(first.geometry_column)})"
            end

            result

          _ ->
            raise mix_error(key)
        end
      end

    GeoDataFrame.concat_rows(frames)
  end

  defp concat_results([{_, %DataFrame{}} | _] = results) do
    results
    |> Enum.map(fn
      {_, %DataFrame{} = result} -> result
      {key, _} -> raise mix_error(key)
    end)
    |> DataFrame.concat_rows()
  end

  defp mix_error(key),
    do:
      ArgumentError.exception(
        "group_map function returned a mix of GeoDataFrames and Explorer dataframes " <>
          "(at group #{inspect(key)}); every group must return the same kind"
      )

  # -- specs ---------------------------------------------------------------------------------

  defp geometry_option!(func) when func in @geometry_funcs, do: func

  defp geometry_option!(other) do
    raise ArgumentError,
          "geometry must be one of #{inspect(@geometry_funcs)} (the aggregation of the " <>
            "geometry column), got #{inspect(other)}"
  end

  defp parse!(aggregations, gdf, by) when is_list(aggregations) or is_map(aggregations) do
    names = DataFrame.names(gdf.data)
    geometry_columns = GeoDataFrame.geometry_columns(gdf)

    specs =
      for item <- aggregations do
        {out, func, column} = split!(item)
        column = to_string(column)
        kind = classify!(func, column, names, geometry_columns)
        %{out: to_string(out), func: func, column: column, kind: kind}
      end

    check_outputs!(specs, by, geometry_columns)
    specs
  end

  defp split!({out, {func, column}}) when is_atom(func), do: {out, func, column}

  defp split!(other) do
    raise ArgumentError,
          "each aggregation must be output_name: {function, column}, got #{inspect(other)}"
  end

  defp classify!(func, column, names, geometry_columns) do
    cond do
      column in geometry_columns and (func in @geometry_funcs or func == :count) ->
        :geometry

      column in geometry_columns ->
        raise ArgumentError,
              "aggregation #{inspect(func)} cannot aggregate the geometry column " <>
                "#{inspect(column)}; use one of #{inspect([:count | @geometry_funcs])}"

      column not in names ->
        raise ArgumentError, "column #{inspect(column)} not found"

      func in @attribute_funcs ->
        :attribute

      true ->
        raise ArgumentError,
              "unknown aggregation #{inspect(func)} for the attribute column #{inspect(column)}; " <>
                "use one of #{inspect(@attribute_funcs)}"
    end
  end

  defp check_outputs!(specs, by, geometry_columns) do
    outs = Enum.map(specs, & &1.out)

    Enum.each(outs, fn out ->
      cond do
        out in by ->
          raise ArgumentError, "output name #{inspect(out)} is a grouping key"

        out == hd(geometry_columns) ->
          raise ArgumentError,
                "output name #{inspect(out)} is the active geometry column; set it with :geometry"

        true ->
          :ok
      end
    end)

    if Enum.uniq(outs) != outs do
      raise ArgumentError, "output names repeat: #{inspect(outs -- Enum.uniq(outs))}"
    end
  end

  defp geometry_series(%GeoDataFrame{geometry_column: name, geometry: geometry}, name),
    do: geometry

  defp geometry_series(%GeoDataFrame{secondary_geometries: secondary}, name),
    do: secondary |> List.keyfind(name, 0) |> elem(1)

  # Non-null geometries per group, as an integer column.
  defp count_non_null(series, gids, n_groups) do
    counts =
      series
      |> GeoSeries.isna()
      |> Series.to_list()
      |> Enum.zip(gids)
      |> Enum.reject(&elem(&1, 0))
      |> Enum.frequencies_by(&elem(&1, 1))

    Series.from_list(Enum.map(0..(n_groups - 1)//1, &Map.get(counts, &1, 0)), dtype: {:s, 64})
  end

  defp put_column(data, name, series) do
    if DataFrame.names(data) == [],
      do: DataFrame.new([{name, series}]),
      else: DataFrame.put(data, name, series)
  end

  defp named(columns) when is_list(columns), do: Enum.flat_map(columns, &named/1)
  defp named(column) when is_binary(column), do: [column]
  defp named(column) when is_atom(column) and not is_nil(column), do: [Atom.to_string(column)]
  defp named(_), do: []
end
