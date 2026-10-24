defmodule GeoFrame.ExplorerVerbsTest do
  use ExUnit.Case, async: true

  alias Explorer.DataFrame
  alias Explorer.Series
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  require GeoDataFrame

  # Row i has id i, a pop of i * 10, and POINT (i i).
  defp frame(n \\ 5, opts \\ []) do
    rows = Enum.to_list(0..(n - 1))

    data =
      DataFrame.new(id: rows, pop: Enum.map(rows, &(&1 * 10)), name: Enum.map(rows, &"n#{&1}"))

    geometry = GeoSeries.from_wkt!(Enum.map(rows, &"POINT (#{&1} #{&1})"), opts)
    GeoDataFrame.new(data, geometry, Keyword.take(opts, [:geometry_column]))
  end

  defp col(gdf, name), do: gdf |> GeoDataFrame.data() |> DataFrame.pull(name) |> Series.to_list()
  defp wkt(gdf), do: gdf |> GeoDataFrame.geometry() |> GeoSeries.to_wkt()

  # Every row must still carry the point that matches its id.
  defp assert_aligned(gdf) do
    expected = Enum.map(col(gdf, "id"), &"POINT(#{&1} #{&1})")
    assert wkt(gdf) == expected
  end

  describe "filter/2, mutate/2, sort_by/2,3 macros" do
    test "filter matches filter_with and keeps geometry aligned" do
      gdf = frame()
      out = GeoDataFrame.filter(gdf, pop > 10 and id < 4)
      assert col(out, "id") == [2, 3]
      assert_aligned(out)

      same = GeoDataFrame.filter_with(gdf, &Series.greater(&1["pop"], 10))
      assert col(GeoDataFrame.filter(gdf, pop > 10), "id") == col(same, "id")
    end

    test "filter supports pinned variables" do
      limit = 20
      out = GeoDataFrame.filter(frame(), pop >= ^limit)
      assert col(out, "id") == [2, 3, 4]
      assert_aligned(out)
    end

    test "mutate adds columns and supports pins" do
      bonus = 5
      out = GeoDataFrame.mutate(frame(3), total: pop + ^bonus)
      assert col(out, "total") == [5, 15, 25]
      assert GeoDataFrame.names(out) == ["id", "pop", "name", "total", "geometry"]
      assert wkt(out) == wkt(frame(3))
    end

    test "mutate raises when producing the geometry column" do
      assert_raise ArgumentError, ~r/reserved/, fn ->
        GeoDataFrame.mutate(frame(2), geometry: id)
      end
    end

    test "sort_by orders attributes and geometry together" do
      out = GeoDataFrame.sort_by(frame(), desc: pop)
      assert col(out, "id") == [4, 3, 2, 1, 0]
      assert_aligned(out)
    end

    test "sort_by accepts options" do
      out = GeoDataFrame.sort_by(frame(), pop, nils: :last)
      assert col(out, "id") == [0, 1, 2, 3, 4]
      assert_aligned(out)
    end

    test "the query cannot see the geometry column" do
      assert_raise ArgumentError, fn -> GeoDataFrame.filter(frame(), geometry == 1) end
    end
  end

  describe "distinct/2" do
    setup do
      data = DataFrame.new(id: [1, 2, 3, 4], k: ["a", "b", "a", "b"])
      geometry = GeoSeries.from_wkt!(Enum.map(1..4, &"POINT (#{&1} 0)"))
      %{gdf: GeoDataFrame.new(data, geometry)}
    end

    test "keeps the first occurrence with its geometry", %{gdf: gdf} do
      out = GeoDataFrame.distinct(gdf, ["k"])
      assert col(out, "id") == [1, 2]
      assert col(out, "k") == ["a", "b"]
      assert wkt(out) == ["POINT(1 0)", "POINT(2 0)"]
    end

    test "defaults to all attribute columns", %{gdf: gdf} do
      assert GeoDataFrame.n_rows(GeoDataFrame.distinct(gdf)) == 4
    end

    test "accepts atoms and ignores a naming of the geometry column", %{gdf: gdf} do
      assert col(GeoDataFrame.distinct(gdf, [:k, :geometry]), "id") == [1, 2]
    end

    test "a frame without attributes is unchanged" do
      gdf =
        GeoDataFrame.new(DataFrame.new([]), GeoSeries.from_wkt!(["POINT (0 0)", "POINT (0 0)"]))

      assert GeoDataFrame.n_rows(GeoDataFrame.distinct(gdf)) == 2
    end
  end

  describe "drop_nil/2" do
    setup do
      data = DataFrame.new(id: [1, 2, 3], v: [1.0, nil, 3.0], w: [nil, "x", "y"])
      geometry = GeoSeries.from_wkt!(Enum.map(1..3, &"POINT (#{&1} 0)"))
      %{gdf: GeoDataFrame.new(data, geometry)}
    end

    test "drops rows with nulls in any column", %{gdf: gdf} do
      out = GeoDataFrame.drop_nil(gdf)
      assert col(out, "id") == [3]
      assert wkt(out) == ["POINT(3 0)"]
    end

    test "limits the check to the given columns", %{gdf: gdf} do
      out = GeoDataFrame.drop_nil(gdf, ["v"])
      assert col(out, "id") == [1, 3]
      assert wkt(out) == ["POINT(1 0)", "POINT(3 0)"]
      assert col(GeoDataFrame.drop_nil(gdf, :w), "id") == [2, 3]
    end

    test "a frame without attributes is unchanged" do
      gdf = GeoDataFrame.new(DataFrame.new([]), GeoSeries.from_wkt!(["POINT (0 0)"]))
      assert GeoDataFrame.n_rows(GeoDataFrame.drop_nil(gdf)) == 1
    end
  end

  describe "pull/2" do
    test "returns attribute columns as series" do
      assert %Series{} = s = GeoDataFrame.pull(frame(3), "pop")
      assert Series.to_list(s) == [0, 10, 20]
      assert GeoDataFrame.pull(frame(3), :id) |> Series.to_list() == [0, 1, 2]
    end

    test "returns the geometry for the geometry column name" do
      gdf = frame(2)
      assert %GeoSeries{} = geometry = GeoDataFrame.pull(gdf, "geometry")
      assert GeoSeries.to_wkt(geometry) == wkt(gdf)
      assert %GeoSeries{} = GeoDataFrame.pull(gdf, :geometry)
    end

    test "honours a custom geometry column name" do
      gdf = frame(2, geometry_column: "geom")
      assert %GeoSeries{} = GeoDataFrame.pull(gdf, "geom")
    end
  end

  describe "discard/2" do
    test "removes attribute columns and keeps the geometry" do
      out = GeoDataFrame.discard(frame(2), ["pop", "name"])
      assert GeoDataFrame.names(out) == ["id", "geometry"]
      assert wkt(out) == wkt(frame(2))
    end

    test "naming the geometry is harmless" do
      out = GeoDataFrame.discard(frame(2), ["pop", "geometry"])
      assert GeoDataFrame.names(out) == ["id", "name", "geometry"]
    end

    test "accepts a single column" do
      assert GeoDataFrame.names(GeoDataFrame.discard(frame(2), "pop")) ==
               ["id", "name", "geometry"]
    end
  end

  describe "put/3" do
    test "adds and replaces attribute columns" do
      out = GeoDataFrame.put(frame(3), "flag", [true, false, true])
      assert col(out, "flag") == [true, false, true]
      assert GeoDataFrame.names(out) == ["id", "pop", "name", "flag", "geometry"]

      replaced = GeoDataFrame.put(frame(3), "pop", Series.from_list([7, 8, 9]))
      assert col(replaced, "pop") == [7, 8, 9]
    end

    test "works on a frame without attributes" do
      gdf =
        GeoDataFrame.new(DataFrame.new([]), GeoSeries.from_wkt!(["POINT (0 0)", "POINT (1 1)"]))

      out = GeoDataFrame.put(gdf, "a", [1, 2])
      assert col(out, "a") == [1, 2]
      assert GeoDataFrame.names(out) == ["a", "geometry"]

      assert_raise ArgumentError, ~r/rows/, fn -> GeoDataFrame.put(gdf, "a", [1]) end
    end

    test "raises for the geometry column name" do
      assert_raise ArgumentError, ~r/reserved/, fn ->
        GeoDataFrame.put(frame(2), "geometry", [1, 2])
      end
    end
  end

  describe "sample/3 and shuffle/2" do
    test "sample keeps geometry aligned and is reproducible with a seed" do
      gdf = frame(20)
      a = GeoDataFrame.sample(gdf, 5, seed: 7)
      b = GeoDataFrame.sample(gdf, 5, seed: 7)
      assert GeoDataFrame.n_rows(a) == 5
      assert col(a, "id") == col(b, "id")
      assert_aligned(a)
    end

    test "sample accepts fractions and replacement" do
      out = GeoDataFrame.sample(frame(10), 1.0, replace: true, seed: 1)
      assert GeoDataFrame.n_rows(out) == 10
      assert_aligned(out)
    end

    test "sampling more rows than available without replacement raises" do
      assert_raise ArgumentError, fn -> GeoDataFrame.sample(frame(3), 5) end
    end

    test "shuffle permutes rows with their geometry" do
      gdf = frame(30)
      out = GeoDataFrame.shuffle(gdf, seed: 3)
      assert Enum.sort(col(out, "id")) == Enum.to_list(0..29)
      assert col(out, "id") != Enum.to_list(0..29)
      assert col(out, "id") == col(GeoDataFrame.shuffle(gdf, seed: 3), "id")
      assert_aligned(out)
    end

    test "a frame without attributes can be sampled" do
      geometry = GeoSeries.from_wkt!(Enum.map(0..9, &"POINT (#{&1} 0)"))
      gdf = GeoDataFrame.new(DataFrame.new([]), geometry)
      assert GeoDataFrame.n_rows(GeoDataFrame.sample(gdf, 4, seed: 1)) == 4
      assert GeoDataFrame.n_rows(GeoDataFrame.shuffle(gdf, seed: 1)) == 10
    end
  end

  describe "shape/1, n_columns/1, dtypes/1" do
    test "count the geometry as a column" do
      gdf = frame(4)
      assert GeoDataFrame.shape(gdf) == {4, 4}
      assert GeoDataFrame.n_columns(gdf) == 4
    end

    test "frames without attributes have one column" do
      gdf = GeoDataFrame.new(DataFrame.new([]), GeoSeries.from_wkt!(["POINT (0 0)"]))
      assert GeoDataFrame.shape(gdf) == {1, 1}
      assert GeoDataFrame.dtypes(gdf) == %{"geometry" => :geometry}
    end

    test "dtypes adds the geometry" do
      assert GeoDataFrame.dtypes(frame(2)) ==
               %{"id" => {:s, 64}, "pop" => {:s, 64}, "name" => :string, "geometry" => :geometry}
    end
  end
end
