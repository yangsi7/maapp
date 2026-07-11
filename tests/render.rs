// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Render verb tests: exit codes, artifact shape, and output snapshots.
//!
//! Each render verb's ASCII/markdown output is frozen as an `insta` snapshot
//! (in `tests/snapshots/`), pinning the produced artifact byte-for-byte.

use assert_cmd::Command;
use predicates::prelude::*;

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

/// Run `maapp render <args>`, assert exit 0, return stdout as a UTF-8 string.
fn render_out(args: &[&str]) -> String {
    let out = maapp()
        .arg("render")
        .args(args)
        .output()
        .expect("maapp render ran");
    assert!(out.status.success(), "render {args:?}: exit code != 0");
    String::from_utf8(out.stdout).expect("render output is valid UTF-8")
}

// ---------------------------------------------------------------------------
// render hub — markdown blueprint
// ---------------------------------------------------------------------------

#[test]
fn render_hub_exits_zero() {
    maapp()
        .args(["render", "hub", "examples/chat.json"])
        .assert()
        .success();
}

#[test]
fn render_hub_starts_with_blueprint_header() {
    maapp()
        .args(["render", "hub", "examples/chat.json"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "# App-Spec Graph — BLUEPRINT hub (generated; do NOT hand-edit)",
        ));
}

#[test]
fn render_hub_unknown_file_exits_two() {
    maapp()
        .args(["render", "hub", "examples/does-not-exist.json"])
        .assert()
        .code(2);
}

#[test]
fn render_hub_chat_snapshot() {
    insta::assert_snapshot!(render_out(&["hub", "examples/chat.json"]));
}

#[test]
fn render_hub_checkout_snapshot() {
    insta::assert_snapshot!(render_out(&["hub", "examples/checkout.json"]));
}

#[test]
fn render_hub_dashboard_snapshot() {
    insta::assert_snapshot!(render_out(&["hub", "examples/dashboard.json"]));
}

// ---------------------------------------------------------------------------
// render deps — dependency view
// ---------------------------------------------------------------------------

#[test]
fn render_deps_exits_zero() {
    maapp()
        .args(["render", "deps", "examples/chat.json"])
        .assert()
        .success();
}

#[test]
fn render_deps_starts_with_header() {
    maapp()
        .args(["render", "deps", "examples/chat.json"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "# App-Spec Graph — dependency view (generated; do NOT hand-edit)",
        ));
}

#[test]
fn render_deps_unknown_file_exits_two() {
    maapp()
        .args(["render", "deps", "examples/does-not-exist.json"])
        .assert()
        .code(2);
}

#[test]
fn render_deps_chat_snapshot() {
    insta::assert_snapshot!(render_out(&["deps", "examples/chat.json"]));
}

#[test]
fn render_deps_checkout_snapshot() {
    insta::assert_snapshot!(render_out(&["deps", "examples/checkout.json"]));
}

// ---------------------------------------------------------------------------
// render storyboard — ASCII storyboard (--ascii flag)
// ---------------------------------------------------------------------------

#[test]
fn render_storyboard_ascii_exits_zero() {
    maapp()
        .args(["render", "storyboard", "--ascii", "examples/chat.json"])
        .assert()
        .success();
}

#[test]
fn render_storyboard_ascii_starts_with_header() {
    maapp()
        .args(["render", "storyboard", "--ascii", "examples/chat.json"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("===="));
}

#[test]
fn render_storyboard_ascii_unknown_file_exits_two() {
    maapp()
        .args([
            "render",
            "storyboard",
            "--ascii",
            "examples/does-not-exist.json",
        ])
        .assert()
        .code(2);
}

#[test]
fn render_storyboard_ascii_chat_snapshot() {
    insta::assert_snapshot!(render_out(&["storyboard", "--ascii", "examples/chat.json"]));
}

#[test]
fn render_storyboard_ascii_checkout_snapshot() {
    insta::assert_snapshot!(render_out(&[
        "storyboard",
        "--ascii",
        "examples/checkout.json"
    ]));
}

// ---------------------------------------------------------------------------
// render spine — ASCII spine (--ascii flag)
// ---------------------------------------------------------------------------

#[test]
fn render_spine_ascii_exits_zero() {
    maapp()
        .args(["render", "spine", "--ascii", "examples/chat.json"])
        .assert()
        .success();
}

#[test]
fn render_spine_ascii_starts_with_header() {
    maapp()
        .args(["render", "spine", "--ascii", "examples/chat.json"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "BACKEND PULL SPINE — STALACTITE VIEW",
        ));
}

#[test]
fn render_spine_ascii_unknown_file_exits_two() {
    maapp()
        .args(["render", "spine", "--ascii", "examples/does-not-exist.json"])
        .assert()
        .code(2);
}

#[test]
fn render_spine_ascii_chat_snapshot() {
    insta::assert_snapshot!(render_out(&["spine", "--ascii", "examples/chat.json"]));
}

#[test]
fn render_spine_ascii_checkout_snapshot() {
    insta::assert_snapshot!(render_out(&["spine", "--ascii", "examples/checkout.json"]));
}

// ---------------------------------------------------------------------------
// render html — writes to --out file
// ---------------------------------------------------------------------------

#[test]
fn render_html_without_out_exits_two() {
    maapp()
        .args(["render", "html", "examples/chat.json"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--out"));
}

#[test]
fn render_html_with_out_exits_zero() {
    let dir = tempdir();
    let out = dir.join("chat.html");
    maapp()
        .args([
            "render",
            "html",
            "examples/chat.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(out.exists(), "HTML file was not created");
    let content = std::fs::read_to_string(&out).unwrap();
    assert!(
        content.contains("<!DOCTYPE html>") || content.contains("<html"),
        "output does not look like HTML"
    );
}

/// The interactive html render ships the app-flow visualization (3-tier
/// Map/Flow/Full, iPhone device frames, ELK layout vendored inline). Because the
/// full HTML is large and layout-dependent, the html case asserts STRUCTURAL /
/// smoke properties instead of a byte snapshot (the ASCII/markdown render verbs
/// above ARE pinned as byte snapshots).
///
/// Properties asserted (for ANY graph):
///   1. render succeeds without panic; output is non-empty and looks like HTML;
///   2. the source graph JSON is injected (placeholder is NOT the literal `null`);
///   3. the output contains the graph's node slugs + its schema title;
///   4. the output is self-contained — NO external resource-loading reference
///      (`<script src>`, `src="http"`, `href="http"`, `@import`). Inert URL string
///      literals inside the inlined ELK library + SVG XML namespace IRIs are data,
///      not network loads, and are expected;
///   5. byte-STABLE across two renders of the SAME graph (determinism contract).
fn assert_html_smoke(probe: &str) {
    let dir = tempdir();
    let out_a = dir.join(format!("{probe}-a.html"));
    let out_b = dir.join(format!("{probe}-b.html"));
    let src = format!("examples/{probe}.json");

    // 1. render twice (determinism) — both must succeed.
    maapp()
        .args(["render", "html", &src, "--out", out_a.to_str().unwrap()])
        .assert()
        .success();
    maapp()
        .args(["render", "html", &src, "--out", out_b.to_str().unwrap()])
        .assert()
        .success();

    let bytes_a = std::fs::read(&out_a).expect("first html written");
    let bytes_b = std::fs::read(&out_b).expect("second html written");

    // 5. byte-stable across two runs of the same graph.
    assert_eq!(
        bytes_a, bytes_b,
        "render html {probe}: output is not byte-stable across two runs (determinism)"
    );

    let html = String::from_utf8(bytes_a).expect("html is valid UTF-8");

    // 1. non-empty + looks like HTML.
    assert!(!html.is_empty(), "render html {probe}: output is empty");
    assert!(
        html.contains("<!DOCTYPE html>") || html.contains("<html"),
        "render html {probe}: output does not look like HTML"
    );

    // 2. the graph JSON was injected (render.rs replaces the entire literal
    //    `/*__GRAPH_JSON__*/null` with the compact object, so the marker comment
    //    is consumed and `const GRAPH = {…}` remains — never the literal `null`).
    assert!(
        html.contains("const GRAPH = {\"meta\":"),
        "render html {probe}: GRAPH placeholder was not replaced with injected JSON"
    );
    assert!(
        !html.contains("const GRAPH = /*__GRAPH_JSON__*/null;"),
        "render html {probe}: GRAPH placeholder left as literal null"
    );

    // 3a. the schema title is present (from the injected document).
    let doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&src).expect("source graph readable"))
            .expect("source graph is valid JSON");
    let schema = doc
        .get("schema")
        .and_then(serde_json::Value::as_str)
        .expect("graph has a schema");
    assert!(
        html.contains(schema),
        "render html {probe}: schema title `{schema}` not found in output"
    );

    // 3b. every node slug from the source graph appears in the injected JSON.
    let nodes = doc
        .get("nodes")
        .and_then(serde_json::Value::as_object)
        .expect("graph has nodes");
    let mut checked = 0usize;
    for cat in nodes.values() {
        if let Some(cat_obj) = cat.as_object() {
            for slug in cat_obj.keys() {
                assert!(
                    html.contains(slug),
                    "render html {probe}: node slug `{slug}` not found in output"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "render html {probe}: no node slugs to check");

    // 4. self-contained: no external resource-loading reference. (Inert URL string
    //    literals in the inlined ELK lib + SVG namespace IRIs are data, not loads.)
    for bad in [
        "<script src=",
        "src=\"http",
        "src='http",
        "href=\"http",
        "href='http",
        "@import",
    ] {
        assert!(
            !html.contains(bad),
            "render html {probe}: external resource reference found (`{bad}`) — not self-contained"
        );
    }
}

#[test]
fn render_html_chat_smoke() {
    assert_html_smoke("chat");
}

#[test]
fn render_html_checkout_smoke() {
    assert_html_smoke("checkout");
}

#[test]
fn render_html_dashboard_smoke() {
    assert_html_smoke("dashboard");
}

#[test]
fn render_html_maps_smoke() {
    assert_html_smoke("maps");
}

#[test]
fn render_html_media_smoke() {
    assert_html_smoke("media");
}

#[test]
fn render_html_wizard_smoke() {
    assert_html_smoke("wizard");
}

#[test]
fn render_html_unknown_file_exits_two() {
    let dir = tempdir();
    let out = dir.join("out.html");
    maapp()
        .args([
            "render",
            "html",
            "examples/does-not-exist.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .code(2);
}

// ---------------------------------------------------------------------------
// unknown render verb
// ---------------------------------------------------------------------------

#[test]
fn render_unknown_verb_exits_two() {
    maapp()
        .args(["render", "foobar", "examples/chat.json"])
        .assert()
        .code(2);
}

/// Minimal per-test temp dir under the OS temp root (no extra deps).
fn tempdir() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("maapp-render-test-{nanos}-{}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
