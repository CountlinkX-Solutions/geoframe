defmodule GeoFrame.MixProject do
  use Mix.Project

  @version "0.1.0-dev"
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
      docs: [main: "readme", extras: ["README.md"]]
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
                Cargo.toml Cargo.lock mix.exs README.md LICENSE)
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
      {:ex_doc, "~> 0.40", only: :dev, runtime: false}
    ]
  end
end
