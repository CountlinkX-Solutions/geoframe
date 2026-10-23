defmodule GeoFrame.Kino.ReadFileCellTest do
  use ExUnit.Case, async: true

  import Kino.Test

  alias Explorer.DataFrame
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.Kino.ReadFileCell

  setup :configure_livebook_bridge

  @io Path.expand("../reference/fixtures/io", __DIR__)
  @gpkg Path.join(@io, "vector/multi.gpkg")
  @shp Path.join(@io, "vector/points.shp")
  @parquet Path.expand("../fixtures/neighborhoods.parquet", __DIR__)

  defp read_attrs(overrides \\ %{}) do
    Map.merge(
      %{
        "variable" => "gdf",
        "path" => @shp,
        "driver" => "auto",
        "layer" => "",
        "bbox" => "",
        "to_crs" => "",
        "show_map" => false
      },
      overrides
    )
  end

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

  defp run(source, binding \\ []) do
    {value, binding} = Code.eval_string(source, binding)
    {value, binding}
  end

  describe "ReadFileCell.to_source/1" do
    test "reads the file with the format chosen from the extension" do
      assert source(ReadFileCell, read_attrs()) ==
               ~s|gdf = GeoFrame.read_file!("#{@shp}")|
    end

    test "an explicit driver is passed through" do
      assert source(ReadFileCell, read_attrs(%{"path" => "/data/x.dat", "driver" => "geojson"})) ==
               ~s|gdf = GeoFrame.read_file!("/data/x.dat", driver: :geojson)|
    end

    test "layer only applies to a GeoPackage" do
      gpkg = read_attrs(%{"path" => @gpkg, "layer" => "polys"})
      assert source(ReadFileCell, gpkg) =~ ~s|layer: "polys"|

      shp = read_attrs(%{"layer" => "polys"})
      refute source(ReadFileCell, shp) =~ "layer"

      forced = read_attrs(%{"path" => "/d/x.bin", "driver" => "gpkg", "layer" => "a"})
      assert source(ReadFileCell, forced) =~ ~s|driver: :gpkg, layer: "a"|
    end

    test "bbox is parsed from four comma separated numbers" do
      attrs = read_attrs(%{"path" => "/d/x.parquet", "bbox" => "-80.5, 25, -80, 26.25"})

      assert source(ReadFileCell, attrs) ==
               ~s|gdf = GeoFrame.read_file!("/d/x.parquet", bbox: {-80.5, 25.0, -80.0, 26.25})|
    end

    test "an unparsable bbox is left out" do
      for bbox <- ["1,2,3", "a,b,c,d", "1,2,3,4,5", "  "] do
        refute source(ReadFileCell, read_attrs(%{"bbox" => bbox})) =~ "bbox"
      end
    end

    test "to_crs adds a reprojection; digits become an EPSG integer" do
      assert source(ReadFileCell, read_attrs(%{"to_crs" => "3857"})) ==
               ~s|gdf = GeoFrame.read_file!("#{@shp}")\n| <>
                 "gdf = GeoFrame.GeoDataFrame.to_crs(gdf, 3857)"

      assert source(ReadFileCell, read_attrs(%{"to_crs" => " EPSG:4269 "})) =~
               ~s|to_crs(gdf, "EPSG:4269")|
    end

    test "show_map appends the deck widget as the last expression" do
      src = source(ReadFileCell, read_attrs(%{"variable" => "places", "show_map" => true}))

      assert src ==
               ~s|places = GeoFrame.read_file!("#{@shp}")\nGeoFrame.Kino.deck(places)|
    end

    test "an empty path or an invalid variable generates nothing" do
      assert source(ReadFileCell, read_attrs(%{"path" => ""})) == ""
      assert source(ReadFileCell, read_attrs(%{"path" => "   "})) == ""
      assert source(ReadFileCell, read_attrs(%{"variable" => "Not Valid"})) == ""
    end

    test "paths with quotes and interpolation characters are escaped" do
      path = ~S|/d/a"b#{x}.shp|
      src = source(ReadFileCell, read_attrs(%{"path" => path}))

      assert {:ok, {:=, _, [_, {{:., _, _}, _, [^path]}]}} = Code.string_to_quoted(src)
    end

    test "every field combination evaluates against a fixture" do
      cases = [
        read_attrs(),
        read_attrs(%{"to_crs" => "EPSG:3857"}),
        read_attrs(%{"path" => @gpkg, "layer" => "polys"}),
        read_attrs(%{"path" => @gpkg, "driver" => "gpkg", "layer" => "pts", "to_crs" => "4326"}),
        read_attrs(%{"path" => @parquet, "bbox" => "-180,-90,180,90", "show_map" => true}),
        read_attrs(%{"path" => @parquet, "driver" => "parquet", "show_map" => true})
      ]

      for attrs <- cases do
        {value, binding} = run(source(ReadFileCell, attrs))
        assert is_struct(binding[:gdf], GeoDataFrame), inspect(attrs)

        if attrs["show_map"], do: assert(%Kino.JS.Live{} = value)
      end
    end
  end

  describe "ReadFileCell as a smart cell" do
    test "reports the layers of an existing GeoPackage on connect" do
      {kino, _source} = start_smart_cell!(ReadFileCell, read_attrs(%{"path" => @gpkg}))
      payload = connect(kino)

      assert payload.fields["path"] == @gpkg
      assert "polys" in payload.layers

      assert payload.layers ==
               GeoFrame.list_layers(@gpkg) |> DataFrame.pull("name") |> Explorer.Series.to_list()
    end

    test "no layers for a missing path or another format" do
      {kino, _} = start_smart_cell!(ReadFileCell, read_attrs(%{"path" => "/nope/x.gpkg"}))
      assert connect(kino).layers == []

      {kino, _} = start_smart_cell!(ReadFileCell, read_attrs())
      assert connect(kino).layers == []
    end

    test "the initial source comes from the attrs" do
      {_kino, source} = start_smart_cell!(ReadFileCell, read_attrs())
      assert unwrap(source) == ~s|gdf = GeoFrame.read_file!("#{@shp}")|
    end

    test "field updates regenerate the source and attrs" do
      {kino, _} = start_smart_cell!(ReadFileCell, read_attrs())

      push_event(kino, "update_field", %{"field" => "show_map", "value" => true})

      assert_smart_cell_update(
        kino,
        %{"show_map" => true, "variable" => "gdf"},
        source
      )

      assert source =~ "GeoFrame.Kino.deck(gdf)"
    end

    test "changing the path to a GeoPackage broadcasts its layers" do
      {kino, _} = start_smart_cell!(ReadFileCell, read_attrs())
      connect(kino)

      push_event(kino, "update_field", %{"field" => "path", "value" => @gpkg})

      assert_broadcast_event(kino, "update_field", %{"layers" => layers} = payload)
      assert "polys" in layers
      assert payload["fields"]["path"] == @gpkg
    end

    test "a GeoPackage path selects its first layer when none is chosen" do
      {kino, source} = start_smart_cell!(ReadFileCell, read_attrs(%{"path" => @gpkg}))
      assert source =~ ~s|layer: "pts"|
      assert connect(kino).fields["layer"] == "pts"

      {kino, _} = start_smart_cell!(ReadFileCell, read_attrs())
      push_event(kino, "update_field", %{"field" => "path", "value" => @gpkg})
      assert_smart_cell_update(kino, %{"layer" => "pts"}, source)
      assert source =~ ~s|layer: "pts"|
    end

    test "default attrs allocate a variable name" do
      {kino, _} = start_smart_cell!(ReadFileCell, %{})
      assert connect(kino).fields["variable"] =~ ~r/^gdf\d*$/
    end
  end

  test "the application registers the cell" do
    definitions = Kino.SmartCell.definitions()
    assert Enum.find(definitions, &(&1.module == ReadFileCell)).name == "Read spatial file"
  end
end
