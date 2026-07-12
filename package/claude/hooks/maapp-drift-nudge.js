#!/usr/bin/env node
/**
 * maapp-drift-nudge.js — PostToolUse hook (matcher: Edit|Write) for a consumer
 * repo that keeps a maapp app-structure graph as agent structural memory.
 *
 * WHAT IT DOES
 *   After every Edit/Write it (a) runs `maapp check-drift <graph> --repo . --json`
 *   (cheap, local, read-only git) and (b) checks whether the just-edited file is
 *   named by any node's `refs.source` anchor in the graph. check-drift only sees
 *   COMMITTED history (asOf..HEAD), so (b) is what catches the in-session edit.
 *   On drift OR an anchored edit it injects a one-line nudge into the agent's
 *   context via the documented PostToolUse `hookSpecificOutput.additionalContext`
 *   pattern, so the agent updates the touched nodes instead of letting the graph rot.
 *
 * GRAPH LOCATION CONVENTION (one rule, shared with the routing patch + CI gate)
 *   $MAAPP_GRAPH if set (absolute, or relative to the project root), else
 *   `.maapp/graph.json` at the project root. Set MAAPP_GRAPH via the "env" block
 *   of .claude/settings.json if the graph lives elsewhere.
 *
 * QUIET-MODE FOR UNANCHORED GRAPHS (T1b)
 *   A graph with ZERO `refs.source` anchors makes `check-drift`'s "unmapped"
 *   count grow with every commit past meta.provenance.asOf, so the hook would
 *   otherwise fire an "N unmapped change(s)" nudge on EVERY Edit/Write — pure
 *   noise that trains ignoring. When the anchored-node count is 0 the hook
 *   SUPPRESSES the unmapped-driven nudge and instead emits ONE quiet "graph is
 *   unanchored — anchor per slice" hint per session. STALE/ROT nudges require
 *   anchors, so they are structurally impossible here and unaffected. Per-session
 *   dedupe uses a zero-byte marker file keyed by (session_id, graph path); the
 *   marker dir is `os.tmpdir()`, overridable via MAAPP_NUDGE_STATE_DIR (tests
 *   pin it for hermetic runs). If the graph is too large to parse or unparseable,
 *   the anchored count is UNKNOWN and the hook keeps its legacy drift-only
 *   behavior (never enters quiet-mode on a graph it cannot read).
 *
 * FAILURE CONTRACT — SILENT, ALWAYS
 *   A broken hook must never block the consumer's session. This hook exits 0
 *   with NO output when: the maapp binary is not on PATH, the graph file does
 *   not exist, git or provenance is unusable (check-drift exit 2), stdin is not
 *   valid hook JSON, the subprocess times out, or anything at all throws. The
 *   ONLY output it ever produces is the single additionalContext JSON object on
 *   a genuine nudge. Repeated nudges across edits are by design (one line each).
 *
 * INSTALL
 *   1. Copy this file to <repo>/.claude/hooks/maapp-drift-nudge.js
 *   2. Merge this block into <repo>/.claude/settings.json
 *      (also shipped as package/claude/settings-snippet.json):
 *      {
 *        "hooks": {
 *          "PostToolUse": [
 *            {
 *              "matcher": "Edit|Write",
 *              "hooks": [
 *                {
 *                  "type": "command",
 *                  "command": "node \"$CLAUDE_PROJECT_DIR/.claude/hooks/maapp-drift-nudge.js\"",
 *                  "timeout": 15
 *                }
 *              ]
 *            }
 *          ]
 *        }
 *      }
 *
 * No dependencies. Node >= 14 (fs, path, child_process only).
 */
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');

/** Max graph size we are willing to JSON.parse for anchor matching (bytes).
 *  Above this, the check-drift result alone still drives the nudge. */
const MAX_GRAPH_BYTES = 25 * 1024 * 1024;
/** Wall-clock budget for the check-drift subprocess (ms). The settings-level
 *  hook timeout (15 s) is the outer backstop. */
const DRIFT_TIMEOUT_MS = 8000;

/**
 * The file-path part of a `refs.source` anchor: strip the `#L<n>[-L<m>]`
 * fragment or the `@<symbol>` suffix. Mirrors the engine's split order
 * (first `#`, else first `@`) so the hook and the CLI never disagree on
 * which file an anchor names.
 */
function anchorPath(anchor) {
  const hash = anchor.indexOf('#');
  if (hash !== -1) return anchor.slice(0, hash);
  const at = anchor.indexOf('@');
  if (at !== -1) return anchor.slice(0, at);
  return anchor;
}

/** A node is any object carrying a string `kind` or an object `refs`. */
function isNode(v) {
  return (
    v !== null &&
    typeof v === 'object' &&
    !Array.isArray(v) &&
    (typeof v.kind === 'string' || (v.refs !== null && typeof v.refs === 'object'))
  );
}

/**
 * Collect [slug, anchorFilePath] pairs from a graph document. Handles both
 * node layouts defensively: flat `{ slug: node }` and layered
 * `{ layer: { slug: node } }`. `refs.source` may be one string or an array
 * of strings; anything else is validate's problem, not this hook's.
 */
function collectAnchors(graph) {
  const out = [];
  const nodes = graph && graph.nodes;
  if (nodes === null || typeof nodes !== 'object' || Array.isArray(nodes)) return out;
  const addNode = (slug, node) => {
    const src = node.refs && node.refs.source;
    const list = typeof src === 'string' ? [src] : Array.isArray(src) ? src : [];
    for (const a of list) {
      if (typeof a === 'string' && a.length > 0) out.push([slug, anchorPath(a)]);
    }
  };
  for (const [key, val] of Object.entries(nodes)) {
    if (isNode(val)) {
      addNode(key, val); // flat: key is the slug
    } else if (val !== null && typeof val === 'object' && !Array.isArray(val)) {
      for (const [slug, node] of Object.entries(val)) {
        if (isNode(node)) addNode(slug, node); // layered: key was the layer
      }
    }
  }
  return out;
}

/** Emit the single documented additionalContext object (one line of context).
 *  The hook writes AT MOST one of these per invocation. */
function emitContext(msg) {
  process.stdout.write(
    JSON.stringify({
      hookSpecificOutput: {
        hookEventName: 'PostToolUse',
        additionalContext: msg,
      },
    })
  );
}

/** Directory for per-session dedupe markers: MAAPP_NUDGE_STATE_DIR if set,
 *  else the OS temp dir. The override keeps tests hermetic. */
function nudgeStateDir() {
  const override = process.env.MAAPP_NUDGE_STATE_DIR;
  return typeof override === 'string' && override.length > 0 ? override : os.tmpdir();
}

/** Small stable hex hash (djb2) — keeps markers for different graphs/repos
 *  apart without pulling in `crypto`. */
function shortHash(s) {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
  return (h >>> 0).toString(16);
}

/** The "already hinted this session" marker path, keyed by (session, graph). */
function unanchoredMarker(sessionId, graphAbs) {
  const sid = typeof sessionId === 'string' && sessionId ? sessionId : 'nosession';
  return path.join(nudgeStateDir(), `maapp-nudge-unanchored-${sid}-${shortHash(graphAbs)}.seen`);
}

/**
 * Quiet-mode hint for an UNANCHORED graph: emit the one-per-session "anchor per
 * slice" line, replacing the per-edit unmapped nudge. Best-effort dedupe — a
 * marker read/write failure at worst re-hints, and never throws out (the caller
 * is inside the silent try/catch regardless).
 */
function hintUnanchoredOncePerSession(sessionId, graphAbs, graphDisp, unmapped) {
  const marker = unanchoredMarker(sessionId, graphAbs);
  let seen = false;
  try {
    seen = fs.existsSync(marker);
  } catch (_e) {
    seen = false; // prefer hinting to crashing on a stat error
  }
  if (seen) return;
  emitContext(
    `maapp: graph has no refs.source anchors — drift detection is inactive ` +
      `(${unmapped} committed change(s) unmapped by design until you anchor). ` +
      `Anchor the touched nodes per slice, then \`maapp stamp ${graphDisp} --repo .\`.`
  );
  try {
    fs.writeFileSync(marker, '');
  } catch (_e) {
    // dedupe is best-effort; never let a write failure block the session.
  }
}

function main() {
  // 1. Hook payload arrives on stdin as one JSON object.
  const input = JSON.parse(fs.readFileSync(0, 'utf8'));
  const toolInput = input.tool_input || {};
  const editedAbs = toolInput.file_path;
  if (typeof editedAbs !== 'string' || editedAbs.length === 0) return;

  // 2. Resolve the graph per the one convention. No graph file -> silent.
  const root = typeof input.cwd === 'string' && input.cwd ? input.cwd : process.cwd();
  const graphAbs = process.env.MAAPP_GRAPH
    ? path.resolve(root, process.env.MAAPP_GRAPH)
    : path.join(root, '.maapp', 'graph.json');
  let stat;
  try {
    stat = fs.statSync(graphAbs);
  } catch (_e) {
    return;
  }
  if (!stat.isFile()) return;

  // Editing the graph itself IS the maintenance step — never nudge on it.
  if (path.resolve(editedAbs) === graphAbs) return;

  // 3. (a) Committed drift since meta.provenance.asOf — the accumulated debt.
  //    spawnSync never throws on a missing binary; it reports error (ENOENT).
  const drift = spawnSync('maapp', ['check-drift', graphAbs, '--repo', root, '--json'], {
    cwd: root,
    encoding: 'utf8',
    timeout: DRIFT_TIMEOUT_MS,
  });
  if (drift.error) return; // binary absent or timed out -> silent
  if (drift.status !== 0 && drift.status !== 1) return; // exit 2 = git/provenance/input error -> silent

  let report = null;
  if (drift.status === 1) {
    try {
      report = JSON.parse(drift.stdout);
    } catch (_e) {
      return;
    }
  }

  // 4. Parse the graph ONCE (size-guarded) so we know both the anchored-node
  //    COUNT (the quiet-mode decision) and the anchors for local stale-matching.
  //    Oversized/unparseable -> anchors = null (UNKNOWN): the hook keeps its
  //    legacy drift-only behavior and never enters quiet-mode blind.
  let anchors = null;
  if (stat.size <= MAX_GRAPH_BYTES) {
    try {
      anchors = collectAnchors(JSON.parse(fs.readFileSync(graphAbs, 'utf8')));
    } catch (_e) {
      anchors = null;
    }
  }
  const anchoredCount = anchors ? new Set(anchors.map((a) => a[0])).size : null;

  // 5. (b) Is the just-edited file anchored by any node's refs.source?
  const staleNodes = new Set();
  const rel = path.relative(root, editedAbs).split(path.sep).join('/');
  if (anchors && rel && !rel.startsWith('..') && !path.isAbsolute(rel)) {
    for (const [slug, p] of anchors) {
      if (p === rel) staleNodes.add(slug);
    }
  }

  // 6. Merge the two signals.
  let unmapped = 0;
  let rot = 0;
  if (report) {
    for (const s of report.stale_candidates || []) {
      if (s && typeof s.node === 'string') staleNodes.add(s.node);
    }
    unmapped = (report.unmapped_changes || []).length;
    rot = (report.anchor_rot || []).length;
  }

  const graphDisp = path.relative(root, graphAbs).split(path.sep).join('/') || graphAbs;

  // 6a. QUIET-MODE (T1b): an UNANCHORED graph makes `unmapped` fire on every
  //     edit. With 0 anchors, stale/rot are structurally impossible, so
  //     `unmapped` is the only live signal — suppress its per-edit nudge and
  //     emit ONE per-session "anchor per slice" hint instead.
  if (anchoredCount === 0) {
    if (unmapped > 0) {
      hintUnanchoredOncePerSession(input.session_id, graphAbs, graphDisp, unmapped);
    }
    return; // never emit the verbose unmapped nudge for an unanchored graph
  }

  // 7. Anchored (or unknown) graph: stay silent when there is nothing to say.
  if (staleNodes.size === 0 && unmapped === 0 && rot === 0) return;

  const sample = [...staleNodes].sort().slice(0, 3).join(', ');
  const bits = [];
  if (staleNodes.size > 0) {
    const suffix = staleNodes.size > 3 ? ', ...' : '';
    bits.push(`${staleNodes.size} node(s) may be stale after this edit (${sample}${suffix})`);
  }
  if (unmapped > 0) bits.push(`${unmapped} unmapped change(s)`);
  if (rot > 0) bits.push(`${rot} rotted anchor(s)`);
  const msg =
    `maapp: ${bits.join('; ')} — run \`maapp check-drift ${graphDisp} --repo .\` / ` +
    `update the touched nodes, then \`maapp validate ${graphDisp}\`.`;

  // 8. The documented PostToolUse additionalContext shape (exit 0 + stdout JSON).
  emitContext(msg);
}

try {
  main();
} catch (_e) {
  // Silent by contract: a broken hook must never block the session.
}
process.exit(0);
