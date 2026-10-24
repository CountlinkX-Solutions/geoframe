defmodule GeoFrame.PackageTest do
  use ExUnit.Case, async: true

  # Issue #29: Rustler copies every cdylib artifact (including proj4rs.so) into priv/native.
  # Built libraries must never ship in the Hex package.
  test "Hex package files never include priv/native build outputs" do
    files = Mix.Project.config()[:package][:files]

    assert is_list(files)
    refute Enum.any?(files, &String.starts_with?(&1, "priv"))
    assert "lib" in files
    assert "native/geoframe_native/src" in files
  end
end
