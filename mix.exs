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
      docs: [main: "readme", extras: ["README.md"]]
    ]
  end

  def application do
    [
      extra_applications: [:logger]
    ]
  end

  defp elixirc_paths(:test), do: ["lib", "test/support"]
  defp elixirc_paths(_), do: ["lib"]

  defp deps do
    [
      {:rustler, "~> 0.38", runtime: false},
      {:explorer, "~> 0.13"},
      {:jason, "~> 1.4"},
      {:kino, "~> 0.19", optional: true},
      {:maplibre, "~> 0.1.9", optional: true},
      {:kino_maplibre, "~> 0.1.13", optional: true},
      {:stream_data, "~> 1.4", only: [:dev, :test]},
      {:credo, "~> 1.7", only: [:dev, :test], runtime: false},
      {:ex_doc, "~> 0.40", only: :dev, runtime: false}
    ]
  end
end
