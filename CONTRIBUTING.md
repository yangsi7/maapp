# Contributing to maapp

Thanks for your interest in improving maapp. This document covers the dev
setup, the verification gate every change must pass, and what we expect from
pull requests.

## Dev setup

You need:

- **Rust stable** (the crate pins `rust-version = "1.96"`, edition 2024)
- **cargo-nextest** (test runner): `cargo install cargo-nextest`
- **cargo-deny** (license/advisory audit): `cargo install cargo-deny`
- **wasm32 target** (the core library must stay wasm-clean):
  `rustup target add wasm32-unknown-unknown`
- optional: **cargo-insta** (snapshot review): `cargo install cargo-insta`

Build and run the CLI:

```
cargo build
cargo run -- validate examples/checkout.json
```

## The verification gate

Every Rust change must pass the full gate before it is considered done:

```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo nextest run --all-features --locked && cargo test --doc
cargo deny check
cargo build --target wasm32-unknown-unknown --locked
```

Notes on what the gate enforces:

- **Determinism is a hard contract.** Nothing iterated from a
  `std::HashMap`/`HashSet` may reach a serialized or rendered artifact; the
  `--json` output is byte-stable. No floats, timestamps, RNG, absolute paths,
  or env values in any artifact.
- **The library stays wasm-clean.** Core graph/validate/query/render logic is
  feature-free and must compile for `wasm32-unknown-unknown`; native-only
  dependencies live behind the `cli` feature.
- **Warnings are errors** in CI (clippy `-D warnings`); do not add
  `#![deny(warnings)]` to source.

## The eval harness must stay green

The eval harness is the project's verification spine, not an optional extra.
Engine changes (validator rules, query semantics, render output) must keep it
green:

- The battery runs as normal tests under `tests/`: the mutation battery
  (defect recall plus false-positive guard over the example graphs), the
  ground-truth task battery, the frozen differential snapshots, and the
  conformance test.
- All graphs in `examples/` must keep validating clean
  (`cargo run -- validate examples/<name>.json` exits 0 with 0 errors).
- Snapshot updates (`cargo insta review`) are part of the diff and get
  reviewed like code; if your change legitimately shifts a snapshot, re-accept
  it deliberately rather than hand-editing the `.snap` file.

## Commit convention

We use conventional commits:

```
feat: add trace --terminals query
fix: dedupe findings in branch-set overlap check
docs: clarify profile overlay semantics
test: add FP guard for legal guardedBy edge
refactor: extract edge kind table
chore: bump insta
```

Keep the subject imperative and under ~72 characters; use the body for the
why when it is not obvious.

## Pull requests

- Branch from `main`; keep PRs small and focused on one change.
- Tests first where practical: write the failing check, watch it fail, then
  make it pass. "Compiles" or "ran without error" is not done; the gate above
  is.
- No drive-by refactors or unrelated cleanups in the same PR.
- Extensibility is declarative data (schema overlays, declarative lint
  rules), never executable plugins. If a feature seems to need a code plugin,
  open an issue first.
- CI must be green before review.

## License

By contributing you agree that your contributions are licensed under the MIT
License (see `LICENSE`).
