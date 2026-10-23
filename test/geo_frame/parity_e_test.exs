defmodule GeoFrame.ParityETest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.Series
  alias GeoFrame.GeoSeries

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp wkt(series), do: GeoSeries.to_wkt(series)
  defp list(%Series{} = series), do: Series.to_list(series)
  defp one(series), do: series |> list() |> hd()
  defp area(series), do: series |> GeoSeries.area() |> one()

  defp multipoint(points) do
    "MULTIPOINT (" <> Enum.map_join(points, ", ", fn {x, y} -> "(#{x} #{y})" end) <> ")"
  end

  # Point sets with at least three distinct, non-collinear points.
  defp point_set do
    gen all(
          points <-
            uniq_list_of({integer(0..30), integer(0..30)}, min_length: 4, max_length: 20),
          not collinear?(points)
        ) do
      points
    end
  end

  defp collinear?([{x0, y0}, {x1, y1} | rest]) do
    Enum.all?(rest, fn {x, y} -> (x1 - x0) * (y - y0) - (y1 - y0) * (x - x0) == 0 end)
  end

  @l_shape "POLYGON ((0 0, 6 0, 6 2, 2 2, 2 6, 0 6, 0 0))"
  @square "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))"
  @holed "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 4 2, 4 4, 2 4, 2 2))"

  describe "concave_hull/2" do
    test "ratio 0 erodes the notch of the L shape; ratio 1 is the convex hull" do
      series = s([@l_shape])
      assert series |> GeoSeries.concave_hull() |> area() == 22.0
      assert series |> GeoSeries.concave_hull(ratio: 1.0) |> area() == 28.0
    end

    test "degenerate inputs follow GEOS" do
      series = s(["POINT (1 1)", "MULTIPOINT ((0 0), (2 2))", "LINESTRING (0 0, 10 0)", nil])

      assert series |> GeoSeries.concave_hull() |> wkt() ==
               ["POINT(1 1)", "LINESTRING(0 0,2 2)", "LINESTRING(0 0,10 0)", nil]
    end

    test "allow_holes never grows the hull" do
      ring =
        for i <- 0..11, r <- [10.0, 6.0] do
          a = i * :math.pi() / 6
          {Float.round(r * :math.cos(a), 6), Float.round(r * :math.sin(a), 6)}
        end

      series = s([multipoint(ring)])
      solid = series |> GeoSeries.concave_hull(ratio: 0.3) |> area()
      holed = series |> GeoSeries.concave_hull(ratio: 0.3, allow_holes: true) |> area()
      assert holed <= solid + 1.0e-9
    end

    test "validates its options" do
      series = s([@square])
      assert_raise ArgumentError, ~r/ratio/, fn -> GeoSeries.concave_hull(series, ratio: 2) end
      assert_raise ArgumentError, ~r/ratio/, fn -> GeoSeries.concave_hull(series, ratio: "x") end
      assert_raise ArgumentError, fn -> GeoSeries.concave_hull(series, bogus: 1) end
    end

    property "the convex hull covers the concave hull and the concave hull covers the points" do
      check all(points <- point_set(), ratio <- float(min: 0.0, max: 1.0)) do
        series = s([multipoint(points)])
        concave = GeoSeries.concave_hull(series, ratio: ratio)
        convex = GeoSeries.convex_hull(series)
        assert [true] = convex |> GeoSeries.covers(concave) |> list()
        assert [true] = concave |> GeoSeries.covers(series) |> list()
      end
    end
  end

  describe "minimum_rotated_rectangle/1" do
    test "degenerate inputs" do
      series =
        s(["POINT (1 1)", "LINESTRING (0 0, 10 0)", "MULTIPOINT ((0 0), (1 1), (3 3))", nil])

      assert series |> GeoSeries.minimum_rotated_rectangle() |> wkt() ==
               ["POINT(1 1)", "LINESTRING(0 0,10 0)", "LINESTRING(0 0,3 3)", nil]
    end

    test "a tilted rectangle is its own minimum rotated rectangle" do
      series = s(["POLYGON ((0 0, 4 4, 3 5, -1 1, 0 0))"])
      rect = GeoSeries.minimum_rotated_rectangle(series)
      assert_in_delta area(rect), area(series), 1.0e-9
    end

    property "covers the geometry and is never smaller" do
      check all(points <- point_set()) do
        series = s([multipoint(points)])
        rect = GeoSeries.minimum_rotated_rectangle(series)
        hull = GeoSeries.convex_hull(series)
        assert [true] = rect |> GeoSeries.buffer(1.0e-6) |> GeoSeries.covers(hull) |> list()
        assert area(rect) >= area(hull) - 1.0e-9
      end
    end
  end

  describe "minimum_bounding_circle/1 and minimum_bounding_radius/1" do
    test "radius of known shapes" do
      series =
        s([
          "POINT (1 1)",
          "LINESTRING (0 0, 10 0)",
          @square,
          "POLYGON ((0 0, 6 0, 3 5, 0 0))",
          nil
        ])

      [point, line, square, triangle, nil_row] =
        series |> GeoSeries.minimum_bounding_radius() |> list()

      assert point == 0.0
      assert_in_delta line, 5.0, 1.0e-12
      assert_in_delta square, :math.sqrt(8), 1.0e-12
      assert_in_delta triangle, 6 * :math.sqrt(34) * :math.sqrt(34) / (4 * 15), 1.0e-9
      assert nil_row == nil
    end

    test "the circle is a 32-gon around the center and a point stays a point" do
      circle = GeoSeries.minimum_bounding_circle(s([@square, "POINT (1 1)"]))
      assert [_, "POINT(1 1)"] = wkt(circle)
      assert [33, 1] = circle |> GeoSeries.count_coordinates() |> list()
    end

    property "the radius reaches the farthest point and the circle is centered on the hull" do
      check all(points <- point_set()) do
        series = s([multipoint(points)])
        radius = series |> GeoSeries.minimum_bounding_radius() |> one()
        center = series |> GeoSeries.minimum_bounding_circle() |> GeoSeries.centroid()
        [farthest] = center |> GeoSeries.hausdorff_distance(series) |> list()
        assert_in_delta farthest, radius, 1.0e-7
      end
    end
  end

  describe "maximum_inscribed_circle/2" do
    test "returns the center-to-boundary line" do
      line = s([@holed]) |> GeoSeries.maximum_inscribed_circle()
      [wkt] = wkt(line)
      assert wkt =~ ~r/^LINESTRING\(6\.484375 6\.484375,/
    end

    test "tolerance tightens the search" do
      series = s([@l_shape])

      coarse =
        series
        |> GeoSeries.maximum_inscribed_circle(tolerance: 0.5)
        |> GeoSeries.length()
        |> one()

      fine =
        series
        |> GeoSeries.maximum_inscribed_circle(tolerance: 1.0e-6)
        |> GeoSeries.length()
        |> one()

      assert fine >= coarse - 1.0e-12
      # The corner of the L: the circle touches the notch vertex and both outer sides.
      assert_in_delta fine, 4 - 2 * :math.sqrt(2), 1.0e-5
    end

    test "non-polygons are null" do
      assert [nil, nil, nil] =
               s(["POINT (1 1)", "LINESTRING (0 0, 1 1)", nil])
               |> GeoSeries.maximum_inscribed_circle()
               |> wkt()
    end

    property "the center lies inside the polygon" do
      check all(points <- point_set()) do
        hull = s([multipoint(points)]) |> GeoSeries.convex_hull()
        line = GeoSeries.maximum_inscribed_circle(hull)
        center = GeoSeries.interpolate(line, 0.0)
        assert [true] = hull |> GeoSeries.covers(center) |> list()
      end
    end
  end

  describe "delaunay_triangles/2" do
    test "a quad of points gives two triangles, one row each" do
      tri = s([multipoint([{0, 0}, {4, 0}, {4, 4}, {0, 4}])]) |> GeoSeries.delaunay_triangles()
      assert length(wkt(tri)) == 2
      assert ["Polygon", "Polygon"] = tri |> GeoSeries.geom_type() |> list()
    end

    test "the whole series is one point cloud and the CRS is kept" do
      series = s(["POINT (0 0)", "POINT (4 0)", nil, "LINESTRING (0 4, 0 5)"], crs: "EPSG:3857")
      tri = GeoSeries.delaunay_triangles(series)
      assert length(wkt(tri)) == 2
      assert GeoSeries.crs(tri) == "EPSG:3857"
    end

    test "too few or collinear points give no rows" do
      for wkt <- ["POINT (1 1)", "MULTIPOINT ((0 0), (2 2))", "MULTIPOINT ((0 0), (1 1), (2 2))"] do
        assert [] = [wkt] |> s() |> GeoSeries.delaunay_triangles() |> wkt()
      end
    end

    test "only_edges returns one line per unique edge" do
      edges =
        s([multipoint([{0, 0}, {4, 0}, {0, 4}])])
        |> GeoSeries.delaunay_triangles(only_edges: true)

      assert ["LineString", "LineString", "LineString"] = edges |> GeoSeries.geom_type() |> list()
    end

    test "tolerance snaps close vertices" do
      series = s([multipoint([{0, 0}, {0.0001, 0}, {5, 5}, {5, 0}])])
      assert length(wkt(GeoSeries.delaunay_triangles(series))) == 2
      assert length(wkt(GeoSeries.delaunay_triangles(series, tolerance: 0.01))) == 1
    end

    test "validates its options" do
      series = s(["POINT (0 0)"])

      assert_raise ArgumentError, ~r/tolerance/, fn ->
        GeoSeries.delaunay_triangles(series, tolerance: -1)
      end

      assert_raise ArgumentError, fn -> GeoSeries.delaunay_triangles(series, bogus: 1) end
    end

    property "the triangles tile the convex hull" do
      check all(points <- point_set()) do
        series = s([multipoint(points)])
        total = series |> GeoSeries.delaunay_triangles() |> GeoSeries.area() |> Series.sum()
        assert_in_delta total, series |> GeoSeries.convex_hull() |> area(), 1.0e-7
      end
    end
  end

  describe "constrained_delaunay_triangles/1" do
    test "fills polygons, respecting concavity and holes" do
      l = s([@l_shape]) |> GeoSeries.constrained_delaunay_triangles()
      assert [4] = l |> GeoSeries.count_geometries() |> list()

      holed = s([@holed]) |> GeoSeries.constrained_delaunay_triangles()
      assert [8] = holed |> GeoSeries.count_geometries() |> list()
      total = holed |> GeoSeries.explode() |> elem(0) |> GeoSeries.area() |> Series.sum()
      assert_in_delta total, 96.0, 1.0e-9
    end

    test "non-polygons give an empty collection" do
      series = s(["POINT (1 1)", "LINESTRING (0 0, 3 4)", nil])

      assert [true, true, nil] =
               series
               |> GeoSeries.constrained_delaunay_triangles()
               |> GeoSeries.is_empty()
               |> list()
    end
  end

  describe "voronoi_polygons/2" do
    test "one cell per unique site, clipped to the envelope expanded by its larger side" do
      series = s(["MULTIPOINT ((0 0), (2 2), (5 1), (0 0))"])
      cells = GeoSeries.voronoi_polygons(series)
      assert length(wkt(cells)) == 3
      assert_in_delta cells |> GeoSeries.area() |> Series.sum(), 15 * 12, 1.0e-6
    end

    test "cells come in site order and the whole series is one cloud" do
      series = s(["POINT (0 0)", "POINT (2 0)", "POINT (5 0)"])

      assert [60.0, 25.0, 65.0] =
               series |> GeoSeries.voronoi_polygons() |> GeoSeries.area() |> list()
    end

    test "extend_to widens the clip envelope, a smaller one changes nothing" do
      series = s(["MULTIPOINT ((0 0), (2 2), (5 1))"])
      big = "POLYGON ((-20 -20, 30 -20, 30 20, -20 20, -20 -20))"
      small = "POLYGON ((1 1, 2 1, 2 2, 1 2, 1 1))"
      sum = fn cells -> cells |> GeoSeries.area() |> Series.sum() end

      assert_in_delta sum.(GeoSeries.voronoi_polygons(series, extend_to: big)), 50 * 40, 1.0e-6
      assert_in_delta sum.(GeoSeries.voronoi_polygons(series, extend_to: small)), 15 * 12, 1.0e-6
    end

    test "only_edges returns interior sides; fewer than two sites give no rows" do
      series = s(["MULTIPOINT ((0 0), (2 2), (5 1))"])
      edges = GeoSeries.voronoi_polygons(series, only_edges: true)
      assert ["LineString", "LineString", "LineString"] = edges |> GeoSeries.geom_type() |> list()
      assert [] = s(["POINT (1 1)"]) |> GeoSeries.voronoi_polygons() |> wkt()
    end

    property "the cell count equals the number of unique input points" do
      check all(points <- point_set()) do
        series = s([multipoint(points)])
        assert length(points) == series |> GeoSeries.voronoi_polygons() |> wkt() |> length()
      end
    end

    property "every site lies in its own cell" do
      check all(points <- point_set()) do
        series = s([multipoint(points)])
        cells = GeoSeries.voronoi_polygons(series)
        sites = s(Enum.map(points, fn {x, y} -> "POINT (#{x} #{y})" end))
        assert Enum.all?(cells |> GeoSeries.contains(sites) |> list())
      end
    end
  end

  describe "sample_points/3" do
    test "counts, containment and reproducibility with a seed" do
      series = s([@l_shape, @holed, "LINESTRING (0 0, 10 0)"])
      a = GeoSeries.sample_points(series, 50, seed: 7)
      b = GeoSeries.sample_points(series, 50, seed: 7)
      c = GeoSeries.sample_points(series, 50, seed: 8)

      assert wkt(a) == wkt(b)
      refute wkt(a) == wkt(c)
      assert [50, 50, 50] = a |> GeoSeries.count_geometries() |> list()
      assert ["MultiPoint", "MultiPoint", "MultiPoint"] = a |> GeoSeries.geom_type() |> list()

      {points, _} = GeoSeries.explode(a)
      assert length(wkt(points)) == 150
      source = s([@l_shape, @holed])
      {polygon_points, rows} = a |> GeoSeries.take([0, 1]) |> GeoSeries.explode()
      assert length(rows) == 100

      inside =
        polygon_points
        |> GeoSeries.within(GeoSeries.take(source, rows))
        |> list()

      assert Enum.all?(inside)
    end

    test "per-row sizes, unsupported types and nulls" do
      series = s([@square, @square, "POINT (1 1)", "POLYGON EMPTY", nil])
      out = GeoSeries.sample_points(series, [3, 0, 5, 5, 5], seed: 1)
      assert [3, 0, 0, 0, nil] = out |> GeoSeries.count_geometries() |> list()
    end

    test "validates size, method and options" do
      series = s([@square])
      assert_raise ArgumentError, ~r/size/, fn -> GeoSeries.sample_points(series, -1) end
      assert_raise ArgumentError, ~r/size/, fn -> GeoSeries.sample_points(series, 1.5) end
      assert_raise ArgumentError, ~r/length/, fn -> GeoSeries.sample_points(series, [1, 2]) end

      assert_raise ArgumentError, ~r/method/, fn ->
        GeoSeries.sample_points(series, 1, method: :cluster_poisson)
      end
    end

    property "every sample lies in the polygon, whatever the seed" do
      check all(points <- point_set(), seed <- integer(0..1_000_000), size <- integer(1..20)) do
        hull = s([multipoint(points)]) |> GeoSeries.convex_hull()

        if area(hull) > 0 do
          sampled = GeoSeries.sample_points(hull, size, seed: seed)
          assert [^size] = sampled |> GeoSeries.count_geometries() |> list()
          {pts, rows} = GeoSeries.explode(sampled)
          assert Enum.all?(GeoSeries.within(pts, GeoSeries.take(hull, rows)) |> list())
        end
      end
    end
  end

  describe "make_grid/3" do
    test "square grid layout, features and intersect filter" do
      series = s(["POLYGON ((0 0, 5 0, 5 3, 0 3, 0 0))"])

      polys = GeoSeries.make_grid(series, 2, intersect: false)
      assert length(wkt(polys)) == 6
      assert hd(wkt(polys)) == "POLYGON((0 0,2 0,2 2,0 2,0 0))"

      centers = GeoSeries.make_grid(series, 2, feature_type: :centers, intersect: false)
      assert hd(wkt(centers)) == "POINT(1 1)"

      corners = GeoSeries.make_grid(series, 2, feature_type: :corners, intersect: false)
      assert length(wkt(corners)) == 12

      triangle = s(["POLYGON ((0 0, 3 0, 0 3, 0 0))"])
      assert length(wkt(GeoSeries.make_grid(triangle, 2, intersect: false))) == 4
      assert length(wkt(GeoSeries.make_grid(triangle, 2))) == 3
    end

    test "hexagons have area 2 sqrt(3) for a cell size of 2, flat or pointy" do
      series = s(["POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))"])

      for flat <- [false, true] do
        hex = GeoSeries.make_grid(series, 2, grid_type: :hexagon, flat_topped: flat)
        assert length(wkt(hex)) > 10
        assert Enum.all?(list(GeoSeries.area(hex)), &(abs(&1 - 2 * :math.sqrt(3)) < 1.0e-9))
      end
    end

    test "keeps the CRS and validates its arguments" do
      series = s([@square], crs: "EPSG:3857")
      assert series |> GeoSeries.make_grid(2) |> GeoSeries.crs() == "EPSG:3857"
      assert_raise ArgumentError, ~r/cell_size/, fn -> GeoSeries.make_grid(series, 0) end

      assert_raise ArgumentError, ~r/feature_type/, fn ->
        GeoSeries.make_grid(series, 1, feature_type: :nope)
      end

      assert_raise ArgumentError, ~r/grid_type/, fn ->
        GeoSeries.make_grid(series, 1, grid_type: :tri)
      end

      assert_raise ArgumentError, ~r/empty/, fn -> GeoSeries.make_grid(s([nil]), 1) end
    end
  end
end
