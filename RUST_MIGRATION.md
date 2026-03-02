# PrismarineLauncher Rust Migration

This repository now includes a Rust bootstrap in `rust/`.

## Current state

- Existing launcher remains in C++/Qt.
- New Rust entry point is available for incremental migration.

## Proposed migration order

1. Core domain models (instances, accounts, metadata parsing).
2. Network/update layer.
3. Launch pipeline.
4. UI shell (native Rust GUI or hybrid embedding strategy).
5. Full replacement and removal of legacy C++ targets.

## Immediate next engineering steps

1. Add Rust CI job (`cargo fmt`, `cargo clippy`, `cargo test`).
2. Move one isolated parser module from C++ to Rust with parity tests.
3. Introduce FFI boundary for calling Rust from the current launcher.
