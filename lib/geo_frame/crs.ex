defmodule GeoFrame.CRS do
  @moduledoc """
  Helpers on CRS strings (`"EPSG:<code>"`, `"OGC:CRS84"`).
  """

  alias GeoFrame.Native

  @doc """
  Whether `crs` names a geographic (lon/lat) CRS.

  Returns `true` for `OGC:CRS84`, `EPSG:4326` and any EPSG code whose definition is
  `+proj=longlat`, `false` for projected CRSs, and `nil` for `nil` or a CRS that cannot be
  resolved.

      iex> GeoFrame.CRS.geographic?("EPSG:4326")
      true
      iex> GeoFrame.CRS.geographic?("EPSG:3857")
      false
      iex> GeoFrame.CRS.geographic?(nil)
      nil
  """
  @spec geographic?(String.t() | nil) :: boolean() | nil
  def geographic?(nil), do: nil
  def geographic?(crs) when is_binary(crs), do: Native.crs_is_geographic(crs)
end
