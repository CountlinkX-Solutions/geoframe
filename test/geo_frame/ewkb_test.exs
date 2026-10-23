defmodule GeoFrame.EwkbTest do
  use ExUnit.Case, async: true

  alias GeoFrame.GeoSeries

  @point_srid "0101000020E6100000000000000000F03F0000000000000040"

  test "from_wkb accepts EWKB and takes the CRS from the SRID" do
    s = GeoSeries.from_wkb!([Base.decode16!(@point_srid), nil])
    assert GeoSeries.crs(s) == "EPSG:4326"
    assert GeoSeries.to_wkt(s) == ["POINT(1 2)", nil]
  end

  test "an explicit crs overrides the SRID" do
    s = GeoSeries.from_wkb!([Base.decode16!(@point_srid)], crs: "EPSG:3857")
    assert GeoSeries.crs(s) == "EPSG:3857"
  end

  test "mixed SRIDs are an error" do
    other = GeoSeries.from_wkt!(["POINT (1 2)"], crs: "EPSG:3857") |> GeoSeries.to_wkb(srid: 3857)
    assert {:error, message} = GeoSeries.from_wkb([Base.decode16!(@point_srid) | other])
    assert message =~ "mixed SRIDs"
  end

  test "to_wkb with srid: emits EWKB" do
    s = GeoSeries.from_wkt!(["POINT (1 2)", nil])
    assert GeoSeries.to_wkb(s, srid: 4326, hex: true) == [@point_srid, nil]
  end

  test "to_wkb with srid: true uses the EPSG code of the CRS" do
    s = GeoSeries.from_wkt!(["POINT (1 2)"], crs: "EPSG:4326")
    assert GeoSeries.to_wkb(s, srid: true, hex: true) == [@point_srid]

    assert s |> GeoSeries.to_wkb(srid: true) |> GeoSeries.from_wkb!() |> GeoSeries.crs() ==
             "EPSG:4326"
  end

  test "srid: true needs an EPSG CRS" do
    s = GeoSeries.from_wkt!(["POINT (1 2)"])
    assert_raise ArgumentError, ~r/EPSG/, fn -> GeoSeries.to_wkb(s, srid: true) end
    assert_raise ArgumentError, ~r/srid/, fn -> GeoSeries.to_wkb(s, srid: "x") end
  end
end
