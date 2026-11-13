defmodule GeoFrame.CurvesTest do
  # Curved geometries (CircularString, CompoundCurve, CurvePolygon, MultiCurve, MultiSurface)
  # are read linearized as GDAL does (B10c). The fixture is a GeoPackage whose rows hold curve
  # WKB, and the expected geometries are what pyogrio 0.13 (GDAL 3.12) returns for it; both come
  # from `test/reference/generate_curves.py`. Writing curves is unsupported: GeoFrame never
  # produces them.
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @fixtures Path.expand("../reference/fixtures/curves", __DIR__)
  @expected @fixtures |> Path.join("curves.json") |> File.read!() |> Jason.decode!()
  @cases Map.new(@expected["cases"], &{&1["name"], &1})

  defp from_hex(hex), do: Base.decode16!(hex, case: :mixed)

  defp wkt_of_expected(case_),
    do: [from_hex(case_["expected"])] |> GeoSeries.from_wkb!() |> GeoSeries.to_wkt()

  describe "GeoPackage with curve rows" do
    setup do
      gdf = GeoFrame.read_file!(Path.join(@fixtures, "curves.gpkg"))
      names = gdf |> GeoDataFrame.data() |> DataFrame.pull("name") |> Series.to_list()
      wkts = gdf |> GeoDataFrame.geometry() |> GeoSeries.to_wkt()
      {:ok, rows: Enum.zip(names, wkts), gdf: gdf}
    end

    test "reads every row instead of failing on the first curve", %{rows: rows} do
      assert length(rows) == 28
    end

    test "every row equals the geometry GDAL returns (GeoPandas read_file)", %{rows: rows} do
      for {name, wkt} <- rows do
        assert [wkt] == wkt_of_expected(@cases[name]), "case #{name}"
      end
    end

    test "geometry types are the linear equivalents", %{gdf: gdf} do
      types =
        gdf
        |> GeoDataFrame.geometry()
        |> GeoSeries.geom_type()
        |> Series.to_list()
        |> Enum.uniq()
        |> Enum.sort()

      assert types == [
               "GeometryCollection",
               "LineString",
               "MultiLineString",
               "MultiPolygon",
               "Polygon"
             ]
    end

    test "a half circle has 46 vertices, 45 steps of 4 degrees", %{rows: rows} do
      {_, wkt} = List.keyfind(rows, "circularstring_half", 0)
      assert wkt |> String.split(",") |> length() == 46
    end
  end

  describe "EWKB / WKB input" do
    test "EWKB with an SRID and Z is linearized and keeps the CRS" do
      case_ = @cases["ewkb_srid_z"]
      series = GeoSeries.from_wkb!([from_hex(case_["input"])])
      assert GeoSeries.crs(series) == "EPSG:4326"
      assert GeoSeries.to_wkt(series) == wkt_of_expected(case_)
    end

    test "a curve next to ordinary rows does not abort the read" do
      line = @cases["compoundcurve_mixed"]
      rows = [from_hex(line["input"]), nil, from_hex(@cases["multisurface"]["input"])]
      assert [a, nil, c] = rows |> GeoSeries.from_wkb!() |> GeoSeries.to_wkt()
      assert a == hd(wkt_of_expected(line))
      assert c == hd(wkt_of_expected(@cases["multisurface"]))
    end
  end
end
