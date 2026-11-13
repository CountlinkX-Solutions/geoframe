defmodule GeoFrame.ParityATest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @donut "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2), (6 6, 8 6, 8 8, 6 8, 6 6))"
  @bowtie "POLYGON ((0 0, 2 2, 2 0, 0 2, 0 0))"
  @square "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(%Series{} = series), do: Series.to_list(series)

  describe "from_xy / points_from_xy" do
    test "builds points from lists, nil gives a null point, and keeps the CRS" do
      out = GeoSeries.from_xy([1, 2.5, nil, 4], [2, 3.5, 5, nil], crs: "EPSG:4326")
      assert GeoSeries.crs(out) == "EPSG:4326"
      assert list(GeoSeries.x(out)) == [1.0, 2.5, nil, nil]
      assert list(GeoSeries.y(out)) == [2.0, 3.5, nil, nil]
      assert list(GeoSeries.isna(out)) == [false, false, true, true]
    end

    test "accepts Explorer series, treating NaN as missing" do
      x = Series.from_list([1.0, :nan, 3.0])
      y = Series.from_list([4.0, 5.0, nil])
      out = GeoSeries.from_xy(x, y)
      assert GeoSeries.to_wkt(out) == ["POINT(1 4)", nil, nil]
    end

    test "z builds a POINT Z and is length-checked" do
      out = GeoSeries.from_xy([1], [2], z: [3])
      assert GeoSeries.to_wkt(out) == ["POINT Z(1 2 3)"]
      assert_raise ArgumentError, fn -> GeoSeries.from_xy([1], [2], z: [3, 4]) end
    end

    test "rejects mismatched lengths and unknown options" do
      assert_raise ArgumentError, fn -> GeoSeries.from_xy([1, 2], [1]) end
      assert_raise ArgumentError, fn -> GeoSeries.from_xy([1], [1], bogus: 1) end
    end

    test "GeoFrame.points_from_xy/3 is an alias" do
      out = GeoFrame.points_from_xy([1.0], [2.0], crs: "EPSG:3857")
      assert GeoSeries.to_wkt(out) == ["POINT(1 2)"]
      assert GeoSeries.crs(out) == "EPSG:3857"
    end

    property "x and y round-trip" do
      check all(
              pairs <-
                list_of({float(min: -1.0e6, max: 1.0e6), float(min: -1.0e6, max: 1.0e6)},
                  max_length: 30
                )
            ) do
        {xs, ys} = Enum.unzip(pairs)
        out = GeoSeries.from_xy(xs, ys)
        assert list(GeoSeries.x(out)) == xs
        assert list(GeoSeries.y(out)) == ys
      end
    end
  end

  describe "from_features" do
    @features [
      %{
        "type" => "Feature",
        "properties" => %{"name" => "a", "n" => 1},
        "geometry" => %{"type" => "Point", "coordinates" => [1, 2]}
      },
      %{
        "type" => "Feature",
        "properties" => %{"name" => "b", "n" => 2.5},
        "geometry" => nil
      }
    ]

    test "builds a frame from a feature list with GeoJSON type inference" do
      {:ok, gdf} = GeoDataFrame.from_features(@features)
      # Elixir maps carry no insertion order, so properties come out sorted by key.
      assert GeoDataFrame.names(gdf) == ["n", "name", "geometry"]
      assert GeoDataFrame.n_rows(gdf) == 2
      assert gdf.data["n"] |> list() == [1.0, 2.5]
      assert GeoSeries.to_wkt(gdf.geometry) == ["POINT(1 2)", nil]
      assert GeoDataFrame.crs(gdf) == nil
    end

    test "accepts a FeatureCollection map, atom keys, crs and columns" do
      collection = %{type: "FeatureCollection", features: @features}

      gdf =
        GeoDataFrame.from_features!(collection,
          crs: "EPSG:4326",
          columns: ["name"],
          geometry_column: "geom"
        )

      assert GeoDataFrame.names(gdf) == ["name", "geom"]
      assert GeoDataFrame.crs(gdf) == "EPSG:4326"
    end

    # GeoPandas 1.2: from_features([{"b":1,"a":2}, {"c":3,"a":5}]).columns
    #   == ["geometry", "b", "a", "c"] (first appearance across features); columns=["c","a"]
    #   gives ["c","a"].
    test "keeps property order for keyword lists and pair lists (first appearance)" do
      point = %{"type" => "Point", "coordinates" => [0, 0]}

      features = [
        %{"type" => "Feature", "geometry" => point, "properties" => [{"b", 1}, {"a", 2}]},
        %{"type" => "Feature", "geometry" => point, "properties" => [c: 3, a: 5]}
      ]

      gdf = GeoDataFrame.from_features!(features)
      assert GeoDataFrame.names(gdf) == ["b", "a", "c", "geometry"]
      assert gdf.data["b"] |> list() == [1, nil]
      assert gdf.data["c"] |> list() == [nil, 3]

      assert GeoDataFrame.names(GeoDataFrame.from_features!(features, columns: ["c", "a"])) ==
               ["c", "a", "geometry"]
    end

    test "keeps property order of JSON decoded with ordered objects" do
      json = ~s({"type":"FeatureCollection","features":[{"type":"Feature","geometry":null,
        "properties":{"z":1,"y":2,"x":3}}]})

      collection = Jason.decode!(json, objects: :ordered_objects)

      assert GeoDataFrame.names(GeoDataFrame.from_features!(collection)) == [
               "z",
               "y",
               "x",
               "geometry"
             ]
    end

    test "plain map properties stay sorted by name" do
      features = [
        %{"type" => "Feature", "geometry" => nil, "properties" => %{"b" => 1, "a" => 2}}
      ]

      assert GeoDataFrame.names(GeoDataFrame.from_features!(features)) == ["a", "b", "geometry"]
    end

    test "errors on invalid input" do
      assert {:error, _} = GeoDataFrame.from_features([%{"type" => "Feature", "geometry" => 5}])
      assert {:error, message} = GeoDataFrame.from_features(@features, columns: ["missing"])
      assert message =~ "missing"
    end
  end

  describe "get_coordinates" do
    test "returns x and y, skipping nulls and empties" do
      out = s(["POINT (1 2)", nil, "LINESTRING (0 0, 1 1)", "POINT EMPTY"])
      df = GeoSeries.get_coordinates(out)
      assert DataFrame.names(df) == ["x", "y"]
      assert df["x"] |> list() == [1.0, 0.0, 1.0]
      assert df["y"] |> list() == [2.0, 0.0, 1.0]
    end

    test "include_index adds the row position" do
      df =
        GeoSeries.get_coordinates(s(["POINT (1 2)", nil, "LINESTRING (0 0, 1 1)"]),
          include_index: true
        )

      assert DataFrame.names(df) == ["index", "x", "y"]
      assert df["index"] |> list() == [0, 2, 2]
    end

    test "empty result keeps the columns" do
      df = GeoSeries.get_coordinates(s([nil]))
      assert DataFrame.n_rows(df) == 0
      assert DataFrame.names(df) == ["x", "y"]
    end

    property "row count equals the sum of count_coordinates" do
      check all(
              n <- integer(1..6),
              coords <- list_of(float(min: -100.0, max: 100.0), length: n * 2)
            ) do
        points = coords |> Enum.chunk_every(2) |> Enum.map(fn [a, b] -> "POINT (#{a} #{b})" end)

        line =
          "LINESTRING (" <>
            Enum.map_join(Enum.chunk_every(coords, 2), ", ", fn [a, b] -> "#{a} #{b}" end) <> ")"

        series = s([line, nil | points])
        total = series |> GeoSeries.count_coordinates() |> Series.sum()
        assert DataFrame.n_rows(GeoSeries.get_coordinates(series)) == total
      end
    end
  end

  describe "counts, parts and interiors" do
    test "count_geometries" do
      series =
        s(["POINT (0 0)", "MULTIPOINT (0 0, 1 1, 2 2)", "GEOMETRYCOLLECTION EMPTY", nil])

      assert list(GeoSeries.count_geometries(series)) == [1, 3, 0, nil]
    end

    test "count_interior_rings" do
      series = s([@donut, @square, "POINT (0 0)", nil])
      assert list(GeoSeries.count_interior_rings(series)) == [2, 0, 0, nil]
    end

    test "get_geometry with positive, negative and out-of-range indices" do
      series = s(["MULTIPOINT (0 0, 1 1, 2 2)", "POINT (7 7)", nil])
      assert GeoSeries.to_wkt(GeoSeries.get_geometry(series, 1)) == ["POINT(1 1)", nil, nil]

      assert GeoSeries.to_wkt(GeoSeries.get_geometry(series, -1)) == [
               "POINT(2 2)",
               "POINT(7 7)",
               nil
             ]

      assert GeoSeries.to_wkt(GeoSeries.get_geometry(series, 3)) == [nil, nil, nil]
      assert_raise FunctionClauseError, fn -> GeoSeries.get_geometry(series, 1.5) end
    end

    test "interiors returns a multilinestring of holes" do
      out = GeoSeries.interiors(s([@donut, @square, "POINT (0 0)"]))

      assert GeoSeries.to_wkt(out) == [
               "MULTILINESTRING((2 2,4 2,4 4,2 4,2 2),(6 6,8 6,8 8,6 8,6 6))",
               "MULTILINESTRING EMPTY",
               nil
             ]
    end

    test "extract_unique_points and mean_coordinate" do
      series = s(["POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))", nil])

      assert GeoSeries.to_wkt(GeoSeries.extract_unique_points(series)) ==
               ["MULTIPOINT((0 0),(4 0),(4 4),(0 4))", nil]

      assert GeoSeries.to_wkt(GeoSeries.mean_coordinate(series)) == ["POINT(2 2)", nil]
    end
  end

  describe "collect" do
    test "combines by family and keeps the CRS" do
      out = GeoSeries.collect(s(["POINT (0 0)", nil, "POINT (1 1)"], crs: "EPSG:3857"))
      assert GeoSeries.to_wkt(out) == ["MULTIPOINT((0 0),(1 1))"]
      assert GeoSeries.crs(out) == "EPSG:3857"
    end

    test "mixed families give a GeometryCollection and multi: true forces Multi*" do
      mixed = s(["POINT (0 0)", "LINESTRING (0 0, 1 1)"])
      assert list(GeoSeries.geom_type(GeoSeries.collect(mixed))) == ["GeometryCollection"]
      single = s(["POINT (3 3)"])
      assert GeoSeries.to_wkt(GeoSeries.collect(single)) == ["POINT(3 3)"]
      assert GeoSeries.to_wkt(GeoSeries.collect(single, multi: true)) == ["MULTIPOINT((3 3))"]
      assert_raise ArgumentError, fn -> GeoSeries.collect(single, multi: 1) end
    end
  end

  describe "isna / notna / fillna" do
    test "only actual nulls are missing, not empty geometries" do
      series = s(["POINT (0 0)", nil, "POINT EMPTY"])
      assert list(GeoSeries.isna(series)) == [false, true, false]
      assert list(GeoSeries.notna(series)) == [true, false, true]
    end

    property "isna xor notna" do
      check all(flags <- list_of(boolean(), max_length: 20)) do
        series = s(Enum.map(flags, &if(&1, do: "POINT (1 1)")))
        na = list(GeoSeries.isna(series))
        notna = list(GeoSeries.notna(series))
        assert Enum.zip_with(na, notna, &(&1 != &2)) |> Enum.all?()
      end
    end

    test "fillna defaults to an empty collection and accepts WKT or a one-row series" do
      series = s(["POINT (0 0)", nil])

      assert GeoSeries.to_wkt(GeoSeries.fillna(series)) == [
               "POINT(0 0)",
               "GEOMETRYCOLLECTION EMPTY"
             ]

      assert GeoSeries.to_wkt(GeoSeries.fillna(series, "POINT (9 9)")) == [
               "POINT(0 0)",
               "POINT(9 9)"
             ]

      assert GeoSeries.to_wkt(GeoSeries.fillna(series, s(["POINT (8 8)"]))) == [
               "POINT(0 0)",
               "POINT(8 8)"
             ]

      assert_raise ArgumentError, fn ->
        GeoSeries.fillna(series, s(["POINT (1 1)", "POINT (2 2)"]))
      end

      assert_raise ArgumentError, fn ->
        GeoSeries.fillna(s([nil], crs: "EPSG:4326"), s(["POINT (1 1)"], crs: "EPSG:3857"))
      end
    end
  end

  describe "line predicates and validity" do
    test "is_closed" do
      series = s(["LINESTRING (0 0, 1 0, 1 1, 0 0)", "LINESTRING (0 0, 1 0)", @square, nil])
      assert list(GeoSeries.is_closed(series)) == [true, false, false, nil]
    end

    test "is_ring" do
      series =
        s([
          "LINESTRING (0 0, 4 0, 4 4, 0 4, 0 0)",
          "LINESTRING (0 0, 2 2, 2 0, 0 2, 0 0)",
          "POINT (0 0)",
          nil
        ])

      assert list(GeoSeries.is_ring(series)) == [true, false, false, nil]
    end

    # Shapely 2.2.0 / GEOS 3.14.1:
    #   g = wkt.loads("POLYGON ((0 0, 1 1, 2 2, 0 0))")
    #   g.is_valid -> False; explain_validity(g) -> "Self-intersection[1 1]"
    #   make_valid(g).wkt -> "MULTILINESTRING ((0 0, 1 1), (1 1, 2 2))"
    test "a zero-area collinear ring is invalid and make_valid collapses it to its lines" do
      flat = "POLYGON ((0 0, 1 1, 2 2, 0 0))"
      series = s([flat, @square, nil])

      assert list(GeoSeries.is_valid(series)) == [false, true, nil]
      assert [reason, "Valid Geometry", nil] = list(GeoSeries.is_valid_reason(series))
      assert reason =~ "Self-intersection"
      assert list(GeoSeries.isna(GeoSeries.null_out_invalid(series))) == [true, false, true]

      repaired = GeoSeries.make_valid(series)
      assert list(GeoSeries.geom_type(repaired)) == ["MultiLineString", "Polygon", nil]
      assert list(GeoSeries.is_valid(repaired)) == [true, true, nil]

      assert [line | _] = GeoSeries.to_wkt(repaired)
      assert line =~ ~r/^MULTILINESTRING\s*\(\(0 0,\s*1 1\),\s*\(1 1,\s*2 2\)\)$/
    end

    test "is_valid_reason and null_out_invalid" do
      series = s([@square, @bowtie, nil])
      assert [valid, reason, nil] = list(GeoSeries.is_valid_reason(series))
      assert valid == "Valid Geometry"
      assert is_binary(reason) and reason != "Valid Geometry"

      out = GeoSeries.null_out_invalid(series)
      assert list(GeoSeries.isna(out)) == [false, true, true]

      assert GeoSeries.crs(GeoSeries.null_out_invalid(s([@bowtie], crs: "EPSG:4326"))) ==
               "EPSG:4326"
    end
  end

  describe "to_wkb hex" do
    test "hex: true gives upper-case hex strings" do
      series = s(["POINT (1 2)", nil])
      [binary, nil] = GeoSeries.to_wkb(series)
      assert GeoSeries.to_wkb(series, hex: true) == [Base.encode16(binary), nil]
      assert GeoSeries.to_wkb(series, hex: false) == [binary, nil]
      assert GeoSeries.to_wkb(series) == [binary, nil]
      assert_raise ArgumentError, fn -> GeoSeries.to_wkb(series, hex: 1) end
    end

    test "hex output parses back through from_wkb" do
      [hex] = GeoSeries.to_wkb(s(["POINT (1 2)"]), hex: true)
      assert GeoSeries.to_wkt(GeoSeries.from_wkb!([Base.decode16!(hex)])) == ["POINT(1 2)"]
    end
  end
end
