defmodule GeoFrame.ParityKTest do
  use ExUnit.Case, async: true

  alias GeoFrame.GeoSeries

  defp s(rows, opts \\ []), do: GeoSeries.from_wkt!(rows, opts)
  defp canon(wkt), do: [wkt] |> s() |> GeoSeries.to_wkt() |> hd()

  @a "POLYGON ((0 0, 2 0, 2 2, 0 2, 0 0))"
  @b "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))"

  describe "is_valid_coverage/2" do
    test "accepts edge-matched polygons and rejects overlaps and mismatches" do
      assert GeoSeries.is_valid_coverage(s([@a, @b]))
      refute GeoSeries.is_valid_coverage(s([@a, "POLYGON ((1 0, 3 0, 3 2, 1 2, 1 0))"]))

      refute GeoSeries.is_valid_coverage(s([@a, "POLYGON ((2 0, 4 0, 4 2, 2 2, 2 1, 2 0))"]))
    end

    test "gap_width detects narrow gaps" do
      series = s([@a, "POLYGON ((2.1 0, 4 0, 4 2, 2.1 2, 2.1 0))"])
      assert GeoSeries.is_valid_coverage(series)
      assert GeoSeries.is_valid_coverage(series, gap_width: 0.05)
      refute GeoSeries.is_valid_coverage(series, gap_width: 0.2)
    end

    test "ignores nulls and non-polygons; an empty series is valid" do
      assert GeoSeries.is_valid_coverage(s([@a, nil, "POINT (1 1)", @b]))
      assert GeoSeries.is_valid_coverage(s([]))
    end

    test "rejects bad options" do
      assert_raise ArgumentError, ~r/gap_width/, fn ->
        GeoSeries.is_valid_coverage(s([@a]), gap_width: "x")
      end

      assert_raise ArgumentError, fn -> GeoSeries.is_valid_coverage(s([@a]), bogus: 1) end
    end
  end

  describe "invalid_coverage_edges/2" do
    test "returns the invalid edges per row" do
      series = s([@a, "POLYGON ((1 0, 3 0, 3 2, 1 2, 1 0))"])

      assert series |> GeoSeries.invalid_coverage_edges() |> GeoSeries.to_wkt() ==
               [
                 canon("LINESTRING (0 0, 2 0, 2 2, 0 2)"),
                 canon("LINESTRING (3 2, 1 2, 1 0, 3 0)")
               ]
    end

    test "valid rows give LINESTRING EMPTY, nulls stay null, CRS is kept" do
      series = s([@a, nil, "POINT (1 1)", @b], crs: "EPSG:3857")
      edges = GeoSeries.invalid_coverage_edges(series)
      empty = canon("LINESTRING EMPTY")

      assert GeoSeries.to_wkt(edges) == [empty, nil, empty, empty]
      assert GeoSeries.crs(edges) == "EPSG:3857"
    end

    test "separate invalid stretches make a multi-line" do
      series =
        s([
          "POLYGON ((0 0, 1 0, 2 0, 3 0, 4 0, 5 0, 5 5, 0 5, 0 0))",
          "POLYGON ((0.2 -1, 0.8 -1, 0.8 1, 0.2 1, 0.2 -1))",
          "POLYGON ((3.2 -1, 3.8 -1, 3.8 1, 3.2 1, 3.2 -1))"
        ])

      assert series |> GeoSeries.invalid_coverage_edges() |> GeoSeries.to_wkt() ==
               [
                 canon("MULTILINESTRING ((3 0, 4 0), (0 0, 1 0))"),
                 canon("LINESTRING (0.8 -1, 0.8 1, 0.2 1, 0.2 -1)"),
                 canon("LINESTRING (3.8 -1, 3.8 1, 3.2 1, 3.2 -1)")
               ]
    end

    test "gap_width reports both sides of a narrow gap" do
      series = s([@a, "POLYGON ((2.1 0, 4 0, 4 2, 2.1 2, 2.1 0))"])

      assert series |> GeoSeries.invalid_coverage_edges(gap_width: 0.2) |> GeoSeries.to_wkt() ==
               [canon("LINESTRING (2 0, 2 2)"), canon("LINESTRING (2.1 2, 2.1 0)")]
    end
  end

  describe "simplify_coverage/3" do
    @doc_rows [
      "POLYGON ((0 0, 1 1.1, 2 0, 1.5 1, 2 2, 0 2, 0 0))",
      "POLYGON ((2 0, 4 0, 4 2, 2 2, 1.5 1, 2 0))"
    ]

    test "simplifies the shared edge once for both polygons and the boundary too" do
      result = @doc_rows |> s() |> GeoSeries.simplify_coverage(1)

      assert GeoSeries.to_wkt(result) ==
               [
                 canon("POLYGON ((2 0, 2 2, 0 2, 2 0))"),
                 canon("POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))")
               ]

      assert GeoSeries.is_valid_coverage(result)
    end

    test "simplify_boundary: false leaves the outer boundary alone" do
      result = @doc_rows |> s() |> GeoSeries.simplify_coverage(1, simplify_boundary: false)

      assert GeoSeries.to_wkt(result) ==
               [
                 canon("POLYGON ((2 0, 2 2, 0 2, 0 0, 1 1.1, 2 0))"),
                 canon("POLYGON ((2 0, 4 0, 4 2, 2 2, 2 0))")
               ]
    end

    test "keeps CRS, nulls and non-polygons as nulls, and empty polygons" do
      series =
        s(Enum.take(@doc_rows, 1) ++ [nil, "POINT (1 1)", "POLYGON EMPTY"], crs: "EPSG:3857")

      result = GeoSeries.simplify_coverage(series, 1.0)

      assert GeoSeries.crs(result) == "EPSG:3857"
      assert [_, nil, nil, empty] = GeoSeries.to_wkt(result)
      assert empty == canon("POLYGON EMPTY")
    end

    test "rejects a bad tolerance and unknown options" do
      assert_raise ArgumentError, ~r/tolerance/, fn ->
        GeoSeries.simplify_coverage(s(@doc_rows), "x")
      end

      assert_raise ArgumentError, fn ->
        GeoSeries.simplify_coverage(s(@doc_rows), 1, bogus: true)
      end
    end
  end
end
