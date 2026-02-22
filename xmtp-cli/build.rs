//! Build script for xmtp-cli.
//!
//! On Windows (MSVC), suppress duplicate Rust runtime symbols that arise from
//! statically linking a Rust `staticlib` (xmtp_ffi) into another Rust binary.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows-msvc") {
        println!("cargo:rustc-link-arg=/FORCE:MULTIPLE");
    }
}
