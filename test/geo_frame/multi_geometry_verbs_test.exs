defmodule GeoFrame.MultiGeometryVerbsTest do
  # Secondary geometry columns through clip, overlay, sjoin, sjoin_nearest and dissolve
  # (issue #93). The expected frames come from GeoPandas 1.2, which treats every geometry
  # column other than the active one as an ordinary column: kept (with its CRS) by clip, the
  # left side of an overlay and both sides of a join, suffixed on a name clash, null for rows
  # without a partner, and aggregated by dissolve (`first`, `last` or `count`).
  # Fixtures: test/reference/fixtures/multi_verbs.json, from test/reference/generate_multi_verbs.py.
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Testing

  @fixture Path.expand("../reference/fixtures/multi_verbs.json", __DIR__)
  @cases @fixture |> File.read!() |> Jason.decode!() |> Map.fetch!("cases")

  defp build(%{"attrs" => attrs, "geoms" => [active | secondary]}) do
    series = fn %{"wkt" => wkt, "crs" => crs} -> GeoSeries.from_wkt!(wkt, crs: crs) end
    columns = Enum.map(attrs, fn [name, values] -> {name, values} end)
    data = if columns == [], do: DataFrame.new([]), else: DataFrame.new(columns)

    gdf = GeoDataFrame.new(data, series.(active), geometry_column: active["name"])

    Enum.reduce(secondary, gdf, fn g, acc ->
      GeoDataFrame.put_geometry(acc, g["name"], series.(g))
    end)
  end

  defp attrs(%GeoDataFrame{data: data}) do
    for name <- DataFrame.names(data),
        do: {name, data |> DataFrame.pull(name) |> Series.to_list()}
  end

  # Fixture option values to the atoms and tuples the verbs take.
  defp opts(args) do
    for {key, value} <- args, key != "mask", into: [] do
      {String.to_atom(key), option(key, value)}
    end
  end

  defp option(key, value) when key in ~w(how predicate), do: String.to_atom(value)
  defp option("aggfunc", value) when is_binary(value), do: String.to_atom(value)
  defp option("aggfunc", map), do: Map.new(map, fn {k, v} -> {k, String.to_atom(v)} end)
  defp option(_key, value), do: value

  defp mask([a, b, c, d]), do: {a, b, c, d}
  defp mask(wkt) when is_binary(wkt), do: wkt

  defp run("sjoin", args, [left, right]), do: GeoFrame.sjoin(left, right, opts(args))
  defp run("sjoin_nearest", args, [l, r]), do: GeoFrame.sjoin_nearest(l, r, opts(args))
  defp run("overlay", args, [a, b]), do: GeoFrame.overlay(a, b, opts(args))
  defp run("clip", args, [gdf]), do: GeoFrame.clip(gdf, mask(args["mask"]))
  defp run("dissolve", args, [gdf]), do: GeoFrame.dissolve(gdf, opts(args))

  for {%{"op" => op, "args" => args}, index} <- Enum.with_index(@cases) do
    test "#{op} #{inspect(args)} (case #{index}) matches GeoPandas" do
      case_ = Enum.at(@cases, unquote(index))
      inputs = Enum.map(case_["inputs"], &build/1)
      result = run(unquote(op), case_["args"], inputs)
      expected = case_["expected"]

      # JSON objects are unordered; `columns` has the pandas order.
      expected_attrs =
        for name <- expected["columns"],
            Map.has_key?(expected["attrs"], name),
            do: {name, expected["attrs"][name]}

      assert Enum.map(attrs(result), &elem(&1, 0)) == Enum.map(expected_attrs, &elem(&1, 0))

      for {{name, got}, {_, want}} <- Enum.zip(attrs(result), expected_attrs) do
        assert got == want, "attribute #{name}: #{inspect(got)} != #{inspect(want)}"
      end

      # Active geometry first, the other geometry columns in GeoPandas' column order.
      expected_geometries =
        for name <- expected["columns"], Map.has_key?(expected["geoms"], name), do: name

      active = expected["active"]
      assert result.geometry_column == active

      assert GeoDataFrame.geometry_columns(result) ==
               [active | List.delete(expected_geometries, active)]

      for {name, %{"wkt" => wkt, "crs" => crs}} <- expected["geoms"] do
        got = GeoDataFrame.pull(result, name)
        assert GeoSeries.crs(got) == crs, "CRS of #{name}"
        Testing.assert_geoseries_equal(got, GeoSeries.from_wkt!(wkt, crs: crs))
      end
    end
  end

  describe "edge cases" do
    defp small do
      data = DataFrame.new(id: [1, 2], g: ["a", "a"])

      GeoDataFrame.new(
        data,
        GeoSeries.from_wkt!(["POLYGON((0 0,1 0,1 1,0 0))", "POLYGON((1 0,2 0,2 1,1 0))"],
          crs: "EPSG:4326"
        )
      )
      |> GeoDataFrame.put_geometry("pt", GeoSeries.from_wkt!(["POINT(1 1)", "POINT(2 2)"]))
    end

    test "dissolve refuses a numeric aggfunc for a geometry column, like pandas" do
      assert_raise ArgumentError, ~r/cannot aggregate the geometry column "pt"/, fn ->
        GeoFrame.dissolve(small(), by: "g", aggfunc: :sum)
      end

      assert_raise ArgumentError, ~r/cannot aggregate the geometry column "pt"/, fn ->
        GeoFrame.dissolve(small(), by: "g", aggfunc: %{"pt" => :max})
      end

      # Listing only the attributes works: the geometry column is simply left out.
      out = GeoFrame.dissolve(small(), by: "g", aggfunc: %{"id" => :sum})
      assert GeoDataFrame.geometry_columns(out) == ["geometry"]
    end

    test "dissolve cannot group by a secondary geometry column" do
      assert_raise ArgumentError, ~r/geometry column "pt"/, fn ->
        GeoFrame.dissolve(small(), by: "pt")
      end
    end

    test "dissolving an empty frame keeps its secondary columns" do
      empty = GeoDataFrame.take(small(), [])

      assert empty |> GeoFrame.dissolve(by: "g") |> GeoDataFrame.geometry_columns() == [
               "geometry",
               "pt"
             ]
    end

    test "the index column of a join is named after the suffix, like GeoPandas" do
      out = GeoFrame.sjoin(small(), small(), rsuffix: "r2")
      assert "index_r2" in DataFrame.names(GeoDataFrame.data(out))
    end

    test "a clash between a secondary column and an attribute of the other side is suffixed" do
      other =
        GeoDataFrame.new(DataFrame.new(pt: [10, 20]), GeoDataFrame.geometry(small()))

      out = GeoFrame.sjoin(small(), other)
      assert GeoDataFrame.geometry_columns(out) == ["geometry", "pt_left"]
      assert "pt_right" in DataFrame.names(GeoDataFrame.data(out))
    end

    test "overlay repairs an invalid polygon without losing its secondary columns" do
      bowtie = "POLYGON((0 0,2 2,2 0,0 2,0 0))"
      square = "POLYGON((0 0,4 0,4 4,0 4,0 0))"

      df1 =
        GeoDataFrame.new(DataFrame.new(a: [1, 2]), GeoSeries.from_wkt!([bowtie, square]))
        |> GeoDataFrame.put_geometry("pt", GeoSeries.from_wkt!(["POINT(1 1)", "POINT(2 2)"]))

      df2 = GeoDataFrame.new(DataFrame.new(b: [1]), GeoSeries.from_wkt!([square]))
      out = GeoFrame.overlay(df1, df2)
      assert GeoDataFrame.geometry_columns(out) == ["geometry", "pt"]
      assert GeoSeries.size(GeoDataFrame.pull(out, "pt")) == GeoDataFrame.n_rows(out)
    end
  end
end
