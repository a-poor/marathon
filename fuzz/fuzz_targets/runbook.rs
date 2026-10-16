#![no_main]

use libfuzzer_sys::fuzz_target;
use marathon::book::Runbook;

fuzz_target!(|data: &str| {
    // Parsing only: no scratch creation, option-file reads, or command execution.
    // Invalid configuration is an expected Result::Err, never a panic.
    let _ = Runbook::new(None::<&str>, data);
});
