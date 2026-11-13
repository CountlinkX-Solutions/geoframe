defmodule GeoFrame.CrsSupplementaryIoTest do
  use ExUnit.Case, async: true

  import ExUnit.CaptureLog

  alias Explorer.DataFrame
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @moduletag :tmp_dir

  # Codes the `crs-definitions` table lacks or leaves empty: Transverse Mercator (3144), Laborde
  # (29701) and a geographic 2D one (8694).
  @codes [3144, 29_701, 8694]

  defp frame(code) do
    geometry = GeoSeries.from_wkt!(["POINT (1 2)", "POINT (3 4)"], crs: "EPSG:#{code}")
    GeoDataFrame.new(DataFrame.new(id: [1, 2]), geometry)
  end

  for code <- @codes do
    test "EPSG:#{code} survives a Shapefile round trip", %{tmp_dir: dir} do
      path = Path.join(dir, "#{unquote(code)}.shp")

      log =
        capture_log(fn -> assert :ok = GeoFrame.write_shapefile(frame(unquote(code)), path) end)

      refute log =~ "no .prj"

      assert File.read!(Path.join(dir, "#{unquote(code)}.prj")) =~
               ~s(AUTHORITY["EPSG","#{unquote(code)}"])

      assert GeoDataFrame.crs(GeoFrame.read_shapefile!(path)) == "EPSG:#{unquote(code)}"
    end

    test "EPSG:#{code} survives a GeoPackage round trip", %{tmp_dir: dir} do
      path = Path.join(dir, "#{unquote(code)}.gpkg")
      log = capture_log(fn -> assert :ok = GeoFrame.write_gpkg(frame(unquote(code)), path) end)
      refute log =~ "undefined"
      assert GeoDataFrame.crs(GeoFrame.read_gpkg!(path)) == "EPSG:#{unquote(code)}"
    end
  end

  test "a code PROJ cannot write as WKT1 keeps warning and writes no .prj", %{tmp_dir: dir} do
    # EPSG:5515 is Krovak Modified: no WKT1 form exists.
    path = Path.join(dir, "krovak.shp")
    log = capture_log(fn -> assert :ok = GeoFrame.write_shapefile(frame(5515), path) end)
    assert log =~ "EPSG:5515"
    refute File.exists?(Path.join(dir, "krovak.prj"))
  end
end
