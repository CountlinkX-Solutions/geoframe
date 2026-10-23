# PostGIS tests need a database: set GEOFRAME_POSTGIS_URL (for example
# postgres://postgres:geoframe@127.0.0.1:55432/postgres) and run `mix test --only postgis`.
exclude = if System.get_env("GEOFRAME_POSTGIS_URL"), do: [], else: [postgis: true]

ExUnit.start(exclude: exclude)
