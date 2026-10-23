defmodule GeoFrame.ParityG3Test do
  use ExUnit.Case, async: true
  use ExUnitProperties

  import ExUnit.CaptureLog

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @moduletag :tmp_dir

  @fixtures Path.expand("../reference/fixtures/io/vector", __DIR__)
  @manifest @fixtures |> Path.join("manifest.json") |> File.read!() |> Jason.decode!()

  defp attrs do
    DataFrame.new(
      id: [1, nil, 3],
      name: ["a", "b c", nil],
      score: [1.5, nil, 3.25],
      ok: [true, false, nil]
    )
  end

  defp points(crs \\ "EPSG:4326") do
    geometry = GeoSeries.from_wkt!(["POINT (1 2)", nil, "POINT (3.5 -4)"], crs: crs)
    GeoDataFrame.new(attrs(), geometry)
  end

  defp polygons(crs \\ "EPSG:3857") do
    geometry =
      GeoSeries.from_wkt!(
        [
          "POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0), (1 1, 2 1, 2 2, 1 1))",
          nil,
          "MULTIPOLYGON (((10 10, 10 11, 11 11, 10 10)), ((20 20, 20 21, 21 21, 20 20)))"
        ],
        crs: crs
      )

    GeoDataFrame.new(attrs(), geometry)
  end

  defp column(gdf, name),
    do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name) |> Series.to_list()

  defp rows(gdf), do: gdf |> GeoDataFrame.data() |> DataFrame.n_rows()
  defp wkt(gdf), do: gdf |> GeoDataFrame.geometry() |> GeoSeries.to_wkt()

  defp normalize_wkt(rows), do: rows |> GeoSeries.from_wkt!() |> GeoSeries.to_wkt()

  defp comparable(values) do
    Enum.map(values, fn
      %NaiveDateTime{} = t -> t |> NaiveDateTime.truncate(:second) |> NaiveDateTime.to_iso8601()
      v -> v
    end)
  end

  defp assert_same_frame(back, original) do
    assert GeoDataFrame.names(back) == GeoDataFrame.names(original)
    assert wkt(back) == wkt(original)
    assert GeoDataFrame.crs(back) == GeoDataFrame.crs(original)

    for name <- DataFrame.names(GeoDataFrame.data(original)) do
      assert column(back, name) == column(original, name), "column #{name}"
    end
  end

  # -- fixtures written by GeoPandas 1.2 / pyogrio (GDAL) ----------------------------------

  describe "files written by GeoPandas 1.2 / pyogrio" do
    for {key, expected} <- @manifest["files"] do
      test "#{key} reads with the expected CRS, geometry and attributes" do
        key = unquote(key)
        expected = unquote(Macro.escape(expected))

        {file, layer} =
          key |> String.split(":", parts: 2) |> then(fn [f | l] -> {f, List.first(l)} end)

        opts = if layer, do: [layer: layer], else: []

        gdf = GeoFrame.read_file!(Path.join(@fixtures, file), opts)

        assert GeoDataFrame.crs(gdf) == expected["crs"]
        assert wkt(gdf) == normalize_wkt(expected["wkt"])

        assert gdf |> GeoDataFrame.data() |> DataFrame.names() |> Enum.sort() ==
                 expected["columns"] |> Map.keys() |> Enum.sort()

        for {name, values} <- expected["columns"] do
          assert comparable(column(gdf, name)) == values, "#{key}: column #{name}"
        end
      end

      if expected["info"] do
        test "#{key} reports the same file info as pyogrio" do
          key = unquote(key)
          info = unquote(Macro.escape(expected["info"]))

          {file, layer} =
            key |> String.split(":", parts: 2) |> then(fn [f | l] -> {f, List.first(l)} end)

          opts = if layer, do: [layer: layer], else: []

          got = GeoFrame.read_file_info(Path.join(@fixtures, file), opts)

          assert got.features == info["features"]
          assert got.geometry_type == info["geometry_type"]
          assert got.crs == info["crs"]
          assert got.fields == info["fields"]
          assert got.driver == info["driver"]

          if info["total_bounds"] do
            assert got.total_bounds |> Tuple.to_list() == info["total_bounds"]
          end
        end
      end
    end

    test "list_layers matches pyogrio for the multi-layer GeoPackage" do
      layers = GeoFrame.list_layers(Path.join(@fixtures, "multi.gpkg"))
      assert DataFrame.names(layers) == ["name", "geometry_type"]
      assert Series.to_list(layers["name"]) == ["pts", "polys"]
      assert Series.to_list(layers["geometry_type"]) == ["Point", "MultiPolygon"]
    end

    test "a multi-layer GeoPackage needs a layer" do
      assert {:error, message} = GeoFrame.read_gpkg(Path.join(@fixtures, "multi.gpkg"))
      assert message =~ "pts" and message =~ "polys" and message =~ "layer:"

      assert {:error, message} =
               GeoFrame.read_gpkg(Path.join(@fixtures, "multi.gpkg"), layer: "x")

      assert message =~ "x"
    end
  end

  describe "shapefile names" do
    test "truncation matches GDAL (checked against the GeoPandas fixture)", %{tmp_dir: dir} do
      expected = @manifest["files"]["truncated.shp"]["columns"]

      long =
        DataFrame.new(
          population_total: [1, 2],
          population_urban: [3, 4],
          population_rural: [5, 6],
          short: [7, 8]
        )

      gdf =
        GeoDataFrame.new(
          long,
          GeoSeries.from_wkt!(["POINT (1 1)", "POINT (2 2)"], crs: "EPSG:4326")
        )

      path = Path.join(dir, "long.shp")

      log = capture_log(fn -> assert :ok = GeoFrame.write_shapefile(gdf, path) end)
      assert log =~ "truncated"

      back = GeoFrame.read_shapefile!(path)
      names = back |> GeoDataFrame.data() |> DataFrame.names()
      assert Enum.sort(names) == Enum.sort(Map.keys(expected))
      assert names == ["population", "populati_1", "populati_2", "short"]
      assert column(back, "population") == [1, 2]
      assert column(back, "populati_1") == [3, 4]
      assert column(back, "populati_2") == [5, 6]
      assert column(back, "short") == [7, 8]
    end
  end

  # -- shapefile ------------------------------------------------------------------------------

  describe "shapefile" do
    test "points round trip with attributes, nulls and CRS", %{tmp_dir: dir} do
      path = Path.join(dir, "points.shp")
      gdf = points()
      assert :ok = GeoFrame.write_shapefile(gdf, path)

      for ext <- ~w(shp shx dbf prj cpg),
          do: assert(File.exists?(Path.join(dir, "points.#{ext}")))

      assert {:ok, back} = GeoFrame.read_shapefile(path)
      assert_same_frame(back, gdf)
    end

    test "polygons and multipolygons are stored as polygon shapes", %{tmp_dir: dir} do
      path = Path.join(dir, "polys.shp")
      gdf = polygons()
      assert :ok = GeoFrame.write_shapefile(gdf, path)
      back = GeoFrame.read_shapefile!(path)
      assert GeoDataFrame.crs(back) == "EPSG:3857"

      types = back |> GeoDataFrame.geometry() |> GeoSeries.geom_type() |> Series.to_list()
      assert types == ["Polygon", nil, "MultiPolygon"]

      areas = fn frame ->
        frame |> GeoDataFrame.geometry() |> GeoSeries.area() |> Series.to_list()
      end

      assert areas.(back) == areas.(gdf)
    end

    test "dates and datetimes", %{tmp_dir: dir} do
      path = Path.join(dir, "dates.shp")

      data =
        DataFrame.new(
          day: Series.from_list([~D[2020-01-02], nil, ~D[1999-12-31]]),
          stamp: Series.from_list([~N[2020-01-02 03:04:05], nil, ~N[2021-03-04 00:00:00]])
        )

      gdf =
        GeoDataFrame.new(data, GeoSeries.from_wkt!(["POINT (1 1)", "POINT (2 2)", "POINT (3 3)"]))

      assert :ok = GeoFrame.write_shapefile(gdf, path)
      back = GeoFrame.read_shapefile!(path)
      assert column(back, "day") == [~D[2020-01-02], nil, ~D[1999-12-31]]
      # shapefiles have no timestamp type: it is stored as ISO 8601 text, like GDAL
      assert column(back, "stamp") == ["2020-01-02T03:04:05", nil, "2021-03-04T00:00:00"]
    end

    test "a frame of mixed geometry types raises ArgumentError", %{tmp_dir: dir} do
      geometry = GeoSeries.from_wkt!(["POINT (1 2)", "LINESTRING (0 0, 1 1)"])
      gdf = GeoDataFrame.new(DataFrame.new(id: [1, 2]), geometry)

      assert_raise ArgumentError, ~r/single geometry type/, fn ->
        GeoFrame.write_shapefile(gdf, Path.join(dir, "mixed.shp"))
      end

      refute File.exists?(Path.join(dir, "mixed.shp"))
    end

    test "unknown options raise", %{tmp_dir: dir} do
      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.write_shapefile(points(), Path.join(dir, "a.shp"), bogus: 1)
      end

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.read_shapefile(Path.join(dir, "a.shp"), layer: "x")
      end
    end

    test "encoding: is honoured on write and read", %{tmp_dir: dir} do
      path = Path.join(dir, "latin.shp")
      geometry = GeoSeries.from_wkt!(["POINT (1 1)", "POINT (2 2)"], crs: "EPSG:4326")
      gdf = GeoDataFrame.new(DataFrame.new(name: ["café", "ñandú"]), geometry)

      assert :ok = GeoFrame.write_shapefile(gdf, path, encoding: "ISO-8859-1")
      assert File.read!(Path.join(dir, "latin.cpg")) == "ISO-8859-1"
      assert column(GeoFrame.read_shapefile!(path), "name") == ["café", "ñandú"]
      # an explicit encoding wins over the .cpg
      assert column(GeoFrame.read_shapefile!(path, encoding: "UTF-8"), "name") != [
               "café",
               "ñandú"
             ]

      assert {:error, message} = GeoFrame.read_shapefile(path, encoding: "EBCDIC")
      assert message =~ "EBCDIC"
    end

    test "a CRS without a known .prj definition is dropped with a warning", %{tmp_dir: dir} do
      path = Path.join(dir, "odd.shp")
      gdf = points("EPSG:999999")

      log = capture_log(fn -> assert :ok = GeoFrame.write_shapefile(gdf, path) end)
      assert log =~ "EPSG:999999"
      refute File.exists?(Path.join(dir, "odd.prj"))
      assert GeoDataFrame.crs(GeoFrame.read_shapefile!(path)) == nil
    end

    test "zipped shapefiles are read and written", %{tmp_dir: dir} do
      path = Path.join(dir, "bundle.zip")
      gdf = points()
      assert :ok = GeoFrame.write_file(gdf, path, driver: :shapefile)
      assert {:ok, back} = GeoFrame.read_file(path)
      assert_same_frame(back, gdf)
    end

    test "missing files are errors" do
      assert {:error, message} = GeoFrame.read_shapefile("/nonexistent/none.shp")
      assert message =~ "none.shp"
    end

    test "read_file and write_file dispatch on .shp", %{tmp_dir: dir} do
      path = Path.join(dir, "viafile.shp")
      assert :ok = GeoFrame.write_file(points(), path)
      assert_same_frame(GeoFrame.read_file!(path), points())
    end
  end

  # -- GeoPackage -----------------------------------------------------------------------------

  describe "geopackage" do
    test "round trips geometry, attributes and CRS", %{tmp_dir: dir} do
      path = Path.join(dir, "a.gpkg")
      gdf = points()
      assert :ok = GeoFrame.write_gpkg(gdf, path, layer: "pts")
      assert {:ok, back} = GeoFrame.read_gpkg(path)
      assert_same_frame(back, gdf)
    end

    test "the layer defaults to the file name", %{tmp_dir: dir} do
      path = Path.join(dir, "named.gpkg")
      assert :ok = GeoFrame.write_gpkg(points(), path)
      layers = GeoFrame.list_layers(path)
      assert Series.to_list(layers["name"]) == ["named"]
    end

    test "layers, list_layers and mode: :a", %{tmp_dir: dir} do
      path = Path.join(dir, "multi.gpkg")
      assert :ok = GeoFrame.write_gpkg(points(), path, layer: "pts")
      assert :ok = GeoFrame.write_gpkg(polygons(), path, layer: "polys")

      layers = GeoFrame.list_layers(path)
      assert DataFrame.names(layers) == ["name", "geometry_type"]
      assert Series.to_list(layers["name"]) == ["pts", "polys"]
      assert Series.to_list(layers["geometry_type"]) == ["Point", "Unknown"]

      assert {:error, message} = GeoFrame.read_gpkg(path)
      assert message =~ "layer:"
      assert_same_frame(GeoFrame.read_gpkg!(path, layer: "polys"), polygons())

      assert :ok = GeoFrame.write_gpkg(points(), path, layer: "pts", mode: :a)
      assert rows(GeoFrame.read_gpkg!(path, layer: "pts")) == 6

      # mode :w replaces only the named layer
      assert :ok = GeoFrame.write_gpkg(points(), path, layer: "pts")
      assert rows(GeoFrame.read_gpkg!(path, layer: "pts")) == 3
      assert Series.to_list(GeoFrame.list_layers(path)["name"]) == ["polys", "pts"]
    end

    test "appending checks the schema and the CRS", %{tmp_dir: dir} do
      path = Path.join(dir, "a.gpkg")
      assert :ok = GeoFrame.write_gpkg(points(), path, layer: "pts")

      other = GeoDataFrame.new(DataFrame.new(zzz: [1, 2, 3]), points() |> GeoDataFrame.geometry())
      assert {:error, message} = GeoFrame.write_gpkg(other, path, layer: "pts", mode: :a)
      assert message =~ "zzz"

      assert {:error, message} =
               GeoFrame.write_gpkg(points("EPSG:3857"), path, layer: "pts", mode: :a)

      assert message =~ "CRS"
    end

    test "invalid options raise", %{tmp_dir: dir} do
      path = Path.join(dir, "a.gpkg")

      assert_raise ArgumentError, ~r/mode/, fn ->
        GeoFrame.write_gpkg(points(), path, mode: :x)
      end

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.write_gpkg(points(), path, bogus: true)
      end

      assert_raise ArgumentError, ~r/layer/, fn ->
        GeoFrame.write_gpkg(points(), path, layer: "")
      end
    end

    test "the spatial index is optional", %{tmp_dir: dir} do
      path = Path.join(dir, "noindex.gpkg")
      assert :ok = GeoFrame.write_gpkg(points(), path, spatial_index: false)
      assert_same_frame(GeoFrame.read_gpkg!(path), points())
    end

    test "mixed geometry types are kept as they are", %{tmp_dir: dir} do
      path = Path.join(dir, "mixed.gpkg")

      geometry =
        GeoSeries.from_wkt!(["POINT (1 2)", "LINESTRING (0 0, 1 1)", nil], crs: "EPSG:4326")

      gdf = GeoDataFrame.new(DataFrame.new(id: [1, 2, 3]), geometry)
      assert :ok = GeoFrame.write_gpkg(gdf, path, layer: "m")
      assert_same_frame(GeoFrame.read_gpkg!(path), gdf)
    end

    test "dates and datetimes round trip", %{tmp_dir: dir} do
      path = Path.join(dir, "dates.gpkg")

      data =
        DataFrame.new(
          day: Series.from_list([~D[2020-01-02], nil, ~D[1999-12-31]]),
          stamp: Series.from_list([~N[2020-01-02 03:04:05], nil, ~N[2021-03-04 00:00:00]])
        )

      gdf =
        GeoDataFrame.new(data, GeoSeries.from_wkt!(["POINT (1 1)", "POINT (2 2)", "POINT (3 3)"]))

      assert :ok = GeoFrame.write_gpkg(gdf, path)
      back = GeoFrame.read_gpkg!(path)
      assert column(back, "day") == [~D[2020-01-02], nil, ~D[1999-12-31]]

      assert column(back, "stamp") == [
               ~N[2020-01-02 03:04:05.000000],
               nil,
               ~N[2021-03-04 00:00:00.000000]
             ]
    end

    test "read_file and write_file dispatch on .gpkg and pass layer:", %{tmp_dir: dir} do
      path = Path.join(dir, "viafile.gpkg")
      assert :ok = GeoFrame.write_file(points(), path, layer: "one")
      assert :ok = GeoFrame.write_file(polygons(), path, layer: "two")
      assert_same_frame(GeoFrame.read_file!(path, layer: "one"), points())
      assert {:error, _} = GeoFrame.read_file(path)

      other = Path.join(dir, "driver.dat")
      assert :ok = GeoFrame.write_file(points(), other, driver: :gpkg)
      assert_same_frame(GeoFrame.read_file!(other, driver: :gpkg), points())
    end

    test "files that are not GeoPackages are errors", %{tmp_dir: dir} do
      path = Path.join(dir, "bad.gpkg")
      File.write!(path, "this is not a sqlite database, just a long enough text")
      assert {:error, _} = GeoFrame.read_gpkg(path)
      assert_raise ArgumentError, fn -> GeoFrame.list_layers(path) end
    end
  end

  # -- list_layers and read_file_info for the other formats -------------------------------------

  describe "layers and info for single-layer formats" do
    test "list_layers returns one row", %{tmp_dir: dir} do
      path = Path.join(dir, "one.geojson")
      assert :ok = GeoFrame.write_file(points(), path)
      layers = GeoFrame.list_layers(path)
      assert Series.to_list(layers["name"]) == ["one"]
      assert Series.to_list(layers["geometry_type"]) == ["Point"]

      shp = Path.join(dir, "two.shp")
      assert :ok = GeoFrame.write_file(polygons(), shp)
      layers = GeoFrame.list_layers(shp)
      assert Series.to_list(layers["name"]) == ["two"]
      assert Series.to_list(layers["geometry_type"]) == ["Polygon"]
    end

    test "read_file_info describes a written file", %{tmp_dir: dir} do
      path = Path.join(dir, "info.gpkg")
      assert :ok = GeoFrame.write_gpkg(points(), path, layer: "pts")
      info = GeoFrame.read_file_info(path)
      assert info.layer == "pts"
      assert info.crs == "EPSG:4326"
      assert info.features == 3
      assert info.geometry_type == "Point"
      assert info.fields == ["id", "name", "score", "ok"]
      assert info.dtypes == ["int64", "object", "float64", "bool"]
      assert info.total_bounds == {1.0, -4.0, 3.5, 2.0}
      assert info.driver == "GPKG"

      shp = Path.join(dir, "info.shp")
      assert :ok = GeoFrame.write_shapefile(points(), shp)
      info = GeoFrame.read_file_info(shp)
      assert info.driver == "ESRI Shapefile"
      assert info.features == 3
      assert info.encoding == "UTF-8"
    end

    test "read_file_info works for formats without a dedicated reader", %{tmp_dir: dir} do
      path = Path.join(dir, "info.fgb")
      assert :ok = GeoFrame.write_file(points(), path)
      info = GeoFrame.read_file_info(path)
      assert info.features == 3
      assert info.crs == "EPSG:4326"
      assert info.fields == ["id", "name", "score", "ok"]
    end
  end

  # -- properties -----------------------------------------------------------------------------

  describe "properties" do
    defp frame_from(rows, crs) do
      {points, attrs} = Enum.unzip(rows)
      {ids, names} = Enum.unzip(attrs)
      {xs, ys} = Enum.unzip(points)

      data =
        DataFrame.new(%{
          "id" => Series.from_list(ids, dtype: {:s, 64}),
          "name" => Series.from_list(names, dtype: :string)
        })

      GeoDataFrame.new(data, GeoSeries.from_xy(xs, ys, crs: crs))
    end

    defp rows_gen(name_gen) do
      point = {float(min: -180.0, max: 180.0), float(min: -90.0, max: 90.0)}

      row =
        {one_of([integer(-1_000_000_000_000..1_000_000_000_000), constant(nil)]),
         one_of([name_gen, constant(nil)])}

      list_of(tuple({point, row}), min_length: 1, max_length: 25)
    end

    property "shapefile round trip", %{tmp_dir: dir} do
      check all(
              rows <- rows_gen(string(:alphanumeric, min_length: 1, max_length: 20)),
              crs <- member_of(["EPSG:4326", "EPSG:3857", "EPSG:32617", nil]),
              max_runs: 25
            ) do
        gdf = frame_from(rows, crs)
        path = Path.join(dir, "prop_#{System.unique_integer([:positive])}.shp")

        assert :ok = GeoFrame.write_shapefile(gdf, path)
        back = GeoFrame.read_shapefile!(path)

        assert wkt(back) == wkt(gdf)
        assert GeoDataFrame.crs(back) == crs
        assert column(back, "id") == column(gdf, "id")
        assert column(back, "name") == column(gdf, "name")
      end
    end

    property "geopackage round trip", %{tmp_dir: dir} do
      check all(
              rows <- rows_gen(string(:printable, min_length: 0, max_length: 30)),
              crs <- member_of(["EPSG:4326", "EPSG:3857", "EPSG:32617", nil]),
              index <- boolean(),
              max_runs: 25
            ) do
        gdf = frame_from(rows, crs)
        path = Path.join(dir, "prop_#{System.unique_integer([:positive])}.gpkg")

        assert :ok = GeoFrame.write_gpkg(gdf, path, spatial_index: index)
        back = GeoFrame.read_gpkg!(path)

        assert wkt(back) == wkt(gdf)
        assert GeoDataFrame.crs(back) == crs
        assert column(back, "id") == column(gdf, "id")
        assert column(back, "name") == column(gdf, "name")
      end
    end
  end
end
