defmodule GeoFrame.HardeningTest do
  use ExUnit.Case, async: true

  alias GeoFrame.GeoSeries

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)

  test "deeply nested WKT is an error and the VM survives" do
    deep = String.duplicate("GEOMETRYCOLLECTION (", 100_000)
    assert {:error, msg} = GeoSeries.from_wkt([deep])
    assert msg =~ "nesting too deep"
    assert GeoSeries.size(s(["POINT (1 2)"])) == 1
  end

  test "deeply nested WKB is an error and the VM survives" do
    bytes = String.duplicate(<<1, 7, 0, 0, 0, 1, 0, 0, 0>>, 20_000)
    assert {:error, msg} = GeoSeries.from_wkb([bytes])
    assert msg =~ "invalid WKB at row 0"
  end

  test "sample_points rejects totals above the cap" do
    series = s(["POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))"])

    assert_raise ArgumentError, ~r/exceeds the maximum/, fn ->
      GeoSeries.sample_points(series, 10_000_001)
    end

    assert_raise ArgumentError, ~r/exceeds the maximum/, fn ->
      GeoSeries.sample_points(series, [6_000_000, 6_000_000])
    end
  end

  test "union_groups validates group counts and ids" do
    series = s(["POINT (0 0)", "POINT (1 1)"])

    assert_raise ArgumentError, ~r/cannot exceed/, fn ->
      GeoSeries.union_groups(series, [0, 1], 1_000_000_000_000)
    end

    assert_raise ArgumentError, ~r/out of range/, fn ->
      GeoSeries.union_groups(series, [0, 5], 2)
    end
  end

  test "segmentize and make_grid reject outputs above the caps" do
    line = s(["LINESTRING (0 0, 20000000 0)"])

    assert_raise ArgumentError, ~r/segmentize would create/, fn ->
      GeoSeries.segmentize(line, 1)
    end

    extent = s(["POLYGON ((0 0, 3300 0, 3300 3300, 0 3300, 0 0))"])

    assert_raise ArgumentError, ~r/grid too large/, fn ->
      GeoSeries.make_grid(extent, 1, feature_type: :centers, intersect: false)
    end
  end

  test "inspect previews only the first rows" do
    series = s(List.duplicate("POINT (1 2)", 50))
    text = inspect(series)
    assert text =~ "size: 50"
    assert text =~ "..."
    assert inspect(s([])) =~ "size: 0"
  end

  describe "CRS equivalence" do
    alias Explorer.DataFrame
    alias GeoFrame.GeoDataFrame

    @tag :tmp_dir
    test "a GeoJSON frame (OGC:CRS84) joins a FlatGeobuf frame (EPSG:4326)", %{tmp_dir: dir} do
      polygons =
        GeoDataFrame.new(
          DataFrame.new(name: ["A"]),
          s(["POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))"], crs: "EPSG:4326")
        )

      points =
        GeoDataFrame.new(
          DataFrame.new(id: [1, 2]),
          s(["POINT (1 1)", "POINT (9 9)"], crs: "EPSG:4326")
        )

      geojson = Path.join(dir, "points.geojson")
      fgb = Path.join(dir, "polygons.fgb")
      :ok = GeoFrame.write_geojson(points, geojson)
      :ok = GeoFrame.write_flatgeobuf(polygons, fgb)

      {:ok, from_geojson} = GeoFrame.read_geojson(geojson)
      {:ok, from_fgb} = GeoFrame.read_flatgeobuf(fgb)
      assert GeoDataFrame.crs(from_geojson) == "OGC:CRS84"
      assert GeoDataFrame.crs(from_fgb) == "EPSG:4326"

      joined = GeoFrame.sjoin(from_geojson, from_fgb)
      assert DataFrame.n_rows(joined.data) == 1
      assert GeoDataFrame.crs(joined) == "OGC:CRS84"
    end

    test "labels compare case-insensitively and mismatches still raise" do
      assert GeoFrame.CRS.equivalent?("epsg:4326", "EPSG:4326")
      assert GeoFrame.CRS.equivalent?("OGC:CRS84", "EPSG:4326")
      refute GeoFrame.CRS.equivalent?("EPSG:4326", "EPSG:3857")

      a = s(["POINT (1 1)"], crs: "epsg:4326")
      b = s(["POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))"], crs: "EPSG:4326")
      assert GeoSeries.intersects(a, b) |> Explorer.Series.to_list() == [true]
      c = s(["POINT (1 1)"], crs: "EPSG:3857")
      assert_raise ArgumentError, fn -> GeoSeries.intersects(a, c) end
    end
  end
end
