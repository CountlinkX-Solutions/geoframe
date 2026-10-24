defmodule GeoFrame.TestingTest do
  use ExUnit.Case, async: true

  import GeoFrame.Testing

  alias Explorer.DataFrame
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @square "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"
  @reversed "POLYGON ((0 0, 0 4, 4 4, 4 0, 0 0))"

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)

  describe "assert_geoseries_equal/3" do
    test "passes for equal series, including matching nulls" do
      assert assert_geoseries_equal(s(["POINT (1 1)", nil]), s(["POINT (1 1)", nil])) == :ok
    end

    test "default is topological, so a reversed ring passes; normalize does too" do
      assert_geoseries_equal(s([@square]), s([@reversed]))
      assert_geoseries_equal(s([@square]), s([@reversed]), normalize: true)
    end

    test "check_less_precise compares exactly with a 1e-6 tolerance" do
      a = s(["POINT (1 1)"])
      b = s(["POINT (1.0000001 1)"])
      c = s(["POINT (1.001 1)"])
      assert_geoseries_equal(a, b, check_less_precise: true)

      assert_raise ExUnit.AssertionError, fn ->
        assert_geoseries_equal(a, c, check_less_precise: true)
      end

      assert_raise ExUnit.AssertionError, fn ->
        assert_geoseries_equal(s([@square]), s([@reversed]), check_less_precise: true)
      end
    end

    test "reports the first differing rows" do
      left = s(["POINT (1 1)", "POINT (2 2)", "POINT (3 3)"])
      right = s(["POINT (1 1)", "POINT (9 9)", nil])

      error = assert_raise ExUnit.AssertionError, fn -> assert_geoseries_equal(left, right) end
      assert error.message =~ "row 1"
      assert error.message =~ "POINT(2 2)"
      assert error.message =~ "POINT(9 9)"
      assert error.message =~ "row 2"
    end

    test "length mismatch" do
      error =
        assert_raise ExUnit.AssertionError, fn ->
          assert_geoseries_equal(s(["POINT (1 1)"]), s([]))
        end

      assert error.message =~ "length"
    end

    test "check_crs defaults to true" do
      a = s(["POINT (1 1)"], crs: "EPSG:4326")
      b = s(["POINT (1 1)"], crs: "EPSG:3857")
      error = assert_raise ExUnit.AssertionError, fn -> assert_geoseries_equal(a, b) end
      assert error.message =~ "CRS"
      assert_geoseries_equal(a, b, check_crs: false)
    end

    test "check_geom_type" do
      a = s(["MULTIPOINT ((1 1))"])
      b = s(["POINT (1 1)"])
      assert_geoseries_equal(a, b)

      error =
        assert_raise ExUnit.AssertionError, fn ->
          assert_geoseries_equal(a, b, check_geom_type: true)
        end

      assert error.message =~ "geometry type"
    end

    test "rejects unknown options" do
      assert_raise ArgumentError, fn -> assert_geoseries_equal(s([]), s([]), bogus: 1) end
    end
  end

  describe "assert_geodataframe_equal/3" do
    defp gdf(ids, wkt, opts \\ []), do: GeoDataFrame.new(DataFrame.new(id: ids), s(wkt, opts))

    test "passes for equal frames" do
      assert assert_geodataframe_equal(gdf([1], ["POINT (1 1)"]), gdf([1], ["POINT (1 1)"])) ==
               :ok
    end

    test "attribute differences are reported" do
      error =
        assert_raise ExUnit.AssertionError, fn ->
          assert_geodataframe_equal(
            gdf([1, 2], ["POINT (1 1)", "POINT (2 2)"]),
            gdf([1, 3], ["POINT (1 1)", "POINT (2 2)"])
          )
        end

      assert error.message =~ "id"
      assert error.message =~ "row 1"
    end

    test "column names and geometry column name" do
      a = gdf([1], ["POINT (1 1)"])
      b = GeoDataFrame.new(DataFrame.new(other: [1]), s(["POINT (1 1)"]))
      error = assert_raise ExUnit.AssertionError, fn -> assert_geodataframe_equal(a, b) end
      assert error.message =~ "columns"

      c = GeoDataFrame.new(DataFrame.new(id: [1]), s(["POINT (1 1)"]), geometry_column: "geom")
      assert_raise ExUnit.AssertionError, fn -> assert_geodataframe_equal(a, c) end
    end

    test "check_dtype" do
      ints = gdf([1], ["POINT (1 1)"])
      floats = GeoDataFrame.new(DataFrame.new(id: [1.0]), s(["POINT (1 1)"]))

      assert_raise ExUnit.AssertionError, ~r/dtype/, fn ->
        assert_geodataframe_equal(ints, floats)
      end

      assert_geodataframe_equal(ints, floats, check_dtype: false)
    end

    test "geometry and CRS differences are reported" do
      assert_raise ExUnit.AssertionError, fn ->
        assert_geodataframe_equal(gdf([1], ["POINT (1 1)"]), gdf([1], ["POINT (2 2)"]))
      end

      assert_raise ExUnit.AssertionError, ~r/CRS/, fn ->
        assert_geodataframe_equal(
          gdf([1], ["POINT (1 1)"], crs: "EPSG:4326"),
          gdf([1], ["POINT (1 1)"])
        )
      end
    end

    test "check_less_precise also tolerates small attribute float noise" do
      a = GeoDataFrame.new(DataFrame.new(v: [1.0]), s(["POINT (1 1)"]))
      b = GeoDataFrame.new(DataFrame.new(v: [1.0000001]), s(["POINT (1 1)"]))
      assert_raise ExUnit.AssertionError, fn -> assert_geodataframe_equal(a, b) end
      assert_geodataframe_equal(a, b, check_less_precise: true)
    end

    # Secondary geometry columns (issue #93): GeoPandas compares every geometry column.
    defp with_extra(wkt, opts \\ []),
      do:
        GeoDataFrame.put_geometry(
          gdf([1, 2], ["POINT (1 1)", "POINT (2 2)"]),
          "extra",
          s(wkt, opts)
        )

    test "secondary geometry columns are compared" do
      same = ["POINT (10 10)", "POINT (20 20)"]
      assert assert_geodataframe_equal(with_extra(same), with_extra(same)) == :ok

      error =
        assert_raise ExUnit.AssertionError, fn ->
          assert_geodataframe_equal(
            with_extra(same),
            with_extra(["POINT (10 10)", "POINT (21 21)"])
          )
        end

      assert error.message =~ ~s(geometry column "extra")
      assert error.message =~ "row 1"
    end

    test "secondary geometry column CRS, names and order are compared" do
      same = ["POINT (10 10)", "POINT (20 20)"]

      assert_raise ExUnit.AssertionError, ~r/geometry column "extra".*CRS/s, fn ->
        assert_geodataframe_equal(with_extra(same, crs: "EPSG:3857"), with_extra(same))
      end

      assert_geodataframe_equal(with_extra(same, crs: "EPSG:3857"), with_extra(same),
        check_crs: false
      )

      renamed = GeoDataFrame.rename(with_extra(same), %{"extra" => "other"})

      assert_raise ExUnit.AssertionError, ~r/columns/, fn ->
        assert_geodataframe_equal(with_extra(same), renamed)
      end

      two = GeoDataFrame.put_geometry(with_extra(same), "second", s(same))
      swapped = GeoDataFrame.rename(two, %{"extra" => "second", "second" => "extra"})
      assert_raise ExUnit.AssertionError, fn -> assert_geodataframe_equal(two, swapped) end
    end

    test "a missing secondary column is a column difference" do
      same = ["POINT (10 10)", "POINT (20 20)"]

      assert_raise ExUnit.AssertionError, ~r/columns/, fn ->
        assert_geodataframe_equal(with_extra(same), gdf([1, 2], ["POINT (1 1)", "POINT (2 2)"]))
      end
    end

    test "check_less_precise applies to secondary columns" do
      a = with_extra(["POINT (10 10)", "POINT (20 20)"])
      b = with_extra(["POINT (10.0000001 10)", "POINT (20 20)"])
      assert_raise ExUnit.AssertionError, fn -> assert_geodataframe_equal(a, b) end
      assert_geodataframe_equal(a, b, check_less_precise: true)
    end
  end
end
