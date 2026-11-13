defmodule GeoFrame.ZmIoTest do
  # Z and M through the file formats (batch Z4). The fixtures in test/reference/fixtures/io/zm
  # are written by GeoPandas / pyogrio (GDAL) for Z and, because pyogrio cannot write M, built
  # from the ESRI and GeoPackage specifications and verified through GDAL for M and ZM; see
  # test/reference/generate_io.py.
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @moduletag :tmp_dir

  @fixtures Path.expand("../reference/fixtures/io/zm", __DIR__)
  @manifest @fixtures |> Path.join("manifest.json") |> File.read!() |> Jason.decode!()

  defp split(key) do
    case String.split(key, ":", parts: 2) do
      [file, layer] -> {file, [layer: layer]}
      [file] -> {file, []}
    end
  end

  defp wkt(gdf), do: gdf |> GeoDataFrame.geometry() |> GeoSeries.to_wkt()
  defp normalize_wkt(rows), do: rows |> GeoSeries.from_wkt!() |> GeoSeries.to_wkt()
  defp flags(gdf, fun), do: gdf |> GeoDataFrame.geometry() |> fun.() |> Series.to_list()

  defp column(gdf, name),
    do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name) |> Series.to_list()

  describe "files written by GDAL (Z) and built from the specifications (M, ZM)" do
    for {key, expected} <- @manifest["files"] do
      test "#{key} reads with its Z and M ordinates" do
        key = unquote(key)
        expected = unquote(Macro.escape(expected))
        {file, opts} = split(key)

        path = Path.join(@fixtures, file)

        gdf =
          if String.ends_with?(file, ".arrows"),
            do: GeoDataFrame.from_arrow!(File.read!(path)),
            else: GeoFrame.read_file!(path, opts)

        assert wkt(gdf) == normalize_wkt(expected["wkt"])
        assert flags(gdf, &GeoSeries.has_z/1) == expected["has_z"]
        assert flags(gdf, &GeoSeries.has_m/1) == expected["has_m"]

        for {name, values} <- expected["columns"] do
          assert column(gdf, name) == values, "#{key}: column #{name}"
        end
      end
    end

    test "the GeoPackage flags GDAL wrote are readable" do
      info = GeoFrame.read_file_info(Path.join(@fixtures, "m.gpkg"), layer: "pts_zm")
      assert info.geometry_type == "Point ZM"
      info = GeoFrame.read_file_info(Path.join(@fixtures, "m.gpkg"), layer: "pts_m")
      assert info.geometry_type == "Point M"
      info = GeoFrame.read_file_info(Path.join(@fixtures, "z.gpkg"), layer: "pts")
      assert info.geometry_type == "Point Z"
    end

    test "shapefile info names the Z and M shape types" do
      types =
        for {file, _} <- [
              {"points_z.shp", 1},
              {"lines_m.shp", 1},
              {"polygons_zm.shp", 1},
              {"multipoints_z.shp", 1}
            ],
            into: %{},
            do: {file, GeoFrame.read_file_info(Path.join(@fixtures, file)).geometry_type}

      assert types == %{
               "points_z.shp" => "Point Z",
               "lines_m.shp" => "LineString M",
               "polygons_zm.shp" => "Polygon Z",
               "multipoints_z.shp" => "MultiPoint Z"
             }
    end
  end

  # Files with one dimension and one geometry kind per format survive a GeoFrame round trip.
  describe "round trips through every writer" do
    @uniform [
      {"points_z.shp", ~w(.shp .gpkg .fgb .parquet geoarrow)},
      {"lines_z.shp", ~w(.shp .gpkg .parquet)},
      {"polygons_z.shp", ~w(.shp .gpkg .parquet)},
      {"multipoints_z.shp", ~w(.shp .gpkg .fgb .parquet geoarrow)},
      {"points_m.shp", ~w(.shp .gpkg .fgb .parquet geoarrow)},
      {"lines_m.shp", ~w(.shp .gpkg .fgb .parquet geoarrow)},
      {"polygons_m.shp", ~w(.shp .gpkg .fgb .parquet)},
      {"points_zm.shp", ~w(.shp .gpkg .fgb .parquet geoarrow)},
      {"lines_zm.shp", ~w(.shp .gpkg .fgb .parquet)},
      {"polygons_zm.shp", ~w(.shp .gpkg .fgb .parquet geoarrow)}
    ]

    for {file, targets} <- @uniform, target <- targets do
      test "#{file} -> #{target}", %{tmp_dir: dir} do
        file = unquote(file)
        target = unquote(target)
        gdf = GeoFrame.read_file!(Path.join(@fixtures, file))

        {ext, opts} =
          case target do
            "geoarrow" -> {".parquet", [geometry_encoding: :geoarrow]}
            # the spatial index would reorder the rows
            ".fgb" -> {".fgb", [index: false]}
            ext -> {ext, []}
          end

        out = Path.join(dir, "out" <> ext)
        assert :ok = GeoFrame.write_file(gdf, out, opts)
        back = GeoFrame.read_file!(out)

        if target == "geoarrow" do
          # native columns promote single geometries to their multi type, keeping Z and M
          assert flags(back, &GeoSeries.has_z/1) == flags(gdf, &GeoSeries.has_z/1)
          assert flags(back, &GeoSeries.has_m/1) == flags(gdf, &GeoSeries.has_m/1)
        else
          assert wkt(back) == wkt(gdf)
        end
      end
    end
  end

  # GDAL reads a 2D row of a Z layer and a two-value position of a 3D geometry as Z = 0 (files
  # written and read by GDAL 3.12 in generate_io.py: lines_z_mixed.shp, z_positions.geojson).
  describe "a missing Z is 0, as GDAL" do
    @mixed ["LINESTRING Z (0 0 5, 1 1 6)", "LINESTRING (2 2, 3 3)", nil]
    @zero ["LINESTRING Z(0 0 5,1 1 6)", "LINESTRING Z(2 2 0,3 3 0)", nil]

    for {ext, opts} <- [{".shp", []}, {".fgb", [index: false]}] do
      test "a 2D row written to a Z #{ext} file reads back with Z 0", %{tmp_dir: dir} do
        gdf = GeoDataFrame.new(DataFrame.new(id: [1, 2, 3]), GeoSeries.from_wkt!(@mixed))
        out = Path.join(dir, "mixed" <> unquote(ext))
        assert :ok = GeoFrame.write_file(gdf, out, unquote(opts))
        assert wkt(GeoFrame.read_file!(out)) == @zero
      end
    end

    test "the GDAL shapefile with a 2D row reads Z 0 and survives a round trip", %{tmp_dir: dir} do
      gdf = GeoFrame.read_file!(Path.join(@fixtures, "lines_z_mixed.shp"))
      assert wkt(gdf) == @zero
      out = Path.join(dir, "again.shp")
      assert :ok = GeoFrame.write_file(gdf, out)
      assert wkt(GeoFrame.read_file!(out)) == @zero
    end

    test "an explicit NaN Z stays NaN in a shapefile", %{tmp_dir: dir} do
      # Shapefile and FlatGeobuf store NaN: only an absent ordinate is promoted to 0.
      wkb =
        Base.decode16!(
          "01EA03000002000000000000000000F03F000000000000F03F000000000000F87F000000000000004000000000000000400000000000000840"
        )

      gdf = GeoDataFrame.new(DataFrame.new(id: [1]), GeoSeries.from_wkb!([wkb]))
      out = Path.join(dir, "nan.shp")
      assert :ok = GeoFrame.write_file(gdf, out)
      assert wkt(GeoFrame.read_file!(out)) == ["LINESTRING Z(1 1 NaN,2 2 3)"]
    end
  end

  describe "GeoJSON" do
    test "reads a GDAL file with mixed dimensions", %{tmp_dir: dir} do
      gdf = GeoFrame.read_file!(Path.join(@fixtures, "z.geojson"))
      out = Path.join(dir, "out.geojson")
      assert :ok = GeoFrame.write_file(gdf, out)
      assert wkt(GeoFrame.read_file!(out)) == wkt(gdf)
    end

    test "writes Z as the third ordinate and drops M", %{tmp_dir: dir} do
      geometry = GeoSeries.from_wkt!(["POINT Z (1 2 3)", "POINT ZM (1 2 3 4)", "POINT M (5 6 7)"])
      gdf = GeoDataFrame.new(DataFrame.new(id: [1, 2, 3]), geometry)
      out = Path.join(dir, "z.geojson")
      assert :ok = GeoFrame.write_geojson(gdf, out, force: true)

      features = out |> File.read!() |> Jason.decode!() |> Map.fetch!("features")
      coordinates = Enum.map(features, & &1["geometry"]["coordinates"])
      assert coordinates == [[1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [5.0, 6.0]]
    end

    test "GeoSeries.to_geojson carries Z" do
      [geometry] =
        ["LINESTRING Z (0 0 1, 1 1 2)"] |> GeoSeries.from_wkt!() |> GeoSeries.to_geojson()

      assert geometry["coordinates"] == [[0.0, 0.0, 1.0], [1.0, 1.0, 2.0]]
    end
  end
end
