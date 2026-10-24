defmodule GeoFrame.MultiGeometryIoTest do
  # Secondary geometry columns in the non-Parquet writers (issue #93). GeoPandas keeps every
  # geometry column in `to_feather` and `to_arrow`, and refuses them in `to_file` (GPKG,
  # GeoJSON, FlatGeobuf, Shapefile) and `to_json`; GeoFrame does the same.
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @moduletag :tmp_dir

  @fixtures Path.expand("../reference/fixtures/io/multi", __DIR__)
  @manifest @fixtures |> Path.join("manifest.json") |> File.read!() |> Jason.decode!()

  defp frame do
    data = DataFrame.new(id: [1, 2, 3], name: ["a", "b", nil])
    geometry = GeoSeries.from_wkt!(["POINT(0 0)", "POINT(1 1)", nil], crs: "EPSG:4326")

    footprint =
      GeoSeries.from_wkt!(
        ["POLYGON((0 0,1 0,1 1,0 0))", nil, "POLYGON((5 5,6 5,6 6,5 5))"],
        crs: "EPSG:3857"
      )

    centroid = GeoSeries.from_wkt!(["POINT(5 5)", "POINT(6 6)", "POINT(7 7)"])

    data
    |> GeoDataFrame.new(geometry)
    |> GeoDataFrame.put_geometry("footprint", footprint)
    |> GeoDataFrame.put_geometry("centroid", centroid)
  end

  defp wkt(series), do: GeoSeries.to_wkt(series)

  defp normalize(rows), do: rows |> GeoSeries.from_wkt!() |> GeoSeries.to_wkt()

  defp same_frame(back, source) do
    assert GeoDataFrame.names(back) == GeoDataFrame.names(source)
    assert GeoDataFrame.geometry_columns(back) == GeoDataFrame.geometry_columns(source)

    assert back |> GeoDataFrame.data() |> DataFrame.pull("id") |> Series.to_list() ==
             source |> GeoDataFrame.data() |> DataFrame.pull("id") |> Series.to_list()

    for name <- GeoDataFrame.geometry_columns(source) do
      got = GeoDataFrame.pull(back, name)
      want = GeoDataFrame.pull(source, name)
      assert GeoSeries.crs(got) == GeoSeries.crs(want), "CRS of #{name}"
      assert wkt(got) == wkt(want), "geometry #{name}"
    end
  end

  describe "Feather and Arrow keep every geometry column" do
    for compression <- [:lz4, :zstd, :uncompressed], encoding <- [:wkb, :geoarrow] do
      test "feather round trip (#{compression}, #{encoding})", %{tmp_dir: dir} do
        path = Path.join(dir, "t.feather")

        opts = [compression: unquote(compression), geometry_encoding: unquote(encoding)]
        assert :ok = GeoFrame.write_feather(frame(), path, opts)
        same_frame(GeoFrame.read_feather!(path), frame())
      end
    end

    for encoding <- [:wkb, :geoarrow] do
      test "to_arrow and from_arrow round trip (#{encoding})" do
        bytes = GeoDataFrame.to_arrow(frame(), geometry_encoding: unquote(encoding))
        same_frame(GeoDataFrame.from_arrow!(bytes), frame())
      end
    end

    test "columns: can drop a secondary geometry column", %{tmp_dir: dir} do
      path = Path.join(dir, "t.feather")
      assert :ok = GeoFrame.write_feather(frame(), path)
      back = GeoFrame.read_feather!(path, columns: ["id", "footprint"])
      assert GeoDataFrame.names(back) == ["id", "geometry", "footprint"]
    end

    test "a frame without secondary columns reads as before", %{tmp_dir: dir} do
      path = Path.join(dir, "t.feather")
      plain = GeoDataFrame.select(frame(), ["id"])
      assert :ok = GeoFrame.write_feather(plain, path)
      assert GeoFrame.read_feather!(path) |> GeoDataFrame.geometry_columns() == ["geometry"]
    end

    for file <- ~w(two_wkb.feather two_zstd.feather two_wkb.arrows two_native.arrows) do
      test "reads the GeoPandas file #{file}" do
        file = unquote(file)
        expected = @manifest["files"][file]
        path = Path.join(@fixtures, file)

        gdf =
          if String.ends_with?(file, ".arrows"),
            do: path |> File.read!() |> GeoDataFrame.from_arrow!(),
            else: GeoFrame.read_feather!(path)

        assert gdf.geometry_column == expected["primary"]
        assert GeoDataFrame.geometry_columns(gdf) == expected["geometry_columns"]
        assert GeoDataFrame.names(gdf) == ["id", "name" | expected["geometry_columns"]]

        for name <- expected["geometry_columns"] do
          series = GeoDataFrame.pull(gdf, name)
          want = expected["columns"][name]
          assert GeoSeries.crs(series) == want["crs"], "crs of #{name}"
          assert wkt(series) == normalize(want["wkt"]), "geometry #{name}"
        end
      end
    end
  end

  describe "single-geometry formats refuse extra geometry columns, like GeoPandas to_file" do
    for {writer, file} <- [
          {:write_geojson, "t.geojson"},
          {:write_flatgeobuf, "t.fgb"},
          {:write_shapefile, "t.shp"},
          {:write_gpkg, "t.gpkg"},
          {:write_file, "t.gpkg"},
          {:write_file, "t.fgb"}
        ] do
      test "#{writer} (#{file}) raises and writes nothing", %{tmp_dir: dir} do
        path = Path.join(dir, unquote(file))

        error =
          assert_raise ArgumentError, fn ->
            apply(GeoFrame, unquote(writer), [frame(), path])
          end

        assert error.message =~ "multiple geometry columns"
        assert error.message =~ ~s("footprint")
        assert error.message =~ "write_parquet"
        assert File.ls!(dir) == []
      end
    end

    test "a frame with one geometry column still writes", %{tmp_dir: dir} do
      assert :ok =
               GeoFrame.write_gpkg(GeoDataFrame.select(frame(), ["id"]), Path.join(dir, "a.gpkg"))
    end

    test "dropping the extra columns, or turning them into WKT, is the way out", %{tmp_dir: dir} do
      gdf = frame()

      assert :ok =
               GeoFrame.write_gpkg(
                 GeoDataFrame.select(gdf, ["id", "name"]),
                 Path.join(dir, "a.gpkg")
               )

      as_text =
        gdf
        |> GeoDataFrame.select(["id", "name"])
        |> GeoDataFrame.put(
          "footprint_wkt",
          GeoSeries.to_wkt(GeoDataFrame.pull(gdf, "footprint"))
        )

      assert :ok = GeoFrame.write_gpkg(as_text, Path.join(dir, "b.gpkg"))
    end

    test "to_json refuses like GeoPandas (a shape is not JSON serializable)" do
      assert_raise ArgumentError, ~r/multiple geometry columns/, fn ->
        GeoDataFrame.to_json(frame())
      end
    end

    test "to_postgis refuses before it touches the connection" do
      assert_raise ArgumentError, ~r/multiple geometry columns/, fn ->
        GeoFrame.to_postgis(frame(), nil, "t")
      end
    end
  end

  describe "to_geo_interface and to_geojson keep the extra geometries as properties" do
    # GeoPandas puts the shapely object of a secondary column into `properties`; the
    # GeoJSON map of the geometry is the equivalent here.
    test "to_geo_interface" do
      [first, second, third] = GeoDataFrame.to_geo_interface(frame())["features"]

      assert first["properties"]["footprint"] == %{
               "type" => "Polygon",
               "coordinates" => [[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]]
             }

      assert first["properties"]["centroid"]["type"] == "Point"
      assert second["properties"]["footprint"] == nil
      assert third["geometry"] == nil
      assert third["properties"]["footprint"]["type"] == "Polygon"
    end

    test "to_geojson" do
      [first | _] = GeoDataFrame.to_geojson(frame())["features"]
      assert first["properties"]["footprint"]["type"] == "Polygon"
      assert first["properties"]["id"] == 1
    end
  end
end
