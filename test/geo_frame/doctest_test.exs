defmodule GeoFrame.DoctestTest do
  use ExUnit.Case, async: true

  doctest GeoFrame
  doctest GeoFrame.GeoSeries
  doctest GeoFrame.CRS
  doctest GeoFrame.GeoDataFrame
  doctest GeoFrame.Overlay
  doctest GeoFrame.Testing
end
