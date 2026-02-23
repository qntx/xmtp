//! `xmtp-ffi` — C ABI stable bindings for libxmtp.
//!
//! Design principles:
//! - Every public function returns `i32` (0 = ok, -1 = error) unless it returns a primitive.
//! - Errors are stored in a thread-local string, retrieved via [`xmtp_last_error_message`].
//! - Opaque handles are heap-allocated `Box<T>` behind `*mut T` with explicit `_free` functions.
//! - Async operations block internally on a shared tokio runtime.
//! - Streams use C callback function pointers.
//!
//! # Safety (blanket)
//!
//! All `unsafe extern "C"` functions in this crate share the same safety contract:
//! - Pointer arguments must be valid and non-null unless the doc says otherwise.
//! - `*const` pointers must point to live, aligned data for the duration of the call.
//! - `*mut` output pointers must be writable.
//! - Opaque handles must originate from this crate and must not be used after being freed.
#![allow(clippy::missing_safety_doc)]

mod ffi;

pub mod client;
pub mod conversation;
pub mod conversations;
pub mod device_sync;
pub mod identity;
pub mod signature;
pub mod stream;

// Re-export key items so every module can use them without `crate::ffi::` prefix.
#[allow(unused_imports)]
pub(crate) use ffi::*;
