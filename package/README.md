# maapp install package

> **`maapp init` now automates these steps** (hook, settings merge, routing patch,
> and with `--ci` the gate); run it in the consumer repo and you are done. The
> manual copy below remains as the transparent fallback, and these files are the
> single source the binary embeds.

Everything a consumer repo needs to adopt the maapp app-structure graph as agent
structural memory: the drift-nudge hook, the settings snippet, the CLAUDE.md
routing patch, and the CI gate. Each file is self-contained and copy-pasteable.

## What is in here

| File | What it is |
|---|---|
| `claude/hooks/maapp-drift-nudge.js` | PostToolUse hook (Edit and Write): injects a one-line nudge when an edit touches a file anchored by a graph node, or when the graph has drifted. Fails silent on any error; never blocks a session. |
| `claude/settings-snippet.json` | The exact `hooks` block to merge into the consumer repo's `.claude/settings.json`. |
| `claude/CLAUDE-md-patch.md` | The routing patch: a short CLAUDE.md / AGENTS.md section telling agents when to query the graph and when not to. An unwired graph does not get read. |
| `ci/maapp-gate.yml` | GitHub Actions PR gate: `maapp validate` (fail on errors) + `maapp check-drift` (fail on drift) + an advisory orphan report. Fail-closed. |

## Graph location convention

One rule, shared by the hook, the routing patch, and the CI gate:
`$MAAPP_GRAPH` if set (absolute, or relative to the repo root), else
`.maapp/graph.json` at the repo root.

## Install (3 steps)

1. **Binary.** Put the `maapp` CLI on PATH. Release binaries: TODO (not yet
   published). Until then, build from the maapp repo: `cargo build --release`,
   then copy `target/release/maapp` onto your PATH.
2. **Skill.** Install the `/maapp` agent skill (ships separately; see the maapp
   repo). The skill covers graph authoring and ingest; this package covers the
   consumption loop (routing, nudge, gate).
3. **Hook + routing patch + CI.**
   - Copy `claude/hooks/maapp-drift-nudge.js` to `<repo>/.claude/hooks/`.
   - Merge `claude/settings-snippet.json` into `<repo>/.claude/settings.json`.
   - Paste the section from `claude/CLAUDE-md-patch.md` into `<repo>/CLAUDE.md`
     (or `AGENTS.md`).
   - Copy `ci/maapp-gate.yml` to `<repo>/.github/workflows/` and fill in the
     install TODO inside it.

## The loop

```
        agent edits code
              |
              v
   PostToolUse hook fires ................ silent when nothing is stale
              |
              v   nudge: "N node(s) may be stale after this edit"
   agent updates the touched graph nodes
              |
              v
   maapp validate <graph> ................ 0 errors required
              |
              v
   maapp check-drift <graph> --repo . .... drift buckets must be empty
              |
              v
   maapp stamp <graph> --repo . .......... re-pins meta.provenance.asOf
              |
              v
   PR opens -> ci/maapp-gate.yml ......... only CI is authoritative
```

The hook is the nudge, the CLI is the loop, CI is the gate.
