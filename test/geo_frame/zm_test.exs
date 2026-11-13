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

  describe "a collection mixing member dimensions" do
    # Shapely 2.2 / GEOS 3.14 keep each member's dimension: GEOMETRYCOLLECTION Z (POINT (1 2),
    # POINT Z (3 4 5)), has_z true, get_coordinates(include_z=True) [[1, 2, nan], [3, 4, 5]].
    # A GeoArrow collection has one dimension, so the 2D member is stored with a NaN Z.
    @mixed "GEOMETRYCOLLECTION (POINT (1 2), POINT Z (3 4 5))"

    test "is read with the missing ordinates NaN, as Shapely reports them" do
      series = s([@mixed])
      assert list(GeoSeries.has_z(series)) == [true]
      assert list(GeoSeries.has_m(series)) == [false]
      coords = GeoSeries.get_coordinates(series, include_z: true)
      assert list(coords["z"]) == [:nan, 5.0]
      assert list(coords["x"]) == [1.0, 3.0]
      assert GeoSeries.to_wkt(series) == ["GEOMETRYCOLLECTION Z(POINT Z(1 2 NaN),POINT Z(3 4 5))"]

      assert GeoSeries.to_wkt(GeoSeries.force_2d(series)) == [
               "GEOMETRYCOLLECTION(POINT(1 2),POINT(3 4))"
             ]
    end

    test "is read from Shapely's WKB" do
      wkb =
        Base.decode16!(
          "01EF030000020000000101000000000000000000F03F000000000000004001E903000000000000000008" <>
            "4000000000000010400000000000001440"
        )

      assert GeoSeries.to_wkt(GeoSeries.from_wkb!([wkb])) == GeoSeries.to_wkt(s([@mixed]))
    end

    test "its WKT, with the NaN ordinates, reads back" do
      wkt = GeoSeries.to_wkt(s([@mixed]))
      assert GeoSeries.to_wkt(s(wkt)) == wkt
    end

    test "NaN and Infinity ordinates are read as GEOS reads them" do
      series = s(["POINT (1 2 NaN)", "POINT Z (1 2 nan)"])
      assert list(GeoSeries.has_z(series)) == [true, true]
      assert list(GeoSeries.get_coordinates(series, include_z: true)["z"]) == [:nan, :nan]

      assert GeoSeries.to_wkt(s(["POINT Z (1 2 Infinity)", "POINT (1 2 -inf)"])) ==
               ["POINT Z(1 2 inf)", "POINT Z(1 2 -inf)"]
    end

    test "a member of another dimension in a tagged WKT collection is an error, as in GEOS" do
      assert_raise ArgumentError, ~r/cannot mix dimensionality/, fn ->
        s(["GEOMETRYCOLLECTION Z (POINT Z (1 2 3), POINT (3 4))"])
      end
    end
  end

  describe "WKT without a Z, M or ZM tag" do
    # Shapely 2.2 / GEOS 3.14 take the dimension of an untagged geometry from its first
    # coordinate: POINT (1 2 3) is POINT Z (1 2 3), four ordinates are ZM, never M.
    test "takes Z or ZM from the first coordinate, as GEOS" do
      series =
        s([
          "POINT (1 2 3)",
          "LINESTRING (0 0 1, 1 1 2)",
          "MULTIPOINT ((1 2 3 4), (5 6 7 8))",
          "POLYGON ((0 0, 1 0, 1 1, 0 0))"
        ])

      assert list(GeoSeries.has_z(series)) == [true, true, true, false]
      assert list(GeoSeries.has_m(series)) == [false, false, true, false]
      assert list(GeoSeries.z(s(["POINT (1 2 3)"]))) == [3.0]
      assert GeoSeries.to_wkt(s(["POINT (1 2 3 4)"])) == ["POINT ZM(1 2 3 4)"]
    end

    test "a collection member infers its own dimension" do
      assert GeoSeries.to_wkt(s(["GEOMETRYCOLLECTION Z (POINT (1 2 3), POINT (4 5 6))"])) ==
               ["GEOMETRYCOLLECTION Z(POINT Z(1 2 3),POINT Z(4 5 6))"]

      assert_raise ArgumentError, ~r/cannot mix dimensionality/, fn ->
        s(["GEOMETRYCOLLECTION M (POINT (1 2 3))"])
      end
    end

    test "a coordinate count that disagrees with the first one is an error, as in GEOS" do
      for text <- ["LINESTRING (1 2 3, 4 5)", "LINESTRING (1 2, 4 5 6)", "POINT (1 2 3 4 5)"] do
        assert {:error, "invalid WKT at row 0" <> _} = GeoSeries.from_wkt([text])
      end
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

    test "simplify's repair gives a new crossing node the mean Z of its two segments (GEOS)" do
      # Shapely 2.2 / GEOS 3.14: shapely.simplify(g, 0, preserve_topology=False). The bowtie
      # node (2 2) is halfway along 0 0 1 -> 4 4 2 and 4 0 3 -> 0 4 4: (1.5 + 3.5) / 2.
      series =
        s([
          "POLYGON Z ((0 0 1, 4 4 2, 4 0 3, 0 4 4, 0 0 1))",
          "MULTIPOLYGON Z (((0 0 0, 4 0 4, 4 4 4, 0 4 0, 0 0 0)), " <>
            "((2 0 10, 6 0 20, 6 2 20, 2 2 10, 2 0 10)))",
          "POLYGON ZM ((0 0 1 10, 4 4 2 20, 4 0 3 30, 0 4 4 40, 0 0 1 10))"
        ])

      assert series |> GeoSeries.simplify(0, preserve_topology: false) |> GeoSeries.to_wkt() == [
               "POLYGON Z((2 2 2.5,4 4 2,4 0 3,2 2 2.5))",
               "POLYGON Z((0 0 0,0 4 0,4 4 4,4 2 9.5,6 2 20,6 0 20,4 0 4,2 0 10,0 0 0))",
               "POLYGON Z((2 2 2.5,4 4 2,4 0 3,2 2 2.5))"
             ]

      # POLYGON Z ((0 0 1, 4 4 NaN, 4 0 3, 0 4 4, 0 0 1)) as ISO WKB: a segment with one NaN
      # end gives the Z of its other end (1), so the node is (1 + 3.5) / 2.
      nan_end =
        Base.decode16!(
          "01EB030000010000000500000000000000000000000000000000000000000000000000F03F" <>
            "00000000000010400000000000001040000000000000F87F0000000000001040" <>
            "0000000000000000000000000000084000000000000000000000000000001040" <>
            "000000000000104000000000000000000000000000000000000000000000F03F"
        )

      assert [nan_end]
             |> GeoSeries.from_wkb!()
             |> GeoSeries.simplify(0, preserve_topology: false)
             |> GeoSeries.to_wkt() == ["POLYGON Z((2 2 2.25,4 4 NaN,4 0 3,2 2 2.25))"]
    end

    test "centroid and envelope stay 2D" do
      series = s(["POLYGON Z ((0 0 1, 4 0 2, 4 4 3, 0 0 1))"])
      assert series |> GeoSeries.envelope() |> GeoSeries.has_z() |> list() == [false]
      assert series |> GeoSeries.centroid() |> GeoSeries.has_z() |> list() == [false]
    end
  end
end
