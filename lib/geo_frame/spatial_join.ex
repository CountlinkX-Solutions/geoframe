defmodule GeoFrame.SpatialJoin do
  @moduledoc """
  Spatial joins (`sjoin/3`, `sjoin_nearest/3`) between two `GeoFrame.GeoDataFrame`s with GeoPandas semantics.

  The matching pairs come from `GeoFrame.GeoSeries.sindex_query/4` (R-tree probe, exact
  refinement, computed in Rust). Rust also lays out the result rows (join order, unmatched
  rows) and hands the row indices over as native-endian `u32` binaries that Explorer reads
  with `Series.from_binary/3`, so a join of millions of pairs never builds a list of indices
  on the BEAM heap, and geometry never leaves Rust memory.

  ## Result

    * `how: :inner` (default) keeps matched pairs; `:left` also keeps unmatched left rows
      once, with nulls on the right; `:right` is the mirror image.
    * The geometry is the left geometry for `:inner` and `:left`, the right geometry for
      `:right` (with that frame's geometry column name).
    * Columns follow GeoPandas: left attributes, then `index_right` (the zero-based row
      position in the right frame, a nullable `s64`), then right attributes (never the right
      geometry). For `how: :right` it is `index_left`, left attributes, right attributes.
      An attribute name present on both sides becomes `name_left` / `name_right`
      (`:lsuffix`, `:rsuffix`), and the index column is `index_<rsuffix>` (`index_<lsuffix>`
      for a right join), as in GeoPandas. The geometry is always the last column, whichever side it
      comes from (GeoPandas puts the right geometry in the middle of a right join; a
      `GeoFrame.GeoDataFrame` keeps its geometry last).
    * Row order: `:inner` and `:left` by left row then right row; `:right` by right row then
      left row.

  ## Secondary geometry columns

  The secondary geometry columns of both frames (`GeoFrame.GeoDataFrame.put_geometry/3`) are
  attributes of the result, like the other geometry-dtype columns of a GeoPandas frame: each
  keeps its own CRS, a name present on both sides gets the suffixes, and rows without a
  partner hold a null geometry. They follow the active geometry (left columns first), whereas
  GeoPandas leaves each at its original column position.

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
  alias GeoFrame.Native
  alias GeoFrame.Options

  @hows [:inner, :left, :right]
  # `u32::MAX` marks a row without a partner in the pair vectors Rust returns.
  @no_row 4_294_967_295
  @sjoin_options [:how, :predicate, :distance, :lsuffix, :rsuffix]
  @nearest_options [:how, :max_distance, :distance_col, :exclusive, :lsuffix, :rsuffix]

  @doc """
  Joins `left` and `right` on a spatial relation.

  Options:

    * `:how` - `:inner` (default), `:left` or `:right`.
    * `:predicate` - any predicate of `GeoFrame.GeoSeries.sindex_query/4` (default
      `:intersects`), read as `left.predicate(right)`.
    * `:distance` - required for `:dwithin`.
    * `:lsuffix` / `:rsuffix` - suffixes for colliding attribute names (`"left"` / `"right"`).

  Raises `ArgumentError` for bad or unknown options, a CRS mismatch, or when the result would contain
  two columns with the same name (including an existing `index_right` / `index_left`).
  """
  @spec sjoin(GeoDataFrame.t(), GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  def sjoin(%GeoDataFrame{} = left, %GeoDataFrame{} = right, opts \\ []) do
    Options.validate!(opts, @sjoin_options)
    how = fetch_how!(opts)
    predicate = Keyword.get(opts, :predicate, :intersects)

    query_opts = Keyword.take(opts, [:distance])

    {li, ri} =
      GeoSeries.sindex_query_binaries(left.geometry, right.geometry, predicate, query_opts)

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
  attributes. Null and empty geometries never match. Raises `ArgumentError` for bad or unknown options,
  a CRS mismatch or duplicate result column names.
  """
  @spec sjoin_nearest(GeoDataFrame.t(), GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  def sjoin_nearest(%GeoDataFrame{} = left, %GeoDataFrame{} = right, opts \\ []) do
    Options.validate!(opts, @nearest_options)
    how = fetch_how!(opts)
    nearest_opts = Keyword.take(opts, [:max_distance, :exclusive])

    {li, ri, ds} =
      if how == :right do
        {ri, li, ds} =
          GeoSeries.sindex_nearest_binaries(right.geometry, left.geometry, nearest_opts)

        {li, ri, ds}
      else
        GeoSeries.sindex_nearest_binaries(left.geometry, right.geometry, nearest_opts)
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

  # Builds the joined frame from matched row pairs: native-endian binaries of u32 rows (and
  # f64 distances, `ds`, for the nearest join). Rust lays out the result rows (sort order and
  # unmatched rows, `@no_row` marking a missing partner); the row vectors reach Explorer as
  # series through `from_binary`, so no list of pair indices is ever built on the BEAM heap.
  defp assemble(left, right, how, {li, ri, ds}, opts) do
    lsuffix = opts |> Keyword.get(:lsuffix, "left") |> to_string()
    rsuffix = opts |> Keyword.get(:rsuffix, "right") |> to_string()
    distance_col = opts |> Keyword.get(:distance_col) |> then(&(&1 && to_string(&1)))

    {left_bin, right_bin, distance_bin} =
      li
      |> Native.join_rows(
        ri,
        ds,
        Atom.to_string(how),
        GeoDataFrame.n_rows(left),
        GeoDataFrame.n_rows(right)
      )
      |> unwrap!()

    left_rows = Series.from_binary(left_bin, {:u, 32})
    right_rows = Series.from_binary(right_bin, {:u, 32})

    {geometry, geometry_column} =
      case how do
        :right -> {GeoSeries.take(right.geometry, right_bin), right.geometry_column}
        _ -> {GeoSeries.take(left.geometry, left_bin), left.geometry_column}
      end

    index_name = "index_" <> if(how == :right, do: lsuffix, else: rsuffix)
    index_values = if how == :right, do: left_rows, else: right_rows
    index = {index_name, nullable_index(index_values)}

    left_part = gather(left.data, left_rows)
    right_part = gather(right.data, right_rows)
    left_geometries = gather_geometries(left.secondary_geometries, left_rows)
    right_geometries = gather_geometries(right.secondary_geometries, right_rows)

    # Secondary geometry columns are ordinary columns for the suffixes, as in GeoPandas.
    {left_names, right_names} =
      suffixed(
        DataFrame.names(left_part) ++ Enum.map(left_geometries, &elem(&1, 0)),
        DataFrame.names(right_part) ++ Enum.map(right_geometries, &elem(&1, 0)),
        lsuffix,
        rsuffix
      )

    # GeoPandas order: left attributes then `index_right`, or `index_left` then the left
    # attributes for a right join; geometry stays last (GeoDataFrame invariant).
    columns =
      if(how == :right,
        do: [index] ++ columns(left_part, left_names) ++ columns(right_part, right_names),
        else: columns(left_part, left_names) ++ [index] ++ columns(right_part, right_names)
      ) ++ distance_column(distance_col, distance_bin, index_values)

    secondary =
      renamed_geometries(left_geometries, left_names) ++
        renamed_geometries(right_geometries, right_names)

    check_unique!(
      Enum.map(columns, &elem(&1, 0)),
      [geometry_column | Enum.map(secondary, &elem(&1, 0))],
      index_name
    )

    %{
      GeoDataFrame.new(DataFrame.new(columns), geometry, geometry_column: geometry_column)
      | secondary_geometries: secondary
    }
  end

  defp unwrap!({:ok, value}), do: value
  defp unwrap!({:error, message}), do: raise(ArgumentError, message)

  # A u32 row series as the nullable s64 `index_*` column: the missing-partner marker is null.
  defp nullable_index(rows) do
    Series.select(
      missing(rows),
      Series.from_list([nil], dtype: {:s, 64}),
      Series.cast(rows, {:s, 64})
    )
  end

  defp missing(rows), do: Series.equal(rows, @no_row)

  # Distances come with NaN for unmatched rows; those are null in the column. `partner` is the
  # row series of the side that may be missing.
  defp distance_column(nil, _distances, _partner), do: []

  defp distance_column(name, distances, partner) do
    series = Series.from_binary(distances, {:f, 64})
    [{name, Series.select(missing(partner), Series.from_list([nil], dtype: {:f, 64}), series)}]
  end

  # Selects `rows` from the attribute frame. `rows` is a list (`nil` marks a row to fill with
  # nulls) or a u32 series (`@no_row` marks it); a missing row becomes an all-null row.
  @doc false
  def gather(%DataFrame{} = data, rows) do
    names = DataFrame.names(data)

    cond do
      names == [] ->
        data

      is_list(rows) and nil in rows ->
        sentinel = DataFrame.n_rows(data)
        DataFrame.slice(with_null_row(data, names), Enum.map(rows, &(&1 || sentinel)))

      is_list(rows) ->
        DataFrame.slice(data, rows)

      Series.any?(missing(rows)) ->
        sentinel = DataFrame.n_rows(data)
        indices = Series.select(missing(rows), sentinel, rows)
        DataFrame.slice(with_null_row(data, names), indices)

      true ->
        DataFrame.slice(data, rows)
    end
  end

  # The frame with one all-null row appended (index `n_rows`), so missing partners can be
  # gathered with an ordinary slice.
  defp with_null_row(data, names) do
    nulls = for name <- names, do: {name, null_series(data, name)}
    DataFrame.concat_rows([data, DataFrame.new(nulls)])
  end

  defp null_series(data, name) do
    Series.from_list([nil], dtype: data |> DataFrame.pull(name) |> Series.dtype())
  end

  # Renames only the names present on both sides; returns `{old, new}` pairs per side.
  @doc false
  def suffixed(left_names, right_names, lsuffix, rsuffix) do
    shared = MapSet.new(left_names) |> MapSet.intersection(MapSet.new(right_names))

    rename = fn names, suffix ->
      Enum.map(names, fn name ->
        {name, if(name in shared, do: "#{name}_#{suffix}", else: name)}
      end)
    end

    {rename.(left_names, lsuffix), rename.(right_names, rsuffix)}
  end

  @doc false
  def columns(data, names) do
    for {old, new} <- names,
        Enum.member?(DataFrame.names(data), old),
        do: {new, DataFrame.pull(data, old)}
  end

  # The secondary geometry columns of a side, gathered like the attributes: `rows` is a list
  # (`nil` marks a row to fill with a null geometry) or a u32 series (`@no_row`). Returns a
  # keyword-like list of `{name, series}`.
  @doc false
  def gather_geometries([], _rows), do: []

  def gather_geometries(secondary, rows) do
    positions =
      if is_list(rows),
        do: rows,
        else: rows |> Series.to_list() |> Enum.map(&if(&1 == @no_row, do: nil, else: &1))

    Enum.map(secondary, fn {name, series} ->
      {name, GeoSeries.take_nullable(series, positions)}
    end)
  end

  # Applies the `{old, new}` renames of `suffixed/4` to gathered geometry columns.
  @doc false
  def renamed_geometries(geometries, names) do
    for {old, series} <- geometries, do: {names |> List.keyfind(old, 0) |> elem(1), series}
  end

  defp check_unique!(names, geometry_columns, index_name) do
    all = names ++ geometry_columns

    case all -- Enum.uniq(all) do
      [] ->
        :ok

      [dup | _] ->
        hint = if dup == index_name, do: " (drop or rename the existing column)", else: ""
        raise ArgumentError, "sjoin result would have two columns named #{inspect(dup)}#{hint}"
    end
  end
end
