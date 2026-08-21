#![no_main]

use bhtune_core::template::parse_catalog;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let _ = parse_catalog(&input);
});
