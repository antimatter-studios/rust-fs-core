#![no_main]
//! A slice must answer a read inside it with the parent's bytes from
//! the right place, and refuse anything else.
//!
//! The property matters more than the absence of a panic. This crate
//! had a slice that "answered a read inside its own declared length
//! with somebody else's bytes" -- a defect a panic check would sail
//! straight past.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    fs_core_fuzz::check_slice(&fs_core_fuzz::geometry_from(data));
});
