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

  describe "payload/2 legend" do
    defp legend({:binary, info, _}), do: info.legend

    defp numeric_frame(values) do
      wkts = for i <- 0..(length(values) - 1), do: "POINT (#{i} #{i})"
      gdf(wkts, v: values)
    end

    test "a numeric column gets a gradient legend with the viridis stops and ticks" do
      legend = legend(Deck.payload(numeric_frame([0.0, 5.0, 10.0]), color: {:v, :viridis}))

      assert legend.kind == "gradient"
      assert legend.title == "v"
      assert legend.min == 0.0
      assert legend.max == 10.0
      assert hd(legend.stops) == [68, 1, 84]
      assert List.last(legend.stops) == [253, 231, 37]
      assert length(legend.stops) == 9
      assert Enum.map(legend.ticks, & &1.label) == ["0", "5", "10"]
      assert Enum.map(legend.ticks, & &1.position) == [0.0, 0.5, 1.0]
      assert legend.missing == false
    end

    test "large and small tick values are formatted compactly" do
      legend =
        legend(Deck.payload(numeric_frame([0, 1_500_000, 3_000_000]), color: {:v, :viridis}))

      assert Enum.map(legend.ticks, & &1.label) == ["0", "1.5M", "3M"]
    end

    test "tick labels keep three significant digits so they fit the bar" do
      legend =
        legend(Deck.payload(numeric_frame([31_370.0, 2_360_000.0]), color: {:v, :viridis}))

      assert Enum.map(legend.ticks, & &1.label) == ["31.4k", "1.2M", "2.36M"]

      legend = legend(Deck.payload(numeric_frame([0.0, 614_270.0]), color: {:v, :viridis}))
      assert Enum.map(legend.ticks, & &1.label) == ["0", "307k", "614k"]
      legend = legend(Deck.payload(numeric_frame([100.0, 1_220_000.0]), color: {:v, :viridis}))
      assert Enum.map(legend.ticks, & &1.label) == ["100", "610k", "1.22M"]
    end

    test "a numeric legend flags missing values" do
      legend = legend(Deck.payload(numeric_frame([1.0, nil, 3.0]), color: {:v, :viridis}))
      assert legend.missing == true
      assert legend.missing_color == [179, 179, 179]
    end

    test "a constant numeric column has a single tick" do
      legend = legend(Deck.payload(numeric_frame([4.0, 4.0]), color: {:v, :viridis}))
      assert [%{label: "4", position: 0.5}] = legend.ticks
    end

    test "a categorical column lists swatches in assignment order with the assigned colors" do
      frame = gdf(["POINT (0 0)", "POINT (1 1)", "POINT (2 2)"], kind: ["b", "a", "b"])
      {:binary, info, _} = payload = Deck.payload(frame, color: {:kind, :categorical})

      assert info.legend.kind == "categorical"
      assert info.legend.title == "kind"
      assert [%{label: "b", color: c1}, %{label: "a", color: c2}] = info.legend.items
      assert info.legend.more == 0
      assert info.legend.missing == false

      [row0, row1, _] = payload |> buffer("points.colors") |> bytes4()
      assert c1 ++ [255] == row0
      assert c2 ++ [255] == row1
    end

    test "a categorical legend with many categories is capped and counts the rest" do
      wkts = for i <- 1..14, do: "POINT (#{i} #{i})"
      frame = gdf(wkts, kind: for(i <- 1..14, do: "c#{i}"))
      legend = legend(Deck.payload(frame, color: {:kind, :categorical}))

      assert length(legend.items) == 10
      assert legend.more == 4
      assert hd(legend.items).label == "c1"
    end

    test "categorical nils are flagged, not listed" do
      frame = gdf(["POINT (0 0)", "POINT (1 1)"], kind: ["a", nil])
      legend = legend(Deck.payload(frame, color: {:kind, :categorical}))

      assert [%{label: "a"}] = legend.items
      assert legend.missing == true
    end

    test "non-string categories are shown as text" do
      frame = gdf(["POINT (0 0)", "POINT (1 1)"], kind: [3, 7])
      legend = legend(Deck.payload(frame, color: {:kind, :categorical}))
      assert Enum.map(legend.items, & &1.label) == ["3", "7"]
    end

    test "a column with nothing but nils has no legend" do
      frame = gdf(["POINT (0 0)"], v: [nil])
      assert legend(Deck.payload(frame, color: {:v, :viridis})) == nil
    end

    test "a fixed or default color has no legend" do
      assert legend(Deck.payload(points(), [])) == nil
      assert legend(Deck.payload(points(), color: {255, 0, 0})) == nil
    end

    test "legend: false turns it off, legend: true is the default" do
      frame = numeric_frame([1.0, 2.0])
      assert legend(Deck.payload(frame, color: {:v, :viridis}, legend: false)) == nil
      refute legend(Deck.payload(frame, color: {:v, :viridis}, legend: true)) == nil
    end

    test "legend must be a boolean" do
      assert_raise ArgumentError, ~r/:legend must be true or false/, fn ->
        Deck.payload(points(), legend: "yes")
      end
    end
  end

  describe "payload/2 scale bar" do
    test "scale is on by default and travels in the config" do
      {:binary, info, _} = Deck.payload(points(), [])
      assert info.config.scale == true
    end

    test "scale: false turns it off" do
      {:binary, info, _} = Deck.payload(points(), scale: false)
      assert info.config.scale == false
    end

    test "scale must be a boolean" do
      assert_raise ArgumentError, ~r/:scale must be true or false/, fn ->
        Deck.payload(points(), scale: 1)
      end
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

  describe "payload/2 popup" do
    test "is absent unless requested, so the single-frame payload is unchanged" do
      {:binary, info, _} = Deck.payload(gdf(["POINT (0 0)"], n: [1]), tooltip: [:n])
      refute Map.has_key?(info, :popup)
    end

    test "a column list sends JSON-able values aligned with rows" do
      frame = gdf(["POINT (0 0)", "POINT (1 1)"], name: ["<b>a</b>", nil], n: [1.5, :nan])
      {:binary, info, _} = Deck.payload(frame, popup: [:name, "n"])

      assert info.popup == %{columns: ["name", "n"], values: [["<b>a</b>", nil], [1.5, nil]]}
      assert {:ok, _} = Jason.encode(info)
    end

    test "true shows every attribute column except the geometry" do
      frame = gdf(["POINT (0 0)"], a: [1], b: ["x"])
      {:binary, info, _} = Deck.payload(frame, popup: true)
      assert info.popup.columns == ["a", "b"]
    end

    test "coexists with the tooltip" do
      frame = gdf(["POINT (0 0)"], a: [1], b: ["x"])
      {:binary, info, _} = Deck.payload(frame, tooltip: [:a], popup: [:b])
      assert info.tooltip.columns == ["a"]
      assert info.popup.columns == ["b"]
    end

    test "false, nil and an empty list draw no popup" do
      frame = gdf(["POINT (0 0)"], a: [1])

      for popup <- [false, nil, []] do
        {:binary, info, _} = Deck.payload(frame, popup: popup)
        refute Map.has_key?(info, :popup)
      end
    end

    test "long strings are cut" do
      frame = gdf(["POINT (0 0)"], s: [String.duplicate("x", 500)])
      {:binary, info, _} = Deck.payload(frame, popup: [:s])
      assert info.popup.values == [[String.duplicate("x", 200)]]
    end

    test "invalid popups raise" do
      frame = gdf(["POINT (0 0)"], n: [1])
      assert_raise ArgumentError, ~r/missing/, fn -> Deck.payload(frame, popup: [:missing]) end
      assert_raise ArgumentError, ~r/:popup/, fn -> Deck.payload(frame, popup: "n") end
      assert_raise ArgumentError, ~r/GeoDataFrame/, fn -> Deck.payload(points(), popup: [:a]) end
      assert_raise ArgumentError, ~r/GeoDataFrame/, fn -> Deck.payload(points(), popup: true) end
    end

    test "at most 20 columns" do
      columns = for i <- 1..21, do: {:"c#{i}", [i]}
      frame = gdf(["POINT (0 0)"], columns)
      assert_raise ArgumentError, ~r/at most 20/, fn -> Deck.payload(frame, popup: true) end
    end
  end

  describe "payload/2 several layers" do
    defp group_buffer({:binary, _info, bin} = payload, group, name) do
      %{offset: offset, bytes: bytes} = Map.fetch!(elem(payload, 1).buffers, "#{group}.#{name}")
      binary_part(bin, offset, bytes)
    end

    test "a list of frames gives one group per frame with prefixed buffers" do
      a = points(["POINT (1 2)", "POINT (3 4)"])
      b = points(["LINESTRING (10 10, 20 20)"])
      {:binary, info, _} = payload = Deck.payload([a, b], [])

      assert [%{name: "Layer 1", prefix: "0."}, %{name: "Layer 2", prefix: "1."}] =
               Enum.map(info.groups, &Map.take(&1, [:name, :prefix]))

      assert [%{layers: %{points: 2, lines: 0}}, %{layers: %{points: 0, lines: 1}}] =
               info.groups

      assert f64s(group_buffer(payload, 0, "points.positions")) == [1.0, 2.0, 3.0, 4.0]
      assert f64s(group_buffer(payload, 1, "lines.positions")) == [10.0, 10.0, 20.0, 20.0]
      assert Enum.all?(info.buffers, fn {_, %{offset: o}} -> rem(o, 8) == 0 end)
      assert info.bounds == [1.0, 2.0, 20.0, 20.0]
      assert info.config.basemap =~ "positron"
      refute Map.has_key?(info, :layers)
      assert {:ok, _} = Jason.encode(info)
    end

    test "each layer keeps its own name, color, size, opacity, legend, tooltip and popup" do
      a = gdf(["POINT (0 0)", "POINT (1 1)"], v: [1.0, 2.0], k: ["x", "y"])
      b = gdf(["POINT (5 5)"], w: [3])

      {:binary, info, _} =
        Deck.payload(
          [
            {a, name: "Cities", color: {:v, :viridis}, radius: 6, tooltip: [:k], popup: true},
            {b, color: {255, 0, 0}, opacity: 0.3, legend: false}
          ],
          []
        )

      [first, second] = info.groups
      assert first.name == "Cities"
      assert first.config.color == nil and first.config.radius == 6
      assert first.legend.title == "v"
      assert first.tooltip.columns == ["k"]
      assert first.popup.columns == ["v", "k"]
      assert second.name == "Layer 2"
      assert second.config.color == [255, 0, 0, 255] and second.config.opacity == 0.3
      assert second.legend == nil
      refute Map.has_key?(second, :popup)
    end

    test "top-level layer options are defaults for every layer" do
      {:binary, info, _} =
        Deck.payload([points(), {points(), color: {1, 2, 3}}], color: {9, 9, 9}, opacity: 0.5)

      assert [[9, 9, 9, 255], [1, 2, 3, 255]] = Enum.map(info.groups, & &1.config.color)
      assert Enum.all?(info.groups, &(&1.config.opacity == 0.5))
    end

    test "per-row colors are expanded per layer" do
      a = gdf(["POINT (0 0)", "POINT (1 1)"], k: ["x", "y"])
      payload = Deck.payload([{a, color: {:k, :categorical}}, points()], [])
      assert length(bytes4(group_buffer(payload, 0, "points.colors"))) == 2
      refute Map.has_key?(elem(payload, 1).buffers, "1.points.colors")
    end

    test ":max_features and :max_vertices are totals over all layers" do
      assert_raise ArgumentError, ~r/6 features/, fn ->
        Deck.payload([points(), points()], max_features: 5)
      end

      assert_raise ArgumentError, ~r/6 vertices/, fn ->
        Deck.payload([points(), points()], max_vertices: 5)
      end

      assert {:binary, _, _} = Deck.payload([points(), points()], max_features: 6)
    end

    test "layer options that belong to the whole map raise" do
      assert_raise ArgumentError, ~r/whole map/, fn ->
        Deck.payload([{points(), basemap: nil}], [])
      end
    end

    test "invalid lists raise" do
      assert_raise ArgumentError, ~r/at least one/, fn -> Deck.payload([], []) end
      assert_raise ArgumentError, ~r/layer 2/, fn -> Deck.payload([points(), :nope], []) end
      assert_raise ArgumentError, ~r/:name/, fn -> Deck.payload([{points(), name: 1}], []) end

      assert_raise ArgumentError, ~r/unknown option/, fn ->
        Deck.payload([{points(), x: 1}], [])
      end
    end

    test "a layer with nothing to draw raises naming it" do
      empty = GeoSeries.from_wkt!([nil, "POINT EMPTY"])

      assert_raise ArgumentError, ~r/Layer 2.*nothing to draw/, fn ->
        Deck.payload([points(), empty], [])
      end
    end

    test "a single frame keeps the original payload shape" do
      {:binary, info, _} = Deck.payload(points(), [])
      assert Map.has_key?(info, :layers) and not Map.has_key?(info, :groups)
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

    test "accepts a list of layers" do
      kino = GeoFrame.Kino.deck([points(), {points(), name: "B"}], height: 300)
      assert {:binary, %{groups: [_, %{name: "B"}]}, _} = connect(kino, nil, 5_000)
    end
  end
end
