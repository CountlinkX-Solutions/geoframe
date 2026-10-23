# Known differences between GeoFrame and GeoPandas 1.2 / Shapely 2.2 (GEOS 3.14).
#
# Machine-readable list consumed by test/geo_frame/reference_test.exs. Each entry:
#
#   * `:op`     - the operation (as in the fixtures), or `:any` for a class that spans operations;
#   * `:cases`  - exact case ids ("label:a|b"); the suite asserts each one STILL differs, so a
#                 fixed deviation fails the build until the entry is removed;
#   * `:nullish` - (optional) matches every differing case that has a null or empty input;
#   * `:reason` - why GeoFrame differs;
#   * `:doc`    - the docs/parity.md section that explains it.
#
# Cases that differ and are not listed here fail the suite.
[
  %{
    reason:
      "Null and empty inputs: GeoFrame returns nil (and POINT EMPTY is not storable), or its own convention (count_geometries of an empty atomic geometry is 0), where GeoPandas returns False, 0, NaN or an empty geometry.",
    doc: "docs/parity.md#reference-testing",
    op: :any,
    cases: [],
    nullish: true
  },
  %{
    reason:
      "Mitre and bevel joins on the acute corner of a closed line differ between geo-buf and GEOS (the area differs by more than the 2 % tolerance). Round joins and caps agree on every case.",
    doc: "docs/parity.md#reference-testing",
    op: "buffer",
    cases: ["buffer(1.0, flat, mitre):line_closed", "buffer(1.0, square, bevel):line_closed"]
  },
  %{
    reason:
      "Predicates and DE-9IM on an invalid (self-intersecting bowtie) input follow geo's relate, which does not agree with GEOS there. Valid input agrees in every case.",
    doc: "docs/parity.md#reference-testing",
    op: "contains",
    cases: [
      "contains:bowtie|bowtie",
      "contains:bowtie|coll",
      "contains:bowtie|sq_corner",
      "contains:bowtie|sq_touch"
    ]
  },
  %{
    reason:
      "Predicates and DE-9IM on an invalid (self-intersecting bowtie) input follow geo's relate, which does not agree with GEOS there. Valid input agrees in every case.",
    doc: "docs/parity.md#reference-testing",
    op: "covered_by",
    cases: [
      "covered_by:bowtie|bowtie",
      "covered_by:sq_corner|bowtie",
      "covered_by:sq_touch|bowtie"
    ]
  },
  %{
    reason:
      "Predicates and DE-9IM on an invalid (self-intersecting bowtie) input follow geo's relate, which does not agree with GEOS there. Valid input agrees in every case.",
    doc: "docs/parity.md#reference-testing",
    op: "covers",
    cases: ["covers:bowtie|bowtie", "covers:bowtie|sq_corner", "covers:bowtie|sq_touch"]
  },
  %{
    reason:
      "A horizontal or vertical line (or a point) has a degenerate envelope: GeoFrame returns a line, GEOS 3.14 a zero-area polygon with repeated vertices, which is invalid.",
    doc: "docs/parity.md#reference-testing",
    op: "envelope",
    cases: ["envelope:line_cross", "envelope:line_dup", "envelope:line_h"]
  },
  %{
    reason:
      "Predicates and DE-9IM on an invalid (self-intersecting bowtie) input follow geo's relate, which does not agree with GEOS there. Valid input agrees in every case.",
    doc: "docs/parity.md#reference-testing",
    op: "geom_equals",
    cases: ["geom_equals:bowtie|bowtie"]
  },
  %{
    reason:
      "is_ring requires at least 4 vertices; GEOS accepts the degenerate closed two-point line LINESTRING (1 1, 1 1).",
    doc: "docs/parity.md#reference-testing",
    op: "is_ring",
    cases: ["is_ring:line_zero"]
  },
  %{
    reason:
      "geo's validation accepts a zero-area polygon ring (collinear vertices) that GEOS reports as a self-intersection, so is_valid is true and make_valid leaves it unchanged.",
    doc: "docs/parity.md#reference-testing",
    op: "is_valid",
    cases: ["is_valid:poly_flat"]
  },
  %{
    reason:
      "is_valid_reason uses geo's wording (first reason only), not GEOS' text and coordinates.",
    doc: "docs/parity.md#reference-testing",
    op: "is_valid_reason",
    cases: ["is_valid_reason:bowtie", "is_valid_reason:line_zero"]
  },
  %{
    reason:
      "geo's validation accepts a zero-area polygon ring (collinear vertices) that GEOS reports as a self-intersection, so is_valid is true and make_valid leaves it unchanged.",
    doc: "docs/parity.md#reference-testing",
    op: "is_valid_reason",
    cases: ["is_valid_reason:poly_flat"]
  },
  %{
    reason:
      "line_merge on a polygon: GeoFrame gives an empty collection for non-lines; GEOS merges the polygon's boundary.",
    doc: "docs/parity.md#reference-testing",
    op: "line_merge",
    cases: ["line_merge:sq"]
  },
  %{
    reason:
      "geo's validation accepts a zero-area polygon ring (collinear vertices) that GEOS reports as a self-intersection, so is_valid is true and make_valid leaves it unchanged.",
    doc: "docs/parity.md#reference-testing",
    op: "make_valid",
    cases: ["make_valid:poly_flat"]
  },
  %{
    reason:
      "Predicates and DE-9IM on an invalid (self-intersecting bowtie) input follow geo's relate, which does not agree with GEOS there. Valid input agrees in every case.",
    doc: "docs/parity.md#reference-testing",
    op: "overlaps",
    cases: ["overlaps:bowtie|bowtie"]
  },
  %{
    reason:
      "Predicates and DE-9IM on an invalid (self-intersecting bowtie) input follow geo's relate, which does not agree with GEOS there. Valid input agrees in every case.",
    doc: "docs/parity.md#reference-testing",
    op: "relate",
    cases: [
      "relate:bowtie|bowtie",
      "relate:bowtie|coll",
      "relate:bowtie|mline",
      "relate:coll|bowtie",
      "relate:mline|bowtie"
    ]
  },
  %{
    reason:
      "Predicates and DE-9IM on an invalid (self-intersecting bowtie) input follow geo's relate, which does not agree with GEOS there. Valid input agrees in every case.",
    doc: "docs/parity.md#reference-testing",
    op: "relate_pattern",
    cases: ["relate_pattern(T*****FF*):bowtie|bowtie", "relate_pattern(T*****FF*):bowtie|coll"]
  },
  %{
    reason:
      "Predicates and DE-9IM on an invalid (self-intersecting bowtie) input follow geo's relate, which does not agree with GEOS there. Valid input agrees in every case.",
    doc: "docs/parity.md#reference-testing",
    op: "touches",
    cases: ["touches:bowtie|coll", "touches:coll|bowtie"]
  },
  %{
    reason:
      "Predicates and DE-9IM on an invalid (self-intersecting bowtie) input follow geo's relate, which does not agree with GEOS there. Valid input agrees in every case.",
    doc: "docs/parity.md#reference-testing",
    op: "within",
    cases: [
      "within:bowtie|bowtie",
      "within:coll|bowtie",
      "within:sq_corner|bowtie",
      "within:sq_touch|bowtie"
    ]
  }
]
