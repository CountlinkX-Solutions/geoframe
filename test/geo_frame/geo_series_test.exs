defmodule GeoFrame.GeoSeriesTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoSeries

  @square "POLYGON ((0 0, 4 0, 4 3, 0 3, 0 0))"

  describe "from_wkt/2" do
    test "builds a series keeping nulls and order" do
      assert {:ok, s} = GeoSeries.from_wkt(["POINT (1 2)", nil, "LINESTRING (0 0, 3 4)"])
      assert GeoSeries.size(s) == 3
      assert GeoSeries.to_wkt(s) == ["POINT(1 2)", nil, "LINESTRING(0 0,3 4)"]
    end

    test "reports the failing row" do
      assert {:error, message} = GeoSeries.from_wkt(["POINT (1 2)", "nope"])
      assert message =~ "row 1"
    end

    test "from_wkt!/2 raises on invalid input" do
      assert_raise ArgumentError, ~r/row 0/, fn -> GeoSeries.from_wkt!(["nope"]) end
    end

    test "stores the crs option" do
      s = GeoSeries.from_wkt!(["POINT (1 2)"], crs: "EPSG:4326")
      assert GeoSeries.crs(s) == "EPSG:4326"
      assert GeoSeries.crs(GeoSeries.from_wkt!(["POINT (1 2)"])) == nil
    end
  end

  describe "from_wkb/2 and to_wkb/1" do
    test "round-trips through WKB" do
      s = GeoSeries.from_wkt!([@square, nil])
      [wkb, nil] = GeoSeries.to_wkb(s)
      assert is_binary(wkb)

      assert {:ok, back} = GeoSeries.from_wkb([wkb, nil])
      assert GeoSeries.to_wkt(back) == GeoSeries.to_wkt(s)
    end

    test "reports the failing row" do
      assert {:error, message} = GeoSeries.from_wkb([nil, <<1, 2, 3>>])
      assert message =~ "row 1"
      assert_raise ArgumentError, fn -> GeoSeries.from_wkb!([<<1, 2, 3>>]) end
    end
  end

  describe "crs" do
    test "set_crs/2 replaces metadata only" do
      s = GeoSeries.from_wkt!(["POINT (1 2)"], crs: "EPSG:4326")
      moved = GeoSeries.set_crs(s, "EPSG:3857")
      assert GeoSeries.crs(moved) == "EPSG:3857"
      assert GeoSeries.to_wkt(moved) == GeoSeries.to_wkt(s)
      assert GeoSeries.crs(s) == "EPSG:4326"
      assert GeoSeries.crs(GeoSeries.set_crs(s, nil)) == nil
    end
  end

  describe "measures" do
    test "area/1 is planar and unsigned, with nulls" do
      s = GeoSeries.from_wkt!([@square, "POINT (1 1)", nil])
      assert Series.to_list(GeoSeries.area(s)) == [12.0, 0.0, nil]
    end

    test "length/1 is length for lines, perimeter for polygons, zero for points" do
      s = GeoSeries.from_wkt!(["LINESTRING (0 0, 3 4)", @square, "POINT (1 1)", nil])
      assert Series.to_list(GeoSeries.length(s)) == [5.0, 14.0, 0.0, nil]
    end

    test "is_valid/1 returns a boolean series" do
      s = GeoSeries.from_wkt!([@square, "POLYGON ((0 0, 4 4, 4 0, 0 4, 0 0))", nil])
      series = GeoSeries.is_valid(s)
      assert Series.dtype(series) == :boolean
      assert Series.to_list(series) == [true, false, nil]
    end

    test "bounds/1 returns a dataframe with nulls for null and empty rows" do
      s = GeoSeries.from_wkt!(["LINESTRING (1 5, 3 -2)", nil, "POLYGON EMPTY"])
      df = GeoSeries.bounds(s)
      assert DataFrame.names(df) == ["minx", "miny", "maxx", "maxy"]

      assert DataFrame.to_columns(df, atom_keys: true) == %{
               minx: [1.0, nil, nil],
               miny: [-2.0, nil, nil],
               maxx: [3.0, nil, nil],
               maxy: [5.0, nil, nil]
             }
    end

    test "total_bounds/1 is a tuple or nil" do
      s = GeoSeries.from_wkt!(["POINT (1 5)", "POINT (3 -2)", nil])
      assert GeoSeries.total_bounds(s) == {1.0, -2.0, 3.0, 5.0}
      assert GeoSeries.total_bounds(GeoSeries.from_wkt!([nil])) == nil
    end

    test "centroid/1 returns points and keeps the crs" do
      s = GeoSeries.from_wkt!(["POLYGON ((0 0, 4 0, 4 2, 0 2, 0 0))", nil], crs: "EPSG:4326")
      c = GeoSeries.centroid(s)
      assert GeoSeries.to_wkt(c) == ["POINT(2 1)", nil]
      assert GeoSeries.crs(c) == "EPSG:4326"
    end
  end

  describe "take/2" do
    test "selects, repeats and reorders rows" do
      s = GeoSeries.from_wkt!(["POINT (0 0)", nil, "POINT (2 2)"], crs: "EPSG:4326")
      t = GeoSeries.take(s, [2, 1, 0, 2])
      assert GeoSeries.to_wkt(t) == ["POINT(2 2)", nil, "POINT(0 0)", "POINT(2 2)"]
      assert GeoSeries.crs(t) == "EPSG:4326"
    end

    test "raises on out-of-bounds indices" do
      s = GeoSeries.from_wkt!(["POINT (0 0)"])
      assert_raise ArgumentError, ~r/out of bounds/, fn -> GeoSeries.take(s, [1]) end
    end
  end

  describe "Inspect" do
    test "shows size, crs and the first rows as WKT" do
      s = GeoSeries.from_wkt!(["POINT (1 2)", nil], crs: "EPSG:4326")
      text = inspect(s)
      assert text =~ "GeoFrame.GeoSeries"
      assert text =~ "size: 2"
      assert text =~ "EPSG:4326"
      assert text =~ "POINT(1 2)"
      assert text =~ "nil"
    end

    test "truncates long series" do
      s = GeoSeries.from_wkt!(List.duplicate("POINT (0 0)", 20))
      assert inspect(s) =~ "..."
    end
  end

  describe "properties" do
    property "WKT round-trips for random points" do
      check all(
              coords <-
                list_of({integer(-1000..1000), integer(-1000..1000)},
                  min_length: 1,
                  max_length: 20
                )
            ) do
        wkts = Enum.map(coords, fn {x, y} -> "POINT(#{x} #{y})" end)
        assert wkts |> GeoSeries.from_wkt!() |> GeoSeries.to_wkt() == wkts
      end
    end

    property "area of axis-aligned rectangles is width * height and never negative" do
      check all(
              rects <-
                list_of({integer(-50..50), integer(-50..50), integer(1..40), integer(1..40)},
                  min_length: 1,
                  max_length: 10
                )
            ) do
        wkts =
          Enum.map(rects, fn {x, y, w, h} ->
            "POLYGON((#{x} #{y},#{x + w} #{y},#{x + w} #{y + h},#{x} #{y + h},#{x} #{y}))"
          end)

        areas = wkts |> GeoSeries.from_wkt!() |> GeoSeries.area() |> Series.to_list()
        assert areas == Enum.map(rects, fn {_, _, w, h} -> w * h * 1.0 end)
        assert Enum.all?(areas, &(&1 >= 0))
      end
    end
  end
end
