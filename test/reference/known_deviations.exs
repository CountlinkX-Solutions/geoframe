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
    nullish: true,
    cases: []
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
      "Geometry combination not implemented by the set operations (line x line, mixed dimensions, point x polygon for union and symmetric_difference, mixed-family union_all): GeoFrame returns nil. Scheduled for batch B.",
    doc: "docs/parity.md#reference-testing",
    op: "difference",
    cases: [
      "difference:line_cross|line_cross",
      "difference:line_cross|line_diag",
      "difference:line_cross|line_h",
      "difference:line_diag|line_cross",
      "difference:line_diag|line_diag",
      "difference:line_diag|line_h",
      "difference:line_h|line_cross",
      "difference:line_h|line_diag",
      "difference:line_h|line_h"
    ]
  },
  %{
    reason:
      "Geometry combination not implemented by the set operations (line x line, mixed dimensions, point x polygon for union and symmetric_difference, mixed-family union_all): GeoFrame returns nil. Scheduled for batch B.",
    doc: "docs/parity.md#reference-testing",
    op: "dissolve",
    cases: ["dissolve(by=nil,sum)"]
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
      "Lower-dimensional parts of an overlay result are dropped (polygons sharing only an edge or a vertex intersect in POLYGON EMPTY; GEOS returns the shared LINESTRING/POINT or a GEOMETRYCOLLECTION with them). Scheduled for batch B.",
    doc: "docs/parity.md#reference-testing",
    op: "intersection",
    cases: [
      "intersection:ell|sq_overlap",
      "intersection:mpoly|sq_overlap",
      "intersection:sq_corner|sq",
      "intersection:sq_corner|sq_touch",
      "intersection:sq_hole|sq",
      "intersection:sq_hole|sq_overlap",
      "intersection:sq_overlap|ell",
      "intersection:sq_overlap|mpoly",
      "intersection:sq_overlap|sq_hole",
      "intersection:sq_touch|sq",
      "intersection:sq_touch|sq_corner",
      "intersection:sq|sq_corner",
      "intersection:sq|sq_hole",
      "intersection:sq|sq_touch"
    ]
  },
  %{
    reason:
      "Geometry combination not implemented by the set operations (line x line, mixed dimensions, point x polygon for union and symmetric_difference, mixed-family union_all): GeoFrame returns nil. Scheduled for batch B.",
    doc: "docs/parity.md#reference-testing",
    op: "intersection",
    cases: [
      "intersection:line_cross|line_cross",
      "intersection:line_cross|line_diag",
      "intersection:line_cross|line_h",
      "intersection:line_diag|line_cross",
      "intersection:line_diag|line_diag",
      "intersection:line_diag|line_h",
      "intersection:line_h|line_cross",
      "intersection:line_h|line_diag",
      "intersection:line_h|line_h"
    ]
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
      "Geometry combination not implemented by the set operations (line x line, mixed dimensions, point x polygon for union and symmetric_difference, mixed-family union_all): GeoFrame returns nil. Scheduled for batch B.",
    doc: "docs/parity.md#reference-testing",
    op: "symmetric_difference",
    cases: [
      "symmetric_difference:ell|line_cross",
      "symmetric_difference:ell|line_diag",
      "symmetric_difference:ell|line_h",
      "symmetric_difference:ell|mpt",
      "symmetric_difference:ell|pt1",
      "symmetric_difference:ell|pt_edge",
      "symmetric_difference:line_cross|ell",
      "symmetric_difference:line_cross|line_cross",
      "symmetric_difference:line_cross|line_diag",
      "symmetric_difference:line_cross|line_h",
      "symmetric_difference:line_cross|mpoly",
      "symmetric_difference:line_cross|mpt",
      "symmetric_difference:line_cross|pt1",
      "symmetric_difference:line_cross|pt_edge",
      "symmetric_difference:line_cross|sq",
      "symmetric_difference:line_cross|sq_corner",
      "symmetric_difference:line_cross|sq_hole",
      "symmetric_difference:line_cross|sq_inner",
      "symmetric_difference:line_cross|sq_overlap",
      "symmetric_difference:line_cross|sq_touch",
      "symmetric_difference:line_cross|tri",
      "symmetric_difference:line_diag|ell",
      "symmetric_difference:line_diag|line_cross",
      "symmetric_difference:line_diag|line_diag",
      "symmetric_difference:line_diag|line_h",
      "symmetric_difference:line_diag|mpoly",
      "symmetric_difference:line_diag|mpt",
      "symmetric_difference:line_diag|pt1",
      "symmetric_difference:line_diag|pt_edge",
      "symmetric_difference:line_diag|sq",
      "symmetric_difference:line_diag|sq_corner",
      "symmetric_difference:line_diag|sq_hole",
      "symmetric_difference:line_diag|sq_inner",
      "symmetric_difference:line_diag|sq_overlap",
      "symmetric_difference:line_diag|sq_touch",
      "symmetric_difference:line_diag|tri",
      "symmetric_difference:line_h|ell",
      "symmetric_difference:line_h|line_cross",
      "symmetric_difference:line_h|line_diag",
      "symmetric_difference:line_h|line_h",
      "symmetric_difference:line_h|mpoly",
      "symmetric_difference:line_h|mpt",
      "symmetric_difference:line_h|pt1",
      "symmetric_difference:line_h|pt_edge",
      "symmetric_difference:line_h|sq",
      "symmetric_difference:line_h|sq_corner",
      "symmetric_difference:line_h|sq_hole",
      "symmetric_difference:line_h|sq_inner",
      "symmetric_difference:line_h|sq_overlap",
      "symmetric_difference:line_h|sq_touch",
      "symmetric_difference:line_h|tri",
      "symmetric_difference:mpoly|line_cross",
      "symmetric_difference:mpoly|line_diag",
      "symmetric_difference:mpoly|line_h",
      "symmetric_difference:mpoly|mpt",
      "symmetric_difference:mpoly|pt1",
      "symmetric_difference:mpoly|pt_edge",
      "symmetric_difference:mpt|ell",
      "symmetric_difference:mpt|line_cross",
      "symmetric_difference:mpt|line_diag",
      "symmetric_difference:mpt|line_h",
      "symmetric_difference:mpt|mpoly",
      "symmetric_difference:mpt|sq",
      "symmetric_difference:mpt|sq_corner",
      "symmetric_difference:mpt|sq_hole",
      "symmetric_difference:mpt|sq_inner",
      "symmetric_difference:mpt|sq_overlap",
      "symmetric_difference:mpt|sq_touch",
      "symmetric_difference:mpt|tri",
      "symmetric_difference:pt1|ell",
      "symmetric_difference:pt1|line_cross",
      "symmetric_difference:pt1|line_diag",
      "symmetric_difference:pt1|line_h",
      "symmetric_difference:pt1|mpoly",
      "symmetric_difference:pt1|sq",
      "symmetric_difference:pt1|sq_corner",
      "symmetric_difference:pt1|sq_hole",
      "symmetric_difference:pt1|sq_inner",
      "symmetric_difference:pt1|sq_overlap",
      "symmetric_difference:pt1|sq_touch",
      "symmetric_difference:pt1|tri",
      "symmetric_difference:pt_edge|ell",
      "symmetric_difference:pt_edge|line_cross",
      "symmetric_difference:pt_edge|line_diag",
      "symmetric_difference:pt_edge|line_h",
      "symmetric_difference:pt_edge|mpoly",
      "symmetric_difference:pt_edge|sq",
      "symmetric_difference:pt_edge|sq_corner",
      "symmetric_difference:pt_edge|sq_hole",
      "symmetric_difference:pt_edge|sq_inner",
      "symmetric_difference:pt_edge|sq_overlap",
      "symmetric_difference:pt_edge|sq_touch",
      "symmetric_difference:pt_edge|tri",
      "symmetric_difference:sq_corner|line_cross",
      "symmetric_difference:sq_corner|line_diag",
      "symmetric_difference:sq_corner|line_h",
      "symmetric_difference:sq_corner|mpt",
      "symmetric_difference:sq_corner|pt1",
      "symmetric_difference:sq_corner|pt_edge",
      "symmetric_difference:sq_hole|line_cross",
      "symmetric_difference:sq_hole|line_diag",
      "symmetric_difference:sq_hole|line_h",
      "symmetric_difference:sq_hole|mpt",
      "symmetric_difference:sq_hole|pt1",
      "symmetric_difference:sq_hole|pt_edge",
      "symmetric_difference:sq_inner|line_cross",
      "symmetric_difference:sq_inner|line_diag",
      "symmetric_difference:sq_inner|line_h",
      "symmetric_difference:sq_inner|mpt",
      "symmetric_difference:sq_inner|pt1",
      "symmetric_difference:sq_inner|pt_edge",
      "symmetric_difference:sq_overlap|line_cross",
      "symmetric_difference:sq_overlap|line_diag",
      "symmetric_difference:sq_overlap|line_h",
      "symmetric_difference:sq_overlap|mpt",
      "symmetric_difference:sq_overlap|pt1",
      "symmetric_difference:sq_overlap|pt_edge",
      "symmetric_difference:sq_touch|line_cross",
      "symmetric_difference:sq_touch|line_diag",
      "symmetric_difference:sq_touch|line_h",
      "symmetric_difference:sq_touch|mpt",
      "symmetric_difference:sq_touch|pt1",
      "symmetric_difference:sq_touch|pt_edge",
      "symmetric_difference:sq|line_cross",
      "symmetric_difference:sq|line_diag",
      "symmetric_difference:sq|line_h",
      "symmetric_difference:sq|mpt",
      "symmetric_difference:sq|pt1",
      "symmetric_difference:sq|pt_edge",
      "symmetric_difference:tri|line_cross",
      "symmetric_difference:tri|line_diag",
      "symmetric_difference:tri|line_h",
      "symmetric_difference:tri|mpt",
      "symmetric_difference:tri|pt1",
      "symmetric_difference:tri|pt_edge"
    ]
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
      "Geometry combination not implemented by the set operations (line x line, mixed dimensions, point x polygon for union and symmetric_difference, mixed-family union_all): GeoFrame returns nil. Scheduled for batch B.",
    doc: "docs/parity.md#reference-testing",
    op: "union",
    cases: [
      "union:ell|line_cross",
      "union:ell|line_diag",
      "union:ell|line_h",
      "union:ell|mpt",
      "union:ell|pt1",
      "union:ell|pt_edge",
      "union:line_cross|ell",
      "union:line_cross|line_cross",
      "union:line_cross|line_diag",
      "union:line_cross|line_h",
      "union:line_cross|mpoly",
      "union:line_cross|mpt",
      "union:line_cross|pt1",
      "union:line_cross|pt_edge",
      "union:line_cross|sq",
      "union:line_cross|sq_corner",
      "union:line_cross|sq_hole",
      "union:line_cross|sq_inner",
      "union:line_cross|sq_overlap",
      "union:line_cross|sq_touch",
      "union:line_cross|tri",
      "union:line_diag|ell",
      "union:line_diag|line_cross",
      "union:line_diag|line_diag",
      "union:line_diag|line_h",
      "union:line_diag|mpoly",
      "union:line_diag|mpt",
      "union:line_diag|pt1",
      "union:line_diag|pt_edge",
      "union:line_diag|sq",
      "union:line_diag|sq_corner",
      "union:line_diag|sq_hole",
      "union:line_diag|sq_inner",
      "union:line_diag|sq_overlap",
      "union:line_diag|sq_touch",
      "union:line_diag|tri",
      "union:line_h|ell",
      "union:line_h|line_cross",
      "union:line_h|line_diag",
      "union:line_h|line_h",
      "union:line_h|mpoly",
      "union:line_h|mpt",
      "union:line_h|pt1",
      "union:line_h|pt_edge",
      "union:line_h|sq",
      "union:line_h|sq_corner",
      "union:line_h|sq_hole",
      "union:line_h|sq_inner",
      "union:line_h|sq_overlap",
      "union:line_h|sq_touch",
      "union:line_h|tri",
      "union:mpoly|line_cross",
      "union:mpoly|line_diag",
      "union:mpoly|line_h",
      "union:mpoly|mpt",
      "union:mpoly|pt1",
      "union:mpoly|pt_edge",
      "union:mpt|ell",
      "union:mpt|line_cross",
      "union:mpt|line_diag",
      "union:mpt|line_h",
      "union:mpt|mpoly",
      "union:mpt|sq",
      "union:mpt|sq_corner",
      "union:mpt|sq_hole",
      "union:mpt|sq_inner",
      "union:mpt|sq_overlap",
      "union:mpt|sq_touch",
      "union:mpt|tri",
      "union:pt1|ell",
      "union:pt1|line_cross",
      "union:pt1|line_diag",
      "union:pt1|line_h",
      "union:pt1|mpoly",
      "union:pt1|sq",
      "union:pt1|sq_corner",
      "union:pt1|sq_hole",
      "union:pt1|sq_inner",
      "union:pt1|sq_overlap",
      "union:pt1|sq_touch",
      "union:pt1|tri",
      "union:pt_edge|ell",
      "union:pt_edge|line_cross",
      "union:pt_edge|line_diag",
      "union:pt_edge|line_h",
      "union:pt_edge|mpoly",
      "union:pt_edge|sq",
      "union:pt_edge|sq_corner",
      "union:pt_edge|sq_hole",
      "union:pt_edge|sq_inner",
      "union:pt_edge|sq_overlap",
      "union:pt_edge|sq_touch",
      "union:pt_edge|tri",
      "union:sq_corner|line_cross",
      "union:sq_corner|line_diag",
      "union:sq_corner|line_h",
      "union:sq_corner|mpt",
      "union:sq_corner|pt1",
      "union:sq_corner|pt_edge",
      "union:sq_hole|line_cross",
      "union:sq_hole|line_diag",
      "union:sq_hole|line_h",
      "union:sq_hole|mpt",
      "union:sq_hole|pt1",
      "union:sq_hole|pt_edge",
      "union:sq_inner|line_cross",
      "union:sq_inner|line_diag",
      "union:sq_inner|line_h",
      "union:sq_inner|mpt",
      "union:sq_inner|pt1",
      "union:sq_inner|pt_edge",
      "union:sq_overlap|line_cross",
      "union:sq_overlap|line_diag",
      "union:sq_overlap|line_h",
      "union:sq_overlap|mpt",
      "union:sq_overlap|pt1",
      "union:sq_overlap|pt_edge",
      "union:sq_touch|line_cross",
      "union:sq_touch|line_diag",
      "union:sq_touch|line_h",
      "union:sq_touch|mpt",
      "union:sq_touch|pt1",
      "union:sq_touch|pt_edge",
      "union:sq|line_cross",
      "union:sq|line_diag",
      "union:sq|line_h",
      "union:sq|mpt",
      "union:sq|pt1",
      "union:sq|pt_edge",
      "union:tri|line_cross",
      "union:tri|line_diag",
      "union:tri|line_h",
      "union:tri|mpt",
      "union:tri|pt1",
      "union:tri|pt_edge"
    ]
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
