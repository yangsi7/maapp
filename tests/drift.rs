// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `maapp check-drift` + `maapp stamp` — LIFECYCLE-VERBS-SPEC §1 + §3 + §6.
//!
//! check-drift is THE ONLY disk-toucher in the verb set: it reads the graph
//! file plus the named source repo (read-only git: rev-parse, diff
//! --name-only, log, rev-list, path existence). Mechanics (§3): base =
//! `--since` rev or `meta.provenance.asOf`; changed files since base..HEAD
//! are mapped onto `refs.source` anchors:
//!   - anchored node whose file changed        → `stale_candidates`
//!   - changed file no anchor covers           → `unmapped_changes`
//!   - anchor whose path no longer exists      → `anchor_rot`
//!   - node without any `refs.source` anchor   → `unanchored_nodes` (ADVISORY)
//!
//! Exit codes (§1): 0 = no drift, 1 = drift found (stale / unmapped / rot
//! non-empty), 2 = usage or input error. `unanchored_nodes` is advisory only
//! (§1 defines exit 1 as "drift found"; an unanchored node is a blind spot,
//! not drift) — it never drives the exit code.
//!
//! The loop closes via the companion `maapp stamp <graph> --repo <path>
//! [--as-of <rev>]` (flag name from §6.3), which bumps
//! `meta.provenance.asOf` in an atomic write. check-drift itself NEVER
//! mutates the graph (§3 must-not).
//!
//! Fixtures are synthesized temp git repos (hermetic; the Python oracle has
//! no check-drift, so there is no differential — noted in the module docs).

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

/// Run a git command in `dir`, asserting success; return trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf8")
        .trim()
        .to_string()
}

/// Init a hermetic git repo (identity pinned, signing off) in `dir`.
fn init_repo(dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "t@test.invalid"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

/// Stage everything and commit; return the new HEAD SHA (full).
fn commit_all(dir: &Path, msg: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

/// The standard fixture: a repo with three files, and a graph anchored to two
/// of them (`screen:app/Home` → src/home.ts, `op:api/Save` → src/api/save.ts
/// via a `@Symbol` anchor). `vs:app/State` carries NO anchor (the advisory
/// bucket). docs/notes.md is a real file NO anchor covers (the unmapped
/// class). Returns (tempdir, repo path, graph path, base SHA).
fn fixture(as_of: Option<&str>) -> (TempDir, PathBuf, PathBuf, String) {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src/api")).expect("mkdir");
    std::fs::create_dir_all(repo.join("docs")).expect("mkdir");
    std::fs::write(repo.join("src/home.ts"), "export const home = 1;\n").unwrap();
    std::fs::write(repo.join("src/api/save.ts"), "export function Save() {}\n").unwrap();
    std::fs::write(repo.join("docs/notes.md"), "# notes\n").unwrap();
    init_repo(&repo);
    let base = commit_all(&repo, "base");

    let graph = tmp.path().join("graph.json");
    write_graph(
        &graph,
        as_of.map(|s| {
            if s == "HEAD" {
                base.clone()
            } else {
                s.to_string()
            }
        }),
    );
    (tmp, repo, graph, base)
}

/// Write the fixture graph, optionally with `meta.provenance.asOf` set.
fn write_graph(path: &Path, as_of: Option<String>) {
    let mut doc = json!({
        "schema": "maapp-graph",
        "nodes": {
            "surface": {
                "screen:app/Home": {
                    "kind": "Screen",
                    "intent": "the home screen",
                    "refs": {"source": "src/home.ts"}
                }
            },
            "backend": {
                "op:api/Save": {
                    "kind": "BackendOp",
                    "intent": "persist the form",
                    "refs": {"source": "src/api/save.ts@Save"}
                }
            },
            "logic": {
                "vs:app/State": {"kind": "ViewState", "intent": "form state"}
            }
        },
        "edges": []
    });
    if let Some(sha) = as_of {
        doc["meta"] = json!({"provenance": {"origin": "ingested", "asOf": sha}});
    }
    std::fs::write(path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
}

/// Run `maapp check-drift <graph> --repo <repo> --json`, assert the exit
/// code, return the parsed report.
fn check_json(graph: &Path, repo: &Path, want_code: i32) -> Value {
    let assert = maapp()
        .args([
            "check-drift",
            graph.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .code(want_code);
    serde_json::from_slice(&assert.get_output().stdout).expect("check-drift --json is valid JSON")
}

fn arr(report: &Value, key: &str) -> Vec<Value> {
    report
        .get(key)
        .unwrap_or_else(|| panic!("report carries '{key}'"))
        .as_array()
        .unwrap_or_else(|| panic!("'{key}' is an array"))
        .clone()
}

// ===========================================================================
// (a) untouched tree, stamped graph → all fresh, exit 0
// ===========================================================================

#[test]
fn fresh_tree_exits_zero_all_buckets_empty() {
    let (_tmp, repo, graph, base) = fixture(Some("HEAD"));
    let report = check_json(&graph, &repo, 0);

    assert_eq!(
        report["as_of"],
        json!(base),
        "as_of is the resolved base SHA"
    );
    assert_eq!(
        report["head"],
        json!(base),
        "head == base on an untouched tree"
    );
    assert_eq!(report["commits_behind"], json!(0));
    assert_eq!(report["nodes"], json!(3));
    assert_eq!(report["edges"], json!(0));
    assert!(arr(&report, "stale_candidates").is_empty());
    assert!(arr(&report, "unmapped_changes").is_empty());
    assert!(arr(&report, "anchor_rot").is_empty());
}

// ===========================================================================
// (b) committed edit to an anchored file → stale candidate, exit 1
// ===========================================================================

#[test]
fn committed_edit_to_anchored_file_is_a_stale_candidate() {
    let (_tmp, repo, graph, base) = fixture(Some("HEAD"));
    std::fs::write(repo.join("src/api/save.ts"), "export function Save(x) {}\n").unwrap();
    let sha2 = commit_all(&repo, "change save");

    let report = check_json(&graph, &repo, 1);
    assert_eq!(report["as_of"], json!(base));
    assert_eq!(report["head"], json!(sha2));
    assert_eq!(report["commits_behind"], json!(1));
    assert_eq!(
        report["head"].as_str().unwrap().len(),
        40,
        "full SHA, never short"
    );
    // Exactly the @Symbol-anchored node, with the touching commit; the anchor
    // fragment is stripped for the file match but reported verbatim.
    assert_eq!(
        arr(&report, "stale_candidates"),
        vec![json!({
            "node": "op:api/Save",
            "anchor": "src/api/save.ts@Save",
            "commits": [sha2]
        })]
    );
    assert!(
        arr(&report, "unmapped_changes").is_empty(),
        "covered file is never unmapped"
    );
    assert!(arr(&report, "anchor_rot").is_empty());
}

// ===========================================================================
// changed file NO anchor covers → unmapped change (the D1/D2 class), exit 1
// ===========================================================================

#[test]
fn change_to_uncovered_file_is_an_unmapped_change() {
    let (_tmp, repo, graph, _base) = fixture(Some("HEAD"));
    std::fs::write(repo.join("docs/notes.md"), "# notes\nmore\n").unwrap();
    let sha2 = commit_all(&repo, "grow reality");

    let report = check_json(&graph, &repo, 1);
    assert!(arr(&report, "stale_candidates").is_empty());
    assert_eq!(
        arr(&report, "unmapped_changes"),
        vec![json!({"file": "docs/notes.md", "commits": [sha2]})]
    );
}

// ===========================================================================
// (c) anchor path gone from disk → anchor rot, exit 1
// ===========================================================================

#[test]
fn deleted_anchor_path_is_anchor_rot() {
    let (_tmp, repo, graph, _base) = fixture(Some("HEAD"));
    std::fs::remove_file(repo.join("src/home.ts")).unwrap();

    let report = check_json(&graph, &repo, 1);
    assert_eq!(
        arr(&report, "anchor_rot"),
        vec![json!({"node": "screen:app/Home", "anchor": "src/home.ts"})]
    );
    // Working-tree deletion is not a commit: nothing is stale-by-history.
    assert!(arr(&report, "stale_candidates").is_empty());
}

// ===========================================================================
// (d) node without anchor → unanchored bucket, ADVISORY (exit stays 0)
// ===========================================================================

#[test]
fn unanchored_node_is_advisory_only() {
    let (_tmp, repo, graph, _base) = fixture(Some("HEAD"));
    let report = check_json(&graph, &repo, 0);
    assert_eq!(
        arr(&report, "unanchored_nodes"),
        vec![json!("vs:app/State")],
        "the anchor-less node is reported, never silently skipped"
    );
}

// ===========================================================================
// (e) stamp bumps asOf and closes the loop
// ===========================================================================

#[test]
fn stamp_bumps_as_of_and_closes_the_loop() {
    let (_tmp, repo, graph, _base) = fixture(Some("HEAD"));
    std::fs::write(repo.join("src/home.ts"), "export const home = 2;\n").unwrap();
    let sha2 = commit_all(&repo, "drift");

    // Drift is found…
    check_json(&graph, &repo, 1);
    // …the agent reconciles the graph, then stamps…
    maapp()
        .args([
            "stamp",
            graph.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
        ])
        .assert()
        .code(0);
    // …the graph file now carries asOf = HEAD…
    let doc: Value = serde_json::from_slice(&std::fs::read(&graph).unwrap()).unwrap();
    assert_eq!(doc["meta"]["provenance"]["asOf"], json!(sha2));
    assert_eq!(
        doc["meta"]["provenance"]["origin"],
        json!("ingested"),
        "stamp only bumps asOf"
    );
    // …and a subsequent check is clean.
    check_json(&graph, &repo, 0);
}

/// `--as-of <rev>` pins an explicit rev instead of HEAD (§6.3 flag name).
#[test]
fn stamp_as_of_flag_pins_an_explicit_rev() {
    let (_tmp, repo, graph, base) = fixture(Some("HEAD"));
    std::fs::write(repo.join("src/home.ts"), "export const home = 2;\n").unwrap();
    commit_all(&repo, "drift");

    maapp()
        .args([
            "stamp",
            graph.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--as-of",
            &base,
        ])
        .assert()
        .code(0);
    let doc: Value = serde_json::from_slice(&std::fs::read(&graph).unwrap()).unwrap();
    assert_eq!(doc["meta"]["provenance"]["asOf"], json!(base));
}

#[test]
fn stamp_without_provenance_is_exit_two() {
    let (_tmp, repo, graph, _base) = fixture(None);
    maapp()
        .args([
            "stamp",
            graph.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("meta.provenance"));
}

// ===========================================================================
// base resolution: --since vs meta.provenance.asOf
// ===========================================================================

#[test]
fn missing_as_of_without_since_is_exit_two_with_pointed_message() {
    let (_tmp, repo, graph, _base) = fixture(None);
    maapp()
        .args([
            "check-drift",
            graph.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(
            predicates::str::contains("meta.provenance.asOf")
                .and(predicates::str::contains("--since")),
        );
}

#[test]
fn since_flag_overrides_missing_as_of() {
    let (_tmp, repo, graph, base) = fixture(None);
    std::fs::write(repo.join("src/home.ts"), "export const home = 2;\n").unwrap();
    commit_all(&repo, "drift");

    let assert = maapp()
        .args([
            "check-drift",
            graph.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--since",
            &base,
            "--json",
        ])
        .assert()
        .code(1);
    let report: Value = serde_json::from_slice(&assert.get_output().stdout).unwrap();
    assert_eq!(
        arr(&report, "stale_candidates")[0]["node"],
        json!("screen:app/Home")
    );
}

// ===========================================================================
// determinism + purity + error paths
// ===========================================================================

/// Same tree + same graph = byte-identical `--json` (the hard contract).
#[test]
fn check_drift_json_is_byte_stable_across_runs() {
    let (_tmp, repo, graph, _base) = fixture(Some("HEAD"));
    std::fs::write(repo.join("src/api/save.ts"), "export function Save(x) {}\n").unwrap();
    std::fs::write(repo.join("docs/notes.md"), "# notes\nmore\n").unwrap();
    commit_all(&repo, "drift");
    std::fs::remove_file(repo.join("src/home.ts")).unwrap();

    let run = || {
        maapp()
            .args([
                "check-drift",
                graph.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--json",
            ])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone()
    };
    assert_eq!(run(), run(), "byte-identical --json across runs");
}

/// §3 must-not: check-drift NEVER mutates the graph file.
#[test]
fn check_drift_never_mutates_the_graph_file() {
    let (_tmp, repo, graph, _base) = fixture(Some("HEAD"));
    std::fs::write(repo.join("src/home.ts"), "export const home = 2;\n").unwrap();
    commit_all(&repo, "drift");

    let before = std::fs::read(&graph).unwrap();
    check_json(&graph, &repo, 1);
    assert_eq!(before, std::fs::read(&graph).unwrap());
}

#[test]
fn non_git_repo_is_exit_two() {
    let (tmp, _repo, graph, _base) = fixture(Some("HEAD"));
    let plain = tmp.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    maapp()
        .args([
            "check-drift",
            graph.to_str().unwrap(),
            "--repo",
            plain.to_str().unwrap(),
        ])
        .assert()
        .code(2);
}

/// Human mode renders the buckets and a DRIFT verdict.
#[test]
fn human_output_renders_buckets_and_verdict() {
    let (_tmp, repo, graph, _base) = fixture(Some("HEAD"));
    std::fs::write(repo.join("src/api/save.ts"), "export function Save(x) {}\n").unwrap();
    commit_all(&repo, "drift");

    maapp()
        .args([
            "check-drift",
            graph.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
        ])
        .assert()
        .code(1)
        .stdout(predicates::str::contains("op:api/Save").and(predicates::str::contains("DRIFT")));

    // And FRESH on a clean check via --since HEAD.
    let head = git(&repo, &["rev-parse", "HEAD"]);
    maapp()
        .args([
            "check-drift",
            graph.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--since",
            &head,
        ])
        .assert()
        .code(0)
        .stdout(predicates::str::contains("FRESH"));
}

// ===========================================================================
// T1a — default exclusions (graph file itself + .maapp/**) + baseline/ratchet
// ===========================================================================

/// Fixture for the stamp-commit tautology: the graph lives INSIDE the repo,
/// is committed at `base` (with a placeholder asOf never read by check-drift),
/// then a LATER commit rewrites its `asOf` to `base` (exactly what `stamp`
/// does). The graph's own rewrite commit lands in `git diff base..HEAD`; under
/// the default exclusions it must NOT self-report as unmapped. `graph_rel` is
/// where the graph sits relative to the repo root ("`.maapp/graph.json`" or
/// "`plans/graph.json`"). Returns (tmp, repo, graph path, base SHA).
fn in_repo_graph_fixture(graph_rel: &str) -> (TempDir, PathBuf, PathBuf, String) {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    let graph = repo.join(graph_rel);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::create_dir_all(graph.parent().unwrap()).unwrap();
    std::fs::write(repo.join("src/home.ts"), "export const home = 1;\n").unwrap();

    let doc = json!({
        "schema": "maapp-graph",
        "meta": {"provenance": {"origin": "ingested", "asOf": "uninitialized"}},
        "nodes": {"surface": {"screen:app/Home": {
            "kind": "Screen", "intent": "home", "refs": {"source": "src/home.ts"}}}},
        "edges": []
    });
    std::fs::write(&graph, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    init_repo(&repo);
    let base = commit_all(&repo, "base (graph committed)");

    // The graph's own asOf-bump commit — the tautology trigger.
    let mut doc2 = doc.clone();
    doc2["meta"]["provenance"]["asOf"] = json!(base);
    std::fs::write(&graph, serde_json::to_vec_pretty(&doc2).unwrap()).unwrap();
    commit_all(&repo, "stamp: bump asOf (graph self-change)");
    (tmp, repo, graph, base)
}

/// A committed graph under `.maapp/` does not self-report as unmapped: the
/// `.maapp/**` default exclusion kills the stamp-commit tautology.
#[test]
fn stamped_graph_in_dot_maapp_is_fresh_under_default_exclusions() {
    let (_tmp, repo, graph, _base) = in_repo_graph_fixture(".maapp/graph.json");
    let report = check_json(&graph, &repo, 0);
    assert!(
        arr(&report, "unmapped_changes").is_empty(),
        "the graph's own commit must not self-report as unmapped"
    );
    assert!(arr(&report, "stale_candidates").is_empty());
    assert!(arr(&report, "anchor_rot").is_empty());
}

/// The graph-file-itself exclusion holds even when the graph lives OUTSIDE
/// `.maapp/` (the pilot app keeps its graph at `plans/blueprint/…`): its own commit is
/// still excluded from the unmapped set.
#[test]
fn graph_file_itself_is_excluded_outside_dot_maapp() {
    let (_tmp, repo, graph, _base) = in_repo_graph_fixture("plans/graph.json");
    let report = check_json(&graph, &repo, 0);
    assert!(
        arr(&report, "unmapped_changes").is_empty(),
        "the graph file itself is excluded from unmapped, wherever it lives"
    );
}

/// A repo with TWO uncovered doc files and a graph (outside the repo) anchored
/// to src/home.ts, asOf pinned to base. Returns (tmp, repo, graph, base).
fn two_uncovered_fixture() -> (TempDir, PathBuf, PathBuf, String) {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::create_dir_all(repo.join("docs")).unwrap();
    std::fs::write(repo.join("src/home.ts"), "export const home = 1;\n").unwrap();
    std::fs::write(repo.join("docs/a.md"), "# a\n").unwrap();
    std::fs::write(repo.join("docs/b.md"), "# b\n").unwrap();
    init_repo(&repo);
    let base = commit_all(&repo, "base");

    let graph = tmp.path().join("graph.json");
    let doc = json!({
        "schema": "maapp-graph",
        "meta": {"provenance": {"origin": "ingested", "asOf": base}},
        "nodes": {"surface": {"screen:app/Home": {
            "kind": "Screen", "intent": "home", "refs": {"source": "src/home.ts"}}}},
        "edges": []
    });
    std::fs::write(&graph, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    (tmp, repo, graph, base)
}

/// Write a baseline file `{"unmapped": [paths…]}` for the tolerated set.
fn write_baseline_file(path: &Path, paths: &[&str]) {
    std::fs::write(
        path,
        serde_json::to_vec(&json!({ "unmapped": paths })).unwrap(),
    )
    .unwrap();
}

/// Run `check-drift … --baseline <b> --json`, assert the exit code, parse.
fn check_baseline_json(graph: &Path, repo: &Path, baseline: &Path, want: i32) -> Value {
    let assert = maapp()
        .args([
            "check-drift",
            graph.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--baseline",
            baseline.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .code(want);
    serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON")
}

fn files(report: &Value, key: &str) -> Vec<String> {
    arr(report, key)
        .iter()
        .map(|u| u["file"].as_str().unwrap().to_string())
        .collect()
}

/// A baseline tolerates its listed unmapped paths and still fails on a NEW one.
#[test]
fn baseline_tolerates_listed_and_catches_new_unmapped() {
    let (_tmp, repo, graph, _base) = two_uncovered_fixture();
    std::fs::write(repo.join("docs/a.md"), "# a\nmore\n").unwrap();
    std::fs::write(repo.join("docs/b.md"), "# b\nmore\n").unwrap();
    commit_all(&repo, "grow both");
    let baseline = repo.parent().unwrap().join("baseline.json");
    write_baseline_file(&baseline, &["docs/a.md"]);

    let report = check_baseline_json(&graph, &repo, &baseline, 1);
    assert_eq!(
        files(&report, "unmapped_changes"),
        vec!["docs/b.md"],
        "the un-baselined file is a NEW unmapped change (drives exit 1)"
    );
    assert_eq!(
        files(&report, "tolerated_unmapped"),
        vec!["docs/a.md"],
        "the baselined file is tolerated, not exit-driving"
    );
}

/// When only baselined paths changed, the check is FRESH (exit 0).
#[test]
fn baseline_only_change_is_tolerated_exit_zero() {
    let (_tmp, repo, graph, _base) = two_uncovered_fixture();
    std::fs::write(repo.join("docs/a.md"), "# a\nmore\n").unwrap();
    commit_all(&repo, "grow a only");
    let baseline = repo.parent().unwrap().join("baseline.json");
    write_baseline_file(&baseline, &["docs/a.md", "docs/b.md"]);

    let report = check_baseline_json(&graph, &repo, &baseline, 0);
    assert!(files(&report, "unmapped_changes").is_empty());
    assert_eq!(files(&report, "tolerated_unmapped"), vec!["docs/a.md"]);
}

/// A baseline never masks stale candidates or anchor rot (only unmapped is
/// ratcheted).
#[test]
fn baseline_does_not_mask_stale() {
    let (_tmp, repo, graph, _base) = two_uncovered_fixture();
    std::fs::write(repo.join("src/home.ts"), "export const home = 2;\n").unwrap();
    commit_all(&repo, "touch the anchored file");
    let baseline = repo.parent().unwrap().join("baseline.json");
    write_baseline_file(&baseline, &["docs/a.md", "docs/b.md"]);

    let report = check_baseline_json(&graph, &repo, &baseline, 1);
    assert_eq!(
        arr(&report, "stale_candidates").len(),
        1,
        "a stale candidate still fails under a baseline"
    );
}

/// `--write-baseline` snapshots the current unmapped set (sorted, byte-stable);
/// feeding that snapshot back as `--baseline` ratchets to FRESH.
#[test]
fn write_baseline_snapshots_unmapped_and_ratchets_to_fresh() {
    let (_tmp, repo, graph, _base) = two_uncovered_fixture();
    std::fs::write(repo.join("docs/a.md"), "# a\nmore\n").unwrap();
    std::fs::write(repo.join("docs/b.md"), "# b\nmore\n").unwrap();
    commit_all(&repo, "grow both");
    let baseline = repo.parent().unwrap().join("baseline.json");

    let run_write = || {
        maapp()
            .args([
                "check-drift",
                graph.to_str().unwrap(),
                "--repo",
                repo.to_str().unwrap(),
                "--write-baseline",
                baseline.to_str().unwrap(),
            ])
            .assert()
            .code(0);
        std::fs::read(&baseline).unwrap()
    };
    let first = run_write();
    let bdoc: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(
        bdoc["unmapped"],
        json!(["docs/a.md", "docs/b.md"]),
        "the snapshot lists both uncovered files, sorted"
    );
    assert_eq!(
        first,
        run_write(),
        "write-baseline is byte-stable across runs"
    );

    // Ratchet: the snapshot tolerates the whole current unmapped set -> FRESH.
    check_baseline_json(&graph, &repo, &baseline, 0);
}

/// `--baseline` and `--write-baseline` are mutually exclusive (exit 2).
#[test]
fn baseline_and_write_baseline_conflict() {
    let (_tmp, repo, graph, _base) = two_uncovered_fixture();
    let b = repo.parent().unwrap().join("b.json");
    maapp()
        .args([
            "check-drift",
            graph.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--baseline",
            b.to_str().unwrap(),
            "--write-baseline",
            b.to_str().unwrap(),
        ])
        .assert()
        .code(2);
}

/// A malformed baseline file (no `unmapped` array) is a load error (exit 2),
/// never a silent tolerate-nothing.
#[test]
fn malformed_baseline_is_exit_two() {
    let (_tmp, repo, graph, _base) = two_uncovered_fixture();
    std::fs::write(repo.join("docs/a.md"), "# a\nmore\n").unwrap();
    commit_all(&repo, "grow a");
    let baseline = repo.parent().unwrap().join("baseline.json");
    std::fs::write(&baseline, b"{\"nope\": 1}").unwrap();
    maapp()
        .args([
            "check-drift",
            graph.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--baseline",
            baseline.to_str().unwrap(),
        ])
        .assert()
        .code(2);
}

/// No baseline ⇒ the `tolerated_unmapped` key is ABSENT (byte shape unchanged).
#[test]
fn no_baseline_omits_tolerated_key() {
    let (_tmp, repo, graph, _base) = two_uncovered_fixture();
    std::fs::write(repo.join("docs/a.md"), "# a\nmore\n").unwrap();
    commit_all(&repo, "grow a");
    let report = check_json(&graph, &repo, 1);
    assert!(
        report.get("tolerated_unmapped").is_none(),
        "tolerated_unmapped is omitted when no baseline is active"
    );
    assert_eq!(files(&report, "unmapped_changes"), vec!["docs/a.md"]);
}
