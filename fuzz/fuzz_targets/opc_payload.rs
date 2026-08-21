#![no_main]

use bhtune_driver::{
    opcda::{
        opc_value_from_write, quality_from_raw, tag_node_from_browse, tag_value_from_raw,
        write_outcome_from_result,
    },
    types::TagWrite,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data).into_owned();
    let _ = quality_from_raw(&text);
    let _ = tag_value_from_raw(opcda_bridge::TagValue {
        tag_id: text.clone(),
        value: text.clone(),
        quality: text.clone(),
        timestamp: text.clone(),
    });
    let _ = tag_node_from_browse(opcda_bridge::BrowseNode {
        tag_id: text.clone(),
        node_type: text.clone(),
    });
    let _ = write_outcome_from_result(opcda_bridge::WriteResult {
        tag_id: text.clone(),
        success: data.first().is_some_and(|byte| byte % 2 == 0),
        error: Some(text.clone()),
    });
    let _ = opc_value_from_write(TagWrite::Raw(text));

    let mut bytes = [0_u8; 4];
    let copy_len = data.len().min(bytes.len());
    bytes[..copy_len].copy_from_slice(&data[..copy_len]);
    let _ = opc_value_from_write(TagWrite::Float(f32::from_le_bytes(bytes)));
});
