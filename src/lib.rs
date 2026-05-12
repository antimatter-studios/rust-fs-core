//! Pure-Rust block-device framework. The shared substrate every filesystem
//! driver and disk-image reader plugs into.
//!
//! See the crate-level
//! [`README`](https://github.com/antimatter-studios/rust-fs-core) for the
//! intended consumers and design.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod block;
pub mod caching_device;
pub mod callback_device;
pub mod error;
pub mod ffi;
pub mod file_device;
pub mod readonly;
pub mod slice;
pub mod stream;

pub use block::{BlockDevice, BlockRead};
pub use caching_device::CachingDevice;
pub use callback_device::{CallbackDevice, FlushCb, ReadCb, WriteCb};
pub use error::{Error, Result};
pub use file_device::FileDevice;
pub use readonly::ReadOnlyDevice;
pub use slice::{OwnedRwSlice, OwnedSlice, SliceReader};
pub use stream::BlockReadStreamer;
