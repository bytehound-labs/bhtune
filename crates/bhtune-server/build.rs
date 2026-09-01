//! Ensures `frontend/dist/` exists on disk before this crate's own source is compiled.
//!
//! `spa.rs`'s `#[derive(RustEmbed)]` reads that folder differently depending on build
//! profile: embedded into the binary at compile time for `--release` (`build-matrix`), or
//! read live off disk on every request for a plain `cargo build`/`cargo run` (see that
//! module's doc comment) -- but *either way*, `rust-embed`'s generated code calls
//! `Path::canonicalize()` on `#[folder = ...]`'s path exactly once, while *this crate* is
//! being compiled, to build the path-traversal guard that every `Assets::get()` call checks
//! against at runtime. `canonicalize()` requires the target to exist; if `frontend/dist/`
//! is missing at that moment, it fails, and rust-embed silently falls back to the
//! *un-canonicalized* string (`.../bhtune-server/../../frontend/dist/`, `..`s left
//! uncollapsed) instead of erroring loudly.
//!
//! That stale fallback path can never `starts_with`-match the real, canonical path of any
//! file requested later, so **every** asset request 404s from then on -- even after
//! `frontend/dist/` is built and populated, since nothing about a later `pnpm run build`
//! re-runs this crate's already-compiled derive macro. Only a full recompile of
//! `bhtune-server` fixes it. This bit CI directly: `.github/workflows/e2e.yml` builds this
//! crate before `frontend/dist/` exists (deliberately, so it doesn't need to rebuild Rust
//! after the frontend build), and it is just as easy for a contributor to hit by building
//! the workspace before ever running `pnpm install && pnpm run build`.
//!
//! Creating an *empty* `frontend/dist/` here, before rustc ever invokes the derive macro,
//! is sufficient: `canonicalize()` only needs the directory to exist, not to contain
//! anything yet -- `spa.rs`'s own `#[allow_missing = true]` and its `Assets::iter().next()`
//! check already handle "exists but empty" with a clear 503 rather than a confusing 404.
fn main() {
    println!("cargo:rustc-check-cfg=cfg(coverage)");

    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frontend/dist");
    // Best-effort: on the rare filesystem where this fails (read-only checkout, a race with
    // another concurrent build), `rust-embed`'s existing `allow_missing` handling is the
    // fallback safety net -- this build script is a robustness improvement on top of that,
    // not a hard requirement for the crate to compile.
    let _ = std::fs::create_dir_all(dist);

    // No `cargo:rerun-if-changed` lines: the default (no directives emitted at all) makes
    // Cargo rerun this script whenever anything in the package changes, which covers every
    // occasion this crate gets recompiled -- exactly when the stale-canonical-path bug above
    // can be (re-)introduced.
}
