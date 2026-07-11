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
