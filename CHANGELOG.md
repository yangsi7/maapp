# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-07-11

Initial public release. maapp turns an app's screens, flows, state, navigation, and
side-effects into a typed, validated knowledge graph that an AI coding agent queries
instead of reading source or prose.

### Added

- **`validate`** — lint a graph to zero errors: a typed vocabulary of `E_*` hard-fail
  codes plus advisory `W_*` warnings, exercised by a 25-injector mutation battery
  (recall 25/25, false positives 0/7).
- **`query`** — scoped reads and traversals: `blast-radius`, `trace`, `neighbors`,
  `node`, `orphans`, and `view` (nav / dependency / assertion), each answerable without
  loading the whole graph into an agent's context. `--json` on every query.
- **`render`** — human- and agent-readable views: `hub` (markdown blueprint), `deps`,
  `storyboard`, `spine`, and a standalone interactive `html` visualization.
- **`export`** — carve out a self-contained, valid, dispatchable slice of the graph so a
  cold agent reads a neighborhood rather than the entire document.
- **`init`** — wire a repository for the graph loop: the `.maapp/` home, a Claude Code
  drift-nudge hook with a settings merge, a marker-fenced routing section in
  `CLAUDE.md`/`AGENTS.md`, and an optional CI gate. Idempotent; never overwrites files
  you have modified without `--force`.
- **Living loop** — keep the graph honest as code changes: `add-node`, `update-node`,
  `remove-node` (with `--cascade`), `add-edge`, `remove-edge` (atomic, validator-checked
  writes), `diff` (semantic graph diff), `check-drift` (anchors vs. the source repo since
  the last stamp), and `stamp` (pin provenance to a source revision).
- **`schema`** — emit the canonical maapp-graph JSON Schema (draft 2020-12), generated
  from the engine's own vocabulary tables and byte-locked to the committed copy at
  `schema/maapp-graph.schema.json`.
- **Deterministic `--json` contract** — canonical, byte-stable serialization (sorted
  nodes and edges, no wall-clock, RNG, or environment leakage), regression-guarded by
  frozen output snapshots.
- **wasm32 target** — the core library builds for `wasm32-unknown-unknown`
  (`--no-default-features`); the native CLI feature layers on top of a wasm-clean core.
- **Eight example app graphs** under `examples/`, all validating cleanly, plus a
  one-line installer (`install.sh`) and Claude Code / Cursor / AGENTS.md integrations.

[0.1.0]: https://github.com/yangsi7/maapp/releases/tag/v0.1.0
