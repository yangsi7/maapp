# maapp

<!-- The crates.io badge activates after the first crates.io publish. -->
[![CI](https://github.com/yangsi7/maapp/actions/workflows/ci.yml/badge.svg)](https://github.com/yangsi7/maapp/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/maapp.svg)](https://crates.io/crates/maapp)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**maapp turns an app's screens, flows, state, navigation and side-effects into a typed
knowledge graph an AI coding agent queries instead of reading source or prose.**

![maapp validates a 65-node checkout graph, queries the blast radius of its checkout store, and renders the backend pull-spine view, all from one JSON file](demo/demo.gif)

> Early release. The install one-liners activate once the first tagged release and the
> crates.io publish land; until then, build from source. MIT licensed.

## Quickstart

Install (no sudo, lands in `~/.local/bin`):

```sh
# activates once the first tagged release is published
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/yangsi7/maapp/releases/latest/download/maapp-installer.sh | sh
# until then: build from source
git clone https://github.com/yangsi7/maapp && cd maapp && ./install.sh
```

Validate a graph. The repo ships 8 example app graphs under [`examples/`](examples/);
all of them validate clean:

```console
$ maapp validate examples/checkout.json
CLEAN — examples/checkout.json: 0 errors, 0 warnings. (65 nodes, 88 edges)
```

Ask what breaks before you touch a store:

```console
$ maapp query blast-radius store:CheckoutStore examples/checkout.json
BLAST-RADIUS store:CheckoutStore
  DEPENDENTS (depend ON it; break if it changes) [10]:
    act:address/SaveAndContinue
    act:card/SaveCard
    act:pay/SelectMethod
    act:review/PlaceOrder
    act:shipping/Continue
    assert:pay/CardSelected
    assert:pay/WalletSelected
    assert:shipping/AddressPresent
    comp:checkout/AddressCard
    screen:checkout/Payment
  DEPENDENCIES (it depends on) [0]:
```

Render the backend pull spine, which surface pulls which store, down to the
backend-op floor:

```console
$ maapp render spine examples/checkout.json
BACKEND PULL SPINE — STALACTITE VIEW  (downward data-pull only)
================================================================================
...
PER-COLUMN PULL SUMMARY
--------------------------------------------------------------------------------
surface                  depth  fanin floor  ticks
LineItemList             1      1     .      #
OrderSummary             1      1     .      #
AddressCard              1      1     .      #
AddressForm              1      0     .
Complete                 3      1     YES    #
Payment                  1      1     .      #

STALACTITE GRID  (shared substrate rows; | = spine, o = tick,
                  # = stage/op tick)
--------------------------------------------------------------------------------
                    |LI OS AC AF CO PA
                    +------------------
SURFACE             |[] [] [] [] [] []
  bind[0]           | o  o  o  o  o  o
  OrderStore        |             o
op:PlaceOrder       |             #
                    +==================  <- floor (BackendOps)
...
```

Add `--json` to `validate` or any query for byte-stable machine-readable output. Bare
`maapp` prints the full help, and every subcommand's `--help` ends with copy-runnable
examples.

## Why

- **Typed, validated, not remembered.** Screens, flows, stores, navigation, guards and
  side-effects are typed nodes and edges under a versioned schema. `maapp validate`
  enforces the contract: on the eval battery it catches 25 of 25 injected defect
  classes with zero false positives.
- **Scoped structural queries.** Blast-radius before a refactor, trace from a UI
  trigger to the backend, N-hop neighborhoods, nav topology, orphans. An agent pulls
  the slice it needs instead of reading whole files, or the whole graph.
- **A living loop with drift detection.** `maapp check-drift` maps the graph's source
  anchors onto the commits since the last `maapp stamp`, so a stale graph tells on
  itself instead of silently rotting. A CI gate and a Claude Code hook back it up.
- **Agent harness wired in one command.** `maapp init` creates `.maapp/`, installs the
  drift-nudge hook, and adds a routing section to `CLAUDE.md` or `AGENTS.md`.
  Idempotent, previewable with `--dry-run`.
- **Profiles are data, never plugins.** Stack- or category-specific vocabulary and lint
  rules ship as declarative schema overlays; there is no code-execution surface.
- **Deterministic `--json`.** Byte-stable output with no timestamps, floats, randomness
  or absolute paths in any artifact; safe to snapshot, diff and gate in CI.

## Use cases

When to reach for maapp, each with a command that runs against the shipped `examples/`:

- **Blast radius before a refactor.** See every node that depends on a store or component
  before you touch it, so a rename or deletion never breaks something silently.
  `maapp query blast-radius store:CheckoutStore examples/checkout.json`
- **Trace a UI bug to its backend operation.** Follow a screen through its triggers,
  actions and stores down to the backend op it invokes, without opening a source file.
  `maapp query trace screen:checkout/Review examples/checkout.json`
- **Onboard an agent to an unfamiliar codebase.** Hand a cold agent the whole-app
  blueprint hub as its first read: screens, flows and stores at a glance, not grepped source.
  `maapp render hub examples/dashboard.json`
- **Drift detection at review time.** After a feature lands, check the graph's anchors
  against the repo so a stale map tells on itself before anyone trusts it.
  `maapp check-drift examples/checkout.json --repo . --since HEAD`
- **Spec-first blueprint an agent builds against.** Turn a PRD into a validated graph
  first, then read it back as the screen-by-screen plan the agent implements to.
  `maapp render storyboard examples/wizard.json`

## Benchmarks

Generated graphs hold 93% of the human cross-author agreement ceiling on held-out apps,
backed by a validator that catches 25 of 25 injected defect classes with zero false
positives and an engine that is byte-identical to its proven prototype; see
[docs/BENCHMARKS.md](docs/BENCHMARKS.md) for every number, including the negative
real-codebase pilot we published anyway.

| Result | Value | n | Details |
|---|---|---|---|
| Held-out spec-to-graph parity | 0.9306 (bar 0.90, PASS; all graphs validate 0/0) | 3 held-out apps, 6 seeded runs | [BENCHMARKS](docs/BENCHMARKS.md#held-out-generation-parity-spec-to-graph) |
| Validator mutation recall / FP | 25/25 recall, 0/7 false positives | 25 injectors, 7 valid graphs | [BENCHMARKS](docs/BENCHMARKS.md#validator-mutation-battery) |
| Deterministic ground-truth battery | 21/21 | 21 tasks | [BENCHMARKS](docs/BENCHMARKS.md#deterministic-ground-truth-battery) |
| Rust vs prototype-oracle differential | 0 divergences | 19 committed CI differential snapshots | [BENCHMARKS](docs/BENCHMARKS.md#rust-vs-prototype-oracle-differential) |
| Real-codebase ingest pilot | K1 PASS, K2 FAIL (baseline at ceiling; graph won only the 2 hard tasks, +20/+25 pp) | 48 runs (12 tasks x 2 arms x 2 seeds) | [BENCHMARKS](docs/BENCHMARKS.md#real-codebase-ingest-pilot) |

Every row links to its protocol, sample size, reproduce command and limitations.
Negative results are published with the same prominence as positive ones.

## Agent setup (3 steps)

**1. Install the CLI.**

Use the Quickstart one-liner above or any rung of the [install ladder](#install) below.
No sudo: installs to `~/.local/bin` (override with `MAAPP_INSTALL`). Verify with
`maapp --version`.

**2. Wire your agent.**

```sh
maapp init --dry-run   # preview, touches nothing
maapp init             # idempotent; re-running is all skips
```

![maapp init --dry-run previews the project wiring: .maapp/ home, drift-nudge hook, settings merge, AGENTS.md routing](demo/init.gif)

Creates `.maapp/`, installs the drift-nudge hook and merges it into
`.claude/settings.json` when `.claude/` exists (the original is backed up to
`.claude/settings.json.bak`), and adds a marker-fenced routing section to `CLAUDE.md`
(else `AGENTS.md`, created if needed). Claude Code users add the `/maapp` skill with
`/plugin marketplace add yangsi7/maapp` then `/plugin install maapp@maapp`. Cursor,
OpenAI Codex (via the `AGENTS.md` convention), and every other harness: see
[docs/INTEGRATIONS.md](docs/INTEGRATIONS.md).

**3. Init your graph.**

Author or ingest a graph with the `/maapp` skill into `.maapp/graph.json`, then:

```sh
maapp validate .maapp/graph.json
maapp stamp .maapp/graph.json --repo .
```

Commit the stamped graph. The living loop keeps it honest from there: validate,
check-drift, stamp (green means 0 stale candidates and 0 rotted anchors).

## Install

All paths install to `~/.local/bin`; override with `MAAPP_INSTALL`. No sudo, ever.

**1. Shell one-liner** (macOS / Linux):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/yangsi7/maapp/releases/latest/download/maapp-installer.sh | sh
```

Windows:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/yangsi7/maapp/releases/latest/download/maapp-installer.ps1 | iex"
```

Prefer an auditable script? This one checksum-verifies the release artifact and falls
back to a source build:

```sh
curl -fsSL https://raw.githubusercontent.com/yangsi7/maapp/main/install.sh | sh
```

**2. cargo binstall** (after the crates.io publish):

```sh
cargo binstall maapp
```

Release assets are named `maapp-<target-triple>.tar.xz`, matching binstall's defaults,
so no extra metadata is needed.

**3. cargo install** (after the crates.io publish):

```sh
cargo install maapp
```

**4. Build from source** (needs Rust):

```sh
git clone https://github.com/yangsi7/maapp && cd maapp && ./install.sh
```

Runs `cargo build --release --locked` under the hood.

Prebuilt binaries cover Apple Silicon and Intel macOS, x86_64 Linux (gnu), and x86_64
Windows. Releases are built by dist (cargo-dist) v0.32.0 on tag push; every artifact
ships with a `.sha256` and a unified `sha256.sum`.

## Docs

- [docs/BENCHMARKS.md](docs/BENCHMARKS.md): every measured number, with protocol,
  sample size, reproduce command and limitations per result
- [docs/INTEGRATIONS.md](docs/INTEGRATIONS.md): wiring maapp into Claude Code, Cursor,
  Codex / AGENTS.md agents, and any other harness
- [docs/ADOPTION.md](docs/ADOPTION.md): cutting over from bespoke graph tooling, cold-starting
  a graph on an existing app, and the drift-baseline ratchet
- [CONTRIBUTING.md](CONTRIBUTING.md): dev setup and the 5-command verification gate
  (fmt, clippy -D warnings, nextest + doc tests, cargo deny, wasm32 build)
- [examples/](examples/): 8 example app graphs (chat, checkout, dashboard, maps, media,
  onboarding-variant, settings-account, wizard); all validate clean

## License

MIT; see [LICENSE](LICENSE). Copyright (c) 2026 Simon Yang.
