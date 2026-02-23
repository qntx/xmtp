//! Build script for xmtp-cli.
//!
//! Emits platform-specific linker flags to suppress duplicate Rust runtime
//! symbols (e.g. `rust_eh_personality`) that arise from statically linking
//! the `libxmtp_ffi` Rust staticlib into another Rust binary.
//!
//! This is necessary for `cargo install xmtp-cli` to work, since the
//! workspace `.cargo/config.toml` is not available outside the workspace.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows") && target.contains("msvc") {
        println!("cargo:rustc-link-arg=/FORCE:MULTIPLE");
    } else if target.contains("linux") {
        println!("cargo:rustc-link-arg=-Wl,--allow-multiple-definition");
    } else if target.contains("apple") {
        println!("cargo:rustc-link-arg=-Wl,-multiply_defined,suppress");
    }
}
