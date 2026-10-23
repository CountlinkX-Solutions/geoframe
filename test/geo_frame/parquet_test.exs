defmodule GeoFrame.ParquetTest do
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

    GeoDataFrame.new(data, geometry, Keyword.take(opts, [:geometry_column]))
  end

  defp column(gdf, name),
    do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name) |> Series.to_list()

  describe "write_parquet/3 and read_parquet/2 round trip" do
    test "preserves attributes, nulls, geometry and CRS", %{tmp_dir: dir} do
      path = Path.join(dir, "round_trip.parquet")
      gdf = sample()

      assert :ok = GeoFrame.write_parquet(gdf, path)
      assert {:ok, back} = GeoFrame.read_parquet(path)

      assert GeoDataFrame.names(back) == ["id", "name", "score", "active", "geometry"]
      assert column(back, "id") == [1, nil, 3]
      assert column(back, "name") == ["a", "b", nil]
      assert column(back, "score") == [1.5, nil, 3.25]
      assert column(back, "active") == [true, false, nil]

      assert GeoSeries.to_wkt(GeoDataFrame.geometry(back)) ==
               GeoSeries.to_wkt(GeoDataFrame.geometry(gdf))

      assert GeoDataFrame.crs(back) == "EPSG:4326"
    end

    test "dtypes survive the Arrow IPC hop", %{tmp_dir: dir} do
      path = Path.join(dir, "dtypes.parquet")
      assert :ok = GeoFrame.write_parquet(sample(), path)
      {:ok, back} = GeoFrame.read_parquet(path)

      assert back |> GeoDataFrame.data() |> DataFrame.dtypes() ==
               %{"id" => {:s, 64}, "name" => :string, "score" => {:f, 64}, "active" => :boolean}
    end

    test "keeps a custom geometry column name", %{tmp_dir: dir} do
      path = Path.join(dir, "named.parquet")
      assert :ok = GeoFrame.write_parquet(sample(geometry_column: "geom"), path)
      {:ok, back} = GeoFrame.read_parquet(path)
      assert back.geometry_column == "geom"
      assert GeoDataFrame.names(back) == ["id", "name", "score", "active", "geom"]
    end

    test "OGC:CRS84 and an unknown CRS are kept distinct", %{tmp_dir: dir} do
      crs84 = Path.join(dir, "crs84.parquet")
      unknown = Path.join(dir, "unknown.parquet")
      assert :ok = GeoFrame.write_parquet(sample(crs: "OGC:CRS84"), crs84)
      assert :ok = GeoFrame.write_parquet(sample(crs: nil), unknown)

      assert GeoFrame.read_parquet!(crs84) |> GeoDataFrame.crs() == "OGC:CRS84"
      assert GeoFrame.read_parquet!(unknown) |> GeoDataFrame.crs() == nil
    end

    test "writes a frame without attribute columns", %{tmp_dir: dir} do
      path = Path.join(dir, "geometry_only.parquet")
      geometry = GeoSeries.from_wkt!(["POINT (0 0)", "POINT (1 1)"], crs: "EPSG:3857")
      gdf = GeoDataFrame.new(DataFrame.new([]), geometry)

      assert :ok = GeoFrame.write_parquet(gdf, path)
      back = GeoFrame.read_parquet!(path)
      assert GeoDataFrame.n_rows(back) == 2
      assert GeoDataFrame.crs(back) == "EPSG:3857"
    end

    test "writes a frame with zero rows", %{tmp_dir: dir} do
      path = Path.join(dir, "empty.parquet")
      gdf = GeoDataFrame.slice(sample(), 0, 0)
      assert :ok = GeoFrame.write_parquet(gdf, path)
      assert GeoDataFrame.n_rows(GeoFrame.read_parquet!(path)) == 0
    end

    test "the columns option selects attributes", %{tmp_dir: dir} do
      path = Path.join(dir, "columns.parquet")
      assert :ok = GeoFrame.write_parquet(sample(), path)

      assert {:ok, back} = GeoFrame.read_parquet(path, columns: ["score", "id"])
      assert GeoDataFrame.names(back) == ["id", "score", "geometry"]
      assert GeoDataFrame.n_rows(back) == 3

      assert {:error, message} = GeoFrame.read_parquet(path, columns: ["nope"])
      assert message =~ "nope"
    end

    property "round-trips random point frames", %{tmp_dir: dir} do
      point = {float(min: -180.0, max: 180.0), float(min: -90.0, max: 90.0)}
      row = {one_of([integer(), constant(nil)]), one_of([string(:alphanumeric), constant(nil)])}

      check all(
              rows <- list_of(tuple({point, row}), max_length: 25),
              max_runs: 25
            ) do
        {points, attrs} = Enum.unzip(rows)
        {ids, names} = Enum.unzip(attrs)

        wkt = Enum.map(points, fn {x, y} -> "POINT (#{x} #{y})" end)

        data =
          DataFrame.new(%{
            "id" => Series.from_list(ids, dtype: {:s, 64}),
            "name" => Series.from_list(names, dtype: :string)
          })

        gdf = GeoDataFrame.new(data, GeoSeries.from_wkt!(wkt, crs: "EPSG:4326"))

        path = Path.join(dir, "prop_#{System.unique_integer([:positive])}.parquet")
        assert :ok = GeoFrame.write_parquet(gdf, path)
        back = GeoFrame.read_parquet!(path)

        assert GeoDataFrame.n_rows(back) == length(rows)
        assert column(back, "id") == ids
        assert column(back, "name") == names
        assert GeoDataFrame.crs(back) == "EPSG:4326"

        assert GeoSeries.to_wkt(GeoDataFrame.geometry(back)) ==
                 GeoSeries.to_wkt(GeoDataFrame.geometry(gdf))
      end
    end
  end

  describe "committed fixture" do
    test "neighborhoods.parquet reads with attributes, geometry and CRS" do
      assert {:ok, gdf} = GeoFrame.read_parquet(@fixture)

      assert GeoDataFrame.names(gdf) == ["name", "kind", "population", "density", "geometry"]
      assert GeoDataFrame.n_rows(gdf) == 5
      assert GeoDataFrame.crs(gdf) == "EPSG:4326"
      assert column(gdf, "name") == ["Alpha", "Bravo", "Charlie", "Delta", "Echo"]
      assert column(gdf, "population") == [1200, 850, nil, 430, 15]

      areas = gdf |> GeoDataFrame.geometry() |> GeoSeries.area() |> Series.to_list()
      assert [a, b, c, d, nil_or_zero] = areas
      assert_in_delta a, 0.0001, 1.0e-12
      assert_in_delta b, 0.0004, 1.0e-12
      assert_in_delta c, 0.0001, 1.0e-12
      assert_in_delta d, 0.0002, 1.0e-12
      assert nil_or_zero == 0.0
    end
  end

  describe "errors" do
    test "a missing file is a clear error", %{tmp_dir: dir} do
      path = Path.join(dir, "missing.parquet")
      assert {:error, message} = GeoFrame.read_parquet(path)
      assert message =~ "missing.parquet"
      assert message =~ "cannot open"
    end

    test "plain Parquet without geo metadata is rejected", %{tmp_dir: dir} do
      path = Path.join(dir, "plain.parquet")
      :ok = DataFrame.to_parquet(DataFrame.new(id: [1, 2]), path)
      assert {:error, message} = GeoFrame.read_parquet(path)
      assert message =~ "not a GeoParquet file"
    end

    test "a file that is not Parquet is rejected", %{tmp_dir: dir} do
      path = Path.join(dir, "junk.parquet")
      File.write!(path, "definitely not parquet")
      assert {:error, message} = GeoFrame.read_parquet(path)
      assert message =~ "Parquet"
    end

    test "read_parquet!/2 raises on failure", %{tmp_dir: dir} do
      assert_raise ArgumentError, ~r/cannot open/, fn ->
        GeoFrame.read_parquet!(Path.join(dir, "missing.parquet"))
      end
    end

    test "write_parquet/3 reports an unwritable path", %{tmp_dir: dir} do
      path = Path.join([dir, "no_such_dir", "out.parquet"])
      assert {:error, message} = GeoFrame.write_parquet(sample(), path)
      assert message =~ "no_such_dir"
    end

    test "write_parquet/3 rejects a CRS it cannot represent", %{tmp_dir: dir} do
      path = Path.join(dir, "bad_crs.parquet")
      gdf = sample(crs: "not-a-crs")
      assert {:error, message} = GeoFrame.write_parquet(gdf, path)
      assert message =~ "CRS"
      refute File.exists?(path)
    end
  end
end
