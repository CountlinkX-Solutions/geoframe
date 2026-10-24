defmodule GeoFrame.Kino.SpatialJoinCellTest do
  use ExUnit.Case, async: true

  import Kino.Test

  alias Explorer.DataFrame
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries
  alias GeoFrame.Kino.SpatialJoinCell

  setup :configure_livebook_bridge

  # Generous so a loaded machine never turns a slow reply into a failure; assert_receive
  # returns as soon as the message arrives, so the happy path pays nothing.
  @timeout 5_000

  # The generated source on unwrapped lines, so long paths do not hide the structure.
  defp source(module, attrs), do: attrs |> module.to_source() |> unwrap()

  defp unwrap(""), do: ""

  defp unwrap(code) do
    code
    |> Code.string_to_quoted!()
    |> Code.quoted_to_algebra()
    |> Inspect.Algebra.format(100_000)
    |> IO.iodata_to_binary()
  end

  defp run(source, binding) do
    {value, binding} = Code.eval_string(source, binding)
    {value, binding}
  end

  defp gdf_points(wkts) do
    GeoDataFrame.new(
      DataFrame.new(id: Enum.to_list(1..length(wkts))),
      GeoSeries.from_wkt!(wkts, crs: "EPSG:4326")
    )
  end

  defp join_attrs(overrides \\ %{}) do
    Map.merge(
      %{
        "left" => "points",
        "right" => "zones",
        "result" => "joined",
        "mode" => "predicate",
        "predicate" => "intersects",
        "how" => "inner",
        "distance" => "",
        "max_distance" => "",
        "distance_col" => "",
        "show_map" => false
      },
      overrides
    )
  end

  describe "SpatialJoinCell.to_source/1" do
    test "a predicate join" do
      assert source(SpatialJoinCell, join_attrs()) ==
               "joined = GeoFrame.sjoin(points, zones, how: :inner, predicate: :intersects)"
    end

    test "predicate and how are applied" do
      attrs = join_attrs(%{"predicate" => "within", "how" => "left"})

      assert source(SpatialJoinCell, attrs) ==
               "joined = GeoFrame.sjoin(points, zones, how: :left, predicate: :within)"
    end

    test "dwithin takes a distance" do
      attrs = join_attrs(%{"predicate" => "dwithin", "distance" => "250"})

      assert source(SpatialJoinCell, attrs) ==
               "joined = GeoFrame.sjoin(points, zones, how: :inner, predicate: :dwithin, distance: 250.0)"
    end

    test "unknown predicate or how fall back to the defaults" do
      attrs = join_attrs(%{"predicate" => "bogus", "how" => "outer"})
      assert source(SpatialJoinCell, attrs) =~ "how: :inner, predicate: :intersects"
    end

    test "nearest mode with max_distance and distance_col" do
      attrs =
        join_attrs(%{
          "mode" => "nearest",
          "how" => "left",
          "max_distance" => "1.5",
          "distance_col" => "dist"
        })

      assert source(SpatialJoinCell, attrs) ==
               ~s|joined = GeoFrame.sjoin_nearest(points, zones, how: :left, max_distance: 1.5, distance_col: "dist")|
    end

    test "nearest mode without optional fields" do
      assert source(SpatialJoinCell, join_attrs(%{"mode" => "nearest"})) ==
               "joined = GeoFrame.sjoin_nearest(points, zones, how: :inner)"
    end

    test "show_map appends the deck widget for the result" do
      src = source(SpatialJoinCell, join_attrs(%{"show_map" => true, "result" => "out"}))
      assert String.ends_with?(src, "\nGeoFrame.Kino.deck(out)")
    end

    test "missing or invalid variables generate nothing" do
      assert source(SpatialJoinCell, join_attrs(%{"left" => ""})) == ""
      assert source(SpatialJoinCell, join_attrs(%{"right" => nil})) == ""
      assert source(SpatialJoinCell, join_attrs(%{"result" => "Bad Name"})) == ""
    end

    test "generated source evaluates for every mode, how and predicate" do
      points = gdf_points(["POINT (1 1)", "POINT (5 5)", "POINT (20 20)"])

      zones =
        GeoDataFrame.new(
          DataFrame.new(zone: ["a", "b"]),
          GeoSeries.from_wkt!(
            ["POLYGON ((0 0, 0 2, 2 2, 2 0, 0 0))", "POLYGON ((4 4, 4 6, 6 6, 6 4, 4 4))"],
            crs: "EPSG:4326"
          )
        )

      binding = [points: points, zones: zones]

      for how <- ~w(inner left right), predicate <- GeoSeries.sindex_valid_query_predicates() do
        attrs =
          join_attrs(%{"how" => how, "predicate" => to_string(predicate), "distance" => "3"})

        {joined, _} = run(source(SpatialJoinCell, attrs), binding)
        assert is_struct(joined, GeoDataFrame), "#{how} #{predicate}"
      end

      {inner, _} = run(source(SpatialJoinCell, join_attrs()), binding)
      assert GeoDataFrame.n_rows(inner) == 2

      for how <- ~w(inner left right) do
        attrs = join_attrs(%{"mode" => "nearest", "how" => how, "distance_col" => "d"})
        {joined, _} = run(source(SpatialJoinCell, attrs), binding)
        assert "d" in GeoDataFrame.names(joined)
      end

      {limited, _} =
        run(
          source(SpatialJoinCell, join_attrs(%{"mode" => "nearest", "max_distance" => "1.5"})),
          binding
        )

      assert GeoDataFrame.n_rows(limited) == 0 or is_struct(limited, GeoDataFrame)

      {mapped, _} = run(source(SpatialJoinCell, join_attrs(%{"show_map" => true})), binding)
      assert %Kino.JS.Live{} = mapped
    end
  end

  describe "SpatialJoinCell as a smart cell" do
    test "scan_binding reports only GeoDataFrame variables, sorted" do
      {kino, _} = start_smart_cell!(SpatialJoinCell, %{})
      connect(kino, nil, @timeout)

      binding = [
        zones: gdf_points(["POINT (1 1)"]),
        series: GeoSeries.from_wkt!(["POINT (1 1)"]),
        table: DataFrame.new(a: [1]),
        n: 3,
        points: gdf_points(["POINT (2 2)"])
      ]

      SpatialJoinCell.scan_binding(kino.pid, binding, __ENV__)

      assert_broadcast_event(kino, "variables", %{variables: ["points", "zones"]}, @timeout)
    end

    test "connect returns the known variables" do
      {kino, _} = start_smart_cell!(SpatialJoinCell, join_attrs())
      assert connect(kino, nil, @timeout).variables == []

      SpatialJoinCell.scan_binding(kino.pid, [points: gdf_points(["POINT (1 1)"])], __ENV__)
      assert_broadcast_event(kino, "variables", %{variables: ["points"]}, @timeout)
      assert connect(kino, nil, @timeout).variables == ["points"]
    end

    test "empty left and right default to the first variables found" do
      {kino, _} = start_smart_cell!(SpatialJoinCell, join_attrs(%{"left" => "", "right" => ""}))

      binding = [b: gdf_points(["POINT (1 1)"]), a: gdf_points(["POINT (2 2)"])]
      SpatialJoinCell.scan_binding(kino.pid, binding, __ENV__)

      assert_smart_cell_update(kino, %{"left" => "a", "right" => "b"}, source, @timeout)
      assert source =~ "GeoFrame.sjoin(a, b"
    end

    test "chosen variables are kept when they are still present" do
      {kino, _} = start_smart_cell!(SpatialJoinCell, join_attrs())
      binding = [points: gdf_points(["POINT (1 1)"]), zones: gdf_points(["POINT (2 2)"])]
      SpatialJoinCell.scan_binding(kino.pid, binding, __ENV__)
      assert_broadcast_event(kino, "variables", %{variables: ["points", "zones"]}, @timeout)
      assert connect(kino, nil, @timeout).fields["left"] == "points"
    end

    test "field updates regenerate the source" do
      {kino, source} = start_smart_cell!(SpatialJoinCell, join_attrs())
      assert source =~ "GeoFrame.sjoin(points, zones"

      push_event(kino, "update_field", %{"field" => "mode", "value" => "nearest"})
      assert_smart_cell_update(kino, %{"mode" => "nearest"}, source, @timeout)
      assert source =~ "GeoFrame.sjoin_nearest(points, zones"

      push_event(kino, "update_field", %{"field" => "show_map", "value" => true})
      assert_smart_cell_update(kino, %{"show_map" => true}, source, @timeout)
      assert source =~ "GeoFrame.Kino.deck(joined)"
    end

    test "default attrs allocate a result variable" do
      {kino, _} = start_smart_cell!(SpatialJoinCell, %{})
      assert connect(kino, nil, @timeout).fields["result"] =~ ~r/^joined\d*$/
    end
  end

  test "the application registers the cell" do
    definitions = Kino.SmartCell.definitions()
    assert Enum.find(definitions, &(&1.module == SpatialJoinCell)).name == "Spatial join"
  end
end
