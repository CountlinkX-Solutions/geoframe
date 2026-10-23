defmodule GeoFrame.SpatialJoin do
  @moduledoc """
  Spatial joins between two `GeoFrame.GeoDataFrame`s with GeoPandas semantics.

  The matching pairs come from `GeoFrame.GeoSeries.sindex_query/4` (R-tree probe, exact
  refinement, computed in Rust). Attributes are then aligned with the row indices on the
  Elixir side, so geometry never leaves Rust memory.

  ## Result

    * `how: :inner` (default) keeps matched pairs; `:left` also keeps unmatched left rows
      once, with nulls on the right; `:right` is the mirror image.
    * The geometry is the left geometry for `:inner` and `:left`, the right geometry for
      `:right` (with that frame's geometry column name).
    * Columns: left attributes, right attributes (never the right geometry), then
      `index_right` (`index_left` for `:right`), the zero-based row position in the *other*
      frame as a nullable `s64`, then the geometry. An attribute name present on both sides
      becomes `name_left` / `name_right` (`:lsuffix`, `:rsuffix`).
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
    how = Keyword.get(opts, :how, :inner)
    predicate = Keyword.get(opts, :predicate, :intersects)
    lsuffix = opts |> Keyword.get(:lsuffix, "left") |> to_string()
    rsuffix = opts |> Keyword.get(:rsuffix, "right") |> to_string()

    unless how in @hows do
      raise ArgumentError, "how must be one of #{inspect(@hows)}, got #{inspect(how)}"
    end

    query_opts = Keyword.take(opts, [:distance])
    {li, ri} = GeoSeries.sindex_query(left.geometry, right.geometry, predicate, query_opts)

    {left_rows, right_rows} =
      rows(how, li, ri, GeoDataFrame.n_rows(left), GeoDataFrame.n_rows(right))

    {geometry, geometry_column} =
      case how do
        :right -> {GeoSeries.take(right.geometry, right_rows), right.geometry_column}
        _ -> {GeoSeries.take(left.geometry, left_rows), left.geometry_column}
      end

    index_name = if how == :right, do: "index_left", else: "index_right"
    index_values = if how == :right, do: left_rows, else: right_rows

    left_part = gather(left.data, left_rows)
    right_part = gather(right.data, right_rows)
    {left_names, right_names} = suffixed(left_part, right_part, lsuffix, rsuffix)

    columns =
      columns(left_part, left_names) ++
        columns(right_part, right_names) ++
        [{index_name, Series.from_list(index_values, dtype: {:s, 64})}]

    check_unique!(Enum.map(columns, &elem(&1, 0)), geometry_column, index_name)
    GeoDataFrame.new(DataFrame.new(columns), geometry, geometry_column: geometry_column)
  end

  # Row positions of both sides for the result; `nil` marks the unmatched side.
  defp rows(:inner, li, ri, _n_left, _n_right), do: {li, ri}

  defp rows(:left, li, ri, n_left, _n_right) do
    pairs = Enum.zip(li, ri) ++ Enum.map(unmatched(li, n_left), &{&1, nil})
    pairs |> Enum.sort() |> Enum.unzip()
  end

  defp rows(:right, li, ri, _n_left, n_right) do
    pairs = Enum.zip(ri, li) ++ Enum.map(unmatched(ri, n_right), &{&1, nil})
    {right_rows, left_rows} = pairs |> Enum.sort() |> Enum.unzip()
    {left_rows, right_rows}
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
