// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `maapp init` — distribution wiring at the process boundary.
//!
//! Drives the real binary against scratch directories (`tempfile::TempDir`)
//! and asserts on the PRODUCED TREE (files, bytes, merged JSON), not exit
//! codes alone. Contract under test (distribution spec):
//!   - idempotent by default, never clobbers user content without `--force`;
//!   - `.claude/` is wired only when it already exists;
//!   - settings merge preserves every existing key and backs up first;
//!   - routing section lives between `<!-- maapp:begin/end -->` markers;
//!   - exit 0 on success (including all-skips), 2 on parse/IO errors.

use assert_cmd::Command;
use predicates::prelude::*;
use std::collections::BTreeMap;
use std::path::Path;
use tempfile::TempDir;

fn maapp() -> Command {
    let mut cmd = Command::cargo_bin("maapp").expect("binary builds");
    // The graph-location hint reads $MAAPP_GRAPH; pin the default convention.
    cmd.env_remove("MAAPP_GRAPH");
    cmd
}

/// The canonical hook bytes (single source: the package asset).
fn hook_asset() -> Vec<u8> {
    std::fs::read("package/claude/hooks/maapp-drift-nudge.js").unwrap()
}

/// Deterministic recursive tree snapshot: rel-path -> file bytes.
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn read_settings(dir: &Path) -> serde_json::Value {
    let bytes = std::fs::read(dir.join(".claude/settings.json")).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn fresh_empty_dir_creates_maapp_and_agents_md_no_claude() {
    let tmp = TempDir::new().unwrap();
    let out = maapp()
        .args(["init", "--dir", tmp.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();

    assert!(tmp.path().join(".maapp").is_dir(), ".maapp/ created");
    // No graph is authored; the hint points at the /maapp skill instead.
    assert!(!tmp.path().join(".maapp/graph.json").exists());
    assert!(stdout.contains("no graph"), "graph hint printed: {stdout}");
    assert!(
        stdout.contains("/maapp skill"),
        "hint names the skill: {stdout}"
    );

    // AGENTS.md created (no CLAUDE.md present) with the marker-wrapped section.
    let agents = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("<!-- maapp:begin -->"));
    assert!(agents.contains("<!-- maapp:end -->"));
    assert!(agents.contains("## App-structure graph (maapp)"));
    // Only the section BELOW the `---` separator is embedded, not the preamble.
    assert!(!agents.contains("maapp routing patch"));

    // .claude/ did not exist, so it is NOT created.
    assert!(!tmp.path().join(".claude").exists());
    assert!(stdout.contains("write .maapp/"), "action line: {stdout}");
    assert!(stdout.contains("write AGENTS.md"), "action line: {stdout}");
}

#[test]
fn existing_claude_settings_merged_preserving_keys() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
    let original = r#"{
  "permissions": { "allow": ["Bash(ls:*)"] },
  "model": "opus"
}
"#;
    std::fs::write(tmp.path().join(".claude/settings.json"), original).unwrap();

    maapp()
        .args(["init", "--dir", tmp.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "write .claude/hooks/maapp-drift-nudge.js",
        ))
        .stdout(predicate::str::contains("update .claude/settings.json"));

    // Hook bytes == the single-source package asset.
    let written = std::fs::read(tmp.path().join(".claude/hooks/maapp-drift-nudge.js")).unwrap();
    assert_eq!(
        written,
        hook_asset(),
        "hook file is the embedded asset, byte-identical"
    );

    // Settings: user keys preserved, our PostToolUse entry merged in.
    let settings = read_settings(tmp.path());
    assert_eq!(settings["model"], "opus");
    assert_eq!(settings["permissions"]["allow"][0], "Bash(ls:*)");
    let post = settings["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 1);
    let entry = serde_json::to_string(&post[0]).unwrap();
    assert!(
        entry.contains("maapp-drift-nudge.js"),
        "our hook wired: {entry}"
    );

    // Backup carries the ORIGINAL bytes.
    let bak = std::fs::read(tmp.path().join(".claude/settings.json.bak")).unwrap();
    assert_eq!(bak, original.as_bytes());
}

#[test]
fn second_run_all_skips_and_tree_is_byte_identical() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
    let dir = tmp.path().to_str().unwrap();

    maapp()
        .args(["init", "--ci", "--dir", dir])
        .assert()
        .success();
    assert!(
        tmp.path()
            .join(".github/workflows/maapp-gate.yml")
            .is_file()
    );
    let before = tree(tmp.path());

    let out = maapp()
        .args(["init", "--ci", "--dir", dir])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    for line in stdout.lines() {
        assert!(
            line.starts_with("skip ") || line.starts_with("note:"),
            "second run must be all skips/notes, got: {line}"
        );
    }
    assert_eq!(before, tree(tmp.path()), "re-run mutated the tree");
}

#[test]
fn marker_section_restored_user_content_untouched() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("CLAUDE.md"),
        "# My project\n\nUser prose stays.\n",
    )
    .unwrap();
    let dir = tmp.path().to_str().unwrap();

    maapp().args(["init", "--dir", dir]).assert().success();
    // No AGENTS.md: CLAUDE.md wins when present.
    assert!(!tmp.path().join("AGENTS.md").exists());

    // Tamper INSIDE the markers; add user content AFTER them.
    let path = tmp.path().join("CLAUDE.md");
    let content = std::fs::read_to_string(&path).unwrap();
    let begin = content.find("<!-- maapp:begin -->").unwrap();
    let end = content.find("<!-- maapp:end -->").unwrap() + "<!-- maapp:end -->".len();
    let tampered = format!(
        "{}<!-- maapp:begin -->\nTAMPERED\n<!-- maapp:end -->{}\n\nTrailing user text.\n",
        &content[..begin],
        &content[end..]
    );
    std::fs::write(&path, &tampered).unwrap();

    maapp()
        .args(["init", "--dir", dir])
        .assert()
        .success()
        .stdout(predicate::str::contains("update CLAUDE.md"));

    let restored = std::fs::read_to_string(&path).unwrap();
    assert!(
        restored.contains("## App-structure graph (maapp)"),
        "section restored"
    );
    assert!(!restored.contains("TAMPERED"), "tampering replaced");
    assert!(restored.contains("# My project"), "user heading untouched");
    assert!(
        restored.contains("User prose stays."),
        "user prose untouched"
    );
    assert!(
        restored.contains("Trailing user text."),
        "content after markers untouched"
    );
}

#[test]
fn existing_posttooluse_entries_preserved_ours_appended() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
    let existing = serde_json::json!({
        "hooks": {
            "PostToolUse": [
                { "matcher": "Bash", "hooks": [{ "type": "command", "command": "echo hi" }] }
            ]
        }
    });
    std::fs::write(
        tmp.path().join(".claude/settings.json"),
        serde_json::to_string_pretty(&existing).unwrap(),
    )
    .unwrap();

    maapp()
        .args(["init", "--dir", tmp.path().to_str().unwrap()])
        .assert()
        .success();

    let settings = read_settings(tmp.path());
    let post = settings["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 2, "theirs + ours: {post:?}");
    assert_eq!(
        post[0]["hooks"][0]["command"], "echo hi",
        "their entry first, untouched"
    );
    assert!(
        serde_json::to_string(&post[1])
            .unwrap()
            .contains("maapp-drift-nudge.js"),
        "our entry appended"
    );
}

#[test]
fn modified_hook_skipped_without_force_overwritten_with_force() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
    let dir = tmp.path().to_str().unwrap();
    maapp().args(["init", "--dir", dir]).assert().success();

    let hook = tmp.path().join(".claude/hooks/maapp-drift-nudge.js");
    std::fs::write(&hook, "// user edit\n").unwrap();

    // Without --force: skip + warning, exit 0, user bytes intact.
    maapp()
        .args(["init", "--dir", dir])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "skip .claude/hooks/maapp-drift-nudge.js (exists, modified",
        ));
    assert_eq!(std::fs::read(&hook).unwrap(), b"// user edit\n");

    // With --force: overwritten back to the canonical asset.
    maapp()
        .args(["init", "--force", "--dir", dir])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "update .claude/hooks/maapp-drift-nudge.js",
        ));
    assert_eq!(std::fs::read(&hook).unwrap(), hook_asset());
}

#[test]
fn invalid_settings_json_exits_two_file_untouched() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
    let garbage = b"{ this is not json";
    std::fs::write(tmp.path().join(".claude/settings.json"), garbage).unwrap();

    maapp()
        .args(["init", "--dir", tmp.path().to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("settings"));

    let after = std::fs::read(tmp.path().join(".claude/settings.json")).unwrap();
    assert_eq!(
        after, garbage,
        "unparseable settings must never be rewritten"
    );
    assert!(
        !tmp.path().join(".claude/settings.json.bak").exists(),
        "no backup on error"
    );
}

#[test]
fn dry_run_makes_no_filesystem_changes() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
    std::fs::write(tmp.path().join(".claude/settings.json"), "{}\n").unwrap();
    let before = tree(tmp.path());

    let out = maapp()
        .args([
            "init",
            "--dry-run",
            "--ci",
            "--dir",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("would write .maapp/"),
        "dry-run announces: {stdout}"
    );
    assert!(
        stdout.contains("would write .github/workflows/maapp-gate.yml"),
        "dry-run covers --ci: {stdout}"
    );

    assert_eq!(
        before,
        tree(tmp.path()),
        "--dry-run must not touch the tree"
    );
    assert!(
        !tmp.path().join(".maapp").exists(),
        "--dry-run created .maapp/"
    );
}

#[test]
fn ci_gate_written_with_create_dirs_and_matches_asset() {
    let tmp = TempDir::new().unwrap();
    maapp()
        .args(["init", "--ci", "--dir", tmp.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "write .github/workflows/maapp-gate.yml",
        ));
    let written = std::fs::read(tmp.path().join(".github/workflows/maapp-gate.yml")).unwrap();
    let asset = std::fs::read("package/ci/maapp-gate.yml").unwrap();
    assert_eq!(
        written, asset,
        "CI gate is the embedded asset, byte-identical"
    );
}

/// T3a — the CI gate template wires the `fmt --check` canonical-form gate, and
/// the CLAUDE.md/AGENTS.md routing patch names it in the maintenance loop.
#[test]
fn ci_gate_and_routing_patch_wire_fmt_check() {
    let tmp = TempDir::new().unwrap();
    maapp()
        .args(["init", "--ci", "--dir", tmp.path().to_str().unwrap()])
        .assert()
        .success();

    let ci = std::fs::read_to_string(tmp.path().join(".github/workflows/maapp-gate.yml")).unwrap();
    assert!(
        ci.contains("maapp fmt \"$MAAPP_GRAPH\" --check"),
        "CI gate must run `maapp fmt --check` as a hard gate:\n{ci}"
    );

    // The routing patch lands in AGENTS.md (no .claude/ here) — its maintenance
    // loop must name `maapp fmt` so agents keep the graph canonical.
    let agents = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
    assert!(
        agents.contains("maapp fmt"),
        "routing patch maintenance loop must name `maapp fmt`:\n{agents}"
    );
}

// ---------------------------------------------------------------------------
// T1c — binary-aware `init --ci` (the gate is FAIL-CLOSED and dead until R1
// pins release binaries; installing it silently would guarantee a red CI). The
// loud stderr warning fires whenever `--ci` installs the still-TODO template.
// ---------------------------------------------------------------------------

#[test]
fn ci_flag_warns_the_gate_is_a_fail_closed_todo() {
    let tmp = TempDir::new().unwrap();
    let out = maapp()
        .args(["init", "--ci", "--dir", tmp.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "init --ci still exits 0");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("FAIL-CLOSED"),
        "loud fail-closed warning on --ci: {stderr}"
    );
    assert!(
        stderr.to_lowercase().contains("todo"),
        "warning names the unfilled TODO: {stderr}"
    );
    assert!(
        stderr.contains("maapp-gate.yml"),
        "warning names the gate file: {stderr}"
    );
    // The install-pointer is present so the adopter knows how to fix it.
    assert!(
        stderr.contains("INTEGRATIONS.md") || stderr.contains("release"),
        "warning points at the fix: {stderr}"
    );
}

#[test]
fn no_ci_flag_no_gate_warning() {
    let tmp = TempDir::new().unwrap();
    let out = maapp()
        .args(["init", "--dir", tmp.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        !stderr.contains("FAIL-CLOSED"),
        "no gate warning when --ci is absent: {stderr}"
    );
}

#[test]
fn dry_run_ci_still_warns_the_gate_is_dead() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
    let out = maapp()
        .args([
            "init",
            "--dry-run",
            "--ci",
            "--dir",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("FAIL-CLOSED"),
        "dry-run --ci previews the dead-gate warning: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// T1b — drift-nudge hook quiet-mode for unanchored graphs.
//
// The hook (package/claude/hooks/maapp-drift-nudge.js) is a Node script that
// shells out to `maapp check-drift`. We drive it end-to-end with (a) a fake
// `maapp` on PATH emitting a canned check-drift report and (b) a fixture graph,
// asserting on the `additionalContext` it writes to stdout. Unix-only: the shim
// is a chmod+x shell script — a documented platform limitation for this harness
// (the hook itself runs under Claude Code, a unix-dominant environment). The
// per-session dedupe marker dir is pinned via MAAPP_NUDGE_STATE_DIR so the test
// is hermetic (no /tmp litter, no cross-run contamination).
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod hook_quiet_mode {
    use super::TempDir;
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command as StdCommand, Stdio};

    const HOOK: &str = "package/claude/hooks/maapp-drift-nudge.js";

    fn node_available() -> bool {
        StdCommand::new("node")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Write an executable fake `maapp` into `dir` that prints `stdout_json` and
    /// exits with `code` for ANY args (the hook only calls `check-drift`).
    fn fake_maapp(dir: &Path, stdout_json: &str, code: i32) {
        use std::os::unix::fs::PermissionsExt;
        let script = format!("#!/bin/sh\ncat <<'JSON'\n{stdout_json}\nJSON\nexit {code}\n");
        let bin = dir.join("maapp");
        std::fs::write(&bin, script).unwrap();
        let mut perms = std::fs::metadata(&bin).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin, perms).unwrap();
    }

    fn write_graph(root: &Path, json: &str) {
        std::fs::create_dir_all(root.join(".maapp")).unwrap();
        std::fs::write(root.join(".maapp/graph.json"), json).unwrap();
    }

    fn jstr(s: &Path) -> String {
        serde_json::to_string(&s.to_string_lossy()).unwrap()
    }

    /// Run the hook with `stdin_json`, a fake `maapp` prepended to PATH, and the
    /// dedupe state dir pinned to `state_dir`. Asserts exit 0 (silent contract),
    /// returns stdout.
    fn run_hook(fakebin: &Path, state_dir: &Path, stdin_json: &str) -> String {
        let orig_path = std::env::var("PATH").unwrap_or_default();
        let mut child = StdCommand::new("node")
            .arg(HOOK)
            .env("PATH", format!("{}:{}", fakebin.display(), orig_path))
            .env("MAAPP_NUDGE_STATE_DIR", state_dir)
            .env_remove("MAAPP_GRAPH")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin_json.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "hook must always exit 0 (silent contract); stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn unanchored_graph_suppresses_unmapped_nudge_and_hints_once_per_session() {
        assert!(
            node_available(),
            "node is required to run the drift-nudge hook fixture test"
        );
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // 0 anchors: the node carries `refs` but no `source`.
        write_graph(
            root,
            r#"{"meta":{},"nodes":{"Home":{"kind":"Screen","refs":{}}}}"#,
        );
        let fakebin = root.join("fakebin");
        std::fs::create_dir_all(&fakebin).unwrap();
        // check-drift reports unmapped changes (exit 1 = drift) — the noise case.
        fake_maapp(
            &fakebin,
            r#"{"unmapped_changes":["src/foo.rs","src/bar.rs"],"stale_candidates":[],"anchor_rot":[]}"#,
            1,
        );
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let stdin = format!(
            r#"{{"cwd":{root},"session_id":"sess-quiet","tool_input":{{"file_path":{fp}}}}}"#,
            root = jstr(root),
            fp = jstr(&root.join("src/foo.rs")),
        );

        // First edit: ONE quiet hint, and NO verbose per-edit nudge.
        let out1 = run_hook(&fakebin, &state, &stdin);
        assert!(
            out1.contains("no refs.source anchors"),
            "unanchored hint emitted: {out1}"
        );
        assert!(
            !out1.contains("check-drift"),
            "verbose per-edit nudge suppressed for an unanchored graph: {out1}"
        );

        // Second edit, same session: deduped -> fully silent.
        let out2 = run_hook(&fakebin, &state, &stdin);
        assert!(out2.is_empty(), "hint is once-per-session: {out2:?}");
    }

    #[test]
    fn anchored_graph_still_nudges_on_stale_edit() {
        assert!(
            node_available(),
            "node is required to run the drift-nudge hook fixture test"
        );
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // Anchored: the Home node anchors src/home.rs.
        write_graph(
            root,
            r#"{"meta":{},"nodes":{"Home":{"kind":"Screen","refs":{"source":"src/home.rs"}}}}"#,
        );
        let fakebin = root.join("fakebin");
        std::fs::create_dir_all(&fakebin).unwrap();
        // Fresh (exit 0): the stale signal comes from the LOCAL anchor match on
        // the edited file, exactly as before — quiet-mode must not touch this.
        fake_maapp(&fakebin, "{}", 0);
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let stdin = format!(
            r#"{{"cwd":{root},"session_id":"sess-anchored","tool_input":{{"file_path":{fp}}}}}"#,
            root = jstr(root),
            fp = jstr(&root.join("src/home.rs")),
        );

        let out = run_hook(&fakebin, &state, &stdin);
        assert!(
            out.contains("may be stale after this edit") && out.contains("Home"),
            "anchored stale nudge unchanged: {out}"
        );
        assert!(
            !out.contains("no refs.source anchors"),
            "no quiet hint for an anchored graph: {out}"
        );
    }
}
