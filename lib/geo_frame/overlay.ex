defmodule GeoFrame.Overlay do
  @moduledoc """
  `clip/3` and `overlay/3` with GeoPandas semantics.

  Both are built from row positions, like the spatial joins: the R-tree finds the candidate
  pairs (`GeoFrame.GeoSeries.sindex_query/4`), the geometry work runs in Rust on whole
  columns, and attributes are gathered afterwards from the same positions, so attribute and
  geometry rows cannot drift apart.

  ## Differences from GeoPandas

    * No warnings: where GeoPandas emits a `UserWarning` (`keep_geom_type: nil` dropping
      geometries, `keep_geom_type: true` on a mixed frame) we call `Logger.warning/1`.
    * The pandas index does not exist, so the result has fresh rows only; use an id column
      to keep identity.
    * A CRS mismatch raises `ArgumentError` (GeoPandas warns), like every binary operation here.
    * Overlay results are not passed through `make_valid` (GeoPandas does that for polygon
      results): the boolean operations return valid polygons.
    * `clip/3` returns rows in their original order (GeoPandas' R-tree order is unspecified).
  """

  require Logger

  alias Explorer.DataFrame
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.SpatialJoin

  @hows [:intersection, :union, :identity, :symmetric_difference, :difference]

  @polygon ~w(Polygon MultiPolygon)
  @line ~w(LineString MultiLineString LinearRing)
  @point ~w(Point MultiPoint)

  # -- clip ---------------------------------------------------------------------------------

  @doc """
  Clips points, lines or polygons to a polygon mask, like `geopandas.clip`.

  `data` is a `GeoFrame.GeoDataFrame` or `GeoFrame.GeoSeries`. `mask` is a polygon or
  multipolygon as WKT, a `GeoSeries` or `GeoDataFrame` (its geometries are merged with
  `union_all`), or a bounding box `{minx, miny, maxx, maxy}`, which clips with the fast
  rectangle algorithm (`GeoFrame.GeoSeries.clip_by_rect/5`, results may be invalid).

  Rows are first filtered with the R-tree (`intersects`), then intersected with the mask
  (points are returned as they are). Rows that end up empty are dropped. A `GeoSeries` gives a
  `GeoSeries`, a frame gives a frame with the attributes of the surviving rows.

  Options:

    * `:keep_geom_type` - `false` (default). When `true`, a result holding parts of another
      geometry type than the data (a line clipped to a mask it only touches gives a point; a
      polygon gives a collection) is reduced to the parts of the original type: collections are
      exploded and the foreign parts removed. Warns (logs) and does nothing for mixed frames or
      frames containing collections, as GeoPandas does.

  Raises `ArgumentError` for a mask that is not polygonal, an inverted box, or a CRS mismatch.

  ## Example

      iex> s = GeoFrame.GeoSeries.from_wkt!(["POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"])
      iex> mask = "POLYGON ((2 2, 6 2, 6 6, 2 6, 2 2))"
      iex> s |> GeoFrame.Overlay.clip(mask) |> GeoFrame.GeoSeries.to_wkt()
      ["POLYGON((2 4,2 2,4 2,4 4,2 4))"]
  """
  @spec clip(GeoDataFrame.t() | GeoSeries.t(), term(), keyword()) ::
          GeoDataFrame.t() | GeoSeries.t()
  def clip(data, mask, opts \\ [])

  def clip(%GeoDataFrame{geometry: geometry} = gdf, mask, opts) do
    {positions, clipped} = clip_rows(geometry, mask, opts)
    data = if names?(gdf.data), do: DataFrame.slice(gdf.data, positions), else: gdf.data
    GeoDataFrame.new(data, clipped, geometry_column: gdf.geometry_column)
  end

  def clip(%GeoSeries{} = series, mask, opts) do
    {_positions, clipped} = clip_rows(series, mask, opts)
    clipped
  end

  def clip(other, _mask, _opts),
    do: raise(ArgumentError, "clip expects a GeoDataFrame or GeoSeries, got: #{inspect(other)}")

  defp clip_rows(geometry, mask, opts) do
    opts = Keyword.validate!(opts, keep_geom_type: false)
    mask = normalize_mask(mask, geometry)
    {candidates, _} = GeoSeries.sindex_query(geometry, mask.series, :intersects)
    positions = candidates |> Enum.uniq() |> Enum.sort()
    sub = GeoSeries.take(geometry, positions)

    clipped = clip_geometry(sub, mask)
    keep = non_empty(clipped)
    positions = pick(positions, keep)
    clipped = GeoSeries.take(clipped, keep)

    {positions, clipped} =
      if opts[:keep_geom_type],
        do: keep_type_after_clip(geometry, positions, clipped),
        else: {positions, clipped}

    {positions, clipped}
  end

  defp clip_geometry(sub, %{rect: {x0, y0, x1, y1}}) do
    clipped = GeoSeries.clip_by_rect(sub, x0, y0, x1, y1)

    points =
      sub |> GeoSeries.geom_type() |> Explorer.Series.to_list() |> Enum.map(&(&1 == "Point"))

    n = GeoSeries.size(sub)

    # GeoPandas leaves plain points untouched: they met the box in the R-tree already.
    choose =
      points |> Enum.with_index() |> Enum.map(fn {point?, i} -> if point?, do: n + i, else: i end)

    [clipped, sub] |> GeoSeries.concat() |> GeoSeries.take(choose)
  end

  defp clip_geometry(sub, %{series: series}), do: GeoSeries.intersection(sub, series)

  # Positions of the rows that are not empty (null rows go as well).
  defp non_empty(series) do
    series
    |> GeoSeries.is_empty()
    |> Explorer.Series.to_list()
    |> Enum.with_index()
    |> Enum.flat_map(fn
      {false, i} -> [i]
      _ -> []
    end)
  end

  defp pick(list, indices) do
    tuple = List.to_tuple(list)
    Enum.map(indices, &elem(tuple, &1))
  end

  defp keep_type_after_clip(original, positions, clipped) do
    original_types = types(original)
    clipped_types = types(clipped)
    collection_original? = "GeometryCollection" in original_types
    new_collection? = "GeometryCollection" in clipped_types and not collection_original?

    cond do
      collection_original? ->
        Logger.warning(
          "keep_geom_type can not be called on a GeoDataFrame with GeometryCollection."
        )

        {positions, clipped}

      class_count(original_types) > 1 ->
        Logger.warning("keep_geom_type can not be called on a mixed type GeoDataFrame.")
        {positions, clipped}

      new_collection? or class_count(original_types) < class_count(clipped_types) ->
        {positions, clipped} =
          if new_collection? do
            {parts, source} = GeoSeries.explode(clipped)
            {pick(positions, source), parts}
          else
            {positions, clipped}
          end

        filter_class(positions, clipped, class_of(hd(original_types)))

      true ->
        {positions, clipped}
    end
  end

  defp filter_class(positions, series, class) when class in [:polygon, :line] do
    keep =
      series
      |> types()
      |> Enum.with_index()
      |> Enum.flat_map(fn {type, i} -> if class_of(type) == class, do: [i], else: [] end)

    {pick(positions, keep), GeoSeries.take(series, keep)}
  end

  defp filter_class(positions, series, _class), do: {positions, series}

  # -- masks ----------------------------------------------------------------------------------

  defp normalize_mask({x0, y0, x1, y1} = box, geometry)
       when is_number(x0) and is_number(y0) and is_number(x1) and is_number(y1) do
    unless x0 <= x1 and y0 <= y1 do
      raise ArgumentError,
            "invalid mask box: expected minx <= maxx and miny <= maxy, got #{inspect(box)}"
    end

    wkt =
      "POLYGON ((#{num(x0)} #{num(y0)}, #{num(x1)} #{num(y0)}, #{num(x1)} #{num(y1)}, " <>
        "#{num(x0)} #{num(y1)}, #{num(x0)} #{num(y0)}))"

    %{series: GeoSeries.from_wkt!([wkt], crs: GeoSeries.crs(geometry)), rect: {x0, y0, x1, y1}}
  end

  defp normalize_mask(wkt, geometry) when is_binary(wkt) do
    polygonal!(GeoSeries.from_wkt!([wkt], crs: GeoSeries.crs(geometry)))
  end

  defp normalize_mask(%GeoSeries{} = mask, geometry) do
    check_crs!(GeoSeries.crs(geometry), GeoSeries.crs(mask))
    polygonal!(GeoSeries.union_all(mask))
  end

  defp normalize_mask(%GeoDataFrame{geometry: mask}, geometry), do: normalize_mask(mask, geometry)

  defp normalize_mask(other, _geometry) do
    raise ArgumentError,
          "mask must be a (Multi)Polygon as WKT, a GeoSeries, a GeoDataFrame or a " <>
            "{minx, miny, maxx, maxy} tuple, got: #{inspect(other)}"
  end

  defp polygonal!(series) do
    [type] = series |> types()
    [empty] = series |> GeoSeries.is_empty() |> Explorer.Series.to_list()

    if type in @polygon or empty == true do
      %{series: series, rect: nil}
    else
      raise ArgumentError, "mask must be a Polygon or MultiPolygon, got a #{type}"
    end
  end

  defp num(value), do: :erlang.float_to_binary(value * 1.0, [{:decimals, 17}, :compact])

  # -- overlay ------------------------------------------------------------------------------

  @doc """
  Spatial overlay of two frames, like `geopandas.overlay`.

  Options:

    * `:how` - `:intersection` (default), `:union`, `:identity`, `:symmetric_difference` or
      `:difference`.
    * `:keep_geom_type` - `nil` (default), `true` or `false`. With `true` only geometries of
      the geometry type of `df1` are returned: other parts of collections are removed and rows
      without a part of that type are dropped (a shared edge between two polygons is a line and
      goes). `nil` behaves like `true` but logs a warning with `Logger.warning/1` when
      something is dropped. `false` returns everything, possibly mixed types.
    * `:make_valid` - `true` (default): invalid polygons in the inputs are repaired with
      `GeoFrame.GeoSeries.make_valid/1` (only their polygonal parts are kept); `false` raises
      `ArgumentError` on invalid input.

  Each frame must hold a single geometry family (polygons, lines or points). The result keeps
  the CRS and the geometry column name of `df1`.

  ## Result columns and row order

    * `:intersection`, `:union`, `:identity`, `:symmetric_difference`: the attributes of `df1`,
      then those of `df2`, then the geometry. A name present in both gets `_1` / `_2`.
    * `:difference`: only the attributes of `df1` (names unchanged).
    * Rows come in blocks, as in GeoPandas. Intersection: one row per intersecting pair (by
      `df1` row, then `df2` row). Union: the intersection rows, the parts of each `df1` row
      outside `df2`, then the parts of each `df2` row outside `df1`. Identity: the intersection
      rows, then the `df1` remainders. Symmetric difference: the `df1` remainders, then the `df2`
      remainders. A row from only one side has nulls for the other side's attributes.

  A row with a null geometry meets nothing, so it never appears in an intersection. Where a
  side's remainder is part of the result (`:difference`, `:identity`, `:union`,
  `:symmetric_difference`) its null rows are kept, with a null geometry and in place among the
  remainders, exactly as `geopandas.overlay` does (checked against it for every `:how`).

  Implemented as GeoPandas does it: candidate pairs from the R-tree, pairwise intersections,
  and, for the differences, each row minus the union of the rows of the other frame it meets.

  ## Example

      iex> alias GeoFrame.{GeoDataFrame, GeoSeries}
      iex> sq = fn x -> "POLYGON ((\#{x} 0, \#{x + 2} 0, \#{x + 2} 2, \#{x} 2, \#{x} 0))" end
      iex> df1 = GeoDataFrame.new(Explorer.DataFrame.new(a: [1]), GeoSeries.from_wkt!([sq.(0)]))
      iex> df2 = GeoDataFrame.new(Explorer.DataFrame.new(b: [2]), GeoSeries.from_wkt!([sq.(1)]))
      iex> out = GeoFrame.Overlay.overlay(df1, df2)
      iex> {GeoDataFrame.names(out), GeoSeries.to_wkt(out.geometry)}
      {["a", "b", "geometry"], ["POLYGON((1 2,1 0,2 0,2 2,1 2))"]}
  """
  @spec overlay(GeoDataFrame.t(), GeoDataFrame.t(), keyword()) :: GeoDataFrame.t()
  def overlay(df1, df2, opts \\ [])

  def overlay(%GeoDataFrame{} = df1, %GeoDataFrame{} = df2, opts) do
    opts = Keyword.validate!(opts, how: :intersection, keep_geom_type: nil, make_valid: true)
    how = opts[:how]

    unless how in @hows do
      raise ArgumentError, "how must be one of #{inspect(@hows)}, got #{inspect(how)}"
    end

    check_crs!(GeoDataFrame.crs(df1), GeoDataFrame.crs(df2))
    check_single_family!(df1, "df1")
    check_single_family!(df2, "df2")

    if how == :intersection and disjoint_extents?(df1, df2) do
      assemble(how, df1, df2, {[], [], GeoSeries.take(df1.geometry, [])})
    else
      {keep?, warn?} = keep_policy(opts[:keep_geom_type], df1)
      class = keep? && df1 |> first_type() |> class_of()

      if keep? and class == nil do
        raise ArgumentError, "keep_geom_type does not support GeometryCollection in df1"
      end

      df1 = validated(df1, opts[:make_valid])
      df2 = validated(df2, opts[:make_valid])

      rows = pairs(how, df1.geometry, df2.geometry)
      rows = if keep?, do: extract_class(rows, class, warn?), else: rows
      assemble(how, df1, df2, rows)
    end
  end

  def overlay(df1, df2, _opts) do
    raise ArgumentError,
          "overlay currently only works on two GeoDataFrames, got: " <>
            "#{inspect(df1.__struct__)} and #{inspect(df2.__struct__)}"
  end

  defp keep_policy(nil, df1) do
    if GeoDataFrame.n_rows(df1) == 0, do: {false, false}, else: {true, true}
  end

  defp keep_policy(true, df1) do
    if GeoDataFrame.n_rows(df1) == 0 do
      Logger.warning(
        "`keep_geom_type: true` is invalid when df1 is empty as there is no geometry type " <>
          "to keep. Setting `keep_geom_type: false`"
      )

      {false, false}
    else
      {true, false}
    end
  end

  defp keep_policy(false, _df1), do: {false, false}

  defp keep_policy(other, _df1),
    do: raise(ArgumentError, "keep_geom_type must be nil, true or false, got: #{inspect(other)}")

  defp disjoint_extents?(df1, df2) do
    case {GeoSeries.total_bounds(df1.geometry), GeoSeries.total_bounds(df2.geometry)} do
      {{ax0, ay0, ax1, ay1}, {bx0, by0, bx1, by1}} ->
        not (ax0 <= bx1 and bx0 <= ax1 and ay0 <= by1 and by0 <= ay1)

      _ ->
        true
    end
  end

  # Each frame must hold one geometry family.
  defp check_single_family!(gdf, name) do
    classes = gdf.geometry |> types() |> Enum.map(&class_of/1) |> Enum.reject(&is_nil/1)

    if length(Enum.uniq(classes)) > 1 do
      raise ArgumentError, "#{name} contains mixed geometry types."
    end
  end

  # -- make_valid ---------------------------------------------------------------------------

  defp validated(gdf, make_valid?) do
    types = types(gdf.geometry)

    if types != [] and Enum.all?(types, &(&1 in @polygon)) do
      invalid =
        gdf.geometry
        |> GeoSeries.is_valid()
        |> Explorer.Series.to_list()
        |> Enum.any?(&(&1 == false))

      repair(gdf, invalid, make_valid?)
    else
      gdf
    end
  end

  defp repair(gdf, false, _make_valid?), do: gdf

  defp repair(_gdf, true, false) do
    raise ArgumentError,
          "make_valid: false was passed along with invalid input geometries. " <>
            "Use make_valid: true or make sure that all geometries are valid before using overlay."
  end

  defp repair(gdf, true, true) do
    {kept, geometry, _dropped} = extract(GeoSeries.make_valid(gdf.geometry), :polygon)
    data = if names?(gdf.data), do: DataFrame.slice(gdf.data, kept), else: gdf.data
    GeoDataFrame.new(data, geometry, geometry_column: gdf.geometry_column)
  end

  # -- pairs ----------------------------------------------------------------------------------

  # `{left_rows, right_rows, geometry}`: for every result row the source row of each side
  # (`nil` when the side does not contribute) and its geometry.
  defp pairs(:intersection, g1, g2), do: intersections(g1, g2)
  defp pairs(:difference, g1, g2), do: remainder(g1, g2, :left)
  defp pairs(:symmetric_difference, g1, g2), do: symmetric(g1, g2)
  defp pairs(:identity, g1, g2), do: join([intersections(g1, g2), remainder(g1, g2, :left)])
  defp pairs(:union, g1, g2), do: join([intersections(g1, g2), symmetric(g1, g2)])

  defp symmetric(g1, g2), do: join([remainder(g1, g2, :left), remainder(g2, g1, :right)])

  defp join(parts) do
    {Enum.flat_map(parts, &elem(&1, 0)), Enum.flat_map(parts, &elem(&1, 1)),
     parts |> Enum.map(&elem(&1, 2)) |> GeoSeries.concat()}
  end

  defp candidate_pairs(g1, g2) do
    {li, ri} = GeoSeries.sindex_query(g1, g2, :intersects)
    li |> Enum.zip(ri) |> Enum.sort() |> Enum.unzip()
  end

  defp intersections(g1, g2) do
    {li, ri} = candidate_pairs(g1, g2)
    geometry = GeoSeries.intersection(GeoSeries.take(g1, li), GeoSeries.take(g2, ri))
    {li, ri, geometry}
  end

  # The parts of every row of `a` outside the rows of `b` it meets. `side` says which result
  # column the rows of `a` belong to; rows that become empty are dropped.
  defp remainder(a, b, side) do
    {li, ri} = candidate_pairs(a, b)
    subtracted = subtract_neighbours(a, b, li, ri)

    keep =
      subtracted
      |> GeoSeries.is_empty()
      |> Explorer.Series.to_list()
      |> Enum.with_index()
      |> Enum.flat_map(fn
        {true, _} -> []
        {_, i} -> [i]
      end)

    rows = keep
    nothing = List.duplicate(nil, length(rows))
    geometry = GeoSeries.take(subtracted, keep)
    if side == :left, do: {rows, nothing, geometry}, else: {nothing, rows, geometry}
  end

  # Replaces every row of `a` that meets rows of `b` (pairs `li`/`ri`) by its difference with
  # the union of those rows; the other rows are unchanged.
  defp subtract_neighbours(a, _b, [], _ri), do: a

  defp subtract_neighbours(a, b, li, ri) do
    n = GeoSeries.size(a)
    with_neighbours = Enum.uniq(li)
    group_of = with_neighbours |> Enum.with_index() |> Map.new()

    union =
      GeoSeries.union_groups(
        GeoSeries.take(b, ri),
        Enum.map(li, &group_of[&1]),
        length(with_neighbours)
      )

    diff = GeoSeries.difference(GeoSeries.take(a, with_neighbours), union)
    choose = Enum.map(0..(n - 1)//1, &replacement_slot(group_of, &1, n))
    [a, diff] |> GeoSeries.concat() |> GeoSeries.take(choose)
  end

  defp replacement_slot(group_of, i, n) do
    case Map.fetch(group_of, i) do
      {:ok, k} -> n + k
      :error -> i
    end
  end

  # -- geometry type filtering ------------------------------------------------------------------

  # Keeps the parts of `class` (collections are searched), dropping other rows.
  defp extract_class({left, right, geometry}, class, warn?) do
    {keep, geometry, dropped} = extract(geometry, class)

    if warn? and dropped > 0 do
      Logger.warning(
        "`keep_geom_type: true` in overlay resulted in #{dropped} dropped geometries of " <>
          "different geometry types than df1 has. Set `keep_geom_type: false` to retain all geometries"
      )
    end

    {pick(left, keep), pick(right, keep), geometry}
  end

  # Returns `{kept_row_positions, geometry, dropped_count}`. Collections are replaced by the
  # union of their parts of `class`; any other row is kept when it is of `class`.
  defp extract(geometry, class) do
    types = types(geometry)
    n = length(types)
    collections = for {"GeometryCollection", i} <- Enum.with_index(types), do: i
    {replaced, replacements, dropped_parts} = collection_parts(geometry, collections, class)

    plain =
      for {type, i} <- Enum.with_index(types),
          type != "GeometryCollection" and class_of(type) == class,
          do: i

    kept = Enum.sort(plain ++ replaced)
    slot = replaced |> Enum.with_index() |> Map.new()

    combined = if replaced == [], do: geometry, else: GeoSeries.concat([geometry, replacements])
    choose = Enum.map(kept, fn i -> if Map.has_key?(slot, i), do: n + slot[i], else: i end)

    removed_plain = n - length(collections) - length(plain)
    {kept, GeoSeries.take(combined, choose), removed_plain + dropped_parts}
  end

  # Explodes the collection rows and unions the parts of `class` per row. Returns the rows that
  # keep a part, their replacement geometries (same order) and how many parts were dropped.
  defp collection_parts(_geometry, [], _class), do: {[], nil, 0}

  defp collection_parts(geometry, collections, class) do
    {parts, source} = geometry |> GeoSeries.take(collections) |> GeoSeries.explode()
    matching = for {type, j} <- Enum.with_index(types(parts)), class_of(type) == class, do: j
    owners = pick(source, matching)
    distinct = Enum.uniq(owners)
    group_of = distinct |> Enum.with_index() |> Map.new()

    union =
      parts
      |> GeoSeries.take(matching)
      |> GeoSeries.union_groups(Enum.map(owners, &group_of[&1]), length(distinct))

    {pick(collections, distinct), union, GeoSeries.size(parts) - length(matching)}
  end

  # -- helpers ----------------------------------------------------------------------------------

  defp assemble(:difference, df1, _df2, {rows, _none, geometry}) do
    data = if names?(df1.data), do: DataFrame.slice(df1.data, rows), else: df1.data
    GeoDataFrame.new(data, geometry, geometry_column: df1.geometry_column)
  end

  defp assemble(_how, df1, df2, {left, right, geometry}) do
    left_part = SpatialJoin.gather(df1.data, left)
    right_part = SpatialJoin.gather(df2.data, right)
    {left_names, right_names} = SpatialJoin.suffixed(left_part, right_part, "1", "2")

    columns =
      SpatialJoin.columns(left_part, left_names) ++ SpatialJoin.columns(right_part, right_names)

    GeoDataFrame.new(DataFrame.new(columns), geometry, geometry_column: df1.geometry_column)
  end

  defp check_crs!(a, b) when is_nil(a) or is_nil(b), do: :ok

  defp check_crs!(a, b) do
    if GeoFrame.CRS.equivalent?(a, b) do
      :ok
    else
      raise ArgumentError, "CRS mismatch between the inputs: #{inspect(a)} and #{inspect(b)}"
    end
  end

  defp names?(data), do: DataFrame.names(data) != []

  defp types(series), do: series |> GeoSeries.geom_type() |> Explorer.Series.to_list()

  defp first_type(gdf), do: gdf.geometry |> types() |> List.first()

  defp class_count(types),
    do: types |> Enum.map(&class_of/1) |> Enum.reject(&is_nil/1) |> Enum.uniq() |> length()

  defp class_of(type) when type in @polygon, do: :polygon
  defp class_of(type) when type in @line, do: :line
  defp class_of(type) when type in @point, do: :point
  defp class_of(_type), do: nil
end
