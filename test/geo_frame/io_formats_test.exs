defmodule GeoFrame.IoFormatsTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @moduletag :tmp_dir

  @fixture Path.expand("../fixtures/neighborhoods.parquet", __DIR__)

  defp sample(opts \\ []) do
    data =
      DataFrame.new(
        id: [1, nil, 3],
        name: ["a", "b", nil],
        score: [1.5, nil, 3.25],
        active: [true, false, nil]
      )

    geometry =
      GeoSeries.from_wkt!(
        ["POINT (1 2)", nil, "POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))"],
        crs: Keyword.get(opts, :crs, "EPSG:4326")
      )

    GeoDataFrame.new(data, geometry)
  end

  defp column(gdf, name),
    do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name) |> Series.to_list()

  defp wkt(gdf), do: gdf |> GeoDataFrame.geometry() |> GeoSeries.to_wkt()

  defp attributes(gdf) do
    data = GeoDataFrame.data(gdf)
    for name <- DataFrame.names(data), into: %{}, do: {name, column(gdf, name)}
  end

  defp assert_same_frame(back, original) do
    assert GeoDataFrame.names(back) == GeoDataFrame.names(original)
    assert attributes(back) == attributes(original)
    assert wkt(back) == wkt(original)
  end

  describe "GeoJSON" do
    test "round trip preserves attributes, nulls, geometry and column order", %{tmp_dir: dir} do
      path = Path.join(dir, "rt.geojson")
      gdf = sample()

      assert :ok = GeoFrame.write_geojson(gdf, path)
      assert {:ok, back} = GeoFrame.read_geojson(path)

      assert_same_frame(back, gdf)
      assert GeoDataFrame.crs(back) == "OGC:CRS84"
      assert File.read!(path) |> Jason.decode!() |> Map.fetch!("type") == "FeatureCollection"
    end

    test "writing needs a lon/lat CRS unless forced", %{tmp_dir: dir} do
      projected = sample(crs: "EPSG:3857")
      path = Path.join(dir, "projected.geojson")

      assert {:error, message} = GeoFrame.write_geojson(projected, path)
      assert message =~ "force"
      refute File.exists?(path)

      assert :ok = GeoFrame.write_geojson(projected, path, force: true)
      assert {:ok, back} = GeoFrame.read_geojson(path)
      assert wkt(back) == wkt(projected)

      unknown = sample(crs: nil)
      assert {:error, _} = GeoFrame.write_geojson(unknown, Path.join(dir, "unknown.geojson"))
      assert :ok = GeoFrame.write_geojson(sample(crs: "OGC:CRS84"), Path.join(dir, "ok.geojson"))
    end

    test "infers column types across features", %{tmp_dir: dir} do
      path = Path.join(dir, "infer.geojson")

      File.write!(path, """
      {"type":"FeatureCollection","features":[
        {"type":"Feature","id":1,"geometry":{"type":"Point","coordinates":[0,0]},
         "properties":{"i":1,"f":1,"s":"x","mix":1,"nest":{"k":[1]},"b":true}},
        {"type":"Feature","id":2,"geometry":null,
         "properties":{"i":2,"f":2.5,"s":null,"mix":"two","nest":null,"b":null}}
      ]}
      """)

      gdf = GeoFrame.read_geojson!(path)
      assert GeoDataFrame.names(gdf) == ["id", "i", "f", "s", "mix", "nest", "b", "geometry"]
      assert column(gdf, "id") == [1, 2]
      assert column(gdf, "i") == [1, 2]
      assert column(gdf, "f") == [1.0, 2.5]
      assert column(gdf, "s") == ["x", nil]
      assert column(gdf, "mix") == ["1", "two"]
      assert column(gdf, "nest") == [~s({"k":[1]}), nil]
      assert column(gdf, "b") == [true, nil]
      assert wkt(gdf) == ["POINT(0 0)", nil]
    end

    test "a property called geometry does not clash with the geometry column", %{tmp_dir: dir} do
      path = Path.join(dir, "clash.geojson")

      File.write!(
        path,
        ~s({"type":"FeatureCollection","features":[{"type":"Feature","geometry":null,"properties":{"geometry":"p"}}]})
      )

      gdf = GeoFrame.read_geojson!(path)
      assert column(gdf, "geometry") == ["p"]
      refute gdf.geometry_column == "geometry"

      assert {:error, message} = GeoFrame.read_geojson(path, geometry_column: "geometry")
      assert message =~ "geometry"
    end

    test "the geometry column name is configurable", %{tmp_dir: dir} do
      path = Path.join(dir, "named.geojson")
      :ok = GeoFrame.write_geojson(sample(), path)
      gdf = GeoFrame.read_geojson!(path, geometry_column: "geom")
      assert GeoDataFrame.names(gdf) == ["id", "name", "score", "active", "geom"]
    end

    test "malformed input and missing files are errors", %{tmp_dir: dir} do
      path = Path.join(dir, "bad.geojson")
      File.write!(path, "{not json")
      assert {:error, message} = GeoFrame.read_geojson(path)
      assert message =~ "bad.geojson"

      assert {:error, missing} = GeoFrame.read_geojson(Path.join(dir, "nope.geojson"))
      assert missing =~ "nope.geojson"
      assert_raise ArgumentError, fn -> GeoFrame.read_geojson!(path) end
    end
  end

  describe "FlatGeobuf" do
    test "round trip without an index preserves rows, attributes, geometry and CRS",
         %{tmp_dir: dir} do
      path = Path.join(dir, "rt.fgb")
      gdf = sample()

      assert :ok = GeoFrame.write_flatgeobuf(gdf, path, index: false)
      assert {:ok, back} = GeoFrame.read_flatgeobuf(path)

      assert_same_frame(back, gdf)
      assert GeoDataFrame.crs(back) == "EPSG:4326"
    end

    test "the default writes a spatial index and may reorder rows", %{tmp_dir: dir} do
      path = Path.join(dir, "indexed.fgb")
      coords = for i <- 0..29, do: {rem(i * 7, 30), i}
      wkt = Enum.map(coords, fn {x, y} -> "POINT (#{x} #{y})" end)

      gdf =
        GeoDataFrame.new(
          DataFrame.new(id: Enum.to_list(0..29)),
          GeoSeries.from_wkt!(wkt, crs: "EPSG:4326")
        )

      assert :ok = GeoFrame.write_flatgeobuf(gdf, path)
      back = GeoFrame.read_flatgeobuf!(path)

      pairs = Enum.zip(column(back, "id"), wkt(back))
      assert pairs |> Enum.map(&elem(&1, 0)) |> Enum.sort() == Enum.to_list(0..29)

      for {id, text} <- pairs do
        {x, y} = Enum.at(coords, id)
        assert text == "POINT(#{x} #{y})"
      end
    end

    test "bbox reads only features intersecting the box through the index", %{tmp_dir: dir} do
      path = Path.join(dir, "bbox.fgb")
      wkt = for i <- 0..19, do: "POINT (#{i} #{i})"

      gdf =
        GeoDataFrame.new(
          DataFrame.new(id: Enum.to_list(0..19)),
          GeoSeries.from_wkt!(wkt, crs: "EPSG:4326")
        )

      :ok = GeoFrame.write_flatgeobuf(gdf, path)

      back = GeoFrame.read_flatgeobuf!(path, bbox: {4.5, 4.5, 7.5, 7.5})
      assert back |> column("id") |> Enum.sort() == [5, 6, 7]

      assert GeoFrame.read_flatgeobuf!(path, bbox: {100, 100, 101, 101}) |> GeoDataFrame.n_rows() ==
               0

      :ok = GeoFrame.write_flatgeobuf(gdf, path, index: false)
      assert {:error, message} = GeoFrame.read_flatgeobuf(path, bbox: {0, 0, 1, 1})
      assert message =~ "index"

      assert_raise ArgumentError, ~r/bbox/, fn ->
        GeoFrame.read_flatgeobuf(path, bbox: {0, 0, 1})
      end
    end

    test "CRS follows the header", %{tmp_dir: dir} do
      path = Path.join(dir, "crs.fgb")
      :ok = GeoFrame.write_flatgeobuf(sample(crs: "EPSG:3857"), path)
      assert path |> GeoFrame.read_flatgeobuf!() |> GeoDataFrame.crs() == "EPSG:3857"

      :ok = GeoFrame.write_flatgeobuf(sample(crs: nil), path)
      assert path |> GeoFrame.read_flatgeobuf!() |> GeoDataFrame.crs() == nil

      assert {:error, message} = GeoFrame.write_flatgeobuf(sample(crs: "utm"), path)
      assert message =~ "CRS"
    end

    test "narrow Explorer dtypes survive", %{tmp_dir: dir} do
      path = Path.join(dir, "dtypes.fgb")

      data =
        DataFrame.new(%{
          "small" => Series.from_list([1, nil], dtype: {:s, 8}),
          "wide" => Series.from_list([1, 2], dtype: {:u, 64}),
          "single" => Series.from_list([0.5, nil], dtype: {:f, 32})
        })

      gdf =
        GeoDataFrame.new(
          data,
          GeoSeries.from_wkt!(["POINT (0 0)", "POINT (1 1)"], crs: "EPSG:4326")
        )

      :ok = GeoFrame.write_flatgeobuf(gdf, path, index: false)
      back = GeoFrame.read_flatgeobuf!(path)

      assert back |> GeoDataFrame.data() |> DataFrame.dtypes() ==
               %{"small" => {:s, 8}, "wide" => {:u, 64}, "single" => {:f, 32}}

      assert column(back, "small") == [1, nil]
    end

    test "unreadable files are errors", %{tmp_dir: dir} do
      path = Path.join(dir, "junk.fgb")
      File.write!(path, "not flatgeobuf at all")
      assert {:error, message} = GeoFrame.read_flatgeobuf(path)
      assert message =~ "junk.fgb"
      assert_raise ArgumentError, fn -> GeoFrame.read_flatgeobuf!(path) end
    end
  end

  describe "read_file/2 and write_file/3" do
    test "dispatch on the extension, case-insensitively", %{tmp_dir: dir} do
      gdf = sample()

      for name <- ["a.parquet", "a.geoparquet", "a.geojson", "a.json", "a.fgb", "A.GeoJSON"] do
        path = Path.join(dir, name)
        assert :ok = GeoFrame.write_file(gdf, path)
        assert {:ok, back} = GeoFrame.read_file(path)
        assert wkt(back) == wkt(gdf), name
        assert column(back, "name") == ["a", "b", nil], name
      end

      assert File.read!(Path.join(dir, "a.json")) =~ "FeatureCollection"
      assert binary_part(File.read!(Path.join(dir, "a.fgb")), 0, 3) == "fgb"
      assert binary_part(File.read!(Path.join(dir, "a.parquet")), 0, 4) == "PAR1"
    end

    test "driver overrides the extension", %{tmp_dir: dir} do
      path = Path.join(dir, "layer.dat")
      gdf = sample()

      for driver <- [:parquet, :geojson, :flatgeobuf] do
        assert :ok = GeoFrame.write_file(gdf, path, driver: driver)
        assert {:ok, back} = GeoFrame.read_file(path, driver: driver)
        assert wkt(back) == wkt(gdf)
      end

      path = Path.join(dir, "misnamed.parquet")
      :ok = GeoFrame.write_file(gdf, path, driver: :geojson)
      assert File.read!(path) =~ "FeatureCollection"
    end

    test "options reach the format", %{tmp_dir: dir} do
      path = Path.join(dir, "opts.fgb")
      :ok = GeoFrame.write_file(sample(), path, index: false)
      assert path |> GeoFrame.read_file!() |> wkt() == wkt(sample())

      projected = sample(crs: "EPSG:3857")
      json = Path.join(dir, "forced.geojson")
      assert {:error, _} = GeoFrame.write_file(projected, json)
      assert :ok = GeoFrame.write_file(projected, json, force: true)

      parquet = Path.join(dir, "cols.parquet")
      :ok = GeoFrame.write_file(sample(), parquet)

      assert parquet |> GeoFrame.read_file!(columns: ["id"]) |> GeoDataFrame.names() == [
               "id",
               "geometry"
             ]
    end

    test "unknown extensions and drivers raise ArgumentError", %{tmp_dir: dir} do
      assert_raise ArgumentError, ~r/\.kml/, fn -> GeoFrame.read_file(Path.join(dir, "a.kml")) end

      assert_raise ArgumentError, ~r/driver/, fn ->
        GeoFrame.write_file(sample(), Path.join(dir, "noext"))
      end

      assert_raise ArgumentError, ~r/unknown driver/, fn ->
        GeoFrame.read_file(Path.join(dir, "a.fgb"), driver: :gml)
      end

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.write_file(sample(), Path.join(dir, "a.geojson"), index: false)
      end
    end

    test "failures are error tuples; the bang variant raises", %{tmp_dir: dir} do
      path = Path.join(dir, "missing.fgb")
      assert {:error, message} = GeoFrame.read_file(path)
      assert message =~ "missing.fgb"
      assert_raise ArgumentError, fn -> GeoFrame.read_file!(path) end
    end
  end

  describe "cross-format" do
    test "parquet -> flatgeobuf -> geojson reproduces attributes and geometry", %{tmp_dir: dir} do
      original = GeoFrame.read_parquet!(@fixture)

      fgb = Path.join(dir, "n.fgb")
      assert :ok = GeoFrame.write_flatgeobuf(original, fgb, index: false)
      via_fgb = GeoFrame.read_flatgeobuf!(fgb)
      assert_same_frame(via_fgb, original)
      assert GeoDataFrame.crs(via_fgb) == "EPSG:4326"

      json = Path.join(dir, "n.geojson")
      assert :ok = GeoFrame.write_geojson(via_fgb, json)
      via_json = GeoFrame.read_geojson!(json)
      assert_same_frame(via_json, original)

      parquet = Path.join(dir, "n.parquet")
      assert :ok = GeoFrame.write_parquet(via_json, parquet)
      assert_same_frame(GeoFrame.read_parquet!(parquet), original)
    end

    test "the indexed default keeps every row of the fixture", %{tmp_dir: dir} do
      original = GeoFrame.read_parquet!(@fixture)
      fgb = Path.join(dir, "indexed.fgb")
      :ok = GeoFrame.write_flatgeobuf(original, fgb)
      back = GeoFrame.read_flatgeobuf!(fgb)

      assert back |> column("name") |> Enum.sort() == original |> column("name") |> Enum.sort()
      assert back |> wkt() |> Enum.sort() == original |> wkt() |> Enum.sort()
    end
  end

  describe "properties" do
    defp random_frame(ids, names, points) do
      wkt = Enum.map(points, fn {x, y} -> "POINT (#{x} #{y})" end)

      data =
        DataFrame.new(%{
          "id" => Series.from_list(ids, dtype: {:s, 64}),
          "name" => Series.from_list(names, dtype: :string)
        })

      GeoDataFrame.new(data, GeoSeries.from_wkt!(wkt, crs: "EPSG:4326"))
    end

    property "random point frames round-trip through GeoJSON and FlatGeobuf", %{tmp_dir: dir} do
      point = {float(min: -180.0, max: 180.0), float(min: -90.0, max: 90.0)}
      row = {one_of([integer(), constant(nil)]), one_of([string(:alphanumeric), constant(nil)])}

      check all(rows <- list_of(tuple({point, row}), min_length: 1, max_length: 25), max_runs: 25) do
        {points, attrs} = Enum.unzip(rows)
        {ids, names} = Enum.unzip(attrs)
        gdf = random_frame(ids, names, points)
        stem = "prop_#{System.unique_integer([:positive])}"

        json = Path.join(dir, stem <> ".geojson")
        assert :ok = GeoFrame.write_geojson(gdf, json)
        back = GeoFrame.read_geojson!(json)
        assert GeoDataFrame.n_rows(back) == length(rows)
        assert column(back, "id") == ids
        assert column(back, "name") == names
        assert wkt(back) == wkt(gdf)

        fgb = Path.join(dir, stem <> ".fgb")
        assert :ok = GeoFrame.write_flatgeobuf(gdf, fgb, index: false)
        back = GeoFrame.read_flatgeobuf!(fgb)
        assert column(back, "id") == ids
        assert column(back, "name") == names
        assert wkt(back) == wkt(gdf)
      end
    end
  end
end
