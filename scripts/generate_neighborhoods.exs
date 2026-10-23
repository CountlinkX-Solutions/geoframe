# Regenerates test/fixtures/neighborhoods.parquet with GeoFrame's own writer.
# Run from the repo root: mix run scripts/generate_neighborhoods.exs
alias Explorer.DataFrame
alias GeoFrame.{GeoDataFrame, GeoSeries}

square = fn x, y, w, h ->
  "POLYGON ((#{x} #{y}, #{x + w} #{y}, #{x + w} #{y + h}, #{x} #{y + h}, #{x} #{y}))"
end

data =
  DataFrame.new(
    name: ["Alpha", "Bravo", "Charlie", "Delta", "Echo"],
    kind: ["residential", "commercial", "park", "residential", "landmark"],
    population: [1200, 850, nil, 430, 15],
    density: [12.0, 2.125, nil, 21.5, 0.5]
  )

geometry =
  GeoSeries.from_wkt!(
    [
      square.(-80.20, 25.76, 0.01, 0.01),
      square.(-80.19, 25.76, 0.02, 0.02),
      square.(-80.20, 25.78, 0.01, 0.01),
      square.(-80.18, 25.78, 0.02, 0.01),
      "POINT (-80.185 25.775)"
    ],
    crs: "EPSG:4326"
  )

path = Path.join([__DIR__, "..", "test", "fixtures", "neighborhoods.parquet"]) |> Path.expand()
:ok = GeoFrame.write_parquet(GeoDataFrame.new(data, geometry), path)
IO.puts("wrote #{path} (#{File.stat!(path).size} bytes)")
