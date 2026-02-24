# xmtp

[![CI][ci-badge]][ci-url]
[![License][license-badge]][license-url]
[![Rust][rust-badge]][rust-url]

[ci-badge]: https://github.com/qntx/xmtp/actions/workflows/rust.yml/badge.svg
[ci-url]: https://github.com/qntx/xmtp/actions/workflows/rust.yml
[license-badge]: https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg
[license-url]: LICENSE-MIT
[rust-badge]: https://img.shields.io/badge/rust-edition%202024-orange.svg
[rust-url]: https://doc.rust-lang.org/edition-guide/

**Safe, ergonomic Rust SDK for the [XMTP](https://xmtp.org) messaging protocol — E2E encrypted messaging via MLS (RFC 9750), with a batteries-included TUI chat client.**

xmtp wraps the official [`libxmtp`](https://github.com/xmtp/libxmtp) FFI layer with idiomatic Rust types, providing a high-level `Client` → `Conversation` → `Message` API for DMs, groups, content types, identity management, ENS resolution, and real-time streaming. The CLI crate ships a full-featured terminal chat interface with profile-based persistent configuration.

## Crates

| Crate | | Description |
| --- | --- | --- |
| **[`xmtp`](xmtp/)** | [![crates.io][xmtp-crate]][xmtp-crate-url] [![docs.rs][xmtp-doc]][xmtp-doc-url] | SDK — Client, Conversation, Message, content codecs, ENS, Ledger |
| **[`xmtp-sys`](xmtp-sys/)** | [![crates.io][sys-crate]][sys-crate-url] [![docs.rs][sys-doc]][sys-doc-url] | Raw FFI bindings to `libxmtp_ffi` static library |
| **[`xmtp-cli`](xmtp-cli/)** | [![crates.io][cli-crate]][cli-crate-url] [![docs.rs][cli-doc]][cli-doc-url] | TUI chat client + profile management CLI |

[xmtp-crate]: https://img.shields.io/crates/v/xmtp.svg
[xmtp-crate-url]: https://crates.io/crates/xmtp
[sys-crate]: https://img.shields.io/crates/v/xmtp-sys.svg
[sys-crate-url]: https://crates.io/crates/xmtp-sys
[cli-crate]: https://img.shields.io/crates/v/xmtp-cli.svg
[cli-crate-url]: https://crates.io/crates/xmtp-cli
[xmtp-doc]: https://img.shields.io/docsrs/xmtp.svg
[xmtp-doc-url]: https://docs.rs/xmtp
[sys-doc]: https://img.shields.io/docsrs/xmtp-sys.svg
[sys-doc-url]: https://docs.rs/xmtp-sys
[cli-doc]: https://img.shields.io/docsrs/xmtp-cli.svg
[cli-doc-url]: https://docs.rs/xmtp-cli

## Quick Start

### Library

```rust
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

// List conversations.
let convs = client.list_conversations(&Default::default())?;
for c in &convs {
    println!("{}: {}", c.id()?, c.name().unwrap_or_default());
}
```

### Install the CLI

**Shell** (macOS / Linux):

```sh
curl -fsSL https://raw.githubusercontent.com/qntx/xmtp/main/install.sh | sh
```

**PowerShell** (Windows):

```powershell
irm https://raw.githubusercontent.com/qntx/xmtp/main/install.ps1 | iex
```

**Cargo binstall** (prebuilt binary, no compilation):

```bash
cargo binstall xmtp-cli
```

**Cargo install** (build from source):

```bash
cargo install xmtp-cli
```

### CLI

```bash

# Create a profile (generates a new key, registers with XMTP)
xmtp new alice

# Create a profile with a Ledger hardware wallet
xmtp new bob --ledger

# Create a profile with an imported private key
xmtp new carol --import 0xdeadbeef...

# Launch the TUI chat interface
xmtp              # uses default profile
xmtp alice        # uses profile "alice"

# Profile management
xmtp list          # list all profiles (* = default)
xmtp info alice    # show profile details + installations
xmtp default alice # set default profile
xmtp remove alice  # delete a profile
xmtp clear         # delete ALL profiles

# Revoke all other installations (requires wallet signature)
xmtp revoke alice
```

## Architecture

- **xmtp** — High-level SDK. Owns all unsafe FFI calls behind safe types. `Client` is built via `ClientBuilder` with optional signer, ENS resolver, and environment selection. `Conversation` provides send/receive/sync/metadata/consent operations. Content codecs handle text, markdown, reactions, replies, attachments, and read receipts.
- **xmtp-sys** — Auto-generated bindings from `xmtp_ffi.h`. Downloads pre-built static libraries at build time. No `libclang` required for end users.
- **xmtp-cli** — Profile-based TUI chat client. Profiles persist configuration (environment, signer type, wallet address) in platform data directories. Signer is only required for identity-changing operations (`new`, `revoke`); TUI and `info` operate without it.

## Feature Flags

### `xmtp` crate

| Feature | Description |
| --- | --- |
| `content` | Content type codecs (text, reactions, replies, attachments, read receipts) — enabled by default |
| `alloy` | Local private key signer via `alloy-signer-local` |
| `ledger` | Ledger hardware wallet signer via `alloy-signer-ledger` |
| `ens` | ENS name resolution via `alloy-ens` + `alloy-provider` |

### `xmtp-sys` crate

| Feature | Description |
| --- | --- |
| `regenerate` | Re-generate bindings from `xmtp_ffi.h` at build time (requires `libclang`) |

## XMTP Identity Model

```text
Inbox ID
 ├── Identity (EOA / Smart Contract Wallet / Passkey)
 │    └── Installation 1  ← key pair stored in local DB
 │    └── Installation 2
 │    └── ...  (up to 10)
 └── Identity 2
      └── ...
```

- **Wallet signature** is required only for identity-changing operations: creating an inbox, adding/removing identities, revoking installations.
- **Installation key** (stored in the local database) handles all routine messaging — no external signer needed after initial registration.

## Supported Platforms

| Target | Status |
| --- | --- |
| `x86_64-unknown-linux-gnu` | ✅ |
| `aarch64-unknown-linux-gnu` | ✅ |
| `aarch64-apple-darwin` | ✅ |
| `x86_64-pc-windows-msvc` | ✅ |
| `aarch64-pc-windows-msvc` | ✅ |

## Security

This library has **not** been independently audited. See [SECURITY.md](SECURITY.md) for full disclaimer, supported versions, and vulnerability reporting instructions.

- All FFI calls are bounds-checked and null-pointer guarded
- Private keys remain in memory only as long as needed; Ledger keys never leave the device
- No key material is logged or persisted by the SDK (key files are managed by the CLI layer)
- E2E encryption handled by libxmtp's MLS implementation (RFC 9750)

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT License ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project shall be dual-licensed as above, without any additional terms or conditions.
