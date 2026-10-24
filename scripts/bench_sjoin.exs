# Timing of large spatial joins (issue #58). Run: mix run scripts/bench_sjoin.exs [pairs_side]
#
#   * points-in-cells: N points joined to a grid of cells, N pairs (one match per point);
#   * all-overlap: M squares joined to M squares that all intersect, M * M pairs.
#
# Prints `BENCH <case> <how> pairs=<n> <ms> ms`. Deterministic data, nothing written to disk.
alias Explorer.DataFrame
alias GeoFrame.GeoDataFrame
alias GeoFrame.GeoSeries

[side | _] = System.argv() ++ ["1200"]
m = String.to_integer(side)

# Runs the case three times: `first` includes allocator warm-up, `best` is the steadier figure.
time = fn name, how, fun ->
  runs =
    for _ <- 1..3 do
      {us, out} = :timer.tc(fun)
      {us / 1000, GeoDataFrame.n_rows(out)}
    end

  {first, pairs} = hd(runs)
  best = runs |> Enum.map(&elem(&1, 0)) |> Enum.min()

  IO.puts(
    "BENCH #{name} #{how} pairs=#{pairs} first=#{Float.round(first, 1)} ms best=#{Float.round(best, 1)} ms"
  )
end

# points in cells
cells = 100
points_n = 1_000_000

cell_wkt =
  for i <- 0..(cells - 1), j <- 0..(cells - 1) do
    "POLYGON ((#{i} #{j}, #{i + 1} #{j}, #{i + 1} #{j + 1}, #{i} #{j + 1}, #{i} #{j}))"
  end

polygons =
  GeoDataFrame.new(
    DataFrame.new(
      cell: Enum.to_list(1..length(cell_wkt)),
      tag: Enum.map(cell_wkt, &String.length/1)
    ),
    GeoSeries.from_wkt!(cell_wkt)
  )

xs = for k <- 0..(points_n - 1), do: rem(k * 7919, 99_991) / 99_991 * 99.9 + 0.05
ys = for k <- 0..(points_n - 1), do: rem(k * 104_729, 99_989) / 99_989 * 99.9 + 0.05

points =
  GeoDataFrame.new(
    DataFrame.new(id: Enum.to_list(1..points_n)),
    GeoSeries.from_xy(xs, ys)
  )

for how <- [:inner, :left, :right] do
  time.("points_in_cells", how, fn -> GeoFrame.sjoin(points, polygons, how: how) end)
end

# all-overlap squares
wkt = for i <- 1..m, do: "POLYGON ((0 0, #{i} 0, #{i} #{i}, 0 #{i}, 0 0))"

squares =
  GeoDataFrame.new(DataFrame.new(n: Enum.to_list(1..m)), GeoSeries.from_wkt!(wkt))

for how <- [:inner, :left] do
  time.("all_overlap_#{m}", how, fn -> GeoFrame.sjoin(squares, squares, how: how) end)
end

time.("nearest_points", :inner, fn ->
  GeoFrame.sjoin_nearest(
    GeoDataFrame.new(
      DataFrame.new(id: Enum.to_list(1..200_000)),
      GeoSeries.from_xy(Enum.take(xs, 200_000), Enum.take(ys, 200_000))
    ),
    GeoDataFrame.new(
      DataFrame.new(id: Enum.to_list(1..20_000)),
      GeoSeries.from_xy(Enum.take(ys, 20_000), Enum.take(xs, 20_000))
    ),
    how: :inner
  )
end)
