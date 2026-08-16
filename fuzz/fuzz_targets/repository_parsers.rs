#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    related::fuzzing::parse_repository_bytes(data);
});
