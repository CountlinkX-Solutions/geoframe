defmodule GeoFrame.ZmCoordsTest do
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoSeries
  alias GeoFrame.Reference

  @fixture Reference.read("zm_coords")

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(series), do: Series.to_list(series)

  defp column(df, name) do
    df[name] |> Series.to_list() |> Enum.map(&if(&1 in [:nan, "NaN"], do: nil, else: &1))
  end

  describe "from_xy z:" do
    test "builds POINT Z" do
      series = GeoSeries.from_xy([1, 2], [3, 4], z: [5, 6])
      assert GeoSeries.to_wkt(series) == ["POINT Z(1 3 5)", "POINT Z(2 4 6)"]
      assert list(GeoSeries.has_z(series)) == [true, true]
      assert list(GeoSeries.z(series)) == [5.0, 6.0]
    end

    test "a missing z is NaN and a missing x or y is a null row" do
      series = GeoSeries.from_xy([1, nil, 3], [2, 2, 4], z: [nil, 9, 7])
      assert GeoSeries.to_wkt(series) == ["POINT Z(1 2 NaN)", nil, "POINT Z(3 4 7)"]
    end

    test "accepts an Explorer series and a crs" do
      series =
        GeoSeries.from_xy(Series.from_list([1.0]), Series.from_list([2.0]),
          z: Series.from_list([3.0]),
          crs: "EPSG:4326"
        )

      assert GeoSeries.to_wkt(series) == ["POINT Z(1 2 3)"]
      assert GeoSeries.crs(series) =~ "4326"
    end

    test "rejects a z of another length" do
      assert_raise ArgumentError, ~r/same length/, fn ->
        GeoSeries.from_xy([1, 2], [3, 4], z: [1])
      end
    end

    test "without z stays 2D" do
      assert [false] == [1] |> GeoSeries.from_xy([2]) |> GeoSeries.has_z() |> list()
    end

    test "matches points_from_xy of GeoPandas" do
      for c <- @fixture["from_xy"] do
        series = GeoSeries.from_xy(c["x"], c["y"], z: c["z"])
        assert GeoSeries.to_wkt(series) == c["wkt"]
      end
    end
  end

  describe "vertex constructors with Z" do
    @constructors %{
      "line_string" => &GeoSeries.line_strings_from_vertices/2,
      "multi_point" => &GeoSeries.multi_points_from_vertices/2,
      "polygon" => &GeoSeries.polygons_from_rings/2,
      "multi_line_string" => &GeoSeries.multi_line_strings_from_lines/2,
      "multi_polygon" => &GeoSeries.multi_polygons_from_polygons/2
    }

    test "match Shapely 2.2 linestrings, multipoints, polygons and the multi constructors" do
      for c <- @fixture["vertices"] do
        series = @constructors[c["kind"]].(c["rows"], [])
        assert GeoSeries.to_wkt(series) == c["wkt"], "#{c["kind"]} #{inspect(c["rows"])}"
      end
    end

    test "tuples and lists build the same Z geometry" do
      a = GeoSeries.line_strings_from_vertices([[{0, 0, 1}, {1, 1, 2}]])
      b = GeoSeries.line_strings_from_vertices([[[0, 0, 1], [1, 1, 2]]])
      assert GeoSeries.to_wkt(a) == GeoSeries.to_wkt(b)
      assert list(GeoSeries.has_z(a)) == [true]
    end

    test "rows of different dimensions share a column" do
      series = GeoSeries.line_strings_from_vertices([[{0, 0, 1}, {1, 1, 2}], [{0, 0}, {1, 1}]])
      assert list(GeoSeries.has_z(series)) == [true, false]
    end

    test "keeps the crs" do
      series = GeoSeries.multi_points_from_vertices([[{0, 0, 1}]], crs: "EPSG:4979")
      assert GeoSeries.crs(series) =~ "4979"
    end

    test "Shapely rejects these rows and so do the constructors" do
      for c <- @fixture["vertex_errors"] do
        assert_raise ArgumentError, ~r/row 0/, fn ->
          @constructors[c["kind"]].([c["row"]], [])
        end
      end
    end
  end

  describe "get_coordinates include_z / include_m" do
    @rows [
      "POINT Z (1 2 3)",
      "LINESTRING (0 0, 1 1)",
      nil,
      "POINT ZM (1 2 3 4)",
      "POINT M (1 2 5)"
    ]

    # Shapely 2.2 returns the stored values: NaN and infinities included.
    test "NaN and infinite ordinates are returned, as Shapely does" do
      df =
        GeoSeries.get_coordinates(
          s(["POINT (NaN 1)", "LINESTRING (0 0, inf -1)", "POINT ZM (1 2 -Infinity Infinity)"]),
          include_z: true,
          include_m: true
        )

      assert list(df["x"]) == [:nan, 0.0, :infinity, 1.0]
      assert list(df["y"]) == [1.0, 0.0, -1.0, 2.0]
      assert list(df["z"]) == [:nan, :nan, :nan, :neg_infinity]
      assert list(df["m"]) == [:nan, :nan, :nan, :infinity]
    end

    test "default columns are unchanged" do
      assert DataFrame.names(GeoSeries.get_coordinates(s(@rows))) == ["x", "y"]
    end

    test "include_z adds z with NaN for 2D rows" do
      df = GeoSeries.get_coordinates(s(@rows), include_z: true)
      assert DataFrame.names(df) == ["x", "y", "z"]
      assert Series.to_list(df["z"]) == [3.0, :nan, :nan, 3.0, :nan]
    end

    test "include_m adds m and include_z with include_m adds both" do
      df = GeoSeries.get_coordinates(s(@rows), include_m: true, include_index: true)
      assert DataFrame.names(df) == ["index", "x", "y", "m"]
      assert Series.to_list(df["m"]) == [:nan, :nan, :nan, 4.0, 5.0]

      both = GeoSeries.get_coordinates(s(@rows), include_z: true, include_m: true)
      assert DataFrame.names(both) == ["x", "y", "z", "m"]
    end

    test "matches GeoPandas get_coordinates" do
      series = s(@fixture["wkt"])

      for {key, opts, names} <- [
            {"plain", [], ["x", "y"]},
            {"include_z", [include_z: true], ["x", "y", "z"]},
            {"include_m", [include_m: true], ["x", "y", "m"]},
            {"include_zm", [include_z: true, include_m: true], ["x", "y", "z", "m"]}
          ] do
        expected = @fixture[key]
        df = GeoSeries.get_coordinates(series, [include_index: true] ++ opts)
        assert DataFrame.names(df) == ["index" | names], key
        assert Series.to_list(df["index"]) == expected["index"], key

        for name <- names do
          assert column(df, name) == expected[name], "#{key} #{name}"
        end
      end
    end
  end
end
