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

  @doc """
  Whether two CRS labels are compatible for binary operations, joins, overlays and `concat`.

  The comparison ignores case and treats `OGC:CRS84` as `EPSG:4326` (GeoFrame is always
  longitude/latitude). Labels are never rewritten. Two `nil` CRSs are equivalent; `nil` against
  a CRS is not (operations that let an unset CRS match anything handle `nil` themselves).

      iex> GeoFrame.CRS.equivalent?("epsg:4326", "OGC:CRS84")
      true
      iex> GeoFrame.CRS.equivalent?("EPSG:4326", "EPSG:3857")
      false
  """
  @spec equivalent?(String.t() | nil, String.t() | nil) :: boolean()
  def equivalent?(nil, nil), do: true
  def equivalent?(nil, _), do: false
  def equivalent?(_, nil), do: false
  def equivalent?(a, b) when is_binary(a) and is_binary(b), do: Native.crs_equivalent(a, b)
end
