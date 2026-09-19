#![no_main]
//! A cache must answer exactly what the device underneath it would, at
//! every block size and for every read -- including ones that straddle
//! a block boundary, run off the end, or come back a second time.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    fs_core_fuzz::check_cache(&fs_core_fuzz::geometry_from(data));
});
