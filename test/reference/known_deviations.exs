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
# Z/M (file zm.json): one entry per (reason, op), generated from the case lists below.
zm_cases = fn labels_ops, ids ->
  for {label, op} <- labels_ops, do: {op, Enum.map(ids, &"#{label}:#{&1}")}
end

all_zm = [
  {"has_z", "has_z"},
  {"has_m", "has_m"},
  {"z", "z"},
  {"m", "m"},
  {"to_wkb_hex(iso)", "to_wkb_hex"},
  {"to_wkt_zm", "to_wkt_zm"},
  {"force_2d", "force_2d"},
  {"force_3d(0.0)", "force_3d"},
  {"force_3d(7.5)", "force_3d"}
]

one_member = Enum.reject(all_zm, fn {label, _} -> label in ["has_z", "has_m", "m"] end)

zm_deviations =
  Enum.map(zm_cases.(all_zm, ["z_coll_mixed"]), fn {op, cases} ->
    %{
      reason:
        "A GEOMETRYCOLLECTION whose members have different dimensions (here POINT and POINT Z) is rejected with an error: the GeoArrow builder cannot store it. Shapely keeps it and reports Z for the whole collection.",
      doc: "docs/parity.md#z-and-m",
      op: op,
      cases: cases
    }
  end) ++
    Enum.map(zm_cases.(one_member, ["z_coll_one"]), fn {op, cases} ->
      %{
        reason:
          "A one-member GEOMETRYCOLLECTION is stored as that member when parsed (see geom_type), so z/force_*/to_wkb/to_wkt describe the member where Shapely keeps the collection.",
        doc: "docs/parity.md#z-and-m",
        op: op,
        cases: cases
      }
    end) ++
    Enum.map(
      zm_cases.(
        [{"force_3d(0.0)", "force_3d"}, {"force_3d(7.5)", "force_3d"}],
        ~w(z_pt_zm z_line_zm z_poly_zm z_mpt_zm z_mpoly_zm z_coll_zm)
      ),
      fn {op, cases} ->
        %{
          reason:
            "force_3d of an XYZM geometry: Shapely 2.2 (GEOS 3.14) drops M and overwrites the existing Z with the fill value; GeoFrame keeps the existing Z (as for XYZ) and drops M.",
          doc: "docs/parity.md#z-and-m",
          op: op,
          cases: cases
        }
      end
    )

# Z/M constructive operations (file zm_ops.json): one entry per (reason, op).
zm_ops_classes = [
  {"mixed", "affine_transform",
   [
     "affine_transform(12):z_coll_mixed",
     "affine_transform(6):z_coll_mixed"
   ]},
  {"mixed", "boundary",
   [
     "boundary:z_coll_mixed"
   ]},
  {"mixed", "exterior",
   [
     "exterior:z_coll_mixed"
   ]},
  {"mixed", "get_geometry",
   [
     "get_geometry(0):z_coll_mixed",
     "get_geometry(1):z_coll_mixed"
   ]},
  {"mixed", "normalize",
   [
     "normalize:z_coll_mixed"
   ]},
  {"mixed", "reverse",
   [
     "reverse:z_coll_mixed"
   ]},
  {"mixed", "rotate",
   [
     "rotate(30,center):z_coll_mixed",
     "rotate(90,(0,0)):z_coll_mixed"
   ]},
  {"mixed", "scale",
   [
     "scale(-1,3,centroid):z_coll_mixed",
     "scale(2,0.5,zfact=3,center):z_coll_mixed",
     "scale(2,0.5,zfact=4,(0,0)):z_coll_mixed"
   ]},
  {"mixed", "set_precision",
   [
     "set_precision(0.5, keep_collapsed):z_coll_mixed",
     "set_precision(0.5, pointwise):z_coll_mixed",
     "set_precision(0.5, valid_output):z_coll_mixed",
     "set_precision(1.0, keep_collapsed):z_coll_mixed",
     "set_precision(1.0, pointwise):z_coll_mixed",
     "set_precision(1.0, valid_output):z_coll_mixed"
   ]},
  {"mixed", "simplify",
   [
     "simplify(1.0, preserve_topology=false):z_coll_mixed"
   ]},
  {"mixed", "skew",
   [
     "skew(20,10,center):z_coll_mixed"
   ]},
  {"mixed", "translate",
   [
     "translate(2.5,-3):z_coll_mixed",
     "translate(2.5,-3,zoff=3):z_coll_mixed"
   ]},
  {"one", "affine_transform",
   [
     "affine_transform(12):z_coll_one",
     "affine_transform(6):z_coll_one"
   ]},
  {"one", "boundary",
   [
     "boundary:z_coll_one"
   ]},
  {"one", "normalize",
   [
     "normalize:z_coll_one"
   ]},
  {"one", "reverse",
   [
     "reverse:z_coll_one"
   ]},
  {"one", "rotate",
   [
     "rotate(30,center):z_coll_one",
     "rotate(90,(0,0)):z_coll_one"
   ]},
  {"one", "scale",
   [
     "scale(-1,3,centroid):z_coll_one",
     "scale(2,0.5,zfact=3,center):z_coll_one",
     "scale(2,0.5,zfact=4,(0,0)):z_coll_one"
   ]},
  {"one", "set_precision",
   [
     "set_precision(0.5, keep_collapsed):z_coll_one",
     "set_precision(0.5, pointwise):z_coll_one",
     "set_precision(0.5, valid_output):z_coll_one",
     "set_precision(1.0, keep_collapsed):z_coll_one",
     "set_precision(1.0, pointwise):z_coll_one",
     "set_precision(1.0, valid_output):z_coll_one"
   ]},
  {"one", "simplify",
   [
     "simplify(1.0, preserve_topology=false):z_coll_one"
   ]},
  {"one", "skew",
   [
     "skew(20,10,center):z_coll_one"
   ]},
  {"one", "translate",
   [
     "translate(2.5,-3):z_coll_one",
     "translate(2.5,-3,zoff=3):z_coll_one"
   ]},
  {"zmnan", "affine_transform",
   [
     "affine_transform(12):z2_line_zm",
     "affine_transform(12):z2_poly_cw_zm",
     "affine_transform(12):z2_zigzag_zm",
     "affine_transform(12):z_coll_zm",
     "affine_transform(12):z_line_zm",
     "affine_transform(12):z_mpoly_zm",
     "affine_transform(12):z_mpt_zm",
     "affine_transform(12):z_poly_zm",
     "affine_transform(12):z_pt_zm",
     "affine_transform(6):z2_line_zm",
     "affine_transform(6):z2_poly_cw_zm",
     "affine_transform(6):z2_zigzag_zm",
     "affine_transform(6):z_coll_zm",
     "affine_transform(6):z_line_zm",
     "affine_transform(6):z_mpoly_zm",
     "affine_transform(6):z_mpt_zm",
     "affine_transform(6):z_poly_zm",
     "affine_transform(6):z_pt_zm"
   ]},
  {"zmnan", "rotate",
   [
     "rotate(30,center):z2_line_zm",
     "rotate(30,center):z2_poly_cw_zm",
     "rotate(30,center):z2_zigzag_zm",
     "rotate(30,center):z_coll_zm",
     "rotate(30,center):z_line_zm",
     "rotate(30,center):z_mpoly_zm",
     "rotate(30,center):z_mpt_zm",
     "rotate(30,center):z_poly_zm",
     "rotate(30,center):z_pt_zm",
     "rotate(90,(0,0)):z2_line_zm",
     "rotate(90,(0,0)):z2_poly_cw_zm",
     "rotate(90,(0,0)):z2_zigzag_zm",
     "rotate(90,(0,0)):z_coll_zm",
     "rotate(90,(0,0)):z_line_zm",
     "rotate(90,(0,0)):z_mpoly_zm",
     "rotate(90,(0,0)):z_mpt_zm",
     "rotate(90,(0,0)):z_poly_zm",
     "rotate(90,(0,0)):z_pt_zm"
   ]},
  {"zmnan", "scale",
   [
     "scale(-1,3,centroid):z2_line_zm",
     "scale(-1,3,centroid):z2_poly_cw_zm",
     "scale(-1,3,centroid):z2_zigzag_zm",
     "scale(-1,3,centroid):z_coll_zm",
     "scale(-1,3,centroid):z_line_zm",
     "scale(-1,3,centroid):z_mpoly_zm",
     "scale(-1,3,centroid):z_mpt_zm",
     "scale(-1,3,centroid):z_poly_zm",
     "scale(-1,3,centroid):z_pt_zm",
     "scale(2,0.5,zfact=3,center):z2_line_zm",
     "scale(2,0.5,zfact=3,center):z2_poly_cw_zm",
     "scale(2,0.5,zfact=3,center):z2_zigzag_zm",
     "scale(2,0.5,zfact=3,center):z_coll_zm",
     "scale(2,0.5,zfact=3,center):z_line_zm",
     "scale(2,0.5,zfact=3,center):z_mpoly_zm",
     "scale(2,0.5,zfact=3,center):z_mpt_zm",
     "scale(2,0.5,zfact=3,center):z_poly_zm",
     "scale(2,0.5,zfact=3,center):z_pt_zm",
     "scale(2,0.5,zfact=4,(0,0)):z2_line_zm",
     "scale(2,0.5,zfact=4,(0,0)):z2_poly_cw_zm",
     "scale(2,0.5,zfact=4,(0,0)):z2_zigzag_zm",
     "scale(2,0.5,zfact=4,(0,0)):z_coll_zm",
     "scale(2,0.5,zfact=4,(0,0)):z_line_zm",
     "scale(2,0.5,zfact=4,(0,0)):z_mpoly_zm",
     "scale(2,0.5,zfact=4,(0,0)):z_mpt_zm",
     "scale(2,0.5,zfact=4,(0,0)):z_poly_zm",
     "scale(2,0.5,zfact=4,(0,0)):z_pt_zm"
   ]},
  {"zmnan", "skew",
   [
     "skew(20,10,center):z2_line_zm",
     "skew(20,10,center):z2_poly_cw_zm",
     "skew(20,10,center):z2_zigzag_zm",
     "skew(20,10,center):z_coll_zm",
     "skew(20,10,center):z_line_zm",
     "skew(20,10,center):z_mpoly_zm",
     "skew(20,10,center):z_mpt_zm",
     "skew(20,10,center):z_poly_zm",
     "skew(20,10,center):z_pt_zm"
   ]},
  {"zmnan", "to_crs",
   [
     "to_crs(4326->EPSG:32617):z2_ll_mpt_zm",
     "to_crs(4326->EPSG:32617):z2_ll_pt_zm",
     "to_crs(4326->EPSG:3857):z2_ll_mpt_zm",
     "to_crs(4326->EPSG:3857):z2_ll_pt_zm",
     "to_crs(4979->4978):z2_ll_mpt_zm",
     "to_crs(4979->4978):z2_ll_pt_zm"
   ]},
  {"zmnan", "translate",
   [
     "translate(2.5,-3):z2_line_zm",
     "translate(2.5,-3):z2_poly_cw_zm",
     "translate(2.5,-3):z2_zigzag_zm",
     "translate(2.5,-3):z_coll_zm",
     "translate(2.5,-3):z_line_zm",
     "translate(2.5,-3):z_mpoly_zm",
     "translate(2.5,-3):z_mpt_zm",
     "translate(2.5,-3):z_poly_zm",
     "translate(2.5,-3):z_pt_zm",
     "translate(2.5,-3,zoff=3):z2_line_zm",
     "translate(2.5,-3,zoff=3):z2_poly_cw_zm",
     "translate(2.5,-3,zoff=3):z2_zigzag_zm",
     "translate(2.5,-3,zoff=3):z_coll_zm",
     "translate(2.5,-3,zoff=3):z_line_zm",
     "translate(2.5,-3,zoff=3):z_mpoly_zm",
     "translate(2.5,-3,zoff=3):z_mpt_zm",
     "translate(2.5,-3,zoff=3):z_poly_zm",
     "translate(2.5,-3,zoff=3):z_pt_zm"
   ]}
]

zm_ops_reasons = %{
  "mixed" =>
    "A GEOMETRYCOLLECTION whose members have different dimensions (POINT and POINT Z) is rejected with an error: the GeoArrow builder cannot store it. Shapely keeps it.",
  "one" =>
    "A one-member GEOMETRYCOLLECTION is stored as that member when parsed (see geom_type), so the result describes the member where Shapely keeps the collection.",
  "zmnan" =>
    "Shapely 2.2 (GEOS 3.14) affine and transform functions turn an XYZM geometry into XYZM with NaN in both Z and M (it applies the matrix to a 4-column coordinate array). GeoFrame keeps Z (transformed as for XYZ) and drops M, which is what the operation is defined to do."
}

zm_ops_deviations =
  Enum.map(zm_ops_classes, fn {kind, op, cases} ->
    %{
      reason: Map.fetch!(zm_ops_reasons, kind),
      doc: "docs/parity.md#z-and-m",
      op: op,
      cases: cases
    }
  end)

# Z/M interpolation and overlay operations (file zm_overlay.json).
zm_overlay_reasons = %{
  shared:
    "Two inputs with a vertex at the same XY and different ordinates (a shared edge or corner, identical rings, a line vertex on a polygon vertex): which one GEOS keeps follows its noding order (each output ring can take the other input's value at a touching corner, and the start of an input ring loses its M). GeoFrame takes the first input's value for an intersection and the second's for the other operations, the more frequent choice over random inputs.",
  clip:
    "clip_by_rect of a polygon whose edge leaves and re-enters the rectangle: GEOS gives the vertices it adds on the rectangle boundary a Z from its own walk along the boundary (here 60 for 56.67); GeoFrame interpolates along the clipped edge. Lines, points and polygons with every edge crossing once agree."
}

zm_overlay_deviations = [
  %{
    reason: zm_overlay_reasons.shared,
    doc: "docs/parity.md#z-and-m",
    op: "union",
    cases: [
      "union[conflicts]:z3_a_m|z3_a_zm",
      "union[conflicts]:z3_a_zm|z3_a_m",
      "union[conflicts]:z3_adj|z3_a",
      "union[conflicts]:z3_a|z3_a_m",
      "union[conflicts]:z3_a|z3_a_zm",
      "union[conflicts]:z3_a|z3_adj",
      "union[conflicts]:z3_a|z3_corner",
      "union[conflicts]:z3_a|z3_l1",
      "union[conflicts]:z3_a|z3_l2",
      "union[conflicts]:z3_a|z3_same",
      "union[conflicts]:z3_corner|z3_a",
      "union[conflicts]:z3_l1|z3_a",
      "union[conflicts]:z3_l2|z3_a",
      "union[conflicts]:z3_same|z3_a"
    ]
  },
  %{
    reason: zm_overlay_reasons.shared,
    doc: "docs/parity.md#z-and-m",
    op: "intersection",
    cases: [
      "intersection[conflicts]:z3_a_m|z3_a_zm",
      "intersection[conflicts]:z3_a_zm|z3_a_m",
      "intersection[conflicts]:z3_a|z3_a_m",
      "intersection[conflicts]:z3_a|z3_a_zm",
      "intersection[conflicts]:z3_a|z3_l1",
      "intersection[conflicts]:z3_a|z3_l2",
      "intersection[conflicts]:z3_a|z3_same",
      "intersection[conflicts]:z3_corner|z3_a",
      "intersection[conflicts]:z3_l1|z3_a",
      "intersection[conflicts]:z3_l2|z3_a",
      "intersection[conflicts]:z3_same|z3_a"
    ]
  },
  %{
    reason: zm_overlay_reasons.shared,
    doc: "docs/parity.md#z-and-m",
    op: "symmetric_difference",
    cases: [
      "symmetric_difference[conflicts]:z3_adj|z3_a",
      "symmetric_difference[conflicts]:z3_a|z3_adj",
      "symmetric_difference[conflicts]:z3_a|z3_corner",
      "symmetric_difference[conflicts]:z3_a|z3_l1",
      "symmetric_difference[conflicts]:z3_a|z3_l2",
      "symmetric_difference[conflicts]:z3_corner|z3_a",
      "symmetric_difference[conflicts]:z3_l1|z3_a",
      "symmetric_difference[conflicts]:z3_l2|z3_a"
    ]
  },
  %{
    reason: zm_overlay_reasons.shared,
    doc: "docs/parity.md#z-and-m",
    op: "difference",
    cases: [
      "difference[conflicts]:z3_adj|z3_a",
      "difference[conflicts]:z3_a|z3_adj",
      "difference[conflicts]:z3_a|z3_l1",
      "difference[conflicts]:z3_a|z3_l2"
    ]
  },
  %{
    reason: zm_overlay_reasons.shared,
    doc: "docs/parity.md#z-and-m",
    op: "union_all",
    cases: [
      "union_all:z3_a|z3_a_zm"
    ]
  },
  %{
    reason: zm_overlay_reasons.shared,
    doc: "docs/parity.md#z-and-m",
    op: "intersection_all",
    cases: [
      "intersection_all:z3_a|z3_a_zm"
    ]
  },
  %{
    reason: zm_overlay_reasons.clip,
    doc: "docs/parity.md#z-and-m",
    op: "clip_by_rect",
    cases: [
      "clip_by_rect([2, -1, 12, 5]):z3_tri"
    ]
  }
]

zm_deviations ++
  zm_ops_deviations ++
  zm_overlay_deviations ++
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
        "buffer with single_sided on a polygon with a hole: GEOS labels the one-sided ring curves so that the result is the polygon itself for a positive distance and the ordinary negative buffer for a negative one, where GeoFrame (and GEOS for a polygon without holes) returns the band between the polygon and its buffer.",
      doc: "docs/parity.md#reference-testing",
      op: "buffer",
      cases: [
        "buffer(1.0, single_sided, round):sq_hole",
        "buffer(1.0, single_sided, mitre):sq_hole",
        "buffer(1.0, single_sided, bevel):sq_hole",
        "buffer(-1.0, single_sided, round):sq_hole",
        "buffer(-1.0, single_sided, bevel):sq_hole"
      ]
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
        "is_valid_reason, several errors of one class: GEOS reports the one its monotone-chain traversal meets first, GeoFrame the first in ring and segment order. The text agrees, the reported coordinate differs.",
      doc: "docs/parity.md#reference-testing",
      op: "is_valid_reason",
      cases: [
        "is_valid_reason:vr_hole_touch2",
        "is_valid_reason:vr_dup_rings",
        "is_valid_reason:vr_mp_overlap",
        "is_valid_reason:vr_holes_cross"
      ]
    },
    %{
      reason:
        "is_valid_reason, a spike that is also a repeated vertex: GEOS reports the overlap (Self-intersection), GeoFrame the first finding in ring order (Ring Self-intersection).",
      doc: "docs/parity.md#reference-testing",
      op: "is_valid_reason",
      cases: ["is_valid_reason:vr_spike"]
    },
    %{
      reason:
        "is_valid_reason, a hole ring that disconnects the interior by touching the shell at two or more points: geo's validation (and so is_valid) accepts it, GEOS reports Interior is disconnected.",
      doc: "docs/parity.md#reference-testing",
      op: "is_valid_reason",
      cases: ["is_valid_reason:vr_disc2", "is_valid_reason:vr_shell_hole_touch_two"]
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
    },
    %{
      reason:
        "Concave hull of near-symmetric points (the two rings of e_ring, rounded to 3 decimals): many Delaunay edges have exactly equal length, and the order in which GEOS erodes tied border triangles is not reproducible. All other cases (including the other E point sets) agree.",
      doc: "docs/parity.md#batch-e-constructive-geometry",
      op: "concave_hull",
      cases: [
        "concave_hull(ratio=0.0, allow_holes=false):e_ring",
        "concave_hull(ratio=0.0, allow_holes=true):e_ring"
      ]
    },
    %{
      reason:
        "A triangle (and any shape whose hull has several equal-area edge-aligned rectangles, such as the symmetric mpoly and e_star) has more than one minimum-area rotated rectangle; geo and GEOS break the tie differently. The rectangle has the same area either way.",
      doc: "docs/parity.md#batch-e-constructive-geometry",
      op: "minimum_rotated_rectangle",
      cases: [
        "minimum_rotated_rectangle:line_closed",
        "minimum_rotated_rectangle:tri",
        "minimum_rotated_rectangle:mpoly",
        "minimum_rotated_rectangle:e_star"
      ]
    },
    %{
      reason:
        "minimum_bounding_circle of the degenerate line LINESTRING (1 1, 1 1): GEOS returns POLYGON EMPTY, GeoFrame the point (zero radius), as for a point.",
      doc: "docs/parity.md#batch-e-constructive-geometry",
      op: "minimum_bounding_circle",
      cases: ["minimum_bounding_circle:line_zero"]
    },
    %{
      reason:
        "maximum_inscribed_circle of an invalid (self-intersecting bowtie) polygon: GEOS returns a degenerate zero-length line at the pinch point, GeoFrame searches the signed distance to the boundary rings.",
      doc: "docs/parity.md#batch-e-constructive-geometry",
      op: "maximum_inscribed_circle",
      cases: [
        "maximum_inscribed_circle:bowtie",
        "maximum_inscribed_circle(tolerance=0.01):bowtie"
      ]
    },
    %{
      reason:
        "Vertex snapping with a tolerance: GeoFrame merges vertices closer than the tolerance to an already kept vertex (visited in lexicographic order); GEOS merges a new vertex only with the endpoints of the triangle edge it lands on, so for clustered input the kept vertices differ.",
      doc: "docs/parity.md#batch-e-constructive-geometry",
      op: "delaunay_triangles",
      cases: ["delaunay_triangles(tolerance=1.5; e_sliver)"]
    },
    %{
      reason:
        "Vertex snapping with a tolerance: GeoFrame merges vertices closer than the tolerance to an already kept vertex (visited in lexicographic order); GEOS merges a new vertex only with the endpoints of the triangle edge it lands on, so for clustered input the kept vertices differ.",
      doc: "docs/parity.md#batch-e-constructive-geometry",
      op: "voronoi_polygons",
      cases: [
        "voronoi_polygons(tolerance=1.5; pt1+pt2+pt_edge+pt_far)",
        "voronoi_polygons(tolerance=1.5; e_cluster)",
        "voronoi_polygons(tolerance=1.5; e_collinear)",
        "voronoi_polygons(tolerance=1.5; e_sliver)"
      ]
    },
    %{
      reason:
        "Voronoi sides of near-cocircular sites (e_ring and e_star are rounded to 3 decimals): the Delaunay edge between four almost cocircular sites is chosen by exact predicates in spade and by floating-point ones in GEOS, which flips a very short Voronoi side. The cells agree.",
      doc: "docs/parity.md#batch-e-constructive-geometry",
      op: "voronoi_polygons",
      cases: [
        "voronoi_polygons(only_edges=true; e_ring)",
        "voronoi_polygons(only_edges=true; e_star)"
      ]
    },
    %{
      reason:
        "set_precision on an INVALID multipolygon (parts that overlap or lie inside each other). GEOS' overlay noder decides from edge contact after snapping: a part whose edges meet no other part's is kept (including a polygon inside another one in an invalid result), parts whose edges coincide are merged (i_mpoly_holes), and crossing edges make GEOS raise a TopologyException; GeoFrame merges parts that overlap after snapping and never raises. Valid input agrees in every case.",
      doc: "docs/parity.md#batch-i-hard--low-value-defer",
      op: "set_precision",
      cases: [
        "set_precision(0.1, valid_output):i_mpoly_holes",
        "set_precision(0.1, keep_collapsed):i_mpoly_holes",
        "set_precision(0.1, valid_output):i_mpoly_tiny",
        "set_precision(0.1, keep_collapsed):i_mpoly_tiny",
        "set_precision(2.5, valid_output):i_mpoly_sizes",
        "set_precision(2.5, keep_collapsed):i_mpoly_sizes"
      ]
    },
    %{
      reason:
        "offset_curve of a closed LineString on its outer side (negative distance) with a bevel or mitre join: GEOS keeps the closing bevel at the start vertex (bevel) or drops the two pieces next to it (mitre) because it extracts the curve from the buffer outline, whose round caps meet there; GeoFrame draws the one-sided curve without a join at the start (as GEOS does for round joins).",
      doc: "docs/parity.md#reference-testing",
      op: "offset_curve",
      cases: [
        "offset_curve(-0.5, bevel):j_ring_a",
        "offset_curve(-0.5, bevel):j_ring_b",
        "offset_curve(-0.5, bevel):j_tri_ring",
        "offset_curve(-0.5, bevel):line_closed",
        "offset_curve(-0.5, mitre):j_ring_a",
        "offset_curve(-0.5, mitre):j_ring_b",
        "offset_curve(-0.5, mitre):j_tri_ring",
        "offset_curve(-0.5, mitre):line_closed",
        "offset_curve(-1.0, bevel):j_ring_a",
        "offset_curve(-1.0, bevel):j_ring_b",
        "offset_curve(-1.0, bevel):j_tri_ring",
        "offset_curve(-1.0, bevel):line_closed",
        "offset_curve(-1.0, mitre):j_ring_a",
        "offset_curve(-1.0, mitre):j_ring_b",
        "offset_curve(-1.0, mitre):j_tri_ring",
        "offset_curve(-1.0, mitre):line_closed",
        "offset_curve(-2.0, bevel):j_ring_a",
        "offset_curve(-2.0, bevel):j_ring_b",
        "offset_curve(-2.0, bevel):j_tri_ring",
        "offset_curve(-2.0, bevel):line_closed",
        "offset_curve(-2.0, mitre):j_ring_a",
        "offset_curve(-2.0, mitre):j_ring_b",
        "offset_curve(-2.0, mitre):j_tri_ring",
        "offset_curve(-2.0, mitre):line_closed"
      ]
    },
    %{
      reason:
        "offset_curve of a polygon ring whose offset lands exactly on the medial axis (the L-shape and the frame are exactly 2 wide for distance 1): GEOS returns nothing or a different remnant, GeoFrame a short piece.",
      doc: "docs/parity.md#reference-testing",
      op: "offset_curve",
      cases: [
        "offset_curve(-1.0, mitre) polygons:ell",
        "offset_curve(-1.0, mitre) polygons:sq_hole"
      ]
    },
    %{
      reason:
        "offset_curve with a mitre or bevel join around segments shorter than the distance (and a limited mitre around a hairpin): GEOS cuts the curve against the buffer outline, which has the bevel or mitre shape at the corners, GeoFrame against the disc around each vertex, so the cut lands elsewhere.",
      doc: "docs/parity.md#reference-testing",
      op: "offset_curve",
      cases: [
        "offset_curve(-1.0, bevel):j_short",
        "offset_curve(-2.0, bevel):j_short",
        "offset_curve(1.0, bevel):j_short",
        "offset_curve(1.0, mitre):j_loop",
        "offset_curve(1.0, mitre):j_short",
        "offset_curve(1.0, mitre):j_spiral",
        "offset_curve(1.0, mitre, mitre_limit=1.2):j_loop",
        "offset_curve(1.0, mitre, mitre_limit=1.2):j_short",
        "offset_curve(1.0, mitre, mitre_limit=1.2):j_spiral"
      ]
    },
    %{
      reason:
        "snap with several snap points competing for one vertex or segment (a cluster near a ring corner, snap points beyond the end of a line): the order in which GEOS applies them is not documented and GeoFrame reproduces it in about 99 % of random cases; every other case agrees.",
      doc: "docs/parity.md#reference-testing",
      op: "snap",
      cases: [
        "snap(0.3):ell|j_cluster",
        "snap(0.3):line_closed|j_cluster",
        "snap(0.3):mpoly|j_cluster",
        "snap(0.3):sq_hole|j_cluster",
        "snap(0.3):sq|j_cluster",
        "snap(0.3):tri|j_cluster",
        "snap(1.0):ell|j_cluster",
        "snap(1.0):line_closed|j_cluster",
        "snap(1.0):mpoly|j_cluster",
        "snap(1.0):sq_hole|j_cluster",
        "snap(1.0):sq|j_cluster",
        "snap(1.0):tri|j_cluster",
        "snap(2.5):ell|j_cluster",
        "snap(2.5):ell|j_fine_line",
        "snap(2.5):line_closed|j_cluster",
        "snap(2.5):line_closed|j_fine_line",
        "snap(2.5):mpoly|j_cluster",
        "snap(2.5):sq_hole|j_cluster",
        "snap(2.5):sq_hole|sq_inner",
        "snap(2.5):sq|j_cluster",
        "snap(2.5):tri|j_cluster"
      ]
    },
    %{
      reason:
        "build_area with node=false on lon/lat polygons whose rings overlap: the symmetric difference of the faces is computed by geo's overlay, which snaps coordinates to a grid of about 1e-9 degrees.",
      doc: "docs/parity.md#reference-testing",
      op: "build_area",
      cases: [
        "build_area(lonlat, node=false)"
      ]
    },
    %{
      reason:
        "polygonize with node=true merges overlapping input polygons with geo's overlay, which snaps coordinates to a grid of about 1e-9 degrees (lon/lat input), so the polygons differ from GEOS' by that much.",
      doc: "docs/parity.md#reference-testing",
      op: "polygonize",
      cases: [
        "polygonize(lonlat, node=true)"
      ]
    },
    %{
      reason:
        "invalid_coverage_edges over a series with a null row: GeoPandas 1.2 drops the null geometry before calling GEOS and pads the result with NaN at the END, so every result after the null is shifted by one row. GeoFrame keeps each result in its row and returns nil for the null one.",
      doc: "docs/parity.md#batch-i3",
      op: "invalid_coverage_edges",
      cases: [
        "invalid_coverage_edges(null_point_empty, gap_width=0.0)",
        "invalid_coverage_edges(null_point_empty, gap_width=0.2)",
        "invalid_coverage_edges(null_point_empty, gap_width=1.0)"
      ]
    },
    %{
      reason:
        "simplify with preserve_topology=false when the Douglas-Peucker result (or the input) is invalid, a self-crossing ring or parts that overlap or touch along an edge: GEOS repairs it with buffer(0), splitting a bowtie into its lobes and dissolving touching parts. GeoFrame returns the Douglas-Peucker rings as they are (it has no GEOS buffer(0)); a valid result, including the removal of collapsed rings, agrees vertex for vertex.",
      doc: "docs/parity.md#reference-testing",
      op: "simplify",
      cases: [
        "simplify(0.0, preserve_topology=false, collapse):s_bowtie_ring",
        "simplify(0.0, preserve_topology=false, collapse):s_cross",
        "simplify(0.0, preserve_topology=false, collapse):s_touching",
        "simplify(0.05, preserve_topology=false, collapse):s_bowtie_ring",
        "simplify(0.05, preserve_topology=false, collapse):s_cross",
        "simplify(0.05, preserve_topology=false, collapse):s_touching",
        "simplify(1.0, preserve_topology=false, collapse):s_bowtie_ring",
        "simplify(1.0, preserve_topology=false, collapse):s_cross",
        "simplify(1.0, preserve_topology=false, collapse):s_touching"
      ]
    }
  ]
