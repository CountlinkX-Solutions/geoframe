defmodule GeoFrame.MixProject do
  use Mix.Project

  @version "0.1.0"
  @source_url "https://github.com/CountlinkX-Solutions/geoframe"

  def project do
    [
      app: :geoframe,
      version: @version,
      elixir: "~> 1.17",
      elixirc_paths: elixirc_paths(Mix.env()),
      start_permanent: Mix.env() == :prod,
      deps: deps(),
      description:
        "GeoPandas-style geospatial dataframes for Elixir, powered by GeoArrow and Rust.",
      name: "GeoFrame",
      source_url: @source_url,
      package: package(),
      docs: docs()
    ]
  end

  defp docs do
    [
      main: "readme",
      source_ref: "v#{@version}",
      extras: [
        "README.md",
        "CHANGELOG.md": [title: "Changelog"],
        "docs/parity.md": [title: "GeoPandas parity"],
        "notebooks/tour.livemd": [title: "Notebook: tour"],
        "notebooks/deck.livemd": [title: "Notebook: deck.gl widget"],
        "notebooks/compare.livemd": [title: "Notebook: GeoFrame vs GeoPandas"],
        "notebooks/spike.livemd": [title: "Notebook: spike"],
        LICENSE: [title: "License"],
        NOTICE: [title: "Notice"]
      ],
      groups_for_extras: [
        Reference: ["docs/parity.md"],
        Notebooks: ~r{notebooks/},
        Legal: ["LICENSE", "NOTICE"]
      ],
      groups_for_modules: [
        Core: [GeoFrame, GeoFrame.GeoDataFrame, GeoFrame.GeoSeries],
        "Spatial operations": [GeoFrame.SpatialJoin, GeoFrame.Overlay, GeoFrame.Dissolve],
        "CRS and datum grids": [GeoFrame.CRS, GeoFrame.Grids],
        Livebook: [
          GeoFrame.Kino,
          GeoFrame.Kino.Deck,
          GeoFrame.Kino.ReadFileCell,
          GeoFrame.Kino.SpatialJoinCell
        ],
        PostGIS: [GeoFrame.PostGIS, GeoFrame.Postgrex.Extension],
        Testing: [GeoFrame.Testing]
      ],
      # Defined at compile time by Postgrex.Types.define/3, so it has no docs page.
      skip_undefined_reference_warnings_on: ["GeoFrame.Postgrex.Extension"]
    ]
  end

  # Ship the NIF sources only. `priv/native` is never packaged: Rustler copies every cdylib
  # artifact Cargo reports there, including `proj4rs.so` (proj4rs declares
  # `crate-type = ["cdylib", "rlib"]`), and those build outputs must not reach Hex or a
  # precompiled NIF archive. Any rustler_precompiled setup must pick the NIF library by
  # name (`libgeoframe_native`) for the same reason.
  defp package do
    [
      licenses: ["Apache-2.0"],
      links: %{"GitHub" => @source_url},
      files: ~w(lib native/geoframe_core/src native/geoframe_core/Cargo.toml
                native/geoframe_native/src native/geoframe_native/Cargo.toml
                Cargo.toml Cargo.lock mix.exs README.md CHANGELOG.md LICENSE NOTICE)
    ]
  end

  def application do
    [
      extra_applications: [:logger],
      mod: {GeoFrame.Application, []}
    ]
  end

  defp elixirc_paths(:test), do: ["lib", "test/support"]
  defp elixirc_paths(_), do: ["lib"]

  defp deps do
    [
      # Hex has no rustler 0.39 yet; the crate is 0.39 (native/geoframe_native/Cargo.toml).
      # "~> 0.38" admits 0.39 once published, so only the lock needs bumping then.
      {:rustler, "~> 0.38", runtime: false},
      {:explorer, "~> 0.13"},
      {:jason, "~> 1.4"},
      {:postgrex, "~> 0.22", optional: true},
      {:kino, "~> 0.19", optional: true},
      {:maplibre, "~> 0.1.9", optional: true},
      {:kino_maplibre, "~> 0.1.13", optional: true},
      {:stream_data, "~> 1.4", only: [:dev, :test]},
      {:credo, "~> 1.7", only: [:dev, :test], runtime: false},
      # Also in :test so CI can run `mix docs` in its single MIX_ENV (and reuse the test build).
      {:ex_doc, "~> 0.40", only: [:dev, :test], runtime: false}
    ]
  end
end
