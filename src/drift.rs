//! `maapp check-drift` + `maapp stamp` — LIFECYCLE-VERBS-SPEC §3 + §6.
//!
//! **THE ONLY disk-toucher in the verb set.** `check-drift` reads the graph
//! plus the named source repo via read-only git operations (`rev-parse`,
//! `diff --name-only`, `log`, `rev-list`) and working-tree path existence.
//! Everything else in maapp stays repo-independent — `validate` especially
//! NEVER grows a repo dependency (§3).
//!
//! ## Mechanics (§3, "flag, never decide")
//!
//! 1. `base = --since rev || meta.provenance.asOf` (neither ⇒ `NoDriftBase`,
//!    exit 2).
//! 2. `changed_files = git diff --name-only base..HEAD` within `--repo`
//!    (committed history only, per the spec's mechanics; rename detection is
//!    pinned OFF so output never depends on the machine's `diff.renames`).
//! 3. Every node whose `refs.source` anchor path intersects `changed_files`
//!    is a **stale candidate**, with the commits touching that path.
//! 4. Every changed file NO anchor covers is an **unmapped change** (the
//!    D1/D2 class: reality grew where the graph has no eyes). The spec scopes
//!    this to "the graph's ingest scope"; no scope carrier exists in the
//!    schema yet, so the whole changed set is reported — a conservative
//!    superset (no false negatives) with zero schema creep.
//! 5. Every anchor whose path no longer exists in the working tree is
//!    **anchor rot**.
//! 6. The report carries node/edge counts + `asOf` age (commits behind HEAD).
//!
//! Nodes without anchors are never silently skipped: they are the report's
//! `unanchored_nodes` bucket. That bucket is ADVISORY — §1 defines exit 1 as
//! "drift found", and an unanchored node is a blind spot, not drift — so it
//! never drives the exit code (resolves §1 vs §3 "any bucket" tension).
//!
//! ## Determinism
//!
//! Same repo state + same graph ⇒ byte-identical `--json`: revs resolve to
//! full SHAs, changed files land in a `BTreeSet`, buckets sort by their
//! natural keys ((node, anchor) / file / slug), and per-path commit lists use
//! `git log` order (newest → oldest — a pure function of the repo's history).
//! No mtimes anywhere (RES-004 friction #7): staleness derives from commit
//! CONTENT (git SHAs are content-addressed), never timestamps.
//!
//! ## The loop closes via `stamp` (§6.3)
//!
//! `check-drift` NEVER mutates the graph (§3 must-not), so the asOf bump is a
//! separate verb: `maapp stamp <graph> --repo <path> [--as-of <rev>]` writes
//! `meta.provenance.asOf = <resolved rev>` (default HEAD) in an atomic
//! temp-file + rename. Once the P1 mutation verbs land they bump `asOf` in
//! their own atomic writes (`--as-of`, §6.3); `stamp` remains the standalone
//! close for ingest/reconcile time.
//!
//! ## No oracle differential
//!
//! The Python oracle predates the lifecycle verbs and has no `check-drift`;
//! there is nothing to diff against. The contract here is pinned by the
//! hermetic temp-git-repo battery in `tests/drift.rs` instead.

use crate::error::EngineError;
use crate::graph::Graph;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

// ---------------------------------------------------------------------------
// Report shapes (spec §3 `--json`; struct order = key order = byte contract)
// ---------------------------------------------------------------------------

/// An anchored node whose anchor path was touched by a commit since `as_of`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaleCandidate {
    /// The node slug carrying the anchor.
    pub node: String,
    /// The anchor as authored (fragment/symbol kept verbatim).
    pub anchor: String,
    /// Commits touching the anchor's path, `git log` order (newest first).
    pub commits: Vec<String>,
}

/// A changed file NO anchor covers — reality grew where the graph has no eyes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnmappedChange {
    /// Repo-relative path from `git diff --name-only`.
    pub file: String,
    /// Commits touching the path, `git log` order (newest first).
    pub commits: Vec<String>,
}

/// An anchor whose path no longer exists in the repo's working tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnchorRot {
    pub node: String,
    pub anchor: String,
}

/// The `check-drift` report (§3). All buckets sorted; byte-stable `--json`.
#[derive(Debug, Serialize)]
pub struct DriftReport {
    /// The resolved base SHA (from `--since` or `meta.provenance.asOf`).
    pub as_of: String,
    /// The repo's resolved HEAD SHA.
    pub head: String,
    /// Commits reachable from `head` but not `as_of` (`rev-list --count`).
    pub commits_behind: usize,
    /// Graph node count (§3 mechanics step 6).
    pub nodes: usize,
    /// Graph edge count.
    pub edges: usize,
    /// Sorted by (node, anchor).
    pub stale_candidates: Vec<StaleCandidate>,
    /// Sorted by file.
    pub unmapped_changes: Vec<UnmappedChange>,
    /// Sorted by (node, anchor).
    pub anchor_rot: Vec<AnchorRot>,
    /// Node slugs with no `refs.source` anchor, sorted. ADVISORY: reported,
    /// never silently skipped, but never drives the exit code.
    pub unanchored_nodes: Vec<String>,
}

impl DriftReport {
    /// True iff drift was found: stale candidates, unmapped changes, or
    /// anchor rot. `unanchored_nodes` is advisory and deliberately excluded.
    #[must_use]
    pub fn has_drift(&self) -> bool {
        !(self.stale_candidates.is_empty()
            && self.unmapped_changes.is_empty()
            && self.anchor_rot.is_empty())
    }

    /// Human-readable rendering (the default surface; `--json` is machine).
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = vec![format!(
            "CHECK-DRIFT: as_of {} → head {} ({} commit(s) behind; {} node(s), {} edge(s))",
            short(&self.as_of),
            short(&self.head),
            self.commits_behind,
            self.nodes,
            self.edges
        )];
        lines.push(format!(
            "  STALE CANDIDATES [{}]:",
            self.stale_candidates.len()
        ));
        for s in &self.stale_candidates {
            lines.push(format!(
                "    {}  ({})  commits: {}",
                s.node,
                s.anchor,
                shorts(&s.commits)
            ));
        }
        lines.push(format!(
            "  UNMAPPED CHANGES [{}]:",
            self.unmapped_changes.len()
        ));
        for u in &self.unmapped_changes {
            lines.push(format!("    {}  commits: {}", u.file, shorts(&u.commits)));
        }
        lines.push(format!("  ANCHOR ROT [{}]:", self.anchor_rot.len()));
        for r in &self.anchor_rot {
            lines.push(format!("    {}  ({})", r.node, r.anchor));
        }
        lines.push(format!(
            "  UNANCHORED NODES [{}, advisory]:",
            self.unanchored_nodes.len()
        ));
        for n in &self.unanchored_nodes {
            lines.push(format!("    {n}"));
        }
        lines.push(if self.has_drift() {
            format!(
                "DRIFT — {} stale candidate(s), {} unmapped change(s), {} rotted anchor(s)",
                self.stale_candidates.len(),
                self.unmapped_changes.len(),
                self.anchor_rot.len()
            )
        } else {
            format!("FRESH — no drift since {}", short(&self.as_of))
        });
        lines.join("\n")
    }
}

/// Display-only short form of a SHA (JSON always carries the full 40 chars).
fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

fn shorts(shas: &[String]) -> String {
    shas.iter().map(|s| short(s)).collect::<Vec<_>>().join(", ")
}

// ---------------------------------------------------------------------------
// check-drift
// ---------------------------------------------------------------------------

/// Run the drift check: graph anchors vs the source repo's history since the
/// base rev (`--since` override, else `meta.provenance.asOf`).
///
/// Read-only on BOTH inputs (§3 must-not: never mutates graph or repo, never
/// auto-fixes, never suggests re-ingest, no semantic analysis of the changed
/// code — it flags *where* to look, the agent decides *what* it means).
pub fn check_drift(
    g: &Graph,
    repo: &Path,
    since: Option<&str>,
) -> Result<DriftReport, EngineError> {
    // 1. base = --since rev || meta.provenance.asOf.
    let base_ref = match since {
        Some(rev) => rev.to_string(),
        None => g
            .provenance()
            .and_then(|p| p.get("asOf"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(EngineError::NoDriftBase)?,
    };
    let as_of = resolve_rev(repo, &base_ref)?;
    let head = resolve_rev(repo, "HEAD")?;
    let commits_behind = rev_count(repo, &as_of, &head)?;

    // 2. changed files, committed history only.
    let changed = changed_files(repo, &as_of, &head)?;

    // 3–5. map anchors onto the changed set + the working tree.
    let anchors = node_anchors(g);
    let mut covered: BTreeSet<&str> = BTreeSet::new();
    let mut stale: Vec<StaleCandidate> = Vec::new();
    let mut rot: Vec<AnchorRot> = Vec::new();
    // Two anchors may share a path; memoize the per-path git log.
    let mut commit_cache: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (node, anchor) in &anchors {
        let path = anchor_path(anchor);
        // A non-repo-relative anchor (absolute / `..`) can never be a valid
        // anchor into `repo`; report it as rot WITHOUT touching disk with it
        // (validate flags the form error; drift must not path-traverse).
        let escapes =
            path.starts_with('/') || path.split('/').any(|seg| seg == "..") || path.is_empty();
        if escapes {
            rot.push(AnchorRot {
                node: node.clone(),
                anchor: anchor.clone(),
            });
            continue;
        }
        covered.insert(path);
        if changed.contains(path) {
            let commits = commits_touching(repo, &as_of, &head, path, &mut commit_cache)?;
            stale.push(StaleCandidate {
                node: node.clone(),
                anchor: anchor.clone(),
                commits,
            });
        }
        if !repo.join(path).exists() {
            rot.push(AnchorRot {
                node: node.clone(),
                anchor: anchor.clone(),
            });
        }
    }
    stale.sort_by(|a, b| (&a.node, &a.anchor).cmp(&(&b.node, &b.anchor)));
    rot.sort_by(|a, b| (&a.node, &a.anchor).cmp(&(&b.node, &b.anchor)));

    // 4. changed files no anchor covers (BTreeSet ⇒ file-sorted).
    let mut unmapped: Vec<UnmappedChange> = Vec::new();
    for file in &changed {
        if !covered.contains(file.as_str()) {
            let commits = commits_touching(repo, &as_of, &head, file, &mut commit_cache)?;
            unmapped.push(UnmappedChange {
                file: file.clone(),
                commits,
            });
        }
    }

    // Unanchored bucket: every node with zero source anchors (g.nodes is a
    // BTreeMap ⇒ slug-sorted iteration).
    let anchored: BTreeSet<&str> = anchors.iter().map(|(n, _)| n.as_str()).collect();
    let unanchored: Vec<String> = g
        .nodes
        .keys()
        .filter(|slug| !anchored.contains(slug.as_str()))
        .cloned()
        .collect();

    Ok(DriftReport {
        as_of,
        head,
        commits_behind,
        nodes: g.nodes.len(),
        edges: g.edges.len(),
        stale_candidates: stale,
        unmapped_changes: unmapped,
        anchor_rot: rot,
        unanchored_nodes: unanchored,
    })
}

// ---------------------------------------------------------------------------
// stamp
// ---------------------------------------------------------------------------

/// Bump `meta.provenance.asOf` to the resolved rev (default HEAD) in an
/// atomic temp-file + rename write. Returns the stamped SHA.
///
/// Requires an existing `meta.provenance` object: `origin` is REQUIRED
/// authored data the engine must never invent, so a graph without provenance
/// is `NoProvenanceToStamp` (exit 2), not a silent fabrication. Key order is
/// preserved (`serde_json` preserve_order); emission is pretty two-space JSON
/// with one trailing newline.
pub fn stamp(graph_path: &Path, repo: &Path, as_of: Option<&str>) -> Result<String, EngineError> {
    // Load through the normal path so a malformed graph fails with the usual
    // load errors, then operate on the passthrough document.
    let g = crate::load_graph_from_path(graph_path)?;
    let mut doc = g.doc;

    let sha = resolve_rev(repo, as_of.unwrap_or("HEAD"))?;
    let prov = doc
        .get_mut("meta")
        .and_then(|m| m.get_mut("provenance"))
        .and_then(Value::as_object_mut)
        .ok_or(EngineError::NoProvenanceToStamp)?;
    prov.insert("asOf".to_string(), Value::String(sha.clone()));

    let mut bytes = serde_json::to_vec_pretty(&doc)?;
    bytes.push(b'\n');
    let tmp = graph_path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, graph_path)?;
    Ok(sha)
}

// ---------------------------------------------------------------------------
// anchors
// ---------------------------------------------------------------------------

/// All (node, anchor) pairs in the graph, node-slug-sorted (BTreeMap order),
/// anchors in authored order within a node. An anchor is any string in
/// `refs.source` (one string, or an array of strings; non-string elements are
/// validate's problem, not drift's).
fn node_anchors(g: &Graph) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (slug, node) in &g.nodes {
        let Some(source) = node
            .refs
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|o| o.get("source"))
        else {
            continue;
        };
        match source {
            Value::String(s) => out.push((slug.clone(), s.clone())),
            Value::Array(items) => {
                for item in items {
                    if let Value::String(s) = item {
                        out.push((slug.clone(), s.clone()));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The file-path part of an anchor: strip the `#L<n>[-L<m>]` fragment or the
/// `@<symbol>` suffix. MIRRORS `validate::check_source_form`'s split order
/// (first `#`, else first `@`) so drift and validate never disagree on which
/// file an anchor names.
fn anchor_path(anchor: &str) -> &str {
    if let Some((p, _)) = anchor.split_once('#') {
        p
    } else if let Some((p, _)) = anchor.split_once('@') {
        p
    } else {
        anchor
    }
}

// ---------------------------------------------------------------------------
// read-only git plumbing
// ---------------------------------------------------------------------------

/// Run a read-only git command in `repo`; return raw stdout or a
/// context-rich `GitOp` error carrying git's stderr.
fn git_out(repo: &Path, op: &'static str, args: &[&str]) -> Result<Vec<u8>, EngineError> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|e| EngineError::GitOp {
            op,
            repo: repo.to_path_buf(),
            detail: e.to_string(),
        })?;
    if !out.status.success() {
        return Err(EngineError::GitOp {
            op,
            repo: repo.to_path_buf(),
            detail: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(out.stdout)
}

/// Resolve any rev-ish to a full commit SHA (`rev-parse --verify <rev>^{commit}`).
fn resolve_rev(repo: &Path, rev: &str) -> Result<String, EngineError> {
    let commit_ref = format!("{rev}^{{commit}}");
    let out = git_out(repo, "rev-parse", &["rev-parse", "--verify", &commit_ref])?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// `git rev-list --count base..head` — the asOf age in commits.
fn rev_count(repo: &Path, base: &str, head: &str) -> Result<usize, EngineError> {
    let range = format!("{base}..{head}");
    let out = git_out(repo, "rev-list", &["rev-list", "--count", &range])?;
    let text = String::from_utf8_lossy(&out).trim().to_string();
    text.parse::<usize>().map_err(|_| EngineError::GitOp {
        op: "rev-list",
        repo: repo.to_path_buf(),
        detail: format!("unparseable commit count {text:?}"),
    })
}

/// Changed paths between two commits. `-z` (NUL-separated) sidesteps git's
/// path quoting; `--no-renames` pins rename detection OFF so the output never
/// depends on the machine's `diff.renames` config (determinism).
fn changed_files(repo: &Path, base: &str, head: &str) -> Result<BTreeSet<String>, EngineError> {
    let out = git_out(
        repo,
        "diff",
        &["diff", "--name-only", "--no-renames", "-z", base, head],
    )?;
    Ok(out
        .split(|&b| b == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect())
}

/// Commits in `base..head` touching `path`, newest first (`git log` order —
/// a pure function of the repo's history, hence deterministic). Memoized:
/// several anchors may share one path.
fn commits_touching(
    repo: &Path,
    base: &str,
    head: &str,
    path: &str,
    cache: &mut BTreeMap<String, Vec<String>>,
) -> Result<Vec<String>, EngineError> {
    if let Some(hit) = cache.get(path) {
        return Ok(hit.clone());
    }
    let range = format!("{base}..{head}");
    let out = git_out(repo, "log", &["log", "--format=%H", &range, "--", path])?;
    let commits: Vec<String> = String::from_utf8_lossy(&out)
        .lines()
        .map(str::to_string)
        .collect();
    cache.insert(path.to_string(), commits.clone());
    Ok(commits)
}

// ---------------------------------------------------------------------------
// unit tests — anchor parsing (the validate-mirror contract)
// ---------------------------------------------------------------------------

#[cfg(test)]
// Test code may panic on failure: that IS the assertion mechanism.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Path extraction mirrors validate's split order: `#` fragment first,
    /// else `@` symbol, else the whole string.
    #[test]
    fn anchor_path_strips_fragment_and_symbol() {
        assert_eq!(anchor_path("src/a.ts"), "src/a.ts");
        assert_eq!(anchor_path("src/a.ts#L3"), "src/a.ts");
        assert_eq!(anchor_path("src/a.ts#L3-L9"), "src/a.ts");
        assert_eq!(anchor_path("src/a.ts@Save"), "src/a.ts");
        // `#` wins over `@` (same precedence as check_source_form).
        assert_eq!(anchor_path("src/a.ts#L1@x"), "src/a.ts");
    }

    /// Multi-anchor nodes contribute one pair per anchor; non-string array
    /// elements are skipped (validate's problem, not drift's).
    #[test]
    fn node_anchors_handles_string_and_array_forms() {
        let doc = serde_json::json!({
            "nodes": {
                "surface": {
                    "screen:a/B": {"kind": "Screen", "refs": {"source": ["x.ts", "y.ts#L1", 3]}},
                    "screen:a/A": {"kind": "Screen", "refs": {"source": "z.ts"}},
                    "screen:a/C": {"kind": "Screen", "refs": {}}
                }
            },
            "edges": []
        });
        let g = crate::load_graph_from_slice(&serde_json::to_vec(&doc).unwrap()).unwrap();
        assert_eq!(
            node_anchors(&g),
            vec![
                ("screen:a/A".to_string(), "z.ts".to_string()),
                ("screen:a/B".to_string(), "x.ts".to_string()),
                ("screen:a/B".to_string(), "y.ts#L1".to_string()),
            ]
        );
    }
}
