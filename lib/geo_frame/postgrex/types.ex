if Code.ensure_loaded?(Postgrex) do
  # Predefined Postgrex types module: the PostGIS `geometry` extension plus Postgrex's
  # defaults (which `Postgrex.Types.define/3` appends itself).
  # Start your connection with `Postgrex.start_link(types: GeoFrame.Postgrex.Types, ...)`.
  Postgrex.Types.define(GeoFrame.Postgrex.Types, [GeoFrame.Postgrex.Extension], [])
end
