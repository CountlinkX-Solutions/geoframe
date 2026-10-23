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
end
