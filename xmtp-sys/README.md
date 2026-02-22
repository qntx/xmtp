# xmtp-sys

Raw FFI bindings to [`libxmtp_ffi`](https://github.com/qntx/xmtp) — the XMTP messaging protocol static library.

> **Note:** This crate provides unsafe, low-level bindings. Prefer the
> safe [`xmtp`](https://crates.io/crates/xmtp) crate for application code.

## How it works

All types and functions are **auto-generated** by [`bindgen`](https://docs.rs/bindgen) from the C header `xmtp_ffi.h` produced by [`cbindgen`](https://docs.rs/cbindgen). Pre-generated bindings are committed to the repository so end users do **not** need `libclang` installed.

At build time, the build script:

1. Downloads the pre-built static library from [GitHub Releases](https://github.com/qntx/xmtp/releases) for the current target platform (or uses a local path via `XMTP_FFI_DIR`).
2. Configures the linker to link the static library plus required system dependencies.

## Environment variables

| Variable | Description |
| --- | --- |
| `XMTP_FFI_DIR` | Path to a local FFI build directory. Skips downloading. |
| `XMTP_FFI_VERSION` | Override the FFI release version (default: crate version). |
| `XMTP_UPDATE_BINDINGS` | When set with `regenerate` feature, copy generated bindings back to `src/bindings.rs`. |

## Features

| Feature | Description |
| --- | --- |
| `regenerate` | Re-generate bindings from `xmtp_ffi.h` at build time (requires `libclang`). |

## Supported platforms

| Target | Status |
| --- | --- |
| `x86_64-unknown-linux-gnu` | ✅ |
| `aarch64-unknown-linux-gnu` | ✅ |
| `aarch64-apple-darwin` | ✅ |
| `x86_64-pc-windows-msvc` | ✅ |
| `aarch64-pc-windows-msvc` | ✅ |

## License

Licensed under the MIT license.
