#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../tests/support/output_properties.rs"]
mod properties;

fuzz_target!(|data: &[u8]| {
    properties::check_output(data);
    let clean = marathon::ansi::sanitize(&String::from_utf8_lossy(data));
    assert!(clean.bytes().all(|b| b >= 0x20 || b == b'\n'));
    assert_eq!(marathon::ansi::sanitize(&clean), clean);
});
