//! Shared setup for the fuzz targets.
//!
//! The properties live in `fuzz/shared/props.rs`, which
//! `tests/fuzz_decoders.rs` includes too -- see that file for why they
//! are shared textually rather than as a dependency, and for why
//! fuzzing this crate means checking a property rather than checking
//! for a panic.

include!("../shared/props.rs");
