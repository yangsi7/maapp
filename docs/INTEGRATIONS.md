# Harness integrations

How to wire `maapp` into your coding agent. One convention is shared by every path below:
the graph lives at `$MAAPP_GRAPH` if set (absolute, or relative to the repo root), else
`.maapp/graph.json`. The hook, the routing section, and the CI gate all resolve it the same way.

## 1. Claude Code (recommended)

Install the CLI, then let `maapp init` wire the repo:

```sh
curl -fsSL https://raw.githubusercontent.com/yangsi7/maapp/main/install.sh | sh
maapp init --dry-run   # preview: prints "would write / would update", touches nothing
maapp init
```

**Agent note.** Some harnesses block piping a remote script to `sh`. The manual fallback:
download the release tarball for your platform (asset names follow
`maapp-<target-triple>.tar.xz`, e.g. `maapp-aarch64-apple-darwin.tar.xz`, plus a
`.zip` for `x86_64-pc-windows-msvc`), verify it against the matching `.sha256` next to
it, extract, and put the `maapp` binary on `PATH` (e.g. `~/.local/bin`):

```sh
curl -LsSfO https://github.com/yangsi7/maapp/releases/latest/download/maapp-aarch64-apple-darwin.tar.xz
curl -LsSfO https://github.com/yangsi7/maapp/releases/latest/download/maapp-aarch64-apple-darwin.tar.xz.sha256
shasum -a 256 -c maapp-aarch64-apple-darwin.tar.xz.sha256 && tar -xf maapp-aarch64-apple-darwin.tar.xz
install -m 755 maapp-aarch64-apple-darwin/maapp ~/.local/bin/maapp
```

`maapp init` writes, in fixed order:

1. `.maapp/` (the graph home; no graph file is authored, that is the `/maapp` skill's job).
2. When `.claude/` exists: `.claude/hooks/maapp-drift-nudge.js` (a PostToolUse hook on
   `Edit|Write` that injects a one-line nudge when an edit touches a file anchored by a
   graph node, or when the graph has drifted; fails silent, never blocks a session), and
   one matching `hooks.PostToolUse` entry merged into `.claude/settings.json`. Every
   existing key is preserved, and the original file is backed up to
   `.claude/settings.json.bak` before the first modification. An unparseable
   `settings.json` aborts the whole run (exit 2) before anything is written. When
   `.claude/` is absent, init prints a note and skips this step (it never creates `.claude/`).
3. A routing section fenced by `<!-- maapp:begin -->` / `<!-- maapp:end -->` markers, into
   `CLAUDE.md` if present, else `AGENTS.md` if present, else a new `AGENTS.md`. Only the
   region between the markers is owned by maapp; everything outside is never touched.
4. With `--ci`: `.github/workflows/maapp-gate.yml`, a fail-closed PR gate running
   `maapp validate` + `maapp check-drift` (fill in its pinned-binary install TODO).

Idempotent: re-running is all skips (`exists, identical`, `hook already wired`,
`section up to date`). A maapp-owned file you have modified is skipped with a warning;
`--force` overwrites it. `--dir <path>` targets another directory.

For the `/maapp` skill (graph authoring, ingest, queries) add the plugin, which also
bundles the same hook:

```
/plugin marketplace add yangsi7/maapp
/plugin install maapp@maapp
```

## 2. AGENTS.md agents (OpenAI Codex, Amp, and friends)

`maapp init` covers this case automatically when the repo has no `CLAUDE.md`: it creates
or patches `AGENTS.md` with the marker-fenced routing section. If your repo has BOTH files,
init patches `CLAUDE.md` only; paste this block into `AGENTS.md` yourself:

```markdown
<!-- maapp:begin -->
## App-structure graph (maapp)

This repo keeps a typed app-structure graph at `.maapp/graph.json` (override via
the `MAAPP_GRAPH` env var). It is the agent's structural memory: screens, flows,
state, navigation, guards, and side-effects as typed nodes and edges.

**Use it for structural questions, through scoped queries only. Never read the
whole graph JSON into context.**

- Blast radius before a refactor: `maapp query blast-radius <node-id> .maapp/graph.json`
- Trace a UI element to its backend: `maapp query trace <node-id> .maapp/graph.json`
- Enumerate guards and policies: `maapp query view assertion .maapp/graph.json`
- Nav topology: `maapp query view nav .maapp/graph.json`
- One node with its edges: `maapp query node <node-id> .maapp/graph.json`
- Local neighborhood: `maapp query neighbors <node-id> .maapp/graph.json --depth 2`

Add `--json` to any query for machine-readable output.

**Do NOT use the graph for:** symbol or text search (use grep, LSP, or the code
index), API and library documentation (use the project docs), or anything a
single file read answers faster.

**Maintenance loop (keep the graph honest):** edit code -> update the touched
graph nodes -> `maapp validate .maapp/graph.json` -> `maapp check-drift
.maapp/graph.json --repo .` -> when green, `maapp stamp .maapp/graph.json --repo .`.
CI fails the PR on validation errors or unresolved drift.
<!-- maapp:end -->
```

## 3. Cursor

Cursor loads project rules from `.cursor/rules/*.mdc`; a plain `.md` file in that
directory is ignored. Create `.cursor/rules/maapp.mdc`:

```markdown
---
description: App-structure graph (maapp) routing, scoped structural queries + maintenance loop
alwaysApply: true
---

This repo keeps a typed app-structure graph at `.maapp/graph.json` (override via the
`MAAPP_GRAPH` env var): screens, flows, state, navigation, guards, and side-effects as
typed nodes and edges. Use it for structural questions, through scoped queries only.
Never read the whole graph JSON into context.

- Blast radius before a refactor: `maapp query blast-radius <node-id> .maapp/graph.json`
- Trace a UI element to its backend: `maapp query trace <node-id> .maapp/graph.json`
- Guards and policies: `maapp query view assertion .maapp/graph.json`
- Nav topology: `maapp query view nav .maapp/graph.json`
- One node with its edges: `maapp query node <node-id> .maapp/graph.json`
- Local neighborhood: `maapp query neighbors <node-id> .maapp/graph.json --depth 2`

Add `--json` to any query for machine-readable output. Do NOT use the graph for symbol
or text search, API docs, or anything a single file read answers faster.

Maintenance loop: after editing code, update the touched graph nodes, then run
`maapp validate .maapp/graph.json` and `maapp check-drift .maapp/graph.json --repo .`;
when green, `maapp stamp .maapp/graph.json --repo .` and commit the stamped graph.
```

Cursor also reads `AGENTS.md`, so the section-2 block works there too; the `.mdc` rule is
the native, always-applied form.

## 4. Any other harness (manual fallback)

Two pieces, both plain files:

1. **Routing.** Paste the section-2 marker block into whatever instruction file your
   harness always loads. An unwired graph does not get read; this block is what routes
   the agent to it.
2. **Staleness signal.** Claude Code gets it as a hook: after every `Edit`/`Write`,
   `package/claude/hooks/maapp-drift-nudge.js` runs `maapp check-drift <graph> --repo . --json`
   plus an in-session anchor match on the edited file, and injects a one-line nudge
   ("N node(s) may be stale after this edit") only when there is something to say. If your
   harness has a post-edit hook mechanism, port that script (plain Node, no dependencies);
   if not, rely on the routing block's maintenance loop and add the CI gate
   (`package/ci/maapp-gate.yml`) as the authoritative backstop.

## Anchoring

A node with no `refs.source` is invisible to `check-drift`: the check can only compare a
graph's claims against the repo for nodes it knows which file backs them, so an
unanchored graph reports nothing (no stale candidates, no anchor rot) and that silence is
noise, not a clean bill of health. Anchor a node when you add or touch it:

```sh
maapp add-node store:chat/DraftStore examples/chat.json \
    --kind StateStore --intent "Unsent drafts per conversation" \
    --ref source=src/stores/drafts.ts

maapp update-node screen:checkout/Payment examples/checkout.json \
    --ref source=app/checkout/payment/page.tsx
```

`--ref k=v` is repeatable and available on both `add-node` and `update-node`; `source` is
the conventional key `check-drift` reads, but `refs` is an open map, so an app can also
carry `test`, `story`, or any other pointer it finds useful.

**Incremental anchoring.** Do not stop and anchor every node in one pass; anchor
`refs.source` on the nodes a change actually touches, as part of that change, then
`maapp stamp` re-pins `meta.provenance.asOf` to the anchoring commit. A graph anchors
itself over time this way, one touched slice at a time, and `check-drift`'s signal only
gets sharper as coverage grows: more anchored nodes means more of the repo's real churn
is checked against the graph's claims, instead of passing through as unmapped.

## The living loop (all harnesses)

The graph stays honest through three verbs, run from the repo root:

```sh
maapp validate .maapp/graph.json               # exit 0 clean, 1 on E_* errors, 2 load error
maapp check-drift .maapp/graph.json --repo .   # exit 0 fresh, 1 drift, 2 error
maapp stamp .maapp/graph.json --repo .         # pins meta.provenance.asOf to HEAD
```

Cadence after a code change:

1. Update the graph nodes the change touched (`maapp update-node`, `add-node`, `add-edge`, ...).
2. `maapp validate .maapp/graph.json` until it reports 0 errors.
3. `maapp check-drift .maapp/graph.json --repo .` and read the buckets: **green means
   STALE CANDIDATES = 0 and ANCHOR ROT = 0.** Unmapped changes are files no anchor
   covers; an unmapped-only report naming just the graph file itself is expected right
   after committing a stamped graph (that commit is the one commit past `asOf`).
4. When green: `maapp stamp .maapp/graph.json --repo .` and commit the stamped graph.

`stamp` only bumps `meta.provenance.asOf`; the graph must already carry a
`meta.provenance` object (the `/maapp` skill authors it). `check-drift` sees committed
history only (`asOf..HEAD`); the drift-nudge hook is what catches uncommitted in-session
edits. If the graph lives elsewhere, set `MAAPP_GRAPH` (for Claude Code, via the `env`
block of `.claude/settings.json`) and substitute your path in every command above.
