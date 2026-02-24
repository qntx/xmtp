# xmtp-ffi

C ABI stable bindings for [libxmtp](https://github.com/xmtp/libxmtp) — the XMTP messaging protocol core library.

This crate compiles to a **static library** (`libxmtp_ffi.a` / `xmtp_ffi.lib`) with a flat C API, consumed by [`xmtp-sys`](../xmtp-sys/) via `bindgen`.

## Architecture

```text
libxmtp (upstream crates, pinned git rev)
    ↓
xmtp-ffi/src/*.rs          Rust → C wrapper
    ↓ cbindgen (build.rs)
include/xmtp_ffi.h          Generated C header
    ↓ cargo build --release
libxmtp_ffi.a               Static library
```

### Design Principles

- Every public function returns `i32` (`0` = ok, `-1` = error) unless returning a primitive.
- Errors stored in thread-local string — retrieve via `xmtp_last_error_message()`.
- Opaque handles are heap-allocated (`Box::into_raw`) with explicit `_free` functions.
- Async operations block on a shared global tokio runtime.
- Streams use C function pointers with `void *context` for user data.

## Build

Requires **nightly** Rust (for `cbindgen` macro expansion). The toolchain is pinned in `rust-toolchain.toml`.

```sh
cargo build --release
```

Outputs:

| Platform | Library | Header |
| --- | --- | --- |
| Linux / macOS | `target/release/libxmtp_ffi.a` | `include/xmtp_ffi.h` |
| Windows | `target/release/xmtp_ffi.lib` | `include/xmtp_ffi.h` |

### Cross-compilation (Linux ARM64)

```sh
sudo apt-get install -y gcc-aarch64-linux-gnu
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
cargo build --release --target aarch64-unknown-linux-gnu
```

## cbindgen Limitation

cbindgen cannot represent `Option<TypeAlias>` where the alias is a function pointer. It emits an **opaque forward declaration** instead of a nullable function pointer:

```c
// WRONG — cbindgen output for Option<FnOnCloseCallback>:
typedef struct XmtpOption_FnOnCloseCallback XmtpOption_FnOnCloseCallback;

// CORRECT — cbindgen output for inline Option<extern "C" fn(...)>:
void (*on_close)(const char*, void*)
```

**Rule**: In `extern "C"` function signatures, always use the **inline** `Option<unsafe extern "C" fn(...)>` form. Type aliases (e.g., `OnCloseCb`) may be used inside Rust helpers and closures, but never at the FFI boundary.

### Platform ABI Impact

When `bindgen` processes the opaque forward declaration, it generates a **zero-sized type** (ZST). Passing a ZST by value in `extern "C"` functions triggers platform-specific ABI behavior:

| ABI | ZST Handling | Effect |
| --- | --- | --- |
| **System V** (Linux, macOS) | Skipped entirely — occupies no register or stack slot | All subsequent parameters shift, corrupting the call |
| **Windows MSVC x64** | Occupies an 8-byte slot regardless of actual size | Parameters stay aligned, but the `on_close` value is garbage |

On Linux, this manifests as a segfault or "null output pointer" error. On Windows, the program may appear to work because parameter positions are preserved, but the `on_close` callback value is still invalid.

## Modules

| File | Scope |
| --- | --- |
| `ffi.rs` | Core infrastructure: error handling, runtime, memory helpers, type definitions, macros |
| `client.rs` | Client creation, identity, inbox operations |
| `conversation.rs` | Single conversation: messages, metadata, members, permissions |
| `conversations.rs` | Conversation listing, creation (DM / group), sync |
| `stream.rs` | Callback-based streaming (conversations, messages, consent, preferences) |
| `signature.rs` | Signature requests and identity association |
| `identity.rs` | Inbox state queries |
| `device_sync.rs` | Device sync and archive operations |

## Callback Types

| Type | Signature | Ownership |
| --- | --- | --- |
| `FnConversationCallback` | `(conv: *mut FfiConversation, ctx: *mut c_void)` | Caller must free `conv` |
| `FnMessageCallback` | `(msg: *mut FfiMessage, ctx: *mut c_void)` | Caller must free `msg` |
| `FnConsentCallback` | `(records: *const FfiConsentRecord, count: i32, ctx: *mut c_void)` | Borrowed — valid during callback only |
| `FnPreferenceCallback` | `(updates: *const FfiPreferenceUpdate, count: i32, ctx: *mut c_void)` | Borrowed — valid during callback only |
| `FnMessageDeletionCallback` | `(message_id: *const c_char, ctx: *mut c_void)` | Borrowed — valid during callback only |
| `FnOnCloseCallback` | `(error: *const c_char, ctx: *mut c_void)` | Borrowed — null error = normal close |

## Stream Lifecycle

```c
// 1. Start a stream (returns handle via out-pointer)
int32_t rc = xmtp_stream_conversations(client, -1, my_cb, NULL, ctx, &handle);

// 2. Signal stop
xmtp_stream_end(handle);

// 3. Free handle memory
xmtp_stream_free(handle);
```

## Supported Targets

| Target | CI | Notes |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | ✅ | Primary Linux target |
| `aarch64-unknown-linux-gnu` | ✅ | Cross-compiled on `ubuntu-latest` |
| `aarch64-apple-darwin` | ✅ | Native macOS ARM |
| `x86_64-pc-windows-msvc` | ✅ | Native Windows x64 |
| `aarch64-pc-windows-msvc` | ✅ | Native Windows ARM64 |

## Release

CI workflow (`.github/workflows/ffi-build.yml`) triggers on `ffi-v*.*.*` tags:

```sh
git tag ffi-v0.1.9 && git push --tags
```

This builds static libraries for all targets, packages them with the header, and creates a GitHub Release. The `xmtp-sys` crate downloads these artifacts at build time.
