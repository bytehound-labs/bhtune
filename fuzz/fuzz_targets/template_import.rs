#![no_main]

use bhtune_cli::commands::template::parse_import_contents;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let _ = parse_import_contents(&input);
});
