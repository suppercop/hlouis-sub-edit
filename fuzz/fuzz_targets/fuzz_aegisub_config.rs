#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    hlouis_fuzz::fuzz_aegisub_config(data);
});
