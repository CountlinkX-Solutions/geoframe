defmodule GeoFrame.SpatialJoin do
  @moduledoc """
  Spatial joins (`sjoin/3`, `sjoin_nearest/3`) between two `GeoFrame.GeoDataFrame`s with GeoPandas semantics.

  The matching pairs come from `GeoFrame.GeoSeries.sindex_query/4` (R-tree probe, exact
  refinement, computed in Rust). Attributes are then aligned with the row indices on the
  Elixir side, so geometry never leaves Rust memory.

  ## Result

    * `how: :inner` (default) keeps matched pairs; `:left` also keeps unmatched left rows
      once, with nulls on the right; `:right` is the mirror image.
    * The geometry is the left geometry for `:inner` and `:left`, the right geometry for
      `:right` (with that frame's geometry column name).
    * Columns follow GeoPandas: left attributes, then `index_right` (the zero-based row
      position in the right frame, a nullable `s64`), then right attributes (never the right
      geometry). For `how: :right` it is `index_left`, left attributes, right attributes.
      An attribute name present on both sides becomes `name_left` / `name_right`
      (`:lsuffix`, `:rsuffix`). The geometry is always the last column, whichever side it
      comes from (GeoPandas puts the right geometry in the middle of a right join; a
      `GeoFrame.GeoDataFrame` keeps its geometry last).
    * Row order: `:inner` and `:left` by left row then right row; `:right` by right row then
      left row.

  ## Nulls on the unmatched side

  Explorer cannot `take` with missing indices, so the unmatched side is gathered from the
  frame with one all-null row appended (`concat_rows`) and `nil` indices pointing at that
  row. That keeps column dtypes and costs one copy of the unmatched side's attributes.

  Null and empty geometries never match, but still appear in `:left` / `:right` results.
  Frames whose CRS are both set and differ raise `ArgumentError`.
  """

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @hows [:inner, :left, :right]

  @doc """
  Joins `left` and `right` on a spatial relation.

  Options:

    * `:how` - `:inner` (default), `:left` or `:right`.
    * `:predicate` - any predicate of `GeoFrame.GeoSeries.sindex_query/4` (default
      `:intersects`), read as `left.predicate(right)`.
    * `:distance` - required for `:dwithin`.
    * `:lsuffix` / `:rsuffix` - suffixes for colliding attribute names (`"left"` / `"right"`).

  Raises `ArgumentError` for bad options, a CRS mismatch, or when the result would contain
  two columns with the same name (including an existing `index_right` / `index_left`).
  """
  @spec sjoin(GeoDataFrame.t(), GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  def sjoin(%GeoDataFrame{} = left, %GeoDataFrame{} = right, opts \\ []) do
    how = fetch_how!(opts)
    predicate = Keyword.get(opts, :predicate, :intersects)

    query_opts = Keyword.take(opts, [:distance])
    {li, ri} = GeoSeries.sindex_query(left.geometry, right.geometry, predicate, query_opts)

    assemble(left, right, how, {li, ri, nil}, opts)
  end

  @doc """
  Joins each `left` row with its nearest `right` geometry (planar euclidean distance,
  geometry to geometry), like `geopandas.sjoin_nearest`.

  Every right geometry at exactly the minimum distance is returned, so a left row can
  appear several times (ties); ties are ordered by right row. For `how: :right` the roles
  swap: each right row is joined with its nearest left geometries.

  Options:

    * `:how` - `:inner` (default; rows without a match are dropped), `:left` or `:right`.
    * `:max_distance` - non-negative; farther candidates do not match (inclusive limit).
    * `:distance_col` - name of an extra `f64` column holding the distance (omitted by
      default; `nil` for unmatched rows).
    * `:exclusive` - when `true`, geometries equal to the query one are ignored
      (default `false`).
    * `:lsuffix` / `:rsuffix` - suffixes for colliding attribute names (`"left"` / `"right"`).

  Column order and nulls follow `sjoin/3`; the distance column comes after the right
  attributes. Null and empty geometries never match. Raises `ArgumentError` for bad options,
  a CRS mismatch or duplicate result column names.
  """
  @spec sjoin_nearest(GeoDataFrame.t(), GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  def sjoin_nearest(%GeoDataFrame{} = left, %GeoDataFrame{} = right, opts \\ []) do
    how = fetch_how!(opts)
    nearest_opts = Keyword.take(opts, [:max_distance, :exclusive])

    {li, ri, ds} =
      if how == :right do
        {ri, li, ds} = GeoSeries.sindex_nearest(right.geometry, left.geometry, nearest_opts)
        {li, ri, ds}
      else
        GeoSeries.sindex_nearest(left.geometry, right.geometry, nearest_opts)
      end

    assemble(left, right, how, {li, ri, ds}, opts)
  end

  defp fetch_how!(opts) do
    how = Keyword.get(opts, :how, :inner)

    unless how in @hows do
      raise ArgumentError, "how must be one of #{inspect(@hows)}, got #{inspect(how)}"
    end

    how
  end

  # Builds the joined frame from matched row pairs (`ds` are the optional distances).
  defp assemble(left, right, how, {li, ri, ds}, opts) do
    lsuffix = opts |> Keyword.get(:lsuffix, "left") |> to_string()
    rsuffix = opts |> Keyword.get(:rsuffix, "right") |> to_string()
    distance_col = opts |> Keyword.get(:distance_col) |> then(&(&1 && to_string(&1)))

    {left_rows, right_rows, distances} =
      rows(how, li, ri, ds, GeoDataFrame.n_rows(left), GeoDataFrame.n_rows(right))

    {geometry, geometry_column} =
      case how do
        :right -> {GeoSeries.take(right.geometry, right_rows), right.geometry_column}
        _ -> {GeoSeries.take(left.geometry, left_rows), left.geometry_column}
      end

    index_name = if how == :right, do: "index_left", else: "index_right"
    index_values = if how == :right, do: left_rows, else: right_rows
    index = {index_name, Series.from_list(index_values, dtype: {:s, 64})}

    left_part = gather(left.data, left_rows)
    right_part = gather(right.data, right_rows)
    {left_names, right_names} = suffixed(left_part, right_part, lsuffix, rsuffix)

    # GeoPandas order: left attributes then `index_right`, or `index_left` then the left
    # attributes for a right join; geometry stays last (GeoDataFrame invariant).
    columns =
      if(how == :right,
        do: [index] ++ columns(left_part, left_names) ++ columns(right_part, right_names),
        else: columns(left_part, left_names) ++ [index] ++ columns(right_part, right_names)
      ) ++ distance_column(distance_col, distances)

    check_unique!(Enum.map(columns, &elem(&1, 0)), geometry_column, index_name)
    GeoDataFrame.new(DataFrame.new(columns), geometry, geometry_column: geometry_column)
  end

  defp distance_column(nil, _distances), do: []

  defp distance_column(name, distances),
    do: [{name, Series.from_list(distances, dtype: {:f, 64})}]

  # Row positions of both sides (and distances) for the result; `nil` marks the unmatched
  # side. `ds` is `nil` for predicate joins.
  defp rows(how, li, ri, ds, n_left, n_right) do
    ds = ds || List.duplicate(nil, length(li))
    triples = Enum.zip_with([li, ri, ds], &List.to_tuple/1)

    triples =
      case how do
        :inner -> triples
        :left -> sort_with_unmatched(triples, li, n_left, 0)
        :right -> sort_with_unmatched(triples, ri, n_right, 1)
      end

    {l, r, d} = unzip3(triples)
    {l, r, d}
  end

  # `side` is the tuple position of the preserved side; its unmatched rows get `nil` partners.
  defp sort_with_unmatched(triples, matched, n, side) do
    extra =
      for i <- unmatched(matched, n), do: if(side == 0, do: {i, nil, nil}, else: {nil, i, nil})

    Enum.sort_by(triples ++ extra, fn {l, r, _} ->
      if side == 0, do: {l, r}, else: {r, l}
    end)
  end

  defp unzip3(triples) do
    {l, r, d} =
      Enum.reduce(triples, {[], [], []}, fn {l, r, d}, {ls, rs, ds} ->
        {[l | ls], [r | rs], [d | ds]}
      end)

    {Enum.reverse(l), Enum.reverse(r), Enum.reverse(d)}
  end

  defp unmatched(matched, n) do
    seen = MapSet.new(matched)
    for i <- 0..(n - 1)//1, i not in seen, do: i
  end

  # Selects `rows` from the attribute frame; `nil` rows become all-null rows.
  defp gather(%DataFrame{} = data, rows) do
    names = DataFrame.names(data)

    cond do
      names == [] ->
        data

      nil in rows ->
        sentinel = DataFrame.n_rows(data)
        nulls = for name <- names, do: {name, null_series(data, name)}
        padded = DataFrame.concat_rows([data, DataFrame.new(nulls)])
        DataFrame.slice(padded, Enum.map(rows, &(&1 || sentinel)))

      true ->
        DataFrame.slice(data, rows)
    end
  end

  defp null_series(data, name) do
    Series.from_list([nil], dtype: data |> DataFrame.pull(name) |> Series.dtype())
  end

  # Renames only the attribute names present on both sides.
  defp suffixed(left, right, lsuffix, rsuffix) do
    left_names = DataFrame.names(left)
    right_names = DataFrame.names(right)
    shared = MapSet.new(left_names) |> MapSet.intersection(MapSet.new(right_names))

    rename = fn names, suffix ->
      Enum.map(names, fn name ->
        {name, if(name in shared, do: "#{name}_#{suffix}", else: name)}
      end)
    end

    {rename.(left_names, lsuffix), rename.(right_names, rsuffix)}
  end

  defp columns(data, names) do
    for {old, new} <- names, do: {new, DataFrame.pull(data, old)}
  end

  defp check_unique!(names, geometry_column, index_name) do
    all = names ++ [geometry_column]

    case all -- Enum.uniq(all) do
      [] ->
        :ok

      [dup | _] ->
        hint = if dup == index_name, do: " (drop or rename the existing column)", else: ""
        raise ArgumentError, "sjoin result would have two columns named #{inspect(dup)}#{hint}"
    end
  end
end
