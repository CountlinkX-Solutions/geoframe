defmodule GeoFrame.Dissolve do
  @moduledoc """
  `dissolve/2`: merge rows that share a key into one row, like `geopandas.GeoDataFrame.dissolve`.

  ## How rows stay aligned

  Group ids are computed once on the Elixir side from the key columns, one dense id per row,
  numbered in result order (sorted key order, or first appearance with `sort: false`). The
  same ids drive both halves:

    * the geometry: `GeoFrame.GeoSeries.union_groups/3` unions each group in one NIF call and
      returns one row per id;
    * the attributes: a temporary id column is grouped and summarised with Explorer, then
      sorted by id.

  So result row `i` of the attributes and of the geometry always describe group `i`.

  ## Result

  Group keys are kept as leading columns (GeoPandas makes them the index), followed by the
  remaining attributes. Nulls sort last, as in GeoPandas.
  """

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  require DataFrame

  @aggfuncs [:first, :last, :sum, :mean, :min, :max, :count]
  @group_col "__geoframe_group__"

  @doc """
  Dissolves `gdf` by key columns.

  Options:

    * `:by` - a column name (string or atom) or a list of names; `nil` (default) dissolves
      everything into a single row.
    * `:aggfunc` - `:first` (default), `:last`, `:sum`, `:mean`, `:min`, `:max` or `:count`,
      applied to every non-key attribute (numeric functions raise on
      non-numeric columns, so list the columns explicitly for mixed frames); or a keyword list / map of column to function, in
      which case only the listed columns are kept (after the keys).
    * `:dropna` - `true` (default) drops rows with a null key; `false` keeps them as a group.
    * `:sort` - `true` (default) orders groups by key, `false` by first appearance.

  Geometries of a group are merged with `GeoFrame.GeoSeries.union_all/1` semantics (null
  geometries skipped). The CRS is preserved. Raises `ArgumentError` for unknown columns or
  aggregation functions, or when a key is the geometry column.
  """
  @spec dissolve(GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  def dissolve(%GeoDataFrame{} = gdf, opts \\ []) do
    by = opts |> Keyword.get(:by) |> List.wrap() |> Enum.map(&to_string/1)
    names = DataFrame.names(gdf.data)
    check_by!(by, names, gdf.geometry_column)
    aggs = aggregations(Keyword.get(opts, :aggfunc, :first), by, names)

    gdf = if Keyword.get(opts, :dropna, true), do: drop_null_keys(gdf, by), else: gdf

    if GeoDataFrame.n_rows(gdf) == 0 do
      gdf
    else
      {gids, n_groups} = group_ids(gdf, by, Keyword.get(opts, :sort, true))
      geometry = GeoSeries.union_groups(gdf.geometry, gids, n_groups)
      data = aggregate(gdf.data, by, aggs, gids)
      %{gdf | data: data, geometry: geometry}
    end
  end

  defp check_by!(by, names, geometry_column) do
    Enum.each(by, fn name ->
      cond do
        name == geometry_column ->
          raise ArgumentError, "cannot dissolve by the geometry column #{inspect(name)}"

        name not in names ->
          raise ArgumentError, "column #{inspect(name)} not found"

        true ->
          :ok
      end
    end)
  end

  # Normalised to a list of {column, function}, keys excluded when `aggfunc` is a single atom.
  defp aggregations(func, by, names) when is_atom(func) do
    check_func!(func)
    for name <- names, name not in by, do: {name, func}
  end

  defp aggregations(spec, by, names) when is_list(spec) or is_map(spec) do
    for {name, func} <- spec do
      name = to_string(name)
      check_func!(func)

      cond do
        name in by -> raise ArgumentError, "aggfunc column #{inspect(name)} is a grouping key"
        name not in names -> raise ArgumentError, "aggfunc column #{inspect(name)} not found"
        true -> {name, func}
      end
    end
  end

  defp check_func!(func) when func in @aggfuncs, do: :ok

  defp check_func!(func) do
    raise ArgumentError,
          "aggfunc must be one of #{inspect(@aggfuncs)} (or a map of them), got #{inspect(func)}"
  end

  defp key_rows(gdf, []), do: List.duplicate([], GeoDataFrame.n_rows(gdf))

  defp key_rows(gdf, by) do
    by |> Enum.map(&(gdf.data |> DataFrame.pull(&1) |> Series.to_list())) |> Enum.zip_with(& &1)
  end

  defp drop_null_keys(gdf, []), do: gdf

  defp drop_null_keys(gdf, by) do
    kept =
      gdf
      |> key_rows(by)
      |> Enum.with_index()
      |> Enum.reject(fn {key, _} -> nil in key end)
      |> Enum.map(&elem(&1, 1))

    if length(kept) == GeoDataFrame.n_rows(gdf), do: gdf, else: GeoDataFrame.take(gdf, kept)
  end

  # Dense id per row, numbered in result order.
  defp group_ids(gdf, by, sort?) do
    keys = key_rows(gdf, by)
    groups = Enum.uniq(keys)
    groups = if sort?, do: Enum.sort(groups, &(compare_keys(&1, &2) != :gt)), else: groups
    ids = groups |> Enum.with_index() |> Map.new()
    {Enum.map(keys, &Map.fetch!(ids, &1)), length(groups)}
  end

  defp compare_keys([], []), do: :eq

  defp compare_keys([a | as], [b | bs]) do
    case compare_value(a, b) do
      :eq -> compare_keys(as, bs)
      other -> other
    end
  end

  # Nulls last; calendar structs by their own `compare/2`; everything else by term order.
  defp compare_value(a, a), do: :eq
  defp compare_value(nil, _), do: :gt
  defp compare_value(_, nil), do: :lt

  defp compare_value(%module{} = a, %module{} = b) do
    if function_exported?(module, :compare, 2), do: module.compare(a, b), else: term_order(a, b)
  end

  defp compare_value(a, b), do: term_order(a, b)

  defp term_order(a, b), do: if(a < b, do: :lt, else: :gt)

  defp aggregate(data, by, aggs, gids) do
    columns = Enum.map(by, &{&1, :first}) ++ aggs
    gid_col = free_name(DataFrame.names(data))

    if columns == [] do
      DataFrame.new([])
    else
      data
      |> DataFrame.put(gid_col, Series.from_list(gids, dtype: {:s, 64}))
      |> DataFrame.group_by(gid_col)
      |> DataFrame.summarise_with(&summaries(&1, columns))
      |> DataFrame.sort_with(& &1[gid_col])
      |> DataFrame.select(Enum.map(columns, &elem(&1, 0)))
    end
  end

  defp summaries(df, columns) do
    for {name, func} <- columns, do: {name, apply_func(func, df[name])}
  end

  defp free_name(names) do
    Enum.find(
      Stream.concat([@group_col], Stream.map(1..1_000_000, &"#{@group_col}#{&1}")),
      &(&1 not in names)
    )
  end

  defp apply_func(:first, s), do: Series.first(s)
  defp apply_func(:last, s), do: Series.last(s)
  defp apply_func(:sum, s), do: Series.sum(s)
  defp apply_func(:mean, s), do: Series.mean(s)
  defp apply_func(:min, s), do: Series.min(s)
  defp apply_func(:max, s), do: Series.max(s)
  defp apply_func(:count, s), do: Series.count(s)
end
