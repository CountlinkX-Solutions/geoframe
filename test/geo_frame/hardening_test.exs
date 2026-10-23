defmodule GeoFrame.HardeningTest do
  use ExUnit.Case, async: true

  alias GeoFrame.GeoSeries

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)

  test "deeply nested WKT is an error and the VM survives" do
    deep = String.duplicate("GEOMETRYCOLLECTION (", 100_000)
    assert {:error, msg} = GeoSeries.from_wkt([deep])
    assert msg =~ "nesting too deep"
    assert GeoSeries.size(s(["POINT (1 2)"])) == 1
  end

  test "deeply nested WKB is an error and the VM survives" do
    bytes = String.duplicate(<<1, 7, 0, 0, 0, 1, 0, 0, 0>>, 20_000)
    assert {:error, msg} = GeoSeries.from_wkb([bytes])
    assert msg =~ "invalid WKB at row 0"
  end

  test "sample_points rejects totals above the cap" do
    series = s(["POLYGON ((0 0, 1 0, 1 1, 0 1, 0 0))"])

    assert_raise ArgumentError, ~r/exceeds the maximum/, fn ->
      GeoSeries.sample_points(series, 10_000_001)
    end

    assert_raise ArgumentError, ~r/exceeds the maximum/, fn ->
      GeoSeries.sample_points(series, [6_000_000, 6_000_000])
    end
  end

  test "union_groups validates group counts and ids" do
    series = s(["POINT (0 0)", "POINT (1 1)"])

    assert_raise ArgumentError, ~r/cannot exceed/, fn ->
      GeoSeries.union_groups(series, [0, 1], 1_000_000_000_000)
    end

    assert_raise ArgumentError, ~r/out of range/, fn ->
      GeoSeries.union_groups(series, [0, 5], 2)
    end
  end

  test "segmentize and make_grid reject outputs above the caps" do
    line = s(["LINESTRING (0 0, 20000000 0)"])

    assert_raise ArgumentError, ~r/segmentize would create/, fn ->
      GeoSeries.segmentize(line, 1)
    end

    extent = s(["POLYGON ((0 0, 3300 0, 3300 3300, 0 3300, 0 0))"])

    assert_raise ArgumentError, ~r/grid too large/, fn ->
      GeoSeries.make_grid(extent, 1, feature_type: :centers, intersect: false)
    end
  end

  test "inspect previews only the first rows" do
    series = s(List.duplicate("POINT (1 2)", 50))
    text = inspect(series)
    assert text =~ "size: 50"
    assert text =~ "..."
    assert inspect(s([])) =~ "size: 0"
  end
end
