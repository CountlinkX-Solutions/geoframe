defmodule GeoFrame.ParityDTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.Series
  alias GeoFrame.CRS
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @unit "POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))"

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(%Series{} = series), do: Series.to_list(series)

  defp equal?(%GeoSeries{} = actual, expected) when is_list(expected) do
    list(GeoSeries.geom_equals_exact(actual, s(expected), 1.0e-9))
  end

  describe "skew" do
    test "45 degrees in x about the centre matches Shapely" do
      out = GeoSeries.skew(s([@unit, nil]), 45, 0)
      assert equal?(out, ["POLYGON ((-0.5 0, 0.5 0, 1.5 1, 0.5 1, -0.5 0))", nil]) == [true, nil]
    end

    test "origin coordinates, centroid and radians" do
      out = GeoSeries.skew(s([@unit]), 45, 0, origin: {0, 0})
      assert equal?(out, ["POLYGON ((0 0, 1 0, 2 1, 1 1, 0 0))"]) == [true]

      centroid = GeoSeries.skew(s([@unit]), 45, 0, origin: :centroid)
      assert equal?(centroid, ["POLYGON ((-0.5 0, 0.5 0, 1.5 1, 0.5 1, -0.5 0))"]) == [true]

      rad = GeoSeries.skew(s([@unit]), :math.pi() / 4, 0, use_radians: true)
      assert equal?(rad, ["POLYGON ((-0.5 0, 0.5 0, 1.5 1, 0.5 1, -0.5 0))"]) == [true]
    end

    test "skew(0, 0) is the identity and the CRS is kept" do
      series = s([@unit, "POINT (3 4)"], crs: "EPSG:3857")
      out = GeoSeries.skew(series, 0, 0)
      assert GeoSeries.crs(out) == "EPSG:3857"
      assert list(GeoSeries.geom_equals_exact(out, series, 1.0e-12)) == [true, true]
    end

    test "per-row angles from a list or a series, nil nulls the row" do
      series = s([@unit, @unit, @unit])
      out = GeoSeries.skew(series, [45, 0, nil], Series.from_list([0, 0, 0]))

      assert equal?(out, ["POLYGON ((-0.5 0, 0.5 0, 1.5 1, 0.5 1, -0.5 0))", @unit, nil]) ==
               [true, true, nil]
    end
  end

  describe "affine_transform" do
    test "identity, axis swap and a general matrix" do
      series = s(["LINESTRING (1 2, 3 4)", nil])

      assert equal?(GeoSeries.affine_transform(series, [1, 0, 0, 1, 0, 0]), [
               "LINESTRING (1 2, 3 4)",
               nil
             ]) == [true, nil]

      assert equal?(GeoSeries.affine_transform(series, [0, 1, 1, 0, 0, 0]), [
               "LINESTRING (2 1, 4 3)",
               nil
             ]) == [true, nil]

      assert equal?(GeoSeries.affine_transform(series, [2, 1, 0, 3, 10, 20]), [
               "LINESTRING (14 26, 20 32)",
               nil
             ]) == [true, nil]
    end

    test "only 6- or 12-element numeric matrices are accepted" do
      series = s([@unit])

      assert_raise ArgumentError, ~r/6 numbers/, fn ->
        GeoSeries.affine_transform(series, [1, 0, 0])
      end

      assert_raise ArgumentError, ~r/12 numbers/, fn ->
        GeoSeries.affine_transform(series, [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0])
      end

      assert series
             |> GeoSeries.affine_transform([1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0])
             |> GeoSeries.to_wkt() == GeoSeries.to_wkt(series)

      assert_raise ArgumentError, fn ->
        GeoSeries.affine_transform(series, [1, 0, 0, 1, 0, "a"])
      end
    end
  end

  describe "per-row translate / scale / rotate" do
    test "translate with lists and series" do
      series = s(["POINT (0 0)", "POINT (0 0)", "POINT (0 0)"])
      out = GeoSeries.translate(series, [1, 2, nil], Series.from_list([10, 20, 30]))
      assert equal?(out, ["POINT (1 10)", "POINT (2 20)", nil]) == [true, true, nil]
      mixed = GeoSeries.translate(series, [1, 2, 3], 5)
      assert equal?(mixed, ["POINT (1 5)", "POINT (2 5)", "POINT (3 5)"]) == [true, true, true]
    end

    test "scale and rotate with per-row factors and origins" do
      series = s([@unit, @unit])

      out =
        GeoSeries.scale(series, [2, 1], 2, origin: [{0, 0}, {0.5, 0.5}])

      assert equal?(out, [
               "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))",
               "POLYGON ((0 -0.5, 1 -0.5, 1 1.5, 0 1.5, 0 -0.5))"
             ]) == [true, true]

      rot = GeoSeries.rotate(series, [90, 0], origin: {0, 0})

      assert equal?(rot, ["POLYGON ((0 0, 0 1, -1 1, -1 0, 0 0))", @unit]) == [true, true]
    end

    test "length mismatches raise" do
      series = s([@unit, @unit])
      assert_raise ArgumentError, ~r/length/, fn -> GeoSeries.translate(series, [1], 0) end
      assert_raise ArgumentError, ~r/length/, fn -> GeoSeries.rotate(series, [1, 2, 3]) end

      assert_raise ArgumentError, ~r/length/, fn ->
        GeoSeries.scale(series, 1, 1, origin: [{0, 0}])
      end
    end

    test "invalid elements raise" do
      series = s([@unit])
      assert_raise ArgumentError, fn -> GeoSeries.translate(series, ["a"], 0) end
    end
  end

  describe "wrap_longitude" do
    test "wraps into [-180, 180) and keeps the CRS" do
      series =
        s(["POINT (190 10)", "POINT (-190 10)", "POINT (180 0)", "POINT (10 5)", nil],
          crs: "EPSG:4326"
        )

      out = GeoSeries.wrap_longitude(series)
      assert GeoSeries.crs(out) == "EPSG:4326"
      assert list(GeoSeries.x(out)) == [-170.0, 170.0, -180.0, 10.0, nil]
    end

    test "naive series are assumed lon/lat, projected CRSs raise" do
      assert [-160.0] =
               "POINT (200 0)"
               |> List.wrap()
               |> s()
               |> GeoSeries.wrap_longitude()
               |> GeoSeries.x()
               |> list()

      assert_raise ArgumentError, ~r/geographic/, fn ->
        "POINT (200 0)" |> List.wrap() |> s(crs: "EPSG:3857") |> GeoSeries.wrap_longitude()
      end
    end
  end

  describe "is_geographic" do
    test "by CRS" do
      assert GeoSeries.is_geographic(s(["POINT (0 0)"], crs: "EPSG:4326")) == true
      assert GeoSeries.is_geographic(s(["POINT (0 0)"], crs: "OGC:CRS84")) == true
      assert GeoSeries.is_geographic(s(["POINT (0 0)"], crs: "EPSG:3857")) == false
      assert GeoSeries.is_geographic(s(["POINT (0 0)"])) == nil
    end

    test "GeoDataFrame delegate and the CRS helper" do
      gdf = GeoDataFrame.new(Explorer.DataFrame.new(a: [1]), s(["POINT (0 0)"], crs: "EPSG:4326"))
      assert GeoDataFrame.is_geographic(gdf) == true
      assert CRS.geographic?("EPSG:4269") == true
      assert CRS.geographic?("EPSG:32633") == false
      assert CRS.geographic?("bogus") == nil
      assert CRS.geographic?(nil) == nil
    end
  end

  describe "properties" do
    defp coord, do: StreamData.integer(-50..50)

    defp box_wkt do
      gen all(
            x <- coord(),
            y <- coord(),
            w <- StreamData.integer(1..20),
            h <- StreamData.integer(1..20)
          ) do
        "POLYGON ((#{x} #{y}, #{x + w} #{y}, #{x + w} #{y + h}, #{x} #{y + h}, #{x} #{y}))"
      end
    end

    property "affine_transform with a translation matrix equals translate" do
      check all(
              wkts <- StreamData.list_of(box_wkt(), min_length: 1, max_length: 4),
              dx <- coord(),
              dy <- coord()
            ) do
        series = s(wkts)
        a = GeoSeries.affine_transform(series, [1, 0, 0, 1, dx, dy])
        b = GeoSeries.translate(series, dx, dy)
        assert Enum.all?(list(GeoSeries.geom_equals_exact(a, b, 1.0e-9)))
      end
    end

    property "rotate then inverse rotate about the same origin is the identity" do
      check all(
              wkts <- StreamData.list_of(box_wkt(), min_length: 1, max_length: 4),
              deg <- StreamData.integer(-360..360),
              ox <- coord(),
              oy <- coord()
            ) do
        series = s(wkts)

        back =
          series
          |> GeoSeries.rotate(deg, origin: {ox, oy})
          |> GeoSeries.rotate(-deg, origin: {ox, oy})

        assert Enum.all?(list(GeoSeries.geom_equals_exact(series, back, 1.0e-9)))
      end
    end

    property "wrap_longitude is idempotent and lands in range" do
      check all(
              xs <-
                StreamData.list_of(StreamData.integer(-2000..2000), min_length: 1, max_length: 6)
            ) do
        series = xs |> Enum.map(&"POINT (#{&1} 0)") |> s(crs: "EPSG:4326")
        once = GeoSeries.wrap_longitude(series)
        twice = GeoSeries.wrap_longitude(once)
        assert list(GeoSeries.x(once)) == list(GeoSeries.x(twice))
        assert Enum.all?(list(GeoSeries.x(once)), &(&1 >= -180 and &1 < 180))
      end
    end
  end
end
