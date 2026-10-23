defmodule GeoFrame.ZmTest do
  use ExUnit.Case, async: true

  alias Explorer.Series
  alias GeoFrame.GeoSeries

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(series), do: Series.to_list(series)

  @rows [
    "POINT Z (1 2 3)",
    "POINT M (1 2 4)",
    "POINT ZM (1 2 3 4)",
    "POINT (1 2)",
    "LINESTRING Z (0 0 1, 1 1 2)",
    nil
  ]

  test "has_z/1 and has_m/1" do
    series = s(@rows)
    assert list(GeoSeries.has_z(series)) == [true, false, true, false, true, false]
    assert list(GeoSeries.has_m(series)) == [false, true, true, false, false, false]
  end

  test "z/1 and m/1 read points only and alias get_z/1, get_m/1" do
    series = s(@rows)
    assert list(GeoSeries.z(series)) == [3.0, nil, 3.0, nil, nil, nil]
    assert list(GeoSeries.m(series)) == [nil, 4.0, 4.0, nil, nil, nil]
    assert list(GeoSeries.get_z(series)) == list(GeoSeries.z(series))
    assert list(GeoSeries.get_m(series)) == list(GeoSeries.m(series))
  end

  test "force_2d/1 drops Z and M and keeps the CRS" do
    flat = @rows |> s(crs: "EPSG:4326") |> GeoSeries.force_2d()
    assert GeoSeries.crs(flat) == "EPSG:4326"
    assert list(GeoSeries.has_z(flat)) == List.duplicate(false, 6)
    assert list(GeoSeries.has_m(flat)) == List.duplicate(false, 6)
    assert Enum.at(GeoSeries.to_wkt(flat), 4) == "LINESTRING(0 0,1 1)"
    assert Enum.at(GeoSeries.to_wkt(flat), 5) == nil
  end

  test "force_3d/2 takes a number, a list or a series" do
    series = s(["POINT (1 2)", "POINT Z (1 2 3)", nil])
    assert series |> GeoSeries.force_3d() |> GeoSeries.z() |> list() == [0.0, 3.0, nil]
    assert series |> GeoSeries.force_3d(7) |> GeoSeries.z() |> list() == [7.0, 3.0, nil]

    assert series |> GeoSeries.force_3d([5, 6, 7]) |> GeoSeries.z() |> list() == [5.0, 3.0, nil]

    assert series
           |> GeoSeries.force_3d(Series.from_list([1.5, 2.5, 3.5]))
           |> GeoSeries.z()
           |> list() == [1.5, 3.0, nil]
  end

  test "force_3d/2 rejects a wrong length and a non-number" do
    series = s(["POINT (1 2)"])
    assert_raise ArgumentError, ~r/length/, fn -> GeoSeries.force_3d(series, [1, 2]) end
    assert_raise ArgumentError, ~r/number/, fn -> GeoSeries.force_3d(series, "x") end
  end

  test "Z and M survive WKB, EWKB, take and concat" do
    series = s(@rows, crs: "EPSG:4326")

    via_wkb = series |> GeoSeries.to_wkb() |> GeoSeries.from_wkb!()
    assert GeoSeries.to_wkt(via_wkb) == GeoSeries.to_wkt(series)

    via_ewkb = series |> GeoSeries.to_wkb(srid: 4326) |> GeoSeries.from_wkb!()
    assert list(GeoSeries.has_m(via_ewkb)) == list(GeoSeries.has_m(series))

    taken = GeoSeries.take(series, [2, 0])
    assert list(GeoSeries.m(taken)) == [4.0, nil]

    assert list(GeoSeries.has_z(GeoSeries.concat([series, taken]))) |> Enum.take(-2) == [
             true,
             true
           ]
  end

  test "a collection mixing member dimensions is an error" do
    assert_raise ArgumentError, ~r/dimension/, fn ->
      s(["GEOMETRYCOLLECTION (POINT (1 2), POINT Z (1 2 3))"])
    end
  end

  describe "Z/M through constructive operations" do
    test "translate/4 and scale/4 take :zoff and :zfact" do
      series = s(["LINESTRING Z (0 0 1, 1 1 2)", "POINT M (1 2 3)"])

      assert series |> GeoSeries.translate(1, 2, zoff: 3) |> GeoSeries.to_wkt() ==
               ["LINESTRING Z(1 2 4,2 3 5)", "POINT(2 4)"]

      assert series
             |> GeoSeries.scale(2, 3, zfact: 4, origin: {0, 0})
             |> GeoSeries.to_wkt() == ["LINESTRING Z(0 0 4,2 3 8)", "POINT(2 6)"]

      assert_raise ArgumentError, ~r/zoff/, fn -> GeoSeries.translate(series, 1, 2, zoff: "x") end

      assert_raise ArgumentError, ~r/scalar/, fn ->
        GeoSeries.translate(series, [1, 2], [1, 2], zoff: 1)
      end
    end

    test "affine_transform/2 takes 6 or 12 numbers" do
      series = s(["POINT Z (1 2 3)", "POINT (1 2)"])

      assert series |> GeoSeries.affine_transform([1, 0, 0, 1, 5, 5]) |> GeoSeries.to_wkt() ==
               ["POINT Z(6 7 3)", "POINT(6 7)"]

      assert series
             |> GeoSeries.affine_transform([1, 0, 1, 0, 1, 0, 0, 0, 2, 5, 5, 5])
             |> GeoSeries.to_wkt() == ["POINT Z(9 7 11)", "POINT(6 7)"]

      assert_raise ArgumentError, ~r/12 numbers/, fn ->
        GeoSeries.affine_transform(series, [1, 2, 3])
      end
    end

    test "vertex copies keep Z and M, simplify and to_crs drop M" do
      line = s(["LINESTRING ZM (0 0 1 10, 5 0.01 2 20, 10 0 3 30, 10 10 4 40)"])

      assert line |> GeoSeries.reverse() |> GeoSeries.to_wkt() ==
               ["LINESTRING ZM(10 10 4 40,10 0 3 30,5 0.01 2 20,0 0 1 10)"]

      assert line |> GeoSeries.boundary() |> GeoSeries.to_wkt() ==
               ["MULTIPOINT ZM((0 0 1 10),(10 10 4 40))"]

      assert line |> GeoSeries.simplify(1.0, preserve_topology: false) |> GeoSeries.to_wkt() ==
               ["LINESTRING Z(0 0 1,10 0 3,10 10 4)"]

      point = s(["POINT ZM (-80 25 100 5)"], crs: "EPSG:4326")
      assert point |> GeoSeries.to_crs(3857) |> GeoSeries.z() |> list() == [100.0]
      assert point |> GeoSeries.to_crs(3857) |> GeoSeries.has_m() |> list() == [false]
    end

    test "centroid and envelope stay 2D" do
      series = s(["POLYGON Z ((0 0 1, 4 0 2, 4 4 3, 0 0 1))"])
      assert series |> GeoSeries.envelope() |> GeoSeries.has_z() |> list() == [false]
      assert series |> GeoSeries.centroid() |> GeoSeries.has_z() |> list() == [false]
    end
  end
end
