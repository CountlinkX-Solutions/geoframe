defmodule GeoFrame.Native do
  @moduledoc false
  # NIF bindings to the `geoframe_native` Rust crate. Each function below is replaced
  # when the NIF library loads; the bodies only run if loading failed.
  use Rustler, otp_app: :geoframe, crate: "geoframe_native"

  def from_wkt(_rows, _crs), do: err()
  def from_wkb(_rows, _crs), do: err()
  def to_wkt(_ref), do: err()
  def to_wkb(_ref), do: err()
  def len(_ref), do: err()
  def crs(_ref), do: err()
  def set_crs(_ref, _crs), do: err()
  def take(_ref, _indices), do: err()
  def area(_ref), do: err()
  def length(_ref), do: err()
  def validity(_ref), do: err()
  def bounds(_ref), do: err()
  def total_bounds(_ref), do: err()
  def centroid(_ref), do: err()

  defp err, do: :erlang.nif_error(:nif_not_loaded)
end
