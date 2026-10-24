//! Geometry-less FlatGeobuf features.
//!
//! The `flatgeobuf` encoder always writes a `Geometry` table, empty when a row has no
//! coordinates. GDAL cannot parse that for a null row ("Fatal error parsing feature",
//! issue #78): it writes a null as a feature whose `geometry` field is absent, and reads one
//! the same way. The crate offers no way to leave the field out, so the finished file is
//! rewritten: every feature with an empty geometry is re-encoded with its properties only.
//!
//! The index stores each feature's byte offset, so with an index the leaf nodes get their new
//! offsets and the tree is rebuilt over them (the sort order and boxes do not change; a
//! geometry-less feature keeps the empty box the encoder gave it, which no query matches).

use flatbuffers::FlatBufferBuilder;
use flatgeobuf::packed_r_tree::{calc_extent, NodeItem, PackedRTree};
use flatgeobuf::{
    size_prefixed_root_as_feature, size_prefixed_root_as_header, Feature, FeatureArgs,
};

const MAGIC_LEN: usize = 8;
const NODE_LEN: usize = std::mem::size_of::<NodeItem>();

fn size_prefix(file: &[u8], at: usize) -> Result<usize, String> {
    let bytes = file.get(at..at + 4).ok_or("truncated FlatGeobuf output")?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
}

fn has_empty_geometry(feature: &Feature<'_>) -> bool {
    feature.geometry().is_some_and(|g| {
        g.xy().is_none_or(|xy| xy.is_empty()) && g.parts().is_none_or(|parts| parts.is_empty())
    })
}

/// The size-prefixed bytes of `feature` without its geometry.
fn without_geometry(feature: &Feature<'_>) -> Vec<u8> {
    let mut fbb = FlatBufferBuilder::new();
    let properties = feature
        .properties()
        .map(|properties| fbb.create_vector(properties.bytes()));
    let rewritten = Feature::create(
        &mut fbb,
        &FeatureArgs {
            geometry: None,
            properties,
            columns: None,
        },
    );
    fbb.finish_size_prefixed(rewritten, None);
    fbb.finished_data().to_vec()
}

/// Rewrites the features of a finished FlatGeobuf file that have an empty geometry so that
/// they carry none. A file without such features is returned untouched.
pub(crate) fn drop_empty_geometries(file: Vec<u8>) -> Result<Vec<u8>, String> {
    let header_end = MAGIC_LEN + 4 + size_prefix(&file, MAGIC_LEN)?;
    let header = size_prefixed_root_as_header(&file[MAGIC_LEN..header_end.min(file.len())])
        .map_err(|e| format!("FlatGeobuf encoder produced an invalid header: {e}"))?;
    let count = header.features_count() as usize;
    let node_size = header.index_node_size();
    let index_len = if node_size > 0 && count > 0 {
        PackedRTree::index_size(count, node_size)
    } else {
        0
    };
    let features_at = header_end + index_len;

    let mut body = Vec::with_capacity(file.len().saturating_sub(features_at));
    let mut sizes = Vec::with_capacity(count);
    let mut changed = false;
    let mut at = features_at;
    for _ in 0..count {
        let len = 4 + size_prefix(&file, at)?;
        let raw = file
            .get(at..at + len)
            .ok_or("truncated FlatGeobuf feature")?;
        let feature = size_prefixed_root_as_feature(raw)
            .map_err(|e| format!("FlatGeobuf encoder produced an invalid feature: {e}"))?;
        let before = body.len();
        if has_empty_geometry(&feature) && feature.columns().is_none() {
            changed = true;
            body.extend_from_slice(&without_geometry(&feature));
        } else {
            body.extend_from_slice(raw);
        }
        sizes.push(body.len() - before);
        at += len;
    }
    if !changed {
        return Ok(file);
    }

    let mut out = file[..header_end].to_vec();
    if index_len > 0 {
        let leaves_at = features_at - count * NODE_LEN;
        let mut leaves = Vec::with_capacity(count);
        let mut offset = 0_u64;
        for (i, size) in sizes.iter().enumerate() {
            let begin = leaves_at + i * NODE_LEN;
            let mut node = NodeItem::from_reader(&file[begin..begin + NODE_LEN])
                .map_err(|e| format!("FlatGeobuf encoder produced an invalid index: {e}"))?;
            node.offset = offset;
            offset += *size as u64;
            leaves.push(node);
        }
        let tree = PackedRTree::build(&leaves, &calc_extent(&leaves), node_size)
            .map_err(|e| format!("cannot rebuild the FlatGeobuf index: {e}"))?;
        let before = out.len();
        tree.stream_write(&mut out)
            .map_err(|e| format!("cannot rebuild the FlatGeobuf index: {e}"))?;
        if out.len() - before != index_len {
            return Err("rebuilt FlatGeobuf index has an unexpected size".to_string());
        }
    }
    out.extend_from_slice(&body);
    Ok(out)
}
