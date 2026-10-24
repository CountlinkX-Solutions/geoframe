//! EPSG definitions that the bundled `crs-definitions` table lacks (placeholder).

use serde_json::Value;

pub(crate) fn projjson(_code: u32) -> Option<Value> {
    None
}
