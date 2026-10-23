defmodule GeoFrame.UnaryOpsTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.Series
  alias GeoFrame.GeoSeries

  @square "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"
  @l_shape "POLYGON ((0 0, 4 0, 4 1, 1 1, 1 4, 0 4, 0 0))"
  @bowtie "POLYGON ((0 0, 2 2, 2 0, 0 2, 0 0))"

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp list(%Series{} = series), do: Series.to_list(series)

  defp equal?(%GeoSeries{} = actual, expected) when is_list(expected) do
    list(GeoSeries.geom_equals(actual, s(expected)))
  end

  describe "buffer" do
    test "point buffer approximates a circle and keeps the CRS" do
      out = "POINT (1 1)" |> List.wrap() |> s(crs: "EPSG:3857") |> GeoSeries.buffer(2)
      [area] = out |> GeoSeries.area() |> list()
      assert_in_delta area, :math.pi() * 4, 0.1
      assert GeoSeries.crs(out) == "EPSG:3857"
      assert list(GeoSeries.geom_type(out)) == ["Polygon"]
    end

    test "negative distance shrinks polygons; nulls stay null" do
      out = GeoSeries.buffer(s(["POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))", nil]), -1)
      assert [area, nil] = out |> GeoSeries.area() |> list()
      assert_in_delta area, 64.0, 1.0e-9
    end

    test "cap_style and join_style" do
      line = s(["LINESTRING (0 0, 4 0)"])

      flat = GeoSeries.buffer(line, 1, cap_style: :flat, join_style: :mitre)
      assert equal?(flat, ["POLYGON ((0 -1, 4 -1, 4 1, 0 1, 0 -1))"]) == [true]

      square = GeoSeries.buffer(line, 1, cap_style: :square)
      assert GeoSeries.total_bounds(square) == {-1.0, -1.0, 5.0, 1.0}
    end

    test "more quad_segs gets closer to the true circle" do
      point = s(["POINT (0 0)"])
      [coarse] = point |> GeoSeries.buffer(1, quad_segs: 4) |> GeoSeries.area() |> list()
      [fine] = point |> GeoSeries.buffer(1, quad_segs: 32) |> GeoSeries.area() |> list()
      assert abs(fine - :math.pi()) < abs(coarse - :math.pi())
    end

    test "invalid options raise ArgumentError" do
      p = s(["POINT (0 0)"])
      assert_raise ArgumentError, fn -> GeoSeries.buffer(p, 1, cap_style: :pointy) end
      assert_raise ArgumentError, fn -> GeoSeries.buffer(p, 1, join_style: :pointy) end
      assert_raise ArgumentError, fn -> GeoSeries.buffer(p, 1, quad_segs: 0) end
      assert_raise ArgumentError, fn -> GeoSeries.buffer(p, 1, mitre_limit: 0.5) end
      assert_raise FunctionClauseError, fn -> GeoSeries.buffer(p, "1") end
      assert_raise ArgumentError, fn -> GeoSeries.buffer(p, 1, bogus: 1) end
    end
  end

  describe "simplify" do
    test "removes near-collinear vertices in both modes" do
      line = s(["LINESTRING (0 0, 1 0.001, 2 0, 3 0.001, 4 0)"])
      rdp = GeoSeries.simplify(line, 0.1, preserve_topology: false)
      assert list(GeoSeries.count_coordinates(rdp)) == [2]
      assert list(GeoSeries.count_coordinates(GeoSeries.simplify(line, 0.1))) == [2]
    end

    test "points and nulls pass through" do
      out = GeoSeries.simplify(s(["POINT (1 2)", nil]), 1)
      assert equal?(out, ["POINT (1 2)", nil]) == [true, nil]
    end
  end

  describe "constructive unary ops" do
    test "convex_hull" do
      out = GeoSeries.convex_hull(s([@l_shape, "POINT (1 1)"]))

      assert equal?(out, ["POLYGON ((0 0, 4 0, 4 1, 1 4, 0 4, 0 0))", "POINT (1 1)"]) ==
               [true, true]
    end

    test "envelope" do
      out = GeoSeries.envelope(s(["POLYGON ((1 1, 5 2, 3 6, 1 1))", "POINT (2 3)"]))

      assert equal?(out, ["POLYGON ((1 1, 5 1, 5 6, 1 6, 1 1))", "POINT (2 3)"]) == [true, true]
    end

    test "boundary" do
      out = GeoSeries.boundary(s([@square, "LINESTRING (0 0, 1 1, 2 0)", "POINT (0 0)"]))

      assert equal?(out, [
               "LINESTRING (0 0, 4 0, 4 4, 0 4, 0 0)",
               "MULTIPOINT (0 0, 2 0)",
               "GEOMETRYCOLLECTION EMPTY"
             ]) == [true, true, true]
    end

    test "make_valid repairs a bowtie" do
      series = s([@bowtie])
      assert list(GeoSeries.is_valid(series)) == [false]
      fixed = GeoSeries.make_valid(series)
      assert list(GeoSeries.is_valid(fixed)) == [true]
      assert list(GeoSeries.geom_type(fixed)) == ["MultiPolygon"]
      assert [2.0] = fixed |> GeoSeries.area() |> list() |> Enum.map(&Float.round(&1, 9))
    end

    test "representative_point is inside a concave polygon" do
      series = s([@l_shape, nil])
      point = GeoSeries.representative_point(series)
      assert list(GeoSeries.within(point, series)) == [true, nil]
    end

    test "exterior is polygon only" do
      out = GeoSeries.exterior(s([@square, "POINT (1 1)"]))
      assert equal?(out, ["LINESTRING (0 0, 4 0, 4 4, 0 4, 0 0)", nil]) == [true, nil]
    end
  end

  describe "affine" do
    test "translate is exact and keeps the CRS" do
      out =
        GeoSeries.translate(s(["POLYGON ((0 0, 1 0, 1 1, 0 0))", nil], crs: "EPSG:4326"), 10, -2)

      assert GeoSeries.crs(out) == "EPSG:4326"
      assert equal?(out, ["POLYGON ((10 -2, 11 -2, 11 -1, 10 -2))", nil]) == [true, nil]
    end

    test "scale origin defaults to the bounding box centre" do
      tri = s(["POLYGON ((0 0, 6 0, 0 3, 0 0))"])
      assert GeoSeries.total_bounds(GeoSeries.scale(tri, 2, 2)) == {-3.0, -1.5, 9.0, 4.5}

      assert GeoSeries.total_bounds(GeoSeries.scale(tri, 2, 2, origin: :centroid)) ==
               {-2.0, -1.0, 10.0, 5.0}

      scaled = GeoSeries.scale(tri, 2, 3, origin: {0, 0})
      assert equal?(scaled, ["POLYGON ((0 0, 12 0, 0 9, 0 0))"]) == [true]
    end

    test "rotate is counter-clockwise degrees" do
      sq = s(["POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))"])
      {a, b, c, d} = sq |> GeoSeries.rotate(90, origin: {0, 0}) |> GeoSeries.total_bounds()
      assert_in_delta a, -2.0, 1.0e-9
      assert_in_delta b, 0.0, 1.0e-9
      assert_in_delta c, 0.0, 1.0e-9
      assert_in_delta d, 2.0, 1.0e-9

      {a, b, c, d} = sq |> GeoSeries.rotate(90) |> GeoSeries.total_bounds()
      assert_in_delta a, 0.0, 1.0e-9
      assert_in_delta b, 0.0, 1.0e-9
      assert_in_delta c, 2.0, 1.0e-9
      assert_in_delta d, 2.0, 1.0e-9
    end

    test "invalid arguments raise ArgumentError" do
      sq = s([@square])
      assert_raise ArgumentError, fn -> GeoSeries.translate(sq, "1", 0) end
      assert_raise ArgumentError, fn -> GeoSeries.scale(sq, 1, 1, origin: :nowhere) end
      assert_raise ArgumentError, fn -> GeoSeries.rotate(sq, 10, origin: {1, :a}) end
    end
  end

  describe "accessors" do
    test "x and y are point-only" do
      series = s(["POINT (1.5 -2)", "LINESTRING (0 0, 1 1)", "POINT EMPTY", nil])
      assert list(GeoSeries.x(series)) == [1.5, nil, nil, nil]
      assert list(GeoSeries.y(series)) == [-2.0, nil, nil, nil]
    end

    test "geom_type uses Shapely names" do
      series =
        s([
          "POINT (0 0)",
          "LINESTRING (0 0, 1 1)",
          "POLYGON ((0 0, 1 0, 1 1, 0 0))",
          "MULTIPOINT (0 0, 1 1)",
          "MULTILINESTRING ((0 0, 1 1))",
          "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)))",
          "GEOMETRYCOLLECTION (POINT (0 0), LINESTRING (0 0, 1 1))",
          nil
        ])

      assert list(GeoSeries.geom_type(series)) ==
               ~w(Point LineString Polygon MultiPoint MultiLineString MultiPolygon
                  GeometryCollection) ++ [nil]
    end

    test "is_empty and count_coordinates" do
      series = s(["POINT EMPTY", "POLYGON EMPTY", "POINT (0 0)", "MULTIPOINT (0 0, 1 1)", nil])
      assert list(GeoSeries.is_empty(series)) == [true, true, false, false, nil]
      assert list(GeoSeries.count_coordinates(series)) == [0, 0, 1, 2, nil]
      assert Series.dtype(GeoSeries.count_coordinates(series)) == {:s, 64}
    end
  end

  describe "properties" do
    defp coord, do: StreamData.integer(-20..20)

    defp box do
      StreamData.map(
        {coord(), coord(), StreamData.integer(1..10), StreamData.integer(1..10)},
        fn {x, y, w, h} ->
          "POLYGON ((#{x} #{y}, #{x + w} #{y}, #{x + w} #{y + h}, #{x} #{y + h}, #{x} #{y}))"
        end
      )
    end

    defp triangle do
      StreamData.map({coord(), coord(), coord(), coord(), coord(), coord()}, fn
        {a, b, c, d, e, f} -> "POLYGON ((#{a} #{b}, #{c} #{d}, #{e} #{f}, #{a} #{b}))"
      end)
    end

    defp line do
      StreamData.map({coord(), coord(), coord(), coord()}, fn {a, b, c, d} ->
        if {a, b} == {c, d}, do: "POINT (#{a} #{b})", else: "LINESTRING (#{a} #{b}, #{c} #{d})"
      end)
    end

    defp geometry, do: StreamData.one_of([box(), line()])

    defp rows(generator), do: StreamData.list_of(generator, min_length: 1, max_length: 8)

    property "representative_point lies within the polygon" do
      check all(wkts <- rows(box())) do
        series = s(wkts)
        assert Enum.all?(list(GeoSeries.within(GeoSeries.representative_point(series), series)))
      end
    end

    property "convex hull covers the geometry and its area is not smaller" do
      check all(wkts <- rows(geometry())) do
        series = s(wkts)
        hull = GeoSeries.convex_hull(series)
        assert Enum.all?(list(GeoSeries.covers(hull, series)))
        pairs = Enum.zip(list(GeoSeries.area(hull)), list(GeoSeries.area(series)))
        assert Enum.all?(pairs, fn {h, a} -> h >= a - 1.0e-9 end)
      end
    end

    property "envelope covers the geometry" do
      check all(wkts <- rows(geometry())) do
        series = s(wkts)
        assert Enum.all?(list(GeoSeries.covers(GeoSeries.envelope(series), series)))
      end
    end

    property "translate then inverse translate is the identity" do
      check all(wkts <- rows(geometry()), dx <- coord(), dy <- coord()) do
        series = s(wkts)
        back = series |> GeoSeries.translate(dx, dy) |> GeoSeries.translate(-dx, -dy)
        assert Enum.all?(list(GeoSeries.geom_equals(series, back)))
      end
    end

    property "make_valid yields valid geometry" do
      check all(wkts <- rows(triangle())) do
        fixed = GeoSeries.make_valid(s(wkts))
        assert Enum.all?(list(GeoSeries.is_valid(fixed)), &(&1 in [true, nil]))
      end
    end

    property "a positive buffer covers the polygon, a negative one shrinks it" do
      check all(wkts <- rows(box()), d <- StreamData.integer(1..3)) do
        series = s(wkts)
        assert Enum.all?(list(GeoSeries.covers(GeoSeries.buffer(series, d), series)))

        # Offset curves carry float noise, so compare areas instead of exact containment.
        inner = GeoSeries.buffer(series, -0.25)
        pairs = Enum.zip(list(GeoSeries.area(inner)), list(GeoSeries.area(series)))
        assert Enum.all?(pairs, fn {i, a} -> i < a end)
      end
    end

    property "scaling by 1 and rotating by 0 and 360 keep area" do
      check all(wkts <- rows(box())) do
        series = s(wkts)
        areas = list(GeoSeries.area(series))

        for out <- [GeoSeries.scale(series, 1, 1), GeoSeries.rotate(series, 360)] do
          pairs = Enum.zip(list(GeoSeries.area(out)), areas)
          assert Enum.all?(pairs, fn {a, b} -> abs(a - b) < 1.0e-6 end)
        end
      end
    end
  end
end
