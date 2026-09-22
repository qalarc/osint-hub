//! Build script: run tauri-build (codegen, icon embedding, config validation)
//! ONLY when compiling with `--features desktop`. Plain `cargo check` on a
//! machine without webkit2gtk must not touch any of that.
//!
//! NOTE: this must be cfg-gating, NOT an `if CARGO_FEATURE_DESKTOP` env
//! check: `tauri-build` is an OPTIONAL build-dependency, so in a headless
//! build the name `tauri_build` doesn't resolve at all and an env-gated
//! call site would fail to compile. Cargo passes feature cfgs to build
//! scripts, so `#[cfg(feature = ...)]` is the correct gate.

#[cfg(feature = "desktop")]
fn main() {
    tauri_build::build();
}

#[cfg(not(feature = "desktop"))]
fn main() {}
