defmodule GeoFrame.ParityBTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  import ExUnit.CaptureLog

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.Dissolve
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(%Series{} = series), do: Series.to_list(series)
  defp col(gdf, name), do: gdf.data |> DataFrame.pull(name) |> Series.to_list()
  defp areas(gdf), do: gdf |> GeoDataFrame.area() |> list()
  defp types(gdf), do: gdf |> GeoDataFrame.geometry() |> GeoSeries.geom_type() |> list()

  defp rect(x, y, w, h),
    do: "POLYGON ((#{x} #{y}, #{x + w} #{y}, #{x + w} #{y + h}, #{x} #{y + h}, #{x} #{y}))"

  defp frame(columns, wkts, opts \\ []),
    do: GeoDataFrame.new(DataFrame.new(columns), s(wkts, opts))

  # -- explode ------------------------------------------------------------------------------

  describe "GeoSeries.explode/2" do
    test "returns the parts and the source row of every part" do
      series = s(["POINT (1 1)", "MULTIPOINT ((1 1), (2 2))", nil, "POLYGON EMPTY"])
      {parts, positions} = GeoSeries.explode(series)

      assert positions == [0, 1, 1, 3]

      assert GeoSeries.to_wkt(parts) == [
               "POINT(1 1)",
               "POINT(1 1)",
               "POINT(2 2)",
               "POLYGON EMPTY"
             ]
    end

    test "collections explode one level; empty collections and nulls vanish" do
      series =
        s([
          "GEOMETRYCOLLECTION (POINT (1 1), MULTIPOINT ((5 5), (6 6)))",
          "GEOMETRYCOLLECTION EMPTY",
          "MULTIPOINT EMPTY"
        ])

      {parts, positions} = GeoSeries.explode(series)
      assert positions == [0, 0]
      assert list(GeoSeries.geom_type(parts)) == ["Point", "MultiPoint"]
    end

    test "index_parts: true adds the part number of each row" do
      series = s(["MULTIPOINT ((1 1), (2 2), (3 3))", "POINT (0 0)", "MULTIPOINT ((7 7), (8 8))"])
      {_parts, positions, numbers} = GeoSeries.explode(series, index_parts: true)
      assert positions == [0, 0, 0, 1, 2, 2]
      assert numbers == [0, 1, 2, 0, 0, 1]
    end

    test "keeps the CRS" do
      {parts, _} = GeoSeries.explode(s(["MULTIPOINT ((1 1), (2 2))"], crs: "EPSG:3857"))
      assert GeoSeries.crs(parts) == "EPSG:3857"
    end
  end

  describe "GeoDataFrame.explode/2" do
    setup do
      gdf =
        frame(
          [name: ["a", "b", "c"], n: [1, 2, 3]],
          ["MULTIPOINT ((1 1), (2 2))", "POINT (5 5)", "MULTIPOINT ((7 7), (8 8), (9 9))"],
          crs: "EPSG:3857"
        )

      %{gdf: gdf}
    end

    test "repeats the attributes and adds the original row position as `index`", %{gdf: gdf} do
      out = GeoDataFrame.explode(gdf)
      assert GeoDataFrame.names(out) == ["index", "name", "n", "geometry"]
      assert col(out, "index") == [0, 0, 1, 2, 2, 2]
      assert col(out, "name") == ["a", "a", "b", "c", "c", "c"]
      assert col(out, "n") == [1, 1, 2, 3, 3, 3]
      assert GeoDataFrame.crs(out) == "EPSG:3857"
    end

    test "index_parts: true adds a `part` column", %{gdf: gdf} do
      out = GeoDataFrame.explode(gdf, index_parts: true)
      assert GeoDataFrame.names(out) == ["index", "part", "name", "n", "geometry"]
      assert col(out, "part") == [0, 1, 0, 0, 1, 2]
    end

    test "ignore_index: true drops the `index` column", %{gdf: gdf} do
      out = GeoDataFrame.explode(gdf, ignore_index: true)
      assert GeoDataFrame.names(out) == ["name", "n", "geometry"]
      assert GeoDataFrame.n_rows(out) == 6
    end

    test "a name clash with the added columns raises", %{gdf: gdf} do
      clashing = GeoDataFrame.rename(gdf, %{"n" => "index"})
      assert_raise ArgumentError, ~r/index/, fn -> GeoDataFrame.explode(clashing) end
    end
  end

  # -- clip_by_rect -------------------------------------------------------------------------

  describe "GeoSeries.clip_by_rect/5" do
    test "clips row by row and keeps empty results" do
      series =
        s([
          "POLYGON ((-5 -5, 5 -5, 5 5, -5 5, -5 -5))",
          "LINESTRING (-5 5, 5 5)",
          "POINT (11 1)",
          "POINT (1 1)",
          nil
        ])

      out = GeoSeries.clip_by_rect(series, 0, 0, 10, 10)

      expected = [
        "POLYGON ((0 0, 0 5, 5 5, 5 0, 0 0))",
        "LINESTRING (0 5, 5 5)",
        "GEOMETRYCOLLECTION EMPTY",
        "POINT (1 1)"
      ]

      [last | rest] = Enum.reverse(GeoSeries.to_wkt(out))
      assert last == nil
      assert rest |> Enum.reverse() |> Enum.at(2) == "GEOMETRYCOLLECTION EMPTY"

      assert out
             |> GeoSeries.take([0, 1, 3])
             |> GeoSeries.geom_equals(s(Enum.take(expected, 2) ++ ["POINT (1 1)"]))
             |> list() == [true, true, true]
    end

    test "rejects an inverted rectangle" do
      assert_raise ArgumentError, ~r/invalid rectangle/, fn ->
        GeoSeries.clip_by_rect(s(["POINT (1 1)"]), 5, 0, 1, 10)
      end
    end
  end

  # -- intersection_all ---------------------------------------------------------------------

  describe "GeoSeries.intersection_all/1" do
    test "is a one-row series with the common part" do
      out =
        GeoSeries.intersection_all(
          s([rect(0, 0, 4, 4), nil, rect(2, 2, 4, 4), rect(3, 0, 4, 4)], crs: "EPSG:3857")
        )

      assert GeoSeries.size(out) == 1
      assert GeoSeries.crs(out) == "EPSG:3857"
      assert list(GeoSeries.area(out)) == [2.0]
    end

    test "nothing to intersect is an empty collection" do
      assert GeoSeries.to_wkt(GeoSeries.intersection_all(s([nil]))) == [
               "GEOMETRYCOLLECTION EMPTY"
             ]
    end
  end

  # -- clip ---------------------------------------------------------------------------------

  describe "GeoFrame.clip/3" do
    setup do
      gdf =
        frame(
          [id: ["a", "b", "c", "d"]],
          [rect(0, 0, 4, 4), rect(3, 3, 4, 4), rect(20, 20, 2, 2), rect(8, 0, 1, 1)]
        )

      %{gdf: gdf}
    end

    test "intersects every row with the polygon mask and drops what does not meet it", %{gdf: gdf} do
      out = GeoFrame.clip(gdf, rect(2, 2, 3, 3))
      assert col(out, "id") == ["a", "b"]
      assert areas(out) == [4.0, 4.0]
    end

    test "a WKT mask, a GeoSeries mask and a GeoDataFrame mask give the same rows", %{gdf: gdf} do
      mask = rect(2, 2, 3, 3)
      a = GeoFrame.clip(gdf, mask)
      b = GeoFrame.clip(gdf, s([mask]))
      c = GeoFrame.clip(gdf, frame([k: [1]], [mask]))
      assert areas(a) == areas(b)
      assert areas(a) == areas(c)
    end

    test "a bounding box mask clips by rectangle", %{gdf: gdf} do
      out = GeoFrame.clip(gdf, {2, 2, 5, 5})
      assert col(out, "id") == ["a", "b"]
      assert areas(out) == [4.0, 4.0]
    end

    test "rows touching the mask only along its boundary are dropped for a box", %{gdf: gdf} do
      assert col(GeoFrame.clip(gdf, {4, 0, 8, 4}), "id") == ["b"]
    end

    test "a GeoSeries is clipped to a GeoSeries" do
      out = GeoFrame.clip(s([rect(0, 0, 4, 4), rect(20, 20, 1, 1)]), rect(2, 2, 4, 4))
      assert %GeoSeries{} = out
      assert list(GeoSeries.area(out)) == [4.0]
    end

    test "keep_geom_type keeps only the original geometry type" do
      lines =
        frame([id: [1, 2]], ["LINESTRING (-2 2, 6 2)", "LINESTRING (4 4, 6 6)"])

      mask = rect(0, 0, 4, 4)

      loose = GeoFrame.clip(lines, mask)
      assert types(loose) == ["LineString", "Point"]
      strict = GeoFrame.clip(lines, mask, keep_geom_type: true)
      assert col(strict, "id") == [1]
      assert types(strict) == ["LineString"]
    end

    test "keep_geom_type drops the lower-dimensional part of a collection result" do
      polys = frame([id: [1]], [rect(0, 0, 4, 4)])
      mask = "MULTIPOLYGON (((2 2, 6 2, 6 6, 2 6, 2 2)), ((4 0, 6 0, 6 1, 4 1, 4 0)))"
      assert types(GeoFrame.clip(polys, mask)) == ["GeometryCollection"]
      strict = GeoFrame.clip(polys, mask, keep_geom_type: true)
      assert types(strict) == ["Polygon"]
    end

    test "points inside the mask are returned as they are" do
      pts = frame([id: [1, 2, 3]], ["POINT (1 1)", "POINT (9 9)", "POINT (2 2)"])
      assert col(GeoFrame.clip(pts, rect(0, 0, 3, 3)), "id") == [1, 3]
    end

    test "data wholly outside the mask gives an empty frame with the same columns", %{gdf: gdf} do
      out = GeoFrame.clip(gdf, rect(100, 100, 1, 1))
      assert GeoDataFrame.n_rows(out) == 0
      assert GeoDataFrame.names(out) == ["id", "geometry"]
    end

    test "rejects a mask that is not polygonal and a CRS mismatch", %{gdf: gdf} do
      assert_raise ArgumentError, ~r/mask/, fn -> GeoFrame.clip(gdf, "POINT (1 1)") end

      assert_raise ArgumentError, ~r/CRS/, fn ->
        GeoFrame.clip(
          GeoDataFrame.set_crs(gdf, "EPSG:3857"),
          s([rect(0, 0, 1, 1)], crs: "EPSG:4326")
        )
      end
    end
  end

  # -- overlay ------------------------------------------------------------------------------

  describe "GeoFrame.overlay/3" do
    setup do
      left =
        frame(
          [lid: ["A", "B", "C"], k: [1, 2, 3], v: [1.0, 2.0, 3.0]],
          [rect(0, 0, 4, 4), rect(6, 0, 4, 4), rect(0, 6, 4, 4)],
          crs: "EPSG:3857"
        )

      right =
        frame(
          [rid: ["X", "Y", "Z", "W"], k: [10, 20, 30, 40], w: [0.5, 1.5, 2.5, 3.5]],
          [rect(2, 2, 6, 6), rect(12, 12, 2, 2), rect(-2, -2, 3, 3), rect(10, 0, 2, 4)],
          crs: "EPSG:3857"
        )

      %{left: left, right: right}
    end

    test "intersection pairs the overlapping rows and suffixes shared names", ctx do
      out = capture_log(fn -> send(self(), GeoFrame.overlay(ctx.left, ctx.right)) end)
      assert out =~ "dropped"
      assert_received %GeoDataFrame{} = out
      assert GeoDataFrame.names(out) == ["lid", "k_1", "v", "rid", "k_2", "w", "geometry"]
      assert col(out, "lid") == ["A", "A", "B", "C"]
      assert col(out, "rid") == ["X", "Z", "X", "X"]
      assert col(out, "k_1") == [1, 1, 2, 3]
      assert col(out, "k_2") == [10, 30, 10, 10]
      assert areas(out) == [4.0, 1.0, 4.0, 4.0]
      assert GeoDataFrame.crs(out) == "EPSG:3857"
    end

    test "keep_geom_type: false keeps the shared edge as a line", ctx do
      out = GeoFrame.overlay(ctx.left, ctx.right, keep_geom_type: false)
      assert types(out) == ["Polygon", "Polygon", "Polygon", "LineString", "Polygon"]
      assert col(out, "rid") == ["X", "Z", "X", "W", "X"]
    end

    test "keep_geom_type: true drops silently", ctx do
      log =
        capture_log(fn ->
          out = GeoFrame.overlay(ctx.left, ctx.right, keep_geom_type: true)
          send(self(), {:rows, GeoDataFrame.n_rows(out)})
        end)

      assert log == ""
      assert_received {:rows, 4}
    end

    test "union keeps both sides' rows with nulls on the missing side", ctx do
      out = GeoFrame.overlay(ctx.left, ctx.right, how: :union, keep_geom_type: true)
      assert GeoDataFrame.names(out) == ["lid", "k_1", "v", "rid", "k_2", "w", "geometry"]
      assert col(out, "lid") == ["A", "A", "B", "C", "A", "B", "C", nil, nil, nil, nil]
      assert col(out, "rid") == ["X", "Z", "X", "X", nil, nil, nil, "X", "Y", "Z", "W"]
      assert col(out, "k_1") == [1, 1, 2, 3, 1, 2, 3, nil, nil, nil, nil]
      assert col(out, "w") == [0.5, 2.5, 0.5, 0.5, nil, nil, nil, 0.5, 1.5, 2.5, 3.5]
    end

    test "identity is the intersection plus the unmatched parts of the left side", ctx do
      out = GeoFrame.overlay(ctx.left, ctx.right, how: :identity, keep_geom_type: true)
      assert col(out, "lid") == ["A", "A", "B", "C", "A", "B", "C"]
      assert col(out, "rid") == ["X", "Z", "X", "X", nil, nil, nil]
    end

    test "symmetric_difference gives the left remainders then the right remainders", ctx do
      out =
        GeoFrame.overlay(ctx.left, ctx.right, how: :symmetric_difference, keep_geom_type: true)

      assert col(out, "lid") == ["A", "B", "C", nil, nil, nil, nil]
      assert col(out, "rid") == [nil, nil, nil, "X", "Y", "Z", "W"]
    end

    test "difference keeps the left columns only", ctx do
      out = GeoFrame.overlay(ctx.left, ctx.right, how: :difference)
      assert GeoDataFrame.names(out) == ["lid", "k", "v", "geometry"]
      assert col(out, "lid") == ["A", "B", "C"]
      assert Enum.map(areas(out), &Float.round(&1, 9)) == [11.0, 12.0, 12.0]
    end

    test "difference drops rows that are entirely covered" do
      left = frame([id: [1, 2]], [rect(0, 0, 2, 2), rect(10, 10, 2, 2)])
      right = frame([r: [1]], [rect(-1, -1, 5, 5)])
      out = GeoFrame.overlay(left, right, how: :difference)
      assert col(out, "id") == [2]
    end

    test "intersection of frames with disjoint extents is empty but keeps the columns", ctx do
      far = frame([z: [1]], [rect(1000, 1000, 1, 1)], crs: "EPSG:3857")
      out = GeoFrame.overlay(ctx.left, far)
      assert GeoDataFrame.n_rows(out) == 0
      assert GeoDataFrame.names(out) == ["lid", "k", "v", "z", "geometry"]
    end

    test "the documented GeoPandas example" do
      df1 = frame([df1_data: [1, 2]], [rect(0, 0, 2, 2), rect(2, 2, 2, 2)])
      df2 = frame([df2_data: [1, 2]], [rect(1, 1, 2, 2), rect(3, 3, 2, 2)])
      out = GeoFrame.overlay(df1, df2, how: :union)
      assert col(out, "df1_data") == [1, 2, 2, 1, 2, nil, nil]
      assert col(out, "df2_data") == [1, 1, 2, nil, nil, 1, 2]
    end

    test "points overlay points" do
      a = frame([a: [1, 2]], ["POINT (0 0)", "POINT (1 1)"])
      b = frame([b: [9]], ["POINT (1 1)"])
      out = GeoFrame.overlay(a, b)
      assert col(out, "a") == [2]
      assert col(out, "b") == [9]
      assert col(GeoFrame.overlay(a, b, how: :difference), "a") == [1]
    end

    test "lines: crossings are points, which keep_geom_type removes" do
      a = frame([a: [1]], ["LINESTRING (0 0, 10 0)"])
      b = frame([b: [1, 2]], ["LINESTRING (5 -5, 5 5)", "LINESTRING (8 0, 12 0)"])
      assert types(GeoFrame.overlay(a, b, keep_geom_type: false)) == ["Point", "LineString"]
      assert types(GeoFrame.overlay(a, b, keep_geom_type: true)) == ["LineString"]
    end

    test "make_valid repairs invalid polygons; make_valid: false raises" do
      bow = frame([id: [1]], ["POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))"])
      box = frame([r: [1]], [rect(-1, -1, 10, 10)])
      out = GeoFrame.overlay(bow, box, keep_geom_type: true)
      assert_in_delta hd(areas(out)), 8.0, 1.0e-9

      assert_raise ArgumentError, ~r/make_valid/, fn ->
        GeoFrame.overlay(bow, box, make_valid: false)
      end
    end

    test "rejects bad arguments", ctx do
      assert_raise ArgumentError, ~r/how/, fn ->
        GeoFrame.overlay(ctx.left, ctx.right, how: :nope)
      end

      mixed = frame([m: [1, 2]], [rect(0, 0, 1, 1), "POINT (1 1)"])

      assert_raise ArgumentError, ~r/mixed geometry types/, fn ->
        GeoFrame.overlay(mixed, ctx.right)
      end

      other = GeoDataFrame.set_crs(ctx.right, "EPSG:4326")
      assert_raise ArgumentError, ~r/CRS/, fn -> GeoFrame.overlay(ctx.left, other) end
    end
  end

  # -- properties ---------------------------------------------------------------------------

  # Disjoint rectangles: one per distinct 10 x 10 grid cell, at most 5 x 5 in size.
  defp cell_rects(offset) do
    cell = {integer(0..5), integer(0..5)}
    size = {integer(0..4), integer(0..4), integer(1..5), integer(1..5)}

    gen all(
          cells <- uniq_list_of(cell, min_length: 1, max_length: 8),
          sizes <- list_of(size, length: length(cells))
        ) do
      for {{cx, cy}, {ox, oy, w, h}} <- Enum.zip(cells, sizes),
          do: rect(cx * 10 + offset + ox, cy * 10 + offset + oy, w, h)
    end
  end

  property "the pieces of an overlay union tile the union of the inputs" do
    check all(a <- cell_rects(0), b <- cell_rects(5), max_runs: 40) do
      left = frame([i: Enum.to_list(1..length(a))], a)
      right = frame([j: Enum.to_list(1..length(b))], b)
      out = GeoFrame.overlay(left, right, how: :union, keep_geom_type: false)

      [expected] = (a ++ b) |> s() |> GeoSeries.union_all() |> GeoSeries.area() |> list()
      assert_in_delta Enum.sum(areas(out)), expected, 1.0e-6
    end
  end

  property "the area of clip(g, box) equals the area of intersection(g, box)" do
    rect_gen = {integer(0..20), integer(0..20), integer(1..10), integer(1..10)}

    check all(
            rects <- list_of(rect_gen, min_length: 1, max_length: 10),
            box <- rect_gen,
            max_runs: 40
          ) do
      wkts = for {x, y, w, h} <- rects, do: rect(x, y, w, h)
      {bx, by, bw, bh} = box
      mask = rect(bx, by, bw, bh)

      clipped = GeoFrame.clip(frame([i: Enum.to_list(1..length(wkts))], wkts), mask)
      expected = wkts |> s() |> GeoSeries.intersection(mask) |> GeoSeries.area() |> list()

      assert_in_delta Enum.sum(areas(clipped)), Enum.sum(expected), 1.0e-6

      by_box =
        GeoFrame.clip(frame([i: Enum.to_list(1..length(wkts))], wkts), {bx, by, bx + bw, by + bh})

      assert_in_delta Enum.sum(areas(by_box)), Enum.sum(expected), 1.0e-6
    end
  end

  property "explode then dissolve by the original index restores the area" do
    check all(groups <- list_of(cell_rects(0), min_length: 1, max_length: 5), max_runs: 40) do
      multis =
        for rects <- groups,
            do: "MULTIPOLYGON (" <> Enum.map_join(rects, ", ", &polygon_body/1) <> ")"

      gdf = frame([id: Enum.to_list(1..length(multis))], multis)

      restored = gdf |> GeoDataFrame.explode() |> Dissolve.dissolve(by: "index")

      assert_in_delta Enum.sum(areas(restored)), Enum.sum(areas(gdf)), 1.0e-6
      assert col(restored, "index") == Enum.to_list(0..(length(multis) - 1))
    end
  end

  defp polygon_body("POLYGON " <> body), do: body
end
