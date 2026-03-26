# xmtp

[![crates.io][crate-badge]][crate-url]
[![docs.rs][doc-badge]][doc-url]
[![License][license-badge]][license-url]

[crate-badge]: https://img.shields.io/crates/v/xmtp.svg
[crate-url]: https://crates.io/crates/xmtp
[doc-badge]: https://img.shields.io/docsrs/xmtp.svg
[doc-url]: https://docs.rs/xmtp
[license-badge]: https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg
[license-url]: https://github.com/qntx/xmtp/blob/main/LICENSE-MIT

Safe, ergonomic Rust SDK for the [XMTP](https://xmtp.org) messaging protocol.

Wraps the [`xmtp-sys`](https://crates.io/crates/xmtp-sys) FFI bindings with idiomatic Rust types, providing a high-level `Client` → `Conversation` → `Message` API for E2E encrypted DMs, groups, content types, identity management, ENS resolution, and real-time streaming.

## Linking

The underlying `xmtp-sys` crate links a pre-built Rust `staticlib` that bundles its own
copy of `std`. This causes duplicate symbol errors (e.g. `rust_eh_personality`) when
building a Rust binary. Add one of the following workarounds to your **binary crate**:

**Option A** — `build.rs`:

```rust,ignore
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("linux") {
        println!("cargo:rustc-link-arg=-Wl,--allow-multiple-definition");
    } else if target.contains("windows") && target.contains("msvc") {
        println!("cargo:rustc-link-arg=/FORCE:MULTIPLE");
    } else if target.contains("apple") {
        println!("cargo:rustc-link-arg=-Wl,-multiply_defined,suppress");
    }
}
```

**Option B** — `.cargo/config.toml`:

```toml
[target.'cfg(target_os = "linux")']
rustflags = ["-C", "link-arg=-Wl,--allow-multiple-definition"]

[target.'cfg(all(target_os = "windows", target_env = "msvc"))']
rustflags = ["-C", "link-arg=/FORCE:MULTIPLE"]

[target.'cfg(target_os = "macos")']
rustflags = ["-C", "link-arg=-Wl,-multiply_defined,suppress"]
```

## Quick Start

```rust,ignore
use xmtp::{Client, Env, AlloySigner};

// Create a client and register identity.
let signer = AlloySigner::random()?;
let client = Client::builder()
    .env(Env::Dev)
    .db_path("./alice.db3")
    .build(&signer)?;

// Send a DM.
let conv = client.dm(&"0xBob...".into())?;
conv.send_text("hello from Rust")?;

// Stream messages in real time.
let _handle = xmtp::stream::stream_all_messages(
    &client, None, &[], |msg_id, conv_id| {
        println!("new message {msg_id} in {conv_id}");
    },
)?;
```

## Feature Flags

| Feature | Default | Description |
| --- | --- | --- |
| `content` | ✅ | Content type codecs (text, reactions, replies, attachments, read receipts) |
| `alloy` | | Local private key signer via `alloy-signer-local` |
| `ledger` | | Ledger hardware wallet signer via `alloy-signer-ledger` |
| `ens` | | ENS name resolution via `alloy-ens` + `alloy-provider` |

## License

Licensed under either of [Apache License, Version 2.0](../LICENSE-APACHE) or [MIT License](../LICENSE-MIT) at your option.
