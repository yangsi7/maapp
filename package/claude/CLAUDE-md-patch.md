# maapp routing patch

Paste everything below the line into the consumer repo's CLAUDE.md or AGENTS.md.
An unwired graph does not get read; this section is what routes agents to it.

---

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
graph nodes through the CRUD verbs (never hand-edit; the verbs emit canonical
form) -> `maapp validate .maapp/graph.json` -> `maapp fmt .maapp/graph.json
--check` (canonical-form gate; run `maapp fmt .maapp/graph.json` to fix a hand
edit) -> `maapp check-drift .maapp/graph.json --repo .` -> when green,
`maapp stamp .maapp/graph.json --repo .`. CI fails the PR on validation errors,
a non-canonical graph, or unresolved drift.
