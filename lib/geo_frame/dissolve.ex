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
  @geometry_aggfuncs [:first, :last, :count]
  @group_col "__geoframe_group__"

  @doc """
  Dissolves `gdf` by key columns.

  Options:

    * `:by` - a column name (string or atom) or a list of names; `nil` (default) dissolves
      everything into a single row.
    * `:aggfunc` - `:first` (default), `:last`, `:sum`, `:mean`, `:min`, `:max` or `:count`,
      applied to every non-key attribute (numeric functions raise on
      non-numeric columns, so list the columns explicitly for mixed frames); or a keyword list / map of column to function, in
      which case only the listed columns are kept (after the keys). Secondary geometry columns
      are aggregated like the other columns, but only `:first`, `:last` (the first / last
      non-null geometry of the group, keeping the column's CRS) and `:count` (an integer
      attribute) apply to geometry: any other function raises `ArgumentError`, as GeoPandas
      raises `TypeError`. Dissolving by a geometry column raises `ArgumentError`.
    * `:dropna` - `true` (default) drops rows with a null key; `false` keeps them as a group.
    * `:sort` - `true` (default) orders groups by key, `false` by first appearance.
    * `:observed` - for `:category` key columns only. `true` (default) keeps the groups that
      occur in the data; `false` also adds every unobserved category (every combination, for
      several keys) as an empty group, like `observed=False` in GeoPandas: the geometry is
      `GEOMETRYCOLLECTION EMPTY`, `:sum` and `:count` give 0 and the other functions null.
      Groups are ordered by category text; with `sort: false` each key's observed values come first, in order of appearance.
      Without category keys the option has no effect; mixing category and other keys raises
      `ArgumentError`. GeoPandas defaults to `false`; the default here stays `true` so existing
      calls keep their result.
    * `:as_index` - accepted for parity. There is no index: the keys are always leading
      columns, which is what `as_index=False` gives in GeoPandas (except that the geometry
      column is last). `false` is a no-op; `true` raises `ArgumentError`.
    * `:level` - not supported (no index levels); raises `ArgumentError`.

  Geometries of a group are merged with `GeoFrame.GeoSeries.union_all/1` semantics (null
  geometries skipped). The CRS is preserved. Raises `ArgumentError` for unknown columns or
  aggregation functions, or when a key is the geometry column.
  """
  @spec dissolve(GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  def dissolve(%GeoDataFrame{} = gdf, opts \\ []) do
    GeoFrame.Options.validate!(opts, [:by, :aggfunc, :dropna, :sort, :as_index, :level, :observed])

    check_index_options!(opts)
    by = opts |> Keyword.get(:by) |> List.wrap() |> Enum.map(&to_string/1)
    names = DataFrame.names(gdf.data)
    secondary_names = Enum.map(gdf.secondary_geometries, &elem(&1, 0))
    check_by!(by, names, GeoDataFrame.geometry_columns(gdf))
    all_aggs = aggregations(Keyword.get(opts, :aggfunc, :first), by, names, secondary_names)
    {geo_aggs, aggs} = Enum.split_with(all_aggs, fn {name, _} -> name in secondary_names end)

    gdf = if Keyword.get(opts, :dropna, true), do: drop_null_keys(gdf, by), else: gdf

    if GeoDataFrame.n_rows(gdf) == 0 do
      gdf
    else
      sort? = Keyword.get(opts, :sort, true)
      {gids, groups} = group_ids(gdf, by, sort?)
      n_groups = length(groups)
      geometry = GeoSeries.union_groups(gdf.geometry, gids, n_groups)
      data = aggregate(gdf.data, by, aggs, gids)
      {data, secondary} = aggregate_geometries(gdf, geo_aggs, data, gids, n_groups)
      out = %{gdf | data: data, geometry: geometry, secondary_geometries: secondary}

      if Keyword.get(opts, :observed, true) == false and by != [] and
           category_keys?(gdf, by) do
        add_unobserved(out, gdf, by, groups, all_aggs, sort?)
      else
        out
      end
    end
  end

  defp check_index_options!(opts) do
    if Keyword.has_key?(opts, :level) do
      raise ArgumentError, "level is not supported: a GeoDataFrame has no index to group by"
    end

    case Keyword.get(opts, :as_index, false) do
      false ->
        :ok

      true ->
        raise ArgumentError,
              "as_index: true is not supported: there is no index, the keys are always " <>
                "columns (the result of as_index: false in GeoPandas)"

      other ->
        raise ArgumentError, "as_index must be false, got: #{inspect(other)}"
    end

    unless Keyword.get(opts, :observed, true) in [true, false] do
      raise ArgumentError, "observed must be a boolean, got: #{inspect(opts[:observed])}"
    end
  end

  # -- observed: false -----------------------------------------------------------------

  defp category_keys?(gdf, by) do
    kinds = Enum.map(by, &(Series.dtype(DataFrame.pull(gdf.data, &1)) == :category))

    cond do
      Enum.all?(kinds) ->
        true

      Enum.any?(kinds) ->
        raise ArgumentError,
              "observed: false needs every key to be a :category column or none of them"

      true ->
        false
    end
  end

  # Re-lays the dissolved `out` (one row per observed group, in `groups` order) over every
  # combination of the key categories; the added groups are empty.
  defp add_unobserved(out, gdf, by, groups, all_aggs, sort?) do
    categories =
      Enum.map(by, fn name ->
        gdf.data |> DataFrame.pull(name) |> Series.categories() |> Series.to_list()
      end)

    all_groups = level_product(categories, groups, sort?)

    present = groups |> Enum.with_index() |> Map.new()
    positions = Enum.map(all_groups, &Map.get(present, &1))
    funcs = Map.new(all_aggs)

    data =
      for name <- DataFrame.names(out.data), into: [] do
        {name, expanded_column(out.data, name, by, all_groups, positions, funcs)}
      end

    n = GeoDataFrame.n_rows(out)
    empty = GeoSeries.from_wkt!(["GEOMETRYCOLLECTION EMPTY"], crs: GeoSeries.crs(out.geometry))
    padded = GeoSeries.concat([out.geometry, empty])
    geometry = GeoSeries.take(padded, Enum.map(positions, &(&1 || n)))

    secondary =
      Enum.map(out.secondary_geometries, fn {name, series} ->
        {name, GeoSeries.take_nullable(series, positions)}
      end)

    %{out | data: DataFrame.new(data), geometry: geometry, secondary_geometries: secondary}
  end

  # Every combination of the key levels, like pandas: each level in category text order, or
  # (`sort: false`) its observed values in order of appearance and then the unobserved
  # categories. Groups with a null key (`dropna: false`) come last.
  defp level_product(categories, groups, sort?) do
    levels =
      categories
      |> Enum.with_index()
      |> Enum.map(fn {cats, i} ->
        cats = Enum.sort(cats)

        if sort? do
          cats
        else
          seen = groups |> Enum.map(&Enum.at(&1, i)) |> Enum.reject(&is_nil/1) |> Enum.uniq()
          seen ++ (cats -- seen)
        end
      end)

    cartesian(levels) ++ Enum.filter(groups, &(nil in &1))
  end

  defp cartesian([]), do: [[]]
  defp cartesian([values | rest]), do: for(v <- values, tail <- cartesian(rest), do: [v | tail])

  defp expanded_column(data, name, by, all_groups, positions, funcs) do
    series = DataFrame.pull(data, name)
    dtype = Series.dtype(series)
    values = series |> Series.to_list() |> List.to_tuple()

    list =
      case Enum.find_index(by, &(&1 == name)) do
        nil ->
          fill = empty_value(Map.get(funcs, name), dtype)
          Enum.map(positions, &if(&1, do: elem(values, &1), else: fill))

        key_index ->
          Enum.map(all_groups, &Enum.at(&1, key_index))
      end

    Series.from_list(list, dtype: dtype)
  end

  # What an aggregation gives for a group with no rows.
  defp empty_value(:count, _dtype), do: 0
  defp empty_value(:sum, {:f, _}), do: 0.0
  defp empty_value(:sum, _dtype), do: 0
  defp empty_value(_func, _dtype), do: nil

  defp check_by!(by, names, geometry_columns) do
    Enum.each(by, fn name ->
      cond do
        name in geometry_columns ->
          raise ArgumentError, "cannot dissolve by the geometry column #{inspect(name)}"

        name not in names ->
          raise ArgumentError, "column #{inspect(name)} not found"

        true ->
          :ok
      end
    end)
  end

  # Normalised to a list of {column, function}, keys excluded when `aggfunc` is a single atom.
  # Secondary geometry columns are columns like the others, but only `:first`, `:last` and
  # `:count` apply to a geometry (GeoPandas raises a TypeError for the numeric functions).
  defp aggregations(func, by, names, secondary) when is_atom(func) do
    check_func!(func)

    for name <- names ++ secondary,
        name not in by,
        do: check_geometry_func!({name, func}, secondary)
  end

  defp aggregations(spec, by, names, secondary) when is_list(spec) or is_map(spec) do
    for {name, func} <- spec do
      name = to_string(name)
      check_func!(func)

      cond do
        name in by -> raise ArgumentError, "aggfunc column #{inspect(name)} is a grouping key"
        name in names or name in secondary -> check_geometry_func!({name, func}, secondary)
        true -> raise ArgumentError, "aggfunc column #{inspect(name)} not found"
      end
    end
  end

  defp check_geometry_func!({name, func} = pair, secondary) do
    if name in secondary and func not in @geometry_aggfuncs do
      raise ArgumentError,
            "aggfunc #{inspect(func)} cannot aggregate the geometry column #{inspect(name)}; " <>
              "use one of #{inspect(@geometry_aggfuncs)} or leave the column out"
    end

    pair
  end

  defp check_func!(func) when func in @aggfuncs, do: :ok

  defp check_func!(func) do
    raise ArgumentError,
          "aggfunc must be one of #{inspect(@aggfuncs)} (or a map of them), got #{inspect(func)}"
  end

  # Secondary geometry columns per group: `:first` / `:last` keep the first / last non-null
  # geometry of the group (null for an all-null group, like pandas), `:count` becomes an
  # integer attribute counting the non-null geometries. Result columns follow `aggs`.
  defp aggregate_geometries(_gdf, [], data, _gids, _n), do: {data, []}

  defp aggregate_geometries(gdf, aggs, data, gids, n_groups) do
    Enum.reduce(aggs, {data, []}, fn {name, func}, {data, secondary} ->
      {^name, series} = List.keyfind(gdf.secondary_geometries, name, 0)
      nulls = series |> GeoSeries.isna() |> Series.to_list()

      case func do
        :count ->
          counts =
            gids
            |> Enum.zip(nulls)
            |> Enum.reject(&elem(&1, 1))
            |> Enum.frequencies_by(&elem(&1, 0))

          column = Series.from_list(Enum.map(0..(n_groups - 1), &Map.get(counts, &1, 0)))
          {put_column(data, name, column), secondary}

        pick ->
          rows = picked_rows(gids, nulls, pick)
          positions = Enum.map(0..(n_groups - 1), &Map.get(rows, &1))
          {data, secondary ++ [{name, GeoSeries.take_nullable(series, positions)}]}
      end
    end)
  end

  defp picked_rows(gids, nulls, pick) do
    gids
    |> Enum.zip(nulls)
    |> Enum.with_index()
    |> Enum.reduce(%{}, fn
      {{_gid, true}, _row}, acc -> acc
      {{gid, false}, row}, acc when pick == :first -> Map.put_new(acc, gid, row)
      {{gid, false}, row}, acc -> Map.put(acc, gid, row)
    end)
  end

  defp put_column(data, name, series) do
    if DataFrame.names(data) == [],
      do: DataFrame.new([{name, series}]),
      else: DataFrame.put(data, name, series)
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

  # Dense id per row, numbered in result order, and the groups (keys) in that order.
  defp group_ids(gdf, by, sort?) do
    keys = key_rows(gdf, by)
    groups = Enum.uniq(keys)
    groups = if sort?, do: Enum.sort(groups, &(compare_keys(&1, &2) != :gt)), else: groups
    ids = groups |> Enum.with_index() |> Map.new()
    {Enum.map(keys, &Map.fetch!(ids, &1)), groups}
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
      keyed = DataFrame.put(data, gid_col, Series.from_list(gids, dtype: {:s, 64}))

      # pandas `first`/`last` return the first/last non-null value of each group, which the
      # lazy group expressions cannot filter for; those columns are aggregated separately
      # over their non-null rows and joined back (a group with only nulls stays null).
      {skipping, plain} = Enum.split_with(aggs, fn {_, func} -> func in [:first, :last] end)
      base_columns = Enum.map(by, &{&1, :first}) ++ plain

      base =
        if base_columns == [] do
          keyed |> DataFrame.distinct([gid_col]) |> DataFrame.select([gid_col])
        else
          grouped_summary(keyed, gid_col, base_columns)
        end

      skipping
      |> Enum.reduce(base, fn {name, func}, acc ->
        part =
          keyed
          |> DataFrame.drop_nil([name])
          |> grouped_summary(gid_col, [{name, func}])

        DataFrame.join(acc, part, on: gid_col, how: :left)
      end)
      |> DataFrame.sort_with(& &1[gid_col])
      |> DataFrame.select(Enum.map(columns, &elem(&1, 0)))
    end
  end

  defp grouped_summary(keyed, gid_col, columns) do
    keyed
    |> DataFrame.group_by(gid_col)
    |> DataFrame.summarise_with(&summaries(&1, columns))
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
