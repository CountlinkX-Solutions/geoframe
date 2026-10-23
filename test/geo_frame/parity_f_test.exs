defmodule GeoFrame.ParityFTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  alias Explorer.Series
  alias GeoFrame.GeoSeries

  @a 6_378_137.0
  @f 1 / 298.257223563

  defp s(rows, opts \\ [crs: "EPSG:4326"]), do: GeoSeries.from_wkt!(rows, opts)
  defp list(%Series{} = series), do: Series.to_list(series)
  defp pt(lon, lat), do: "POINT (#{lon} #{lat})"

  # Vincenty's inverse formula (converges for points that are not nearly antipodal).
  defp vincenty({lon1, lat1}, {lon2, lat2}) do
    b = (1 - @f) * @a
    u1 = :math.atan((1 - @f) * :math.tan(rad(lat1)))
    u2 = :math.atan((1 - @f) * :math.tan(rad(lat2)))
    l = rad(lon2 - lon1)
    iterate(u1, u2, l, l, b, 0)
  end

  defp iterate(u1, u2, l, lambda, b, n) do
    sin_s =
      :math.sqrt(
        :math.pow(:math.cos(u2) * :math.sin(lambda), 2) +
          :math.pow(
            :math.cos(u1) * :math.sin(u2) - :math.sin(u1) * :math.cos(u2) * :math.cos(lambda),
            2
          )
      )

    cos_s = :math.sin(u1) * :math.sin(u2) + :math.cos(u1) * :math.cos(u2) * :math.cos(lambda)
    sigma = :math.atan2(sin_s, cos_s)
    sin_a = :math.cos(u1) * :math.cos(u2) * :math.sin(lambda) / sin_s
    cos2_a = 1 - sin_a * sin_a
    cos_2sm = cos_s - 2 * :math.sin(u1) * :math.sin(u2) / cos2_a
    c = @f / 16 * cos2_a * (4 + @f * (4 - 3 * cos2_a))

    next =
      l +
        (1 - c) * @f * sin_a *
          (sigma +
             c * sin_s * (cos_2sm + c * cos_s * (-1 + 2 * cos_2sm * cos_2sm)))

    if abs(next - lambda) < 1.0e-14 or n > 200 do
      u_sq = cos2_a * (@a * @a - b * b) / (b * b)
      big_a = 1 + u_sq / 16_384 * (4096 + u_sq * (-768 + u_sq * (320 - 175 * u_sq)))
      big_b = u_sq / 1024 * (256 + u_sq * (-128 + u_sq * (74 - 47 * u_sq)))

      delta =
        big_b * sin_s *
          (cos_2sm +
             big_b / 4 *
               (cos_s * (-1 + 2 * cos_2sm * cos_2sm) -
                  big_b / 6 * cos_2sm * (-3 + 4 * sin_s * sin_s) * (-3 + 4 * cos_2sm * cos_2sm)))

      b * big_a * (sigma - delta)
    else
      iterate(u1, u2, l, next, b, n + 1)
    end
  end

  defp rad(deg), do: deg * :math.pi() / 180

  describe "distance" do
    test "planar is the default and unchanged" do
      assert list(GeoSeries.distance(s([pt(0, 0)]), s([pt(3, 4)]))) == [5.0]
    end

    test "geodesic matches Karney's published value" do
      d = GeoSeries.distance(s([pt(174.81, -41.32)]), s([pt(-5.50, 40.96)]), method: :geodesic)
      assert_in_delta hd(list(d)), 19_959_679.267, 1.0e-3
    end

    test "geodesic JFK to LHR matches an independent Vincenty implementation" do
      jfk = {-73.7781, 40.6413}
      lhr = {-0.4543, 51.4700}

      d =
        GeoSeries.distance(
          s([pt(elem(jfk, 0), elem(jfk, 1))]),
          s([pt(elem(lhr, 0), elem(lhr, 1))]),
          method: :geodesic
        )

      assert_in_delta hd(list(d)), vincenty(jfk, lhr), 1.0e-3
      assert_in_delta hd(list(d)), 5_554_000, 3_000
    end

    test "haversine is a sphere distance within 0.5% of the geodesic" do
      a = s([pt(-73.7781, 40.6413)])
      b = s([pt(-0.4543, 51.47)])
      g = hd(list(GeoSeries.distance(a, b, method: :geodesic)))
      h = hd(list(GeoSeries.distance(a, b, method: :haversine)))
      assert h != g
      assert_in_delta h / g, 1.0, 0.005
    end

    test "a WKT string broadcasts and nulls stay null" do
      d = GeoSeries.distance(s([pt(0, 0), nil]), pt(0, 1), method: :geodesic)
      assert [v, nil] = list(d)
      assert_in_delta v, 110_574.388, 1.0
    end

    test "rejects an unknown method and a missing CRS" do
      assert_raise ArgumentError, ~r/method/, fn ->
        GeoSeries.distance(s([pt(0, 0)]), s([pt(1, 1)]), method: :spherical)
      end

      naive = s([pt(0, 0)], [])

      assert_raise ArgumentError, ~r/set_crs/, fn ->
        GeoSeries.distance(naive, naive, method: :geodesic)
      end
    end

    test "non-point pairs use planar nearest points (documented approximation)" do
      line = s(["LINESTRING (0 -1, 0 1)"])
      d = GeoSeries.distance(line, s([pt(1, 0)]), method: :geodesic)
      assert_in_delta hd(list(d)), 111_319.49, 1.0
    end

    property "geodesic distance is symmetric, non-negative and obeys the triangle inequality" do
      coord = {StreamData.float(min: -179.0, max: 179.0), StreamData.float(min: -80.0, max: 80.0)}

      check all({lon1, lat1} <- coord, {lon2, lat2} <- coord, {lon3, lat3} <- coord) do
        dist = fn {x1, y1}, {x2, y2} ->
          [v] = list(GeoSeries.distance(s([pt(x1, y1)]), s([pt(x2, y2)]), method: :geodesic))
          v
        end

        p1 = {lon1, lat1}
        p2 = {lon2, lat2}
        p3 = {lon3, lat3}
        assert dist.(p1, p2) >= 0.0
        assert_in_delta dist.(p1, p2), dist.(p2, p1), 1.0e-6
        assert dist.(p1, p3) <= dist.(p1, p2) + dist.(p2, p3) + 1.0e-6
      end
    end
  end

  describe "dwithin" do
    test "geodesic limits are in metres" do
      a = s([pt(0, 0), pt(0, 0)])
      b = s([pt(0, 1), pt(0, 2)])
      assert list(GeoSeries.dwithin(a, b, 120_000, method: :geodesic)) == [true, false]
      assert list(GeoSeries.dwithin(a, b, 1.5)) == [true, false]
    end
  end

  describe "area" do
    @cell "POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))"

    test "planar stays the default" do
      assert list(GeoSeries.area(s([@cell]))) == [1.0]
    end

    test "area_geodesic of a 1 degree equatorial cell matches the closed form" do
      e2 = @f * (2 - @f)
      e = :math.sqrt(e2)

      q = fn phi ->
        sn = :math.sin(rad(phi))
        sn / (1 - e2 * sn * sn) + :math.log((1 + e * sn) / (1 - e * sn)) / (2 * e)
      end

      want = 0.5 * @a * @a * (1 - e2) * rad(1) * (q.(1.0) - q.(0.0))
      assert_in_delta want, 12_308_778_361, 1.0e6
      [got] = list(GeoSeries.area_geodesic(s([@cell])))
      assert_in_delta got / want, 1.0, 1.0e-4
      assert list(GeoSeries.area(s([@cell]), method: :geodesic)) == [got]
    end

    test "projected input gives nearly the same area" do
      [lonlat] = list(GeoSeries.area_geodesic(s([@cell])))
      [merc] = list(GeoSeries.area_geodesic(GeoSeries.to_crs(s([@cell]), 3857)))
      assert_in_delta merc / lonlat, 1.0, 1.0e-6
    end

    test "points are zero, nulls stay null, a missing CRS raises" do
      assert list(GeoSeries.area_geodesic(s([pt(0, 0), nil]))) == [0.0, nil]

      assert_raise ArgumentError, ~r/set_crs/, fn ->
        GeoSeries.area_geodesic(s([@cell], []))
      end

      assert_raise ArgumentError, ~r/method/, fn -> GeoSeries.area(s([@cell]), method: :x) end
    end

    property "area_geodesic is non-negative whatever the ring orientation" do
      check all(
              lon <- StreamData.float(min: -170.0, max: 170.0),
              lat <- StreamData.float(min: -70.0, max: 70.0),
              size <- StreamData.float(min: 0.01, max: 5.0),
              cw? <- StreamData.boolean()
            ) do
        {x2, y2} = {lon + size, lat + size}
        ring = [{lon, lat}, {x2, lat}, {x2, y2}, {lon, y2}, {lon, lat}]
        ring = if cw?, do: Enum.reverse(ring), else: ring
        text = Enum.map_join(ring, ", ", fn {x, y} -> "#{x} #{y}" end)
        [a] = list(GeoSeries.area_geodesic(s(["POLYGON ((#{text}))"])))
        assert a > 0.0
      end
    end
  end

  describe "length" do
    test "planar stays the default" do
      assert list(GeoSeries.length(s(["LINESTRING (0 0, 3 4)"]))) == [5.0]
    end

    test "length_geodesic of a line equals the sum of its segment distances" do
      line = s(["LINESTRING (0 0, 10 0, 10 10)"])
      [got] = list(GeoSeries.length_geodesic(line))

      seg = fn a, b ->
        hd(list(GeoSeries.distance(s([a]), s([b]), method: :geodesic)))
      end

      assert_in_delta got, seg.(pt(0, 0), pt(10, 0)) + seg.(pt(10, 0), pt(10, 10)), 1.0e-6
      assert list(GeoSeries.length(line, method: :geodesic)) == [got]
    end

    test "polygons give their geodesic perimeter, points zero, nulls null" do
      assert [p, zero, nil] =
               list(
                 GeoSeries.length_geodesic(
                   s(["POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))", pt(0, 0), nil])
                 )
               )

      assert_in_delta p, 443_770, 100
      assert zero == 0.0
    end

    test "requires a CRS and a known method" do
      assert_raise ArgumentError, ~r/set_crs/, fn ->
        GeoSeries.length_geodesic(s(["LINESTRING (0 0, 1 1)"], []))
      end

      assert_raise ArgumentError, ~r/method/, fn ->
        GeoSeries.length(s(["LINESTRING (0 0, 1 1)"]), method: :x)
      end
    end
  end
end
