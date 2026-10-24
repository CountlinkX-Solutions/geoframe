defmodule GeoFrame.PostgisTest do
  # Needs a PostGIS database: GEOFRAME_POSTGIS_URL=postgres://user:pw@host:port/db
  use ExUnit.Case, async: false

  import GeoFrame.Testing

  alias Explorer.DataFrame
  alias GeoFrame.GeoDataFrame
  alias GeoFrame.GeoSeries

  @moduletag :postgis

  setup_all do
    uri = URI.parse(System.fetch_env!("GEOFRAME_POSTGIS_URL"))
    [username, password] = String.split(uri.userinfo, ":", parts: 2)

    {:ok, conn} =
      Postgrex.start_link(
        hostname: uri.host,
        port: uri.port,
        username: username,
        password: password,
        database: String.trim_leading(uri.path, "/"),
        types: GeoFrame.Postgrex.Types
      )

    Postgrex.query!(conn, "CREATE EXTENSION IF NOT EXISTS postgis", [])
    {:ok, conn: conn}
  end

  setup %{conn: conn} do
    table = "gf_test_#{System.unique_integer([:positive])}"
    on_exit(fn -> Postgrex.query(conn, ~s|DROP TABLE IF EXISTS "#{table}"|, []) end)
    {:ok, table: table}
  end

  defp points(n, crs \\ "EPSG:4326") do
    wkt = Enum.map(1..n, &"POINT (#{&1} #{&1 * 2})")
    GeoSeries.from_wkt!(wkt, crs: crs)
  end

  defp frame do
    data =
      DataFrame.new(
        id: [1, 2, 3],
        score: [1.5, nil, 3.0],
        name: ["a", "b", nil],
        flag: [true, false, nil],
        day: [~D[2024-01-01], nil, ~D[2024-03-05]],
        at: [~N[2024-01-01 10:00:00.000000], nil, ~N[2024-03-05 01:02:03.000000]]
      )

    GeoDataFrame.new(data, points(3), geometry_column: "geom")
  end

  describe "round trip" do
    test "geometry, CRS and attribute dtypes survive", %{conn: conn, table: table} do
      gdf = frame()
      assert :ok = GeoFrame.to_postgis(gdf, conn, table)
      assert {:ok, back} = GeoFrame.read_postgis(conn, ~s|SELECT * FROM "#{table}"|)
      assert GeoDataFrame.crs(back) == "EPSG:4326"
      assert_geodataframe_equal(back, gdf)
    end

    test "null geometries and nulls in every column", %{conn: conn, table: table} do
      geometry = GeoSeries.from_wkt!(["POINT (1 1)", nil, "POINT (2 2)"], crs: "EPSG:4326")
      gdf = GeoDataFrame.new(DataFrame.new(id: [1, nil, 3]), geometry, geometry_column: "geom")
      :ok = GeoFrame.to_postgis(gdf, conn, table)
      back = GeoFrame.read_postgis!(conn, ~s|SELECT * FROM "#{table}" ORDER BY id NULLS LAST|)
      assert GeoSeries.to_wkt(back.geometry) == ["POINT(1 1)", "POINT(2 2)", nil]
    end

    test "mixed geometry types use a generic geometry column", %{conn: conn, table: table} do
      geometry =
        GeoSeries.from_wkt!(
          ["POINT (1 1)", "LINESTRING (0 0, 1 1)", "POLYGON ((0 0, 1 0, 1 1, 0 0))"],
          crs: "EPSG:4326"
        )

      gdf = GeoDataFrame.new(DataFrame.new(id: [1, 2, 3]), geometry, geometry_column: "geom")
      :ok = GeoFrame.to_postgis(gdf, conn, table)

      %{rows: [[type]]} =
        Postgrex.query!(conn, "SELECT type FROM geometry_columns WHERE f_table_name = $1", [
          table
        ])

      assert type == "GEOMETRY"
      assert_geodataframe_equal(GeoFrame.read_postgis!(conn, "SELECT * FROM \"#{table}\""), gdf)
    end

    test "a single geometry type is typed in the column", %{conn: conn, table: table} do
      :ok = GeoFrame.to_postgis(frame(), conn, table)

      assert %{rows: [["POINT", 4326]]} =
               Postgrex.query!(
                 conn,
                 "SELECT type, srid FROM geometry_columns WHERE f_table_name = $1",
                 [table]
               )
    end

    test "a projected CRS is stored as its SRID", %{conn: conn, table: table} do
      gdf = GeoDataFrame.new(DataFrame.new(id: [1, 2]), points(2, "EPSG:3857"))
      :ok = GeoFrame.to_postgis(gdf, conn, table)
      back = GeoFrame.read_postgis!(conn, ~s|SELECT * FROM "#{table}"|, geom_col: "geometry")
      assert GeoDataFrame.crs(back) == "EPSG:3857"
    end

    test "writes more rows than one batch", %{conn: conn, table: table} do
      gdf = GeoDataFrame.new(DataFrame.new(id: Enum.to_list(1..2500)), points(2500))
      :ok = GeoFrame.to_postgis(gdf, conn, table, chunksize: 1000)
      assert %{rows: [[2500]]} = Postgrex.query!(conn, ~s|SELECT count(*) FROM "#{table}"|, [])
    end

    test "a frame without attributes", %{conn: conn, table: table} do
      gdf = GeoDataFrame.new(DataFrame.new([]), points(2), geometry_column: "geom")
      :ok = GeoFrame.to_postgis(gdf, conn, table)

      assert GeoFrame.read_postgis!(conn, ~s|SELECT * FROM "#{table}"|) |> GeoDataFrame.n_rows() ==
               2
    end
  end

  describe "read_postgis" do
    test "default geom_col is geom, with params and crs override", %{conn: conn, table: table} do
      :ok = GeoFrame.to_postgis(frame(), conn, table)

      gdf =
        GeoFrame.read_postgis!(conn, ~s|SELECT id, geom FROM "#{table}" WHERE id > $1|,
          params: [1]
        )

      assert GeoDataFrame.names(gdf) == ["id", "geom"]
      assert GeoDataFrame.n_rows(gdf) == 2

      other = GeoFrame.read_postgis!(conn, ~s|SELECT id, geom FROM "#{table}"|, crs: "EPSG:3857")
      assert GeoDataFrame.crs(other) == "EPSG:3857"
    end

    test "geometry without a SRID has no CRS", %{conn: conn} do
      gdf = GeoFrame.read_postgis!(conn, "SELECT ST_GeomFromText('POINT(1 2)') AS geom")
      assert GeoDataFrame.crs(gdf) == nil
    end

    test "reads geometries made by PostGIS (polygon, SRID)", %{conn: conn} do
      sql = "SELECT 1 AS id, ST_GeomFromText('POLYGON((0 0,1 0,1 1,0 0))', 3857) AS geom"
      gdf = GeoFrame.read_postgis!(conn, sql)
      assert GeoDataFrame.crs(gdf) == "EPSG:3857"
      assert GeoSeries.geom_type(gdf.geometry) |> Explorer.Series.to_list() == ["Polygon"]
    end

    test "accepts bytea (ST_AsEWKB) and hex text", %{conn: conn} do
      base = "ST_SetSRID(ST_MakePoint(1, 2), 4326)"

      bytea = GeoFrame.read_postgis!(conn, "SELECT ST_AsEWKB(#{base}) AS geom")
      assert GeoDataFrame.crs(bytea) == "EPSG:4326"

      hex = GeoFrame.read_postgis!(conn, "SELECT encode(ST_AsEWKB(#{base}), 'hex') AS geom")
      assert GeoDataFrame.crs(hex) == "EPSG:4326"
      assert GeoSeries.to_wkt(hex.geometry) == ["POINT(1 2)"]
    end

    test "maps Postgres types to dtypes", %{conn: conn} do
      sql = """
      SELECT 1::int2 AS a, 2::int4 AS b, 3::int8 AS c, 1.5::float4 AS d, 2.5::float8 AS e,
             3.25::numeric AS f, 'x'::text AS g, true AS h, DATE '2024-02-03' AS i,
             TIMESTAMP '2024-02-03 04:05:06' AS j, TIMESTAMPTZ '2024-02-03 04:05:06+00' AS k,
             '{"a": 1}'::jsonb AS l, ST_MakePoint(0, 0) AS geom
      """

      data = GeoFrame.read_postgis!(conn, sql).data
      dtypes = DataFrame.dtypes(data)

      assert %{
               "a" => {:s, 64},
               "c" => {:s, 64},
               "d" => {:f, 64},
               "g" => :string,
               "h" => :boolean,
               "i" => :date,
               "j" => {:naive_datetime, :microsecond},
               "k" => {:datetime, :microsecond, "Etc/UTC"},
               "l" => :string
             } = dtypes

      assert %{"f" => {:decimal, _, 2}} = dtypes
      assert Explorer.Series.to_list(data["f"]) == [Decimal.new("3.25")]
      assert Explorer.Series.to_list(data["l"]) == [~s|{"a":1}|]
    end

    test "errors are returned, not raised", %{conn: conn} do
      assert {:error, message} = GeoFrame.read_postgis(conn, "SELECT 1 AS id")
      assert message =~ "geom"
      assert {:error, message} = GeoFrame.read_postgis(conn, "SELEKT 1")
      assert message =~ "syntax"

      assert_raise ArgumentError, fn -> GeoFrame.read_postgis!(conn, "SELECT 1 AS id") end
    end

    test "an empty result keeps the columns", %{conn: conn, table: table} do
      :ok = GeoFrame.to_postgis(frame(), conn, table)
      gdf = GeoFrame.read_postgis!(conn, ~s|SELECT * FROM "#{table}" WHERE id < 0|)
      assert GeoDataFrame.n_rows(gdf) == 0
      assert "id" in GeoDataFrame.names(gdf)
    end
  end

  describe "read_postgis numeric precision (#55)" do
    test "numeric above 2^53 keeps every digit", %{conn: conn} do
      sql = "SELECT 18446744073709551615::numeric(20, 0) AS big, ST_MakePoint(0, 0) AS geom"
      data = GeoFrame.read_postgis!(conn, sql).data
      assert %{"big" => {:decimal, 20, 0}} = DataFrame.dtypes(data)
      assert Explorer.Series.to_list(data["big"]) == [Decimal.new("18446744073709551615")]
    end

    test "numeric(p, s) with more than 15 digits keeps its scale", %{conn: conn} do
      sql = """
      SELECT x AS n, ST_MakePoint(0, 0) AS geom
      FROM (VALUES (123456789012345.678901::numeric), (NULL), (0.5)) AS t(x)
      """

      data = GeoFrame.read_postgis!(conn, sql).data
      assert %{"n" => {:decimal, 21, 6}} = DataFrame.dtypes(data)

      assert Explorer.Series.to_list(data["n"]) ==
               [Decimal.new("123456789012345.678901"), nil, Decimal.new("0.500000")]
    end

    test "a u64 column survives a write and a read", %{conn: conn, table: table} do
      big = 18_446_744_073_709_551_615
      data = DataFrame.new(big: Explorer.Series.from_list([big, 1], dtype: {:u, 64}))
      :ok = GeoFrame.to_postgis(GeoDataFrame.new(data, points(2)), conn, table)
      back = GeoFrame.read_postgis!(conn, ~s|SELECT * FROM "#{table}"|, geom_col: "geometry")

      assert Explorer.Series.to_list(back.data["big"]) ==
               [Decimal.new(big), Decimal.new(1)]
    end

    test "a decimal column round-trips", %{conn: conn, table: table} do
      values = [Decimal.new("1.25"), nil, Decimal.new("-300.10")]
      data = DataFrame.new(d: Explorer.Series.from_list(values, dtype: {:decimal, 10, 2}))
      :ok = GeoFrame.to_postgis(GeoDataFrame.new(data, points(3)), conn, table)
      back = GeoFrame.read_postgis!(conn, ~s|SELECT * FROM "#{table}"|, geom_col: "geometry")
      assert Explorer.Series.to_list(back.data["d"]) == values
    end

    test "NaN and Infinity numerics fall back to float64", %{conn: conn} do
      sql = """
      SELECT x AS n, ST_MakePoint(0, 0) AS geom
      FROM (VALUES (1.5::numeric), ('NaN'::numeric), ('Infinity'::numeric)) AS t(x)
      """

      data = GeoFrame.read_postgis!(conn, sql).data
      assert %{"n" => {:f, 64}} = DataFrame.dtypes(data)
      assert Explorer.Series.to_list(data["n"]) == [1.5, :nan, :infinity]
    end

    test "numerics wider than 38 digits become exact text", %{conn: conn} do
      digits = String.duplicate("9", 40)
      sql = "SELECT #{digits}::numeric AS n, ST_MakePoint(0, 0) AS geom"
      data = GeoFrame.read_postgis!(conn, sql).data
      assert %{"n" => :string} = DataFrame.dtypes(data)
      assert Explorer.Series.to_list(data["n"]) == [digits]
    end
  end

  describe "read_postgis edge cases (#56)" do
    test "duplicate column names are an error, not a silent drop", %{conn: conn} do
      sql = "SELECT 1 AS id, 2 AS id, ST_MakePoint(0, 0) AS geom"
      assert {:error, message} = GeoFrame.read_postgis(conn, sql)
      assert message =~ "duplicate column"
      assert message =~ ~s|"id"|
    end

    test "an empty result is typed from the query, not :string", %{conn: conn} do
      sql = """
      SELECT 1::int4 AS a, 2.5::float8 AS b, 'x'::text AS c, true AS d, DATE '2024-01-01' AS e,
             TIMESTAMPTZ '2024-01-01 00:00:00+00' AS f, '{}'::jsonb AS g, 1::numeric AS h,
             ST_MakePoint(0, 0) AS geom
      WHERE false
      """

      gdf = GeoFrame.read_postgis!(conn, sql)
      assert GeoDataFrame.n_rows(gdf) == 0

      assert %{
               "a" => {:s, 64},
               "b" => {:f, 64},
               "c" => :string,
               "d" => :boolean,
               "e" => :date,
               "f" => {:datetime, :microsecond, "Etc/UTC"},
               "g" => :string,
               "h" => {:decimal, _, _}
             } = DataFrame.dtypes(gdf.data)
    end

    test "an all-NULL column keeps its Postgres type", %{conn: conn} do
      sql = "SELECT NULL::int4 AS a, NULL::float8 AS b, ST_MakePoint(0, 0) AS geom"
      dtypes = DataFrame.dtypes(GeoFrame.read_postgis!(conn, sql).data)
      assert %{"a" => {:s, 64}, "b" => {:f, 64}} = dtypes
    end

    test "intervals, ranges, network and geometric types become text", %{conn: conn} do
      sql = """
      SELECT INTERVAL '1 year 2 mons 3 days 04:05:06' AS iv, int4range(1, 5) AS rg,
             '10.0.0.1/24'::inet AS ip, point(1, 2) AS pt, ST_MakePoint(0, 0) AS geom
      """

      data = GeoFrame.read_postgis!(conn, sql).data

      assert %{"iv" => :string, "rg" => :string, "ip" => :string, "pt" => :string} =
               DataFrame.dtypes(data)

      assert Explorer.Series.to_list(data["iv"]) == ["P1Y2M3DT4H5M6S"]
      assert Explorer.Series.to_list(data["ip"]) == ["10.0.0.1/24"]
    end

    test "uuid columns are formatted as text", %{conn: conn} do
      sql = "SELECT '123e4567-e89b-12d3-a456-426614174000'::uuid AS u, ST_MakePoint(0, 0) AS geom"
      data = GeoFrame.read_postgis!(conn, sql).data
      assert Explorer.Series.to_list(data["u"]) == ["123e4567-e89b-12d3-a456-426614174000"]
    end

    test "values that cannot be encoded are an error, not a crash", %{conn: conn} do
      sql = "SELECT ARRAY['\\xff'::bytea] AS bad, ST_MakePoint(0, 0) AS geom"
      assert {:error, message} = GeoFrame.read_postgis(conn, sql)
      assert message =~ "bad"
    end

    test "arrays and jsonb read as JSON text", %{conn: conn} do
      sql =
        "SELECT ARRAY[1, 2] AS xs, '[1, {\"a\": null}]'::jsonb AS js, ST_MakePoint(0, 0) AS geom"

      data = GeoFrame.read_postgis!(conn, sql).data
      assert Explorer.Series.to_list(data["xs"]) == ["[1,2]"]
      assert Explorer.Series.to_list(data["js"]) == [~s|[1,{"a":null}]|]
    end
  end

  describe "to_postgis dtype whitelist (#56)" do
    test "multi-word and parameterised type names are accepted", %{conn: conn, table: table} do
      :ok =
        GeoFrame.to_postgis(frame(), conn, table,
          dtype: %{
            "score" => "double precision",
            "name" => "character varying(20)",
            "at" => "timestamp(3) without time zone",
            "id" => "BIGINT"
          }
        )
    end

    test "constraints and trailing clauses are rejected", %{conn: conn, table: table} do
      for bad <- [
            "integer PRIMARY KEY",
            "integer NOT NULL",
            "text DEFAULT 'x'",
            "int, extra int",
            "text COLLATE \"C\"",
            "integer; DROP TABLE x"
          ] do
        assert_raise ArgumentError, ~r/invalid SQL type/, fn ->
          GeoFrame.to_postgis(frame(), conn, table, dtype: %{"id" => bad})
        end
      end
    end
  end

  describe "to_postgis if_exists" do
    test "fail is the default", %{conn: conn, table: table} do
      :ok = GeoFrame.to_postgis(frame(), conn, table)
      assert {:error, message} = GeoFrame.to_postgis(frame(), conn, table)
      assert message =~ "already exists"
      assert_raise ArgumentError, fn -> GeoFrame.to_postgis!(frame(), conn, table) end
    end

    test "replace drops and recreates", %{conn: conn, table: table} do
      :ok = GeoFrame.to_postgis(frame(), conn, table)
      small = GeoDataFrame.new(DataFrame.new(id: [9]), points(1), geometry_column: "geom")
      :ok = GeoFrame.to_postgis(small, conn, table, if_exists: :replace)
      assert %{rows: [[1]]} = Postgrex.query!(conn, ~s|SELECT count(*) FROM "#{table}"|, [])
    end

    test "append adds rows", %{conn: conn, table: table} do
      :ok = GeoFrame.to_postgis(frame(), conn, table)
      :ok = GeoFrame.to_postgis(frame(), conn, table, if_exists: :append)
      assert %{rows: [[6]]} = Postgrex.query!(conn, ~s|SELECT count(*) FROM "#{table}"|, [])
    end

    test "append to a missing table creates it", %{conn: conn, table: table} do
      :ok = GeoFrame.to_postgis(frame(), conn, table, if_exists: :append)
      assert %{rows: [[3]]} = Postgrex.query!(conn, ~s|SELECT count(*) FROM "#{table}"|, [])
    end

    test "a failed write leaves nothing behind", %{conn: conn, table: table} do
      :ok = GeoFrame.to_postgis(frame(), conn, table)
      bad = GeoDataFrame.new(DataFrame.new(other: [1]), points(1), geometry_column: "geom")
      assert {:error, _} = GeoFrame.to_postgis(bad, conn, table, if_exists: :append)
      assert %{rows: [[3]]} = Postgrex.query!(conn, ~s|SELECT count(*) FROM "#{table}"|, [])
    end

    test "invalid if_exists raises", %{conn: conn, table: table} do
      assert_raise ArgumentError, fn ->
        GeoFrame.to_postgis(frame(), conn, table, if_exists: :nope)
      end
    end
  end

  describe "to_postgis options" do
    test "schema and awkward identifiers are quoted", %{conn: conn} do
      schema = "gf_schema_#{System.unique_integer([:positive])}"
      table = ~s|we"ird; DROP TABLE x; --|
      Postgrex.query!(conn, ~s|CREATE SCHEMA "#{schema}"|, [])
      on_exit(fn -> Postgrex.query(conn, ~s|DROP SCHEMA "#{schema}" CASCADE|, []) end)

      gdf = GeoDataFrame.new(DataFrame.new(%{~s|a"b| => [1]}), points(1), geometry_column: "geom")
      assert :ok = GeoFrame.to_postgis(gdf, conn, table, schema: schema)

      back =
        GeoFrame.read_postgis!(conn, ~s|SELECT * FROM "#{schema}"."we""ird; DROP TABLE x; --"|)

      assert ~s|a"b| in GeoDataFrame.names(back)
    end

    test "spatial_index creates a GiST index", %{conn: conn, table: table} do
      :ok = GeoFrame.to_postgis(frame(), conn, table, spatial_index: true)

      assert %{rows: [[1]]} =
               Postgrex.query!(
                 conn,
                 "SELECT count(*)::int FROM pg_indexes WHERE tablename = $1 AND indexdef ILIKE '%gist%'",
                 [table]
               )
    end

    test "no spatial index by default", %{conn: conn, table: table} do
      :ok = GeoFrame.to_postgis(frame(), conn, table)

      assert %{rows: [[0]]} =
               Postgrex.query!(
                 conn,
                 "SELECT count(*)::int FROM pg_indexes WHERE tablename = $1",
                 [table]
               )
    end

    test "dtype overrides a column type", %{conn: conn, table: table} do
      :ok = GeoFrame.to_postgis(frame(), conn, table, dtype: %{"id" => "smallint"})

      assert %{rows: [["smallint"]]} =
               Postgrex.query!(
                 conn,
                 "SELECT data_type FROM information_schema.columns WHERE table_name = $1 AND column_name = 'id'",
                 [table]
               )
    end

    test "unsafe dtype and bad options raise", %{conn: conn, table: table} do
      assert_raise ArgumentError, fn ->
        GeoFrame.to_postgis(frame(), conn, table, dtype: %{"id" => "int); DROP TABLE x; --"})
      end

      assert_raise ArgumentError, fn -> GeoFrame.to_postgis(frame(), conn, table, index: true) end
      assert_raise ArgumentError, fn -> GeoFrame.to_postgis(frame(), conn, "") end
    end

    test "stores the wider integer and decimal types", %{conn: conn, table: table} do
      data =
        DataFrame.new(
          big: Explorer.Series.from_list([1, 2], dtype: {:u, 64}),
          small: Explorer.Series.from_list([1, 2], dtype: {:s, 8}),
          real: Explorer.Series.from_list([1.5, 2.5], dtype: {:f, 32}),
          dt:
            Explorer.Series.from_list([~U[2024-01-01 00:00:00.000000Z], nil],
              dtype: {:datetime, :microsecond, "Etc/UTC"}
            )
        )

      gdf = GeoDataFrame.new(data, points(2), geometry_column: "geom")
      assert :ok = GeoFrame.to_postgis(gdf, conn, table)
      back = GeoFrame.read_postgis!(conn, ~s|SELECT * FROM "#{table}"|)
      assert Explorer.Series.to_list(back.data["dt"]) == [~U[2024-01-01 00:00:00.000000Z], nil]
    end
  end
end
