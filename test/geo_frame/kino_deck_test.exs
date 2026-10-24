defmodule GeoFrame.Kino.DeckTest do
  use ExUnit.Case, async: true

  import Kino.Test

  alias Explorer.DataFrame
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Kino.Deck

  setup :configure_livebook_bridge

  defp points(wkts \\ ["POINT (1 2)", "POINT (3 4)", "POINT (5 6)"], crs \\ "EPSG:4326"),
    do: GeoSeries.from_wkt!(wkts, crs: crs)

  defp gdf(wkts, columns) do
    GeoDataFrame.new(DataFrame.new(columns), points(wkts))
  end

  # The named buffer of a payload as a binary.
  defp buffer({:binary, info, bin}, name) do
    %{offset: offset, bytes: bytes} = Map.fetch!(info.buffers, name)
    binary_part(bin, offset, bytes)
  end

  defp f64s(bin), do: for(<<v::float-64-little <- bin>>, do: v)
  defp f32s(bin), do: for(<<v::float-32-little <- bin>>, do: v)
  defp i32s(bin), do: for(<<v::signed-32-little <- bin>>, do: v)
  defp bytes4(bin), do: for(<<r, g, b, a <- bin>>, do: [r, g, b, a])

  describe "payload/2 buffers" do
    test "points: binary positions and row indices, 8-byte aligned" do
      {:binary, info, bin} = payload = Deck.payload(points(), [])

      assert info.layers == %{points: 3, lines: 0, polygons: 0}
      assert f64s(buffer(payload, "points.positions")) == [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
      assert i32s(buffer(payload, "points.rows")) == [0, 1, 2]
      assert Enum.all?(info.buffers, fn {_, %{offset: o}} -> rem(o, 8) == 0 end)

      assert byte_size(bin) >=
               Enum.max(for {_, %{offset: o, bytes: b}} <- info.buffers, do: o + b)

      refute Map.has_key?(info.buffers, "lines.positions")
    end

    test "null, empty and multi rows keep their row index; non-finite parts are counted" do
      series = points(["POINT (1 2)", nil, "MULTIPOINT ((3 4), (5 6))", "POINT EMPTY"])
      payload = Deck.payload(series, [])

      assert i32s(buffer(payload, "points.rows")) == [0, 2, 2]
      assert elem(payload, 1).skipped == 0
    end

    test "lines and polygons carry offsets and triangles" do
      series =
        points([
          "LINESTRING (0 0, 1 1, 2 0)",
          "POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0), (1 1, 2 1, 2 2, 1 2, 1 1))"
        ])

      {:binary, info, _} = payload = Deck.payload(series, [])

      assert info.layers == %{points: 0, lines: 1, polygons: 1}
      assert i32s(buffer(payload, "lines.starts")) == [0, 3]
      assert i32s(buffer(payload, "polygons.starts")) == [0, 10]
      assert i32s(buffer(payload, "polygons.ring_starts")) == [0, 5, 10]
      assert byte_size(buffer(payload, "polygons.triangles")) > 0
      assert rem(byte_size(buffer(payload, "polygons.triangles")), 12) == 0
    end

    test "reports total bounds" do
      {:binary, info, _} = Deck.payload(points(), [])
      assert info.bounds == [1.0, 2.0, 5.0, 6.0]
    end
  end

  describe "payload/2 reprojection" do
    test "reprojects to EPSG:4326" do
      series = points(["POINT (111319.49079327357 0)"], "EPSG:3857")
      [x, y] = series |> Deck.payload([]) |> buffer("points.positions") |> f64s()

      assert_in_delta x, 1.0, 1.0e-6
      assert_in_delta y, 0.0, 1.0e-6
    end

    test "uses coordinates as stored when the CRS is unknown" do
      series = GeoSeries.from_wkt!(["POINT (10 20)"])
      assert [10.0, 20.0] == series |> Deck.payload([]) |> buffer("points.positions") |> f64s()
    end
  end

  describe "payload/2 styling" do
    test "a fixed color and size travel in the config, not as buffers" do
      {:binary, info, _} =
        Deck.payload(points(), color: {255, 0, 0}, radius: 7, width: 3, opacity: 0.5)

      assert info.config.color == [255, 0, 0, 255]
      assert info.config.radius == 7
      assert info.config.width == 3
      assert info.config.opacity == 0.5
      refute Map.has_key?(info.buffers, "points.colors")
      refute Map.has_key?(info.buffers, "points.sizes")
    end

    test "a numeric column on the viridis ramp becomes per-feature RGBA bytes" do
      frame =
        gdf(["POINT (0 0)", "POINT (1 1)", "POINT (2 2)", "POINT (3 3)"],
          v: [0.0, 5.0, 10.0, nil]
        )

      payload = Deck.payload(frame, color: {:v, :viridis})
      [low, mid, high, missing] = payload |> buffer("points.colors") |> bytes4()

      assert low == [68, 1, 84, 255]
      assert high == [253, 231, 37, 255]
      assert mid != low and mid != high
      assert missing == [179, 179, 179, 255]
      assert elem(payload, 1).config.color == nil
    end

    test "a categorical column gives one color per distinct value" do
      frame = gdf(["POINT (0 0)", "POINT (1 1)", "POINT (2 2)"], kind: ["a", "b", "a"])

      [c1, c2, c3] =
        frame |> Deck.payload(color: {:kind, :categorical}) |> buffer("points.colors") |> bytes4()

      assert c1 == c3
      assert c1 != c2
    end

    test "a radius column is scaled to a pixel range by area" do
      frame = gdf(["POINT (0 0)", "POINT (1 1)", "POINT (2 2)"], n: [1, 2, 3])
      sizes = frame |> Deck.payload(radius: :n) |> buffer("points.sizes") |> f32s()

      # Circle area, not radius, follows the value: the middle value sits at sqrt(0.5).
      assert [2.0, middle, 20.0] = sizes
      assert_in_delta middle, 2.0 + 18.0 * :math.sqrt(0.5), 1.0e-4
    end

    test "a skewed radius column keeps small values distinguishable from the minimum" do
      frame = gdf(["POINT (0 0)", "POINT (1 1)", "POINT (2 2)"], n: [0, 1_000_000, 100_000_000])
      [low, mid, high] = frame |> Deck.payload(radius: :n) |> buffer("points.sizes") |> f32s()

      assert low == 2.0
      assert high == 20.0
      assert mid > 3.5
    end

    test "a width column is scaled linearly to a pixel range" do
      frame =
        gdf(["LINESTRING (0 0, 1 1)", "LINESTRING (1 1, 2 2)", "LINESTRING (2 2, 3 3)"],
          n: [1, 2, 3]
        )

      sizes = frame |> Deck.payload(width: :n) |> buffer("lines.sizes") |> f32s()

      # One value per vertex: every line has two.
      assert sizes == [1.0, 1.0, 4.5, 4.5, 8.0, 8.0]
    end

    test "polygon and line colors are gathered per vertex through the row index" do
      frame =
        gdf(
          ["POLYGON ((0 0, 1 0, 1 1, 0 0))", "POLYGON ((5 5, 6 5, 6 6, 5 5))"],
          v: [0.0, 1.0]
        )

      colors =
        frame |> Deck.payload(color: {:v, :viridis}) |> buffer("polygons.colors") |> bytes4()

      low = [68, 1, 84, 255]
      high = [253, 231, 37, 255]
      assert colors == List.duplicate(low, 4) ++ List.duplicate(high, 4)
    end
  end

  describe "payload/2 tooltip" do
    test "sends JSON-able column values aligned with rows" do
      frame = gdf(["POINT (0 0)", "POINT (1 1)"], name: ["a", nil], n: [1.5, :nan])
      {:binary, info, _} = Deck.payload(frame, tooltip: [:name, "n"])

      assert info.tooltip == %{columns: ["name", "n"], values: [["a", nil], [1.5, nil]]}
      assert {:ok, _} = Jason.encode(info)
    end

    test "unknown tooltip columns raise" do
      frame = gdf(["POINT (0 0)"], n: [1])
      assert_raise ArgumentError, ~r/missing/, fn -> Deck.payload(frame, tooltip: [:missing]) end
    end

    test "a series has nothing to show" do
      assert_raise ArgumentError, ~r/GeoDataFrame/, fn ->
        Deck.payload(points(), tooltip: [:a])
      end
    end
  end

  describe "payload/2 limits and validation" do
    test "raises above :max_features with a clear message" do
      assert_raise ArgumentError, ~r/max_features/, fn ->
        Deck.payload(points(), max_features: 2)
      end
    end

    test "raises on a bad color" do
      assert_raise ArgumentError, ~r/color/, fn -> Deck.payload(points(), color: "red") end
    end

    test "raises on an unknown option" do
      assert_raise ArgumentError, ~r/unknown option/, fn ->
        Deck.payload(points(), colour: {1, 2, 3})
      end
    end

    test "raises when there is nothing to draw" do
      assert_raise ArgumentError, ~r/nothing to draw/i, fn ->
        Deck.payload(GeoSeries.from_wkt!([nil, "POINT EMPTY"]), [])
      end
    end
  end

  describe "geometry collections" do
    test "members are drawn as separate parts that keep the source row" do
      series =
        points([
          "POINT (9 9)",
          "GEOMETRYCOLLECTION (POINT (1 1), LINESTRING (0 0, 1 1), GEOMETRYCOLLECTION (POLYGON ((0 0, 2 0, 2 2, 0 0))))"
        ])

      payload = Deck.payload(series, [])

      assert elem(payload, 1).layers == %{points: 2, lines: 1, polygons: 1}
      assert i32s(buffer(payload, "points.rows")) == [0, 1]
      assert i32s(buffer(payload, "lines.rows")) == [1]
      assert i32s(buffer(payload, "polygons.rows")) == [1]
    end
  end

  describe "vertex limit" do
    test "an input above :max_vertices raises with advice" do
      series = points(["LINESTRING (0 0, 1 1, 2 2)", "POINT (1 1)"])

      assert_raise ArgumentError, ~r/4 vertices is above the deck.gl limit of 3/, fn ->
        Deck.payload(series, max_vertices: 3)
      end

      assert {:binary, _, _} = Deck.payload(series, max_vertices: 4)
    end

    test "null and empty rows add no vertices" do
      series = points([nil, "POINT EMPTY", "POINT (1 1)"])
      assert {:binary, _, _} = Deck.payload(series, max_vertices: 1)
    end

    test ":max_vertices must be a positive integer" do
      assert_raise ArgumentError, ~r/max_vertices/, fn ->
        Deck.payload(points(), max_vertices: 0)
      end
    end

    test "the default scales to millions of vertices" do
      assert {:binary, _, _} = Deck.payload(points(), [])
    end
  end

  describe "GeoFrame.Kino.deck/2" do
    test "returns a Kino.JS.Live that sends the binary payload on connect" do
      kino = GeoFrame.Kino.deck(points(), color: {0, 128, 255})

      assert %Kino.JS.Live{} = kino
      assert {:binary, info, bin} = connect(kino, nil, 5_000)
      assert info.layers.points == 3
      assert byte_size(bin) > 0
    end

    test "accepts a GeoDataFrame" do
      frame = gdf(["POINT (0 0)"], n: [1])
      assert %Kino.JS.Live{} = GeoFrame.Kino.deck(frame, tooltip: [:n])
    end
  end
end
