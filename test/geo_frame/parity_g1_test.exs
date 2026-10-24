defmodule GeoFrame.ParityG1Test do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @moduletag :tmp_dir

  @fixtures Path.expand("../reference/fixtures/io", __DIR__)
  @manifest @fixtures |> Path.join("manifest.json") |> File.read!() |> Jason.decode!()

  @compressions [:snappy, :zstd, :gzip, :lz4, :brotli, :none]

  defp attrs do
    DataFrame.new(
      id: [1, nil, 3],
      name: ["a", "b", nil],
      score: [1.5, nil, 3.25],
      active: [true, false, nil]
    )
  end

  defp mixed(crs \\ "EPSG:4326") do
    geometry =
      GeoSeries.from_wkt!(
        ["POINT (1 2)", nil, "POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))"],
        crs: crs
      )

    GeoDataFrame.new(attrs(), geometry)
  end

  defp points(n, crs \\ "EPSG:4326") do
    wkt = for i <- 0..(n - 1), do: "POINT (#{i} #{i})"
    data = DataFrame.new(id: Enum.to_list(0..(n - 1)), name: Enum.map(0..(n - 1), &"n#{&1}"))
    GeoDataFrame.new(data, GeoSeries.from_wkt!(wkt, crs: crs))
  end

  defp polygons do
    geometry =
      GeoSeries.from_wkt!(
        [
          "POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))",
          nil,
          "MULTIPOLYGON (((10 10, 11 10, 11 11, 10 10)), ((20 20, 21 20, 21 21, 20 20)))"
        ],
        crs: "EPSG:3857"
      )

    GeoDataFrame.new(attrs(), geometry)
  end

  defp column(gdf, name),
    do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name) |> Series.to_list()

  defp wkt(gdf), do: gdf |> GeoDataFrame.geometry() |> GeoSeries.to_wkt()

  # WKT text differs between writers ("POINT (1 2)" vs "POINT(1 2)"), so normalise both.
  defp normalize_wkt(rows),
    do: rows |> GeoSeries.from_wkt!() |> GeoSeries.to_wkt()

  defp assert_same_frame(back, original) do
    assert GeoDataFrame.names(back) == GeoDataFrame.names(original)

    for name <- DataFrame.names(GeoDataFrame.data(original)) do
      assert column(back, name) == column(original, name), "column #{name}"
    end

    assert wkt(back) == wkt(original)
    assert GeoDataFrame.crs(back) == GeoDataFrame.crs(original)
  end

  # -- GeoParquet write options ---------------------------------------------------------

  describe "write_parquet/3 options" do
    test "every compression round-trips attributes, geometry and CRS", %{tmp_dir: dir} do
      for compression <- @compressions do
        path = Path.join(dir, "#{compression}.parquet")
        gdf = mixed()
        assert :ok = GeoFrame.write_parquet(gdf, path, compression: compression)
        assert {:ok, back} = GeoFrame.read_parquet(path)
        assert_same_frame(back, gdf)
      end
    end

    test "compression_level is accepted where the codec has levels", %{tmp_dir: dir} do
      path = Path.join(dir, "level.parquet")
      assert :ok = GeoFrame.write_parquet(mixed(), path, compression: :zstd, compression_level: 9)
      assert :ok = GeoFrame.write_parquet(mixed(), path, compression: :gzip, compression_level: 3)
      assert {:ok, _} = GeoFrame.read_parquet(path)

      assert {:error, message} =
               GeoFrame.write_parquet(mixed(), path, compression: :gzip, compression_level: 99)

      assert message =~ "compression_level"

      assert {:error, message} =
               GeoFrame.write_parquet(mixed(), path, compression: :snappy, compression_level: 3)

      assert message =~ "compression_level"
    end

    test "row_group_size splits the file and reads back whole", %{tmp_dir: dir} do
      path = Path.join(dir, "groups.parquet")
      gdf = points(10)
      assert :ok = GeoFrame.write_parquet(gdf, path, row_group_size: 3)
      assert {:ok, back} = GeoFrame.read_parquet(path)
      assert_same_frame(back, gdf)
    end

    test "geometry_encoding: :geoarrow round-trips and promotes singles to multi", %{tmp_dir: dir} do
      path = Path.join(dir, "native.parquet")
      gdf = points(6)
      assert :ok = GeoFrame.write_parquet(gdf, path, geometry_encoding: :geoarrow)
      assert {:ok, back} = GeoFrame.read_parquet(path)
      assert_same_frame(back, gdf)

      path = Path.join(dir, "native_poly.parquet")
      assert :ok = GeoFrame.write_parquet(polygons(), path, geometry_encoding: :geoarrow)
      assert {:ok, back} = GeoFrame.read_parquet(path)
      assert GeoDataFrame.crs(back) == "EPSG:3857"
      assert [multi, nil, _] = wkt(back)
      assert multi =~ "MULTIPOLYGON"
    end

    test "geometry_encoding: :geoarrow rejects mixed geometry kinds and leaves no file",
         %{tmp_dir: dir} do
      path = Path.join(dir, "mixed.parquet")

      assert {:error, message} =
               GeoFrame.write_parquet(mixed(), path, geometry_encoding: :geoarrow)

      assert message =~ "geoarrow"
      refute File.exists?(path)
    end

    test "write_covering_bbox adds the covering column but read hides it", %{tmp_dir: dir} do
      path = Path.join(dir, "covering.parquet")
      gdf = mixed()
      assert :ok = GeoFrame.write_parquet(gdf, path, write_covering_bbox: true)
      assert {:ok, back} = GeoFrame.read_parquet(path)
      assert GeoDataFrame.names(back) == ["id", "name", "score", "active", "geometry"]
      assert_same_frame(back, gdf)
    end

    test "schema_version 1.0.0 is accepted for WKB and rejected with 1.1 features",
         %{tmp_dir: dir} do
      path = Path.join(dir, "v1.parquet")
      assert :ok = GeoFrame.write_parquet(mixed(), path, schema_version: "1.0.0")
      assert {:ok, back} = GeoFrame.read_parquet(path)
      assert_same_frame(back, mixed())

      assert {:error, message} =
               GeoFrame.write_parquet(mixed(), path,
                 schema_version: "1.0.0",
                 write_covering_bbox: true
               )

      assert message =~ "schema_version"
    end

    test "invalid option values raise ArgumentError", %{tmp_dir: dir} do
      path = Path.join(dir, "bad.parquet")
      gdf = mixed()

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.write_parquet(gdf, path, compresion: :zstd)
      end

      assert_raise ArgumentError, ~r/compression/, fn ->
        GeoFrame.write_parquet(gdf, path, compression: :rar)
      end

      assert_raise ArgumentError, ~r/row_group_size/, fn ->
        GeoFrame.write_parquet(gdf, path, row_group_size: 0)
      end

      assert_raise ArgumentError, ~r/geometry_encoding/, fn ->
        GeoFrame.write_parquet(gdf, path, geometry_encoding: :wkt)
      end

      assert_raise ArgumentError, ~r/schema_version/, fn ->
        GeoFrame.write_parquet(gdf, path, schema_version: "3.0.0")
      end

      refute File.exists?(path)
    end
  end

  describe "read_parquet/2 bbox" do
    for {label, opts} <- [
          plain: [],
          covering: [write_covering_bbox: true],
          native: [geometry_encoding: :geoarrow]
        ] do
      test "filters rows by bounding box (#{label} file)", %{tmp_dir: dir} do
        path = Path.join(dir, "#{unquote(label)}.parquet")
        opts = Keyword.put(unquote(opts), :row_group_size, 5)
        assert :ok = GeoFrame.write_parquet(points(20), path, opts)

        assert {:ok, back} = GeoFrame.read_parquet(path, bbox: {6.5, 6.5, 9.5, 9.5})
        assert column(back, "id") == [7, 8, 9]
        assert wkt(back) == ["POINT(7 7)", "POINT(8 8)", "POINT(9 9)"]
        assert GeoDataFrame.crs(back) == "EPSG:4326"

        assert {:ok, back} = GeoFrame.read_parquet(path, bbox: {100, 100, 101, 101})
        assert GeoDataFrame.names(back) == ["id", "name", "geometry"]
        assert column(back, "id") == []

        assert {:ok, back} = GeoFrame.read_parquet(path, bbox: {19, 19, 30, 30}, columns: ["id"])
        assert GeoDataFrame.names(back) == ["id", "geometry"]
        assert column(back, "id") == [19]
      end
    end

    test "rejects a malformed bbox", %{tmp_dir: dir} do
      path = Path.join(dir, "b.parquet")
      assert :ok = GeoFrame.write_parquet(points(3), path)

      assert_raise ArgumentError, ~r/bbox/, fn -> GeoFrame.read_parquet(path, bbox: {1, 2, 3}) end

      assert_raise ArgumentError, ~r/bbox/, fn ->
        GeoFrame.read_parquet(path, bbox: {5, 0, 1, 1})
      end
    end
  end

  # -- Feather ----------------------------------------------------------------------------

  describe "write_feather/3 and read_feather/2" do
    test "round-trip every compression and geometry encoding", %{tmp_dir: dir} do
      for compression <- [:lz4, :zstd, :uncompressed],
          {encoding, gdf} <- [wkb: mixed(), geoarrow: polygons()] do
        path = Path.join(dir, "#{compression}_#{encoding}.feather")

        assert :ok =
                 GeoFrame.write_feather(gdf, path,
                   compression: compression,
                   geometry_encoding: encoding
                 )

        assert {:ok, back} = GeoFrame.read_feather(path)

        if encoding == :wkb do
          assert_same_frame(back, gdf)
        else
          # Native encoding promotes the single polygon to a multipolygon.
          assert GeoDataFrame.crs(back) == "EPSG:3857"
          assert column(back, "id") == column(gdf, "id")
          assert [_, nil, _] = wkt(back)
        end
      end
    end

    test "read_file/write_file dispatch .feather and .arrow, and driver: :feather",
         %{tmp_dir: dir} do
      gdf = mixed()

      for name <- ["a.feather", "a.ARROW"] do
        path = Path.join(dir, name)
        assert :ok = GeoFrame.write_file(gdf, path)
        assert {:ok, back} = GeoFrame.read_file(path)
        assert_same_frame(back, gdf)
      end

      path = Path.join(dir, "data.bin")
      assert :ok = GeoFrame.write_file(gdf, path, driver: :feather)
      assert {:ok, back} = GeoFrame.read_file(path, driver: :feather)
      assert_same_frame(back, gdf)
    end

    test "columns: selects attributes and errors name the problem", %{tmp_dir: dir} do
      path = Path.join(dir, "cols.feather")
      assert :ok = GeoFrame.write_feather(mixed(), path)
      assert {:ok, back} = GeoFrame.read_feather(path, columns: ["score"])
      assert GeoDataFrame.names(back) == ["score", "geometry"]
      assert {:error, message} = GeoFrame.read_feather(path, columns: ["nope"])
      assert message =~ "nope"
      assert {:error, message} = GeoFrame.read_feather(Path.join(dir, "missing.feather"))
      assert message =~ "missing.feather"
    end

    test "invalid options raise ArgumentError", %{tmp_dir: dir} do
      path = Path.join(dir, "bad.feather")

      assert_raise ArgumentError, ~r/compression/, fn ->
        GeoFrame.write_feather(mixed(), path, compression: :gzip)
      end

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        GeoFrame.read_feather(path, bbox: {0, 0, 1, 1})
      end
    end
  end

  # -- Arrow IPC interchange ------------------------------------------------------------

  describe "to_arrow/from_arrow" do
    test "GeoDataFrame identity for both encodings" do
      for {encoding, gdf} <- [wkb: mixed(), geoarrow: polygons()] do
        bytes = GeoDataFrame.to_arrow(gdf, geometry_encoding: encoding)
        assert is_binary(bytes)
        assert {:ok, back} = GeoDataFrame.from_arrow(bytes)

        if encoding == :wkb do
          assert_same_frame(back, gdf)
        else
          assert GeoDataFrame.crs(back) == "EPSG:3857"
          assert column(back, "name") == column(gdf, "name")
        end
      end
    end

    test "GeoSeries identity for both encodings" do
      series = GeoSeries.from_wkt!(["POINT (1 2)", nil, "POINT (3 4)"], crs: "EPSG:4326")

      for encoding <- [:wkb, :geoarrow] do
        bytes = GeoSeries.to_arrow(series, geometry_encoding: encoding)
        assert {:ok, back} = GeoSeries.from_arrow(bytes)
        assert GeoSeries.to_wkt(back) == GeoSeries.to_wkt(series)
        assert GeoSeries.crs(back) == "EPSG:4326"
      end

      assert GeoSeries.from_arrow!(GeoSeries.to_arrow(series)) |> GeoSeries.to_wkt() ==
               GeoSeries.to_wkt(series)
    end

    test "the schema carries GeoArrow extension metadata with PROJJSON CRS" do
      series = GeoSeries.from_wkt!(["POINT (1 2)"], crs: "EPSG:4326")
      wkb = GeoSeries.to_arrow(series)
      assert wkb =~ "geoarrow.wkb"
      assert wkb =~ "ARROW:extension:name"
      assert wkb =~ "ARROW:extension:metadata"
      assert wkb =~ "GeographicCRS"

      native = GeoSeries.to_arrow(series, geometry_encoding: :geoarrow)
      assert native =~ "geoarrow.point"
    end

    test "from_arrow accepts plain WKB, plain WKT and honours geometry: and crs:" do
      series = GeoSeries.from_wkt!(["POINT (1 2)", nil], crs: nil)
      wkb = DataFrame.new(%{"g" => Series.from_list(GeoSeries.to_wkb(series), dtype: :binary)})
      {:ok, ipc} = DataFrame.dump_ipc_stream(wkb)
      assert {:ok, back} = GeoSeries.from_arrow(ipc, geometry: "g", crs: "EPSG:4326")
      assert GeoSeries.to_wkt(back) == ["POINT(1 2)", nil]
      assert GeoSeries.crs(back) == "EPSG:4326"

      text = DataFrame.new(%{"geometry" => ["LINESTRING (0 0, 1 1)", nil], "id" => [1, 2]})
      {:ok, ipc} = DataFrame.dump_ipc_stream(text)
      assert {:ok, gdf} = GeoDataFrame.from_arrow(ipc)
      assert wkt(gdf) == ["LINESTRING(0 0,1 1)", nil]
      assert GeoDataFrame.names(gdf) == ["id", "geometry"]
    end

    test "errors are tuples, bang variants raise" do
      assert {:error, message} = GeoSeries.from_arrow("not arrow")
      assert message =~ "Arrow"
      assert_raise ArgumentError, fn -> GeoSeries.from_arrow!("not arrow") end

      {:ok, ipc} = DataFrame.dump_ipc_stream(DataFrame.new(a: [1]))
      assert {:error, message} = GeoDataFrame.from_arrow(ipc)
      assert message =~ "geometry"

      assert_raise ArgumentError, ~r/geometry_encoding/, fn ->
        GeoSeries.to_arrow(GeoSeries.from_wkt!(["POINT (1 1)"]), geometry_encoding: :foo)
      end

      assert_raise ArgumentError, ~r/geoarrow/, fn ->
        GeoSeries.to_arrow(GeoSeries.from_wkt!(["POINT (1 1)", "LINESTRING (0 0, 1 1)"]),
          geometry_encoding: :geoarrow
        )
      end
    end
  end

  # -- GeoPandas-written fixtures -----------------------------------------------------------

  describe "files written by GeoPandas 1.2 / pyarrow" do
    for {file, expected} <- @manifest["files"] do
      test "#{file} reads with the expected CRS, geometry and attributes" do
        file = unquote(file)
        expected = unquote(Macro.escape(expected))
        path = Path.join(@fixtures, file)

        gdf =
          if String.ends_with?(file, ".arrows") do
            GeoDataFrame.from_arrow!(File.read!(path))
          else
            GeoFrame.read_file!(path)
          end

        assert GeoDataFrame.crs(gdf) == expected["crs"]
        assert wkt(gdf) == normalize_wkt(expected["wkt"])

        for {name, values} <- expected["columns"] do
          assert column(gdf, name) == values, "#{file}: column #{name}"
        end

        refute "bbox" in GeoDataFrame.names(gdf)
      end
    end

    test "the covering fixtures answer bbox queries" do
      query = {0.5, 0.5, 3.5, 3.5}

      path = Path.join(@fixtures, "geoarrow_points_covering.parquet")
      assert {:ok, gdf} = GeoFrame.read_parquet(path, bbox: query)
      assert column(gdf, "id") == [2, nil]
      assert wkt(gdf) == ["POINT(1 1)", "POINT(3 3)"]

      path = Path.join(@fixtures, "wkb_covering.parquet")
      assert {:ok, gdf} = GeoFrame.read_parquet(path, bbox: query)
      assert column(gdf, "id") == [1, 2, nil]
    end

    test "GeoFrame-written files read back the same way as the GeoPandas ones", %{tmp_dir: dir} do
      expected = @manifest["files"]["wkb_lz4.feather"]
      path = Path.join(dir, "ours.feather")
      gdf = GeoFrame.read_file!(Path.join(@fixtures, "wkb_lz4.feather"))
      assert :ok = GeoFrame.write_file(gdf, path)
      back = GeoFrame.read_file!(path)
      assert wkt(back) == normalize_wkt(expected["wkt"])
      assert GeoDataFrame.crs(back) == expected["crs"]
    end
  end

  # -- CRS metadata (issue #88) -------------------------------------------------------------

  describe "GeoParquet CRS metadata against pyproj's PROJJSON" do
    @projjson @fixtures |> Path.join("crs_projjson.json") |> File.read!() |> Jason.decode!()

    defp written_crs(dir, code) do
      path = Path.join(dir, "crs_#{code}.parquet")
      assert :ok = GeoFrame.write_parquet(points(2, "EPSG:#{code}"), path)
      {path, GeoFrame.geoparquet_metadata!(path)["columns"]["geometry"]["crs"]}
    end

    # The parts of a PROJJSON object that define the CRS, in a comparable form. Names, datum
    # naming and axis directions are left out: the table GeoFrame translates from carries the
    # WKT1 names, and its axes are x/y as GeoParquet stores coordinates (pyproj lists EPSG:3035
    # as northing first). The axis units are compared.
    defp essence(crs) do
      datum =
        crs["datum"] || crs["datum_ensemble"] || get_in(crs, ["base_crs", "datum"]) ||
          get_in(crs, ["base_crs", "datum_ensemble"])

      %{
        type: crs["type"],
        id: crs["id"]["code"],
        ellipsoid:
          {datum["ellipsoid"]["semi_major_axis"], inverse_flattening(datum["ellipsoid"])},
        method: get_in(crs, ["conversion", "method", "id", "code"]),
        parameters:
          for(
            p <- get_in(crs, ["conversion", "parameters"]) || [],
            into: %{},
            do: {p["id"]["code"], p["value"]}
          ),
        axes: crs["coordinate_system"]["axis"] |> Enum.map(& &1["unit"])
      }
    end

    defp inverse_flattening(%{"inverse_flattening" => f}), do: f

    defp inverse_flattening(%{"semi_major_axis" => a, "semi_minor_axis" => b}),
      do: a / (a - b)

    for {code, reference} <- @projjson["crs"] do
      test "EPSG:#{code} is complete PROJJSON that agrees with pyproj", %{tmp_dir: dir} do
        code = unquote(code)
        reference = unquote(Macro.escape(reference))
        {path, crs} = written_crs(dir, code)

        # `type` is what pyproj insists on: without it proj_create fails on the object.
        assert crs["type"] == reference["type"]
        ours = essence(crs)
        theirs = essence(reference)

        assert ours.id == theirs.id
        assert ours.method == theirs.method
        assert ours.axes == theirs.axes
        assert elem(ours.ellipsoid, 0) == elem(theirs.ellipsoid, 0)
        assert_in_delta elem(ours.ellipsoid, 1), elem(theirs.ellipsoid, 1), 1.0e-6

        assert Map.keys(ours.parameters) |> Enum.sort() ==
                 Map.keys(theirs.parameters) |> Enum.sort()

        for {id, value} <- ours.parameters do
          assert_in_delta value, theirs.parameters[id], 1.0e-9, "parameter #{id}"
        end

        assert {:ok, back} = GeoFrame.read_parquet(path)
        assert GeoDataFrame.crs(back) == "EPSG:#{code}"
      end
    end

    test "GeoFrame reads the files GeoPandas wrote for these CRS" do
      for code <- [3035, 32633] do
        path = Path.join(@fixtures, "wkb_#{code}.parquet")
        assert GeoDataFrame.crs(GeoFrame.read_file!(path)) == "EPSG:#{code}"
      end
    end
  end

  # -- properties ---------------------------------------------------------------------------

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

    defp rows_gen do
      point = {float(min: -180.0, max: 180.0), float(min: -90.0, max: 90.0)}

      row =
        {one_of([integer(), constant(nil)]), one_of([string(:printable), constant(nil)])}

      list_of(tuple({point, row}), max_length: 25)
    end

    property "feather round trip", %{tmp_dir: dir} do
      check all(
              rows <- rows_gen(),
              crs <- member_of(["EPSG:4326", "EPSG:3857", nil]),
              encoding <- member_of([:wkb, :geoarrow]),
              compression <- member_of([:lz4, :zstd, :uncompressed]),
              max_runs: 25
            ) do
        gdf = frame_from(rows, crs)
        path = Path.join(dir, "prop_#{System.unique_integer([:positive])}.feather")

        assert :ok =
                 GeoFrame.write_feather(gdf, path,
                   geometry_encoding: encoding,
                   compression: compression
                 )

        assert {:ok, back} = GeoFrame.read_feather(path)
        assert_same_frame(back, gdf)
      end
    end

    property "arrow stream round trip", %{} do
      check all(
              rows <- rows_gen(),
              crs <- member_of(["EPSG:4326", "EPSG:3857", nil]),
              encoding <- member_of([:wkb, :geoarrow]),
              max_runs: 25
            ) do
        gdf = frame_from(rows, crs)
        bytes = GeoDataFrame.to_arrow(gdf, geometry_encoding: encoding)
        assert {:ok, back} = GeoDataFrame.from_arrow(bytes)
        assert_same_frame(back, gdf)

        series = GeoDataFrame.geometry(gdf)

        back =
          series |> GeoSeries.to_arrow(geometry_encoding: encoding) |> GeoSeries.from_arrow!()

        assert GeoSeries.to_wkt(back) == GeoSeries.to_wkt(series)
        assert GeoSeries.crs(back) == crs
      end
    end

    property "parquet options round trip", %{tmp_dir: dir} do
      check all(
              rows <- rows_gen(),
              compression <- member_of(@compressions),
              covering <- boolean(),
              encoding <- member_of([:wkb, :geoarrow]),
              max_runs: 25
            ) do
        gdf = frame_from(rows, "EPSG:4326")
        path = Path.join(dir, "prop_#{System.unique_integer([:positive])}.parquet")

        assert :ok =
                 GeoFrame.write_parquet(gdf, path,
                   compression: compression,
                   write_covering_bbox: covering,
                   geometry_encoding: encoding,
                   row_group_size: 7
                 )

        assert {:ok, back} = GeoFrame.read_parquet(path)
        assert_same_frame(back, gdf)
      end
    end
  end
end
