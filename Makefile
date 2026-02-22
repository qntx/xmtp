# Makefile for qntx/xmtp — XMTP Rust Client SDK
#
# Workspace members : xmtp-sys, xmtp, xmtp-cli
# Standalone crate  : xmtp-ffi (excluded from workspace, own dependency tree)

FFI_DIR := xmtp-ffi

.PHONY: all
all: pre-commit

# Build the workspace in release mode
.PHONY: build
build:
	cargo build --release --all-features

# Quick compilation check without codegen
.PHONY: check
check:
	cargo check --all-features

# Run all workspace tests
.PHONY: test
test:
	cargo test --all-features

# Run benchmarks
.PHONY: bench
bench:
	cargo bench --all-features

# Run the CLI binary
.PHONY: run
run:
	cargo run --release --all-features

# Lint with Clippy (auto-fix)
.PHONY: clippy
clippy:
	cargo +nightly clippy --fix \
		--all-targets \
		--all-features \
		--allow-dirty \
		--allow-staged \
		-- -D warnings

# Format workspace code
.PHONY: fmt
fmt:
	cargo +nightly fmt

# Check formatting without modifying files
.PHONY: fmt-check
fmt-check:
	cargo +nightly fmt --check

# Generate and open documentation
.PHONY: doc
doc:
	cargo +nightly doc --all-features --no-deps --open

# Build the FFI static library in release mode
.PHONY: ffi-build
ffi-build:
	cargo build --release --manifest-path $(FFI_DIR)/Cargo.toml

# Quick compilation check for FFI
.PHONY: ffi-check
ffi-check:
	cargo check --manifest-path $(FFI_DIR)/Cargo.toml

# Lint FFI code with Clippy (auto-fix)
.PHONY: ffi-clippy
ffi-clippy:
	cargo +nightly clippy --fix \
		--manifest-path $(FFI_DIR)/Cargo.toml \
		--all-targets \
		--allow-dirty \
		--allow-staged \
		-- -D warnings

# Format FFI code
.PHONY: ffi-fmt
ffi-fmt:
	cargo +nightly fmt --manifest-path $(FFI_DIR)/Cargo.toml

# Check FFI formatting without modifying files
.PHONY: ffi-fmt-check
ffi-fmt-check:
	cargo +nightly fmt --manifest-path $(FFI_DIR)/Cargo.toml --check

.PHONY: fmt-all
fmt-all: fmt ffi-fmt

.PHONY: fmt-check-all
fmt-check-all: fmt-check ffi-fmt-check

.PHONY: clippy-all
clippy-all: clippy ffi-clippy

.PHONY: check-all
check-all: check ffi-check

.PHONY: build-all
build-all: build ffi-build

# Update dependencies for both workspace and FFI
.PHONY: update
update:
	cargo update
	cargo update --manifest-path $(FFI_DIR)/Cargo.toml

# Check for unused dependencies (workspace only)
.PHONY: udeps
udeps:
	cargo +nightly udeps --all-features

# Generate CHANGELOG.md using git-cliff
.PHONY: cliff
cliff:
	git cliff --output CHANGELOG.md

.PHONY: pre-commit
pre-commit:
	$(MAKE) fmt-all
	$(MAKE) clippy-all
	$(MAKE) test
	$(MAKE) build
	$(MAKE) cliff
