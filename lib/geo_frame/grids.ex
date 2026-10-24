defmodule GeoFrame.Grids do
  @moduledoc """
  Datum grids for `GeoFrame.GeoSeries.to_crs/2`.

  A datum that is not WGS 84 (NAD27, OSGB36, DHDN, Tokyo, ...) is moved to WGS 84 by a
  **grid** where one is installed and by a **Helmert** set otherwise, like PROJ:

    * Helmert (`+towgs84`, taken from the bundled EPSG table; NAD27 uses the CONUS set
      `-8, 160, 176 m`): the default. It is a ballpark: metres to a few tens of metres
      against the best operation of the region (see `docs/parity.md`).
    * Grids give centimetres to a metre. NTv2 files (`.gsb`) and the GeoTIFF grids that
      PROJ-data publishes (`.tif`) are read. Nothing is downloaded: install the files you need
      (for example with `projsync` or `pyproj.sync`, or from `https://cdn.proj.org`) and point
      GeoFrame at the directory.

  ## Where the files are searched

  In order: the directories given to `set_dirs/1` (or `config :geoframe, grid_dirs: [...]`),
  then the directories of the `PROJ_NADGRIDS` and `PROJ_DATA` environment variables (separated by
  `:`), so a PROJ data directory works unchanged. The names tried for a datum come from a table
  generated from the PROJ database (`status/1` lists them for a CRS, for example
  `uk_os_OSTN15_NTv2_OSGBtoETRS.tif` for OSGB36 and `us_noaa_conus.tif` for NAD27);
  `register/2` replaces the names for one datum.

  A grid is used only if its file is found; a point outside every grid found takes the Helmert
  shift, so a missing or too small grid never makes `to_crs/2` fail. Grid files are read once and
  kept in memory (the engine caches them by file name), so directories added later only matter
  for files not yet loaded. Grids that shift to NAD83, ETRS89, GDA94 and the like count as
  shifting to WGS 84, as PROJ's own null step between them does (differences of 1 to 2 m are
  ignored).

  ## Examples

      GeoFrame.Grids.set_dirs(["/data/proj"])
      GeoFrame.Grids.status("EPSG:27700")
      #=> [%{name: "OSTN02_NTv2.gsb", path: nil},
      #    %{name: "uk_os_OSTN15_NTv2_OSGBtoETRS.tif", path: "/data/proj/uk_os_OSTN15_NTv2_OSGBtoETRS.tif"},
      #    %{name: "OSTN15_NTv2_OSGBtoETRS.gsb", path: nil}]
  """

  alias GeoFrame.Native

  @doc """
  Sets the directories searched for grid files (replacing the previous ones). Also the
  `:grid_dirs` entry of the `:geoframe` application environment, applied at startup.
  """
  @spec set_dirs([Path.t()]) :: :ok
  def set_dirs(dirs) when is_list(dirs) do
    dirs |> Enum.map(&Path.expand/1) |> Native.set_grid_dirs()
  end

  @doc "The directories set with `set_dirs/1` (the environment variables are not included)."
  @spec dirs() :: [String.t()]
  def dirs, do: Native.grid_dirs()

  @doc """
  The grid files that may shift the datum of `crs` (a geographic CRS or a projected CRS on it), in
  the order they are tried, and where each is installed (`path: nil` when not found).

  Returns `[]` for a CRS with no datum grid, such as `"EPSG:4326"` or `"EPSG:3857"`.
  """
  @spec status(String.t() | pos_integer()) :: [%{name: String.t(), path: String.t() | nil}]
  def status(crs) do
    crs
    |> label()
    |> Native.grid_status()
    |> unwrap()
    |> Enum.map(fn {name, path} -> %{name: name, path: path} end)
  end

  @doc """
  Uses `names` (file names relative to the grid directories, or absolute paths) for the datum
  of `crs`, instead of the built-in table. An empty list turns grids off for that datum.

  The entry belongs to the geographic CRS the definition is based on, so registering
  `"EPSG:27700"` also applies to `"EPSG:4277"` and every other OSGB36 CRS.
  """
  @spec register(String.t() | pos_integer(), [String.t()]) :: :ok
  def register(crs, names) when is_list(names) do
    crs |> label() |> Native.register_grids(names) |> unwrap()
    :ok
  end

  @doc "Removes a `register/2` entry, so the built-in table applies again."
  @spec unregister(String.t() | pos_integer()) :: :ok
  def unregister(crs) do
    crs |> label() |> Native.unregister_grids() |> unwrap()
    :ok
  end

  defp label(code) when is_integer(code), do: "EPSG:#{code}"
  defp label(label) when is_binary(label), do: label

  defp unwrap({:ok, value}), do: value
  defp unwrap({:error, message}), do: raise(ArgumentError, message)
end
