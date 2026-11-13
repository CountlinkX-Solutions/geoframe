defmodule GeoFrame.ParityITest do
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @moduletag :tmp_dir

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(%Series{} = series), do: Series.to_list(series)
  defp canon(wkt), do: [wkt] |> s() |> GeoSeries.to_wkt() |> hd()

  describe "is_simple/1 and self_intersects/1" do
    test "follow Shapely" do
      series =
        s([
          "POINT (1 1)",
          "MULTIPOINT ((0 0), (0 0))",
          "LINESTRING (0 0, 4 4, 4 0, 0 4)",
          "LINESTRING (0 0, 4 0, 4 4, 0 0)",
          "MULTILINESTRING ((0 0, 2 0), (2 0, 4 0))",
          "POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))",
          "GEOMETRYCOLLECTION (POINT (1 1), POINT (2 2))",
          nil
        ])

      assert list(GeoSeries.is_simple(series)) ==
               [true, false, false, true, true, false, false, nil]

      assert list(GeoSeries.self_intersects(series)) ==
               [false, false, true, false, false, false, false, nil]
    end
  end

  describe "normalize/1" do
    test "gives the GEOS canonical form and keeps CRS and nulls" do
      series =
        s(["POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))", "LINESTRING (3 3, 1 1)", nil], crs: "EPSG:4326")

      result = GeoSeries.normalize(series)

      assert GeoSeries.crs(result) == "EPSG:4326"

      assert GeoSeries.to_wkt(result) ==
               [canon("POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))"), canon("LINESTRING (1 1, 3 3)"), nil]
    end

    test "sorts the parts of multi geometries in descending order" do
      series = s(["MULTIPOINT ((0 0), (5 5), (1 3))"])

      assert GeoSeries.to_wkt(GeoSeries.normalize(series)) ==
               [canon("MULTIPOINT ((5 5), (1 3), (0 0))")]
    end
  end

  describe "set_precision/3 and get_precision/1" do
    test "an invalid multipolygon is merged where GEOS raises (decision, 0.3.0 B13)" do
      # Shapely 2.2 / GEOS 3.14:
      #   g = from_wkt("MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((1 1, 3 1, 3 3, 1 3, 1 1)))")
      #   set_precision(g, 0.1, mode="valid_output" | "keep_collapsed")
      #     -> GEOSException: TopologyException: side location conflict at 1 2
      #   set_precision(g, 0.1, mode="pointwise") -> g unchanged (still invalid)
      # GeoFrame keeps the merged, valid result instead of raising (see docs/parity.md).
      wkt = "MULTIPOLYGON (((0 0, 2 0, 2 2, 0 2, 0 0)), ((1 1, 3 1, 3 3, 1 3, 1 1)))"

      for mode <- [:valid_output, :keep_collapsed] do
        out = GeoSeries.set_precision(s([wkt]), 0.1, mode: mode)
        assert GeoSeries.to_wkt(out) == ["MULTIPOLYGON(((0 2,0 0,2 0,2 1,3 1,3 3,1 3,1 2,0 2)))"]
        assert Series.to_list(GeoSeries.is_valid(out)) == [true]
      end

      out = GeoSeries.set_precision(s([wkt]), 0.1, mode: :pointwise)

      assert GeoSeries.to_wkt(out) == [
               "MULTIPOLYGON(((0 0,2 0,2 2,0 2,0 0)),((1 1,3 1,3 3,1 3,1 1)))"
             ]
    end

    test "a new series has floating precision" do
      assert list(GeoSeries.get_precision(s(["POINT (1 1)", nil]))) == [0.0, nil]
    end

    test "pointwise snaps coordinates and records the grid" do
      result = GeoSeries.set_precision(s(["POINT (1.234 5.678)", nil]), 0.1, mode: :pointwise)

      assert GeoSeries.to_wkt(result) == [canon("POINT (1.2 5.7)"), nil]
      assert list(GeoSeries.get_precision(result)) == [0.1, nil]
      assert result.precision == 0.1
    end

    test "valid_output is the default and drops collapsed polygons" do
      series = s(["POLYGON ((0 0, 0.2 0, 0.2 0.2, 0 0.2, 0 0))", "POINT (0.6 0.6)"])
      result = GeoSeries.set_precision(series, 1)

      assert GeoSeries.to_wkt(result) == [canon("POLYGON EMPTY"), canon("POINT (1 1)")]
      assert list(GeoSeries.get_precision(result)) == [1.0, 1.0]
    end

    test "keep_collapsed keeps a collapsed line" do
      result =
        GeoSeries.set_precision(s(["LINESTRING (0.1 0.1, 0.2 0.2)"]), 1, mode: :keep_collapsed)

      assert GeoSeries.to_wkt(result) == [canon("LINESTRING (0 0, 0 0)")]
    end

    test "grid size 0 resets to floating precision" do
      snapped = GeoSeries.set_precision(s(["POINT (1.4 1.4)"]), 1)
      reset = GeoSeries.set_precision(snapped, 0)

      assert list(GeoSeries.get_precision(reset)) == [0.0]
      assert GeoSeries.to_wkt(reset) == GeoSeries.to_wkt(snapped)
    end

    test "other operations return floating precision" do
      snapped = GeoSeries.set_precision(s(["POINT (1.4 1.4)"]), 1)
      assert list(GeoSeries.get_precision(GeoSeries.reverse(snapped))) == [0.0]
    end

    test "validates the arguments" do
      series = s(["POINT (1 1)"])
      assert_raise ArgumentError, ~r/mode/, fn -> GeoSeries.set_precision(series, 1, mode: :x) end
      assert_raise ArgumentError, ~r/grid_size/, fn -> GeoSeries.set_precision(series, :nan) end

      assert_raise ArgumentError, ~r/too small/, fn ->
        GeoSeries.set_precision(series, 1.0e-320)
      end

      assert_raise ArgumentError, fn -> GeoSeries.set_precision(series, 1, bogus: true) end
    end
  end

  describe "constructors from vertices" do
    test "line_strings_from_vertices/2" do
      result =
        GeoSeries.line_strings_from_vertices([[{0, 0}, {1, 1}, [2, 0]], nil, []],
          crs: "EPSG:4326"
        )

      assert GeoSeries.crs(result) == "EPSG:4326"

      assert GeoSeries.to_wkt(result) ==
               [canon("LINESTRING (0 0, 1 1, 2 0)"), nil, canon("LINESTRING EMPTY")]
    end

    test "multi_points_from_vertices/2" do
      result = GeoSeries.multi_points_from_vertices([[{0, 0}, {1, 2}]])
      assert GeoSeries.to_wkt(result) == [canon("MULTIPOINT ((0 0), (1 2))")]
    end

    test "polygons_from_rings/2 closes open rings" do
      result =
        GeoSeries.polygons_from_rings([
          [[{0, 0}, {4, 0}, {4, 4}, {0, 4}], [{1, 1}, {2, 1}, {2, 2}]]
        ])

      assert GeoSeries.to_wkt(result) ==
               [canon("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 1))")]
    end

    test "multi_line_strings_from_lines/2 and multi_polygons_from_polygons/2" do
      lines = GeoSeries.multi_line_strings_from_lines([[[{0, 0}, {1, 1}], [{5, 5}, {6, 6}]]])
      assert GeoSeries.to_wkt(lines) == [canon("MULTILINESTRING ((0 0, 1 1), (5 5, 6 6))")]

      square = fn x -> [[{x, 0}, {x + 1, 0}, {x + 1, 1}, {x, 1}]] end
      polygons = GeoSeries.multi_polygons_from_polygons([[square.(0), square.(5)]])

      assert GeoSeries.to_wkt(polygons) ==
               [canon("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 1, 0 0)), ((5 0, 6 0, 6 1, 5 1, 5 0)))")]
    end

    test "invalid input raises ArgumentError naming the row" do
      assert_raise ArgumentError, ~r/row 1.*two/, fn ->
        GeoSeries.line_strings_from_vertices([[{0, 0}, {1, 1}], [{0, 0}]])
      end

      assert_raise ArgumentError, ~r/ring/, fn ->
        GeoSeries.polygons_from_rings([[[{0, 0}, {1, 1}]]])
      end

      assert_raise ArgumentError, ~r/vertex/, fn ->
        GeoSeries.line_strings_from_vertices([[{0, 0}, {"a", 1}]])
      end

      assert_raise ArgumentError, ~r/list of rows/, fn ->
        GeoSeries.line_strings_from_vertices(:nope)
      end
    end
  end

  describe "to_file/3 and from_file/2" do
    @tag :tmp_dir
    test "round-trip a series through GeoJSON and GeoPackage", %{tmp_dir: dir} do
      series = s(["POINT (1 2)", nil, "LINESTRING (0 0, 1 1)"], crs: "EPSG:4326")

      for name <- ["a.geojson", "a.gpkg", "a.fgb", "a.parquet"] do
        path = Path.join(dir, name)
        assert :ok = GeoSeries.to_file(series, path)
        assert {:ok, back} = GeoSeries.from_file(path)
        assert GeoSeries.to_wkt(back) == GeoSeries.to_wkt(series), name
      end
    end

    @tag :tmp_dir
    test "a Shapefile round-trip and a bang variant", %{tmp_dir: dir} do
      series = s(["POINT (1 2)", "POINT (3 4)"], crs: "EPSG:4326")
      path = Path.join(dir, "a.shp")

      assert :ok = GeoSeries.to_file(series, path)
      assert GeoSeries.to_wkt(GeoSeries.from_file!(path)) == GeoSeries.to_wkt(series)
    end

    test "errors are returned or raised", %{tmp_dir: dir} do
      missing = Path.join(dir, "missing.geojson")
      assert {:error, _} = GeoSeries.from_file(missing)
      assert_raise ArgumentError, fn -> GeoSeries.from_file!(missing) end
    end

    @tag :tmp_dir
    test "from_file passes options such as layer", %{tmp_dir: dir} do
      path = Path.join(dir, "layers.gpkg")
      a = s(["POINT (1 1)"], crs: "EPSG:4326")
      b = s(["POINT (2 2)", "POINT (3 3)"], crs: "EPSG:4326")
      :ok = GeoSeries.to_file(a, path, layer: "a")
      :ok = GeoSeries.to_file(b, path, layer: "b", mode: :a)
      assert GeoSeries.size(GeoSeries.from_file!(path, layer: "b")) == 2
    end
  end

  describe "to_json/2" do
    setup do
      series = s(["POINT (1 2)", nil, "POLYGON ((0 0, 1 0, 1 1, 0 0))"], crs: "EPSG:4326")
      %{series: series}
    end

    test "is a FeatureCollection string with ids and bounding boxes", %{series: series} do
      doc = series |> GeoSeries.to_json() |> Jason.decode!()

      assert doc["type"] == "FeatureCollection"
      assert doc["bbox"] == [0.0, 0.0, 1.0, 2.0]
      refute Map.has_key?(doc, "crs")

      assert doc["features"] == [
               %{
                 "id" => "0",
                 "type" => "Feature",
                 "properties" => %{},
                 "geometry" => %{"type" => "Point", "coordinates" => [1.0, 2.0]},
                 "bbox" => [1.0, 2.0, 1.0, 2.0]
               },
               %{
                 "id" => "1",
                 "type" => "Feature",
                 "properties" => %{},
                 "geometry" => nil,
                 "bbox" => nil
               },
               %{
                 "id" => "2",
                 "type" => "Feature",
                 "properties" => %{},
                 "geometry" => %{
                   "type" => "Polygon",
                   "coordinates" => [[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]]
                 },
                 "bbox" => [0.0, 0.0, 1.0, 1.0]
               }
             ]
    end

    test "show_bbox: false and drop_id: true", %{series: series} do
      doc = series |> GeoSeries.to_json(show_bbox: false, drop_id: true) |> Jason.decode!()

      refute Map.has_key?(doc, "bbox")

      assert Enum.all?(
               doc["features"],
               &(not Map.has_key?(&1, "id") and not Map.has_key?(&1, "bbox"))
             )
    end

    test "a projected CRS adds the crs member, to_wgs84 reprojects", %{series: _} do
      projected = s(["POINT (500000 100)"], crs: "EPSG:32617")
      doc = projected |> GeoSeries.to_json() |> Jason.decode!()

      assert doc["crs"] == %{
               "type" => "name",
               "properties" => %{"name" => "urn:ogc:def:crs:EPSG::32617"}
             }

      wgs = projected |> GeoSeries.to_json(to_wgs84: true) |> Jason.decode!()
      refute Map.has_key?(wgs, "crs")
      [x, _y] = hd(wgs["features"])["geometry"]["coordinates"]
      assert_in_delta x, -81.0, 1.0e-9
    end

    test "rejects unknown options", %{series: series} do
      assert_raise ArgumentError, fn -> GeoSeries.to_json(series, na: :keep) end
    end
  end

  describe "GeoDataFrame.to_json/2" do
    setup do
      data =
        DataFrame.new(a: [1, 2, 3], b: ["x", nil, "z"], c: [1.5, nil, :nan])

      geometry = s(["POINT (1 2)", nil, "POINT (3 4)"], crs: "EPSG:4326")
      %{gdf: GeoDataFrame.new(data, geometry)}
    end

    test "null properties are null by default and no bbox", %{gdf: gdf} do
      doc = gdf |> GeoDataFrame.to_json() |> Jason.decode!()

      refute Map.has_key?(doc, "bbox")

      assert [
               %{"id" => "0", "properties" => %{"a" => 1, "b" => "x", "c" => 1.5}},
               %{
                 "id" => "1",
                 "properties" => %{"a" => 2, "b" => nil, "c" => nil},
                 "geometry" => nil
               },
               %{"id" => "2", "properties" => %{"a" => 3, "b" => "z", "c" => nil}}
             ] = doc["features"]

      refute Enum.any?(doc["features"], &Map.has_key?(&1, "bbox"))
    end

    test "na: :drop removes null properties", %{gdf: gdf} do
      doc =
        gdf |> GeoDataFrame.to_json(na: :drop, show_bbox: true, drop_id: true) |> Jason.decode!()

      assert Enum.map(doc["features"], & &1["properties"]) ==
               [%{"a" => 1, "b" => "x", "c" => 1.5}, %{"a" => 2}, %{"a" => 3, "b" => "z"}]

      assert doc["bbox"] == [1.0, 2.0, 3.0, 4.0]
      assert Enum.all?(doc["features"], &(not Map.has_key?(&1, "id")))
    end

    test "na: :keep is rejected because JSON has no NaN", %{gdf: gdf} do
      assert_raise ArgumentError, ~r/na/, fn -> GeoDataFrame.to_json(gdf, na: :keep) end
      assert_raise ArgumentError, ~r/na/, fn -> GeoDataFrame.to_json(gdf, na: :bogus) end
    end

    test "dates are encoded as ISO 8601 strings" do
      data = DataFrame.new(d: [~D[2024-01-02]])
      gdf = GeoDataFrame.new(data, s(["POINT (0 0)"], crs: "EPSG:4326"))
      doc = gdf |> GeoDataFrame.to_json() |> Jason.decode!()
      assert hd(doc["features"])["properties"] == %{"d" => "2024-01-02"}
    end
  end

  describe "to_geo_interface/1" do
    test "series" do
      series = s(["POINT (1 2)", nil], crs: "EPSG:4326")

      assert GeoSeries.to_geo_interface(series) == %{
               "type" => "FeatureCollection",
               "bbox" => [1.0, 2.0, 1.0, 2.0],
               "features" => [
                 %{
                   "id" => "0",
                   "type" => "Feature",
                   "properties" => %{},
                   "geometry" => %{"type" => "Point", "coordinates" => [1.0, 2.0]},
                   "bbox" => [1.0, 2.0, 1.0, 2.0]
                 },
                 %{
                   "id" => "1",
                   "type" => "Feature",
                   "properties" => %{},
                   "geometry" => nil,
                   "bbox" => nil
                 }
               ]
             }
    end

    test "data frame has properties and no crs member" do
      gdf =
        GeoDataFrame.new(DataFrame.new(a: [1]), s(["POINT (1 2)"], crs: "EPSG:32617"))

      geo = GeoDataFrame.to_geo_interface(gdf)
      assert geo["bbox"] == [1.0, 2.0, 1.0, 2.0]
      refute Map.has_key?(geo, "crs")
      assert [%{"id" => "0", "properties" => %{"a" => 1}}] = geo["features"]
    end
  end

  # GeoPandas 1.2: g = GeoDataFrame({"a": [1, None], "geometry": [Point(0, 0), Point(1, 1)]})
  #   g.to_geo_dict()  -> ids, no bbox; g.to_geo_dict(na="drop", show_bbox=True, drop_id=True)
  #   -> no ids, bboxes, the missing "a" dropped; g.to_geo_dict(na="keep") keeps NaN.
  describe "to_geo_dict/2" do
    defp options_frame do
      data = DataFrame.new(a: Series.from_list([1.0, nil]))
      GeoDataFrame.new(data, s(["POINT (0 0)", "POINT (1 1)"], crs: "EPSG:4326"))
    end

    test "an empty option list equals the defaults: ids, null kept, no bbox" do
      gdf = options_frame()
      geo = GeoDataFrame.to_geo_dict(gdf)
      assert geo == GeoDataFrame.to_geo_dict(gdf, [])
      refute Map.has_key?(geo, "bbox")

      assert [
               %{"id" => "0", "properties" => %{"a" => 1.0}} = first,
               %{"id" => "1", "properties" => %{"a" => nil}}
             ] = geo["features"]

      refute Map.has_key?(first, "bbox")
    end

    test "na: :drop, show_bbox and drop_id" do
      geo =
        GeoDataFrame.to_geo_dict(options_frame(), na: :drop, show_bbox: true, drop_id: true)

      assert geo["bbox"] == [0.0, 0.0, 1.0, 1.0]

      assert [
               %{"properties" => %{"a" => 1.0}, "bbox" => [+0.0, +0.0, +0.0, +0.0]} = first,
               %{"properties" => props, "bbox" => [1.0, 1.0, 1.0, 1.0]}
             ] = geo["features"]

      refute Map.has_key?(first, "id")
      assert props == %{}
    end

    test "na: :keep keeps NaN as it is" do
      data = DataFrame.new(a: Series.from_list([1.0, :nan]), b: Series.from_list([1, nil]))
      gdf = GeoDataFrame.new(data, s(["POINT (0 0)", "POINT (1 1)"]))
      geo = GeoDataFrame.to_geo_dict(gdf, na: :keep)
      assert [_, %{"properties" => %{"a" => :nan, "b" => nil}}] = geo["features"]
    end

    test "to_geo_interface/1 keeps the __geo_interface__ defaults" do
      geo = GeoDataFrame.to_geo_interface(options_frame())
      assert geo["bbox"] == [0.0, 0.0, 1.0, 1.0]
      assert [%{"id" => "0", "bbox" => [+0.0, +0.0, +0.0, +0.0]} | _] = geo["features"]
    end

    test "series: GeoSeries.to_geo_dict defaults show_bbox to true" do
      series = s(["POINT (1 2)"])

      assert GeoSeries.to_geo_dict(series) == GeoSeries.to_geo_dict(series, [])
      assert GeoSeries.to_geo_dict(series) == GeoSeries.to_geo_interface(series)

      assert [%{"id" => "0", "bbox" => [1.0, 2.0, 1.0, 2.0]}] =
               GeoSeries.to_geo_dict(series)["features"]

      geo = GeoSeries.to_geo_dict(series, show_bbox: false, drop_id: true)
      refute Map.has_key?(geo, "bbox")
      assert [feature] = geo["features"]
      assert Map.keys(feature) |> Enum.sort() == ["geometry", "properties", "type"]
    end

    test "invalid options raise" do
      assert_raise ArgumentError, ~r/na/, fn ->
        GeoDataFrame.to_geo_dict(options_frame(), na: :bogus)
      end

      assert_raise ArgumentError, fn -> GeoDataFrame.to_geo_dict(options_frame(), foo: 1) end
      assert_raise ArgumentError, fn -> GeoSeries.to_geo_dict(s(["POINT (0 0)"]), foo: 1) end
    end
  end

  # GeoPandas 1.2: g = GeoDataFrame({"a": [1, None], "geometry": [Point(0, 0), Point(1, 1)]},
  #   crs=4326); g["g2"] = GeoSeries([Point(2, 2), None], crs=4326)
  #   g.to_wkb() / g.to_wkt() -> a DataFrame (columns a, geometry, g2) with every geometry
  #   column encoded (a null geometry gives None); to_wkb(hex=True) gives hex strings;
  #   g.rename_geometry("a") and ("g2") and ("geometry") raise
  #   ValueError("Column named <name> already exists"); rename_geometry("shape") renames.
  describe "frame to_wkb/2, to_wkt/2 and rename_geometry/2" do
    defp wkb_frame do
      data = DataFrame.new(a: Series.from_list([1, nil]))
      gdf = GeoDataFrame.new(data, s(["POINT (0 0)", "POINT (1 1)"], crs: "EPSG:4326"))
      GeoDataFrame.put_geometry(gdf, "g2", s(["POINT (2 2)", nil], crs: "EPSG:4326"))
    end

    test "to_wkb encodes every geometry column and keeps the others" do
      df = GeoDataFrame.to_wkb(wkb_frame())
      assert %DataFrame{} = df
      assert DataFrame.names(df) == ["a", "geometry", "g2"]
      assert list(df["a"]) == [1, nil]

      assert list(df["geometry"]) ==
               [
                 Base.decode16!("010100000000000000000000000000000000000000"),
                 Base.decode16!("0101000000000000000000F03F000000000000F03F")
               ]

      assert list(df["g2"]) |> Enum.at(1) == nil
    end

    test "to_wkb hex: and srid:" do
      df = GeoDataFrame.to_wkb(wkb_frame(), hex: true)

      assert list(df["geometry"]) ==
               [
                 "010100000000000000000000000000000000000000",
                 "0101000000000000000000F03F000000000000F03F"
               ]

      assert list(df["g2"]) == ["010100000000000000000000400000000000000040", nil]

      assert list(GeoDataFrame.to_wkb(wkb_frame(), hex: true, srid: true)["geometry"]) |> hd() ==
               "0101000020E610000000000000000000000000000000000000"
    end

    test "to_wkt encodes every geometry column" do
      df = GeoDataFrame.to_wkt(wkb_frame())
      assert DataFrame.names(df) == ["a", "geometry", "g2"]
      assert list(df["geometry"]) == ["POINT(0 0)", "POINT(1 1)"]
      assert list(df["g2"]) == ["POINT(2 2)", nil]
    end

    test "a frame without attributes works and options are validated" do
      gdf = GeoDataFrame.new(DataFrame.new([]), s(["POINT (0 0)"]))
      assert DataFrame.names(GeoDataFrame.to_wkt(gdf)) == ["geometry"]
      assert_raise ArgumentError, fn -> GeoDataFrame.to_wkb(gdf, bogus: 1) end
      assert_raise ArgumentError, fn -> GeoDataFrame.to_wkt(gdf, bogus: 1) end
    end

    test "rename_geometry renames the active geometry column" do
      gdf = GeoDataFrame.rename_geometry(wkb_frame(), "shape")
      assert gdf.geometry_column == "shape"
      assert GeoDataFrame.geometry_columns(gdf) == ["shape", "g2"]
      assert GeoDataFrame.names(gdf) == ["a", "shape", "g2"]
      assert GeoDataFrame.rename_geometry(wkb_frame(), :shape).geometry_column == "shape"
    end

    test "rename_geometry rejects an existing column name" do
      for name <- ["a", "g2", "geometry"] do
        assert_raise ArgumentError, "Column named #{name} already exists", fn ->
          GeoDataFrame.rename_geometry(wkb_frame(), name)
        end
      end
    end
  end

  describe "GeoFrame.geoparquet_metadata/1" do
    test "decodes the geo metadata", %{tmp_dir: dir} do
      path = Path.join(dir, "m.parquet")
      gdf = GeoDataFrame.new(DataFrame.new(a: [1, 2]), s(["POINT (1 2)", nil], crs: "EPSG:4326"))
      :ok = GeoFrame.write_parquet(gdf, path)

      assert {:ok, meta} = GeoFrame.geoparquet_metadata(path)
      assert meta["primary_column"] == "geometry"
      assert meta["columns"]["geometry"]["encoding"] == "WKB"
      assert meta == GeoFrame.geoparquet_metadata!(path)
    end

    test "errors for a missing file and for plain Parquet", %{tmp_dir: dir} do
      assert {:error, _} = GeoFrame.geoparquet_metadata(Path.join(dir, "nope.parquet"))

      plain = Path.join(dir, "plain.parquet")
      DataFrame.to_parquet(DataFrame.new(a: [1]), plain)
      assert {:error, message} = GeoFrame.geoparquet_metadata(plain)
      assert message =~ "geo"
      assert_raise ArgumentError, fn -> GeoFrame.geoparquet_metadata!(plain) end
    end
  end
end
