#![allow(unsafe_code)]
//! Unified error types for the XMTP SDK.

use std::ffi::CStr;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Top-level error type for the XMTP SDK.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error originating from the underlying FFI / native library.
    #[error("xmtp ffi: {0}")]
    Ffi(String),

    /// A returned pointer was unexpectedly null.
    #[error("unexpected null pointer from FFI")]
    NullPointer,

    /// A string received from FFI contained invalid UTF-8.
    #[error("invalid UTF-8 in FFI string")]
    InvalidUtf8,

    /// An argument passed to the SDK was invalid.
    #[error("{0}")]
    InvalidArgument(String),

    /// A signing operation failed.
    #[error("signing: {0}")]
    Signing(String),

    /// No identity resolver configured (needed for ENS names, etc.).
    #[error("no resolver configured — use ClientBuilder::resolver()")]
    NoResolver,

    /// Identity resolution failed (ENS, Lens, etc.).
    #[error("resolution: {0}")]
    Resolution(String),
}

/// Read the last FFI error message from thread-local storage.
pub(crate) fn last_ffi_error() -> Error {
    let len = unsafe { xmtp_sys::xmtp_last_error_length() };
    if len <= 0 {
        return Error::Ffi("unknown FFI error".into());
    }
    let mut buf = vec![0u8; len.unsigned_abs() as usize];
    let written = unsafe { xmtp_sys::xmtp_last_error_message(buf.as_mut_ptr().cast(), len) };
    if written <= 0 {
        return Error::Ffi("failed to read FFI error".into());
    }
    CStr::from_bytes_until_nul(&buf).map_or_else(
        |_| {
            Error::Ffi(
                String::from_utf8_lossy(&buf[..written.unsigned_abs() as usize]).into_owned(),
            )
        },
        |cstr| Error::Ffi(cstr.to_string_lossy().into_owned()),
    )
}

/// Check an FFI return code. `0` = success.
#[inline]
pub(crate) fn check(rc: i32) -> Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(last_ffi_error())
    }
}
