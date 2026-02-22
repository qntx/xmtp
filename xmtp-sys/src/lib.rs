//! Raw FFI bindings to `libxmtp_ffi` — the XMTP messaging protocol static library.
//!
//! All types and functions are **auto-generated** by [`bindgen`](https://docs.rs/bindgen)
//! from the C header `xmtp_ffi.h` produced by `cbindgen`. Do not edit manually.
//!
//! # Build
//!
//! The build script (`build.rs`) automatically:
//! 1. Downloads the pre-built static library from GitHub Releases (or uses a local path).
//! 2. Runs `bindgen` on the included `xmtp_ffi.h` header.
//! 3. Configures the linker to link the static library + system dependencies.
//!
//! For local development, set `XMTP_FFI_DIR` to point at the `xmtp-ffi` crate root
//! (e.g. `../xmtp-ffi`) — it must contain `include/xmtp_ffi.h` and a built static lib.

// sys crate: unsafe FFI, non-idiomatic generated code
#![allow(
    unsafe_code,
    missing_docs,
    non_camel_case_types,
    non_upper_case_globals,
    non_snake_case,
    clippy::missing_safety_doc,
    clippy::upper_case_acronyms
)]

// When the `regenerate` feature is enabled, use freshly generated bindings.
// Otherwise, use the pre-generated bindings committed in the repository.
#[cfg(feature = "regenerate")]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
#[cfg(not(feature = "regenerate"))]
include!("bindings.rs");
