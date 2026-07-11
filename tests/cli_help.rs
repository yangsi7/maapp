// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! CLI help-surface tests (clig.dev conformance at the process boundary):
//! every subcommand's long help carries an Examples section; bare `maapp`
//! prints the full help (never a bare error) and exits 2 per the clap
//! convention for a missing required subcommand; `--version` prints the crate
//! version; help goes to stdout, parse errors to stderr; no ANSI escapes
//! reach a non-TTY pipe (anstream) or a `NO_COLOR` environment.

use assert_cmd::Command;
use predicates::prelude::*;

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

/// Every user-facing subcommand (the built-in `help` is clap's own).
const SUBCOMMANDS: [&str; 14] = [
    "validate",
    "diff",
    "check-drift",
    "stamp",
    "add-node",
    "update-node",
    "remove-node",
    "add-edge",
    "remove-edge",
    "export",
    "query",
    "render",
    "schema",
    "init",
];

/// `<cmd> --help` (long help) must show an Examples section with at least two
/// invocations of the subcommand itself, and exit 0 with a clean stderr.
#[test]
fn every_subcommand_long_help_has_examples() {
    for cmd in SUBCOMMANDS {
        let assert = maapp()
            .args([cmd, "--help"])
            .assert()
            .success()
            .stderr(predicate::str::is_empty());
        let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
        assert!(
            out.contains("Examples:"),
            "`maapp {cmd} --help` lacks an Examples section"
        );
        let invocations = out.matches(&format!("maapp {cmd}")).count();
        assert!(
            invocations >= 2,
            "`maapp {cmd} --help` Examples shows {invocations} `maapp {cmd}` invocation(s), want >= 2"
        );
    }
}

/// Bare `maapp` prints the FULL help (usage + command list) to stderr and
/// exits 2 (clap's missing-subcommand convention) — never a bare one-line
/// error with no guidance.
#[test]
fn no_args_prints_full_help_and_exits_two() {
    maapp()
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("Usage: maapp <COMMAND>"))
        .stderr(predicate::str::contains("Commands:"));
}

/// `--version` prints exactly `maapp <crate version>` to stdout, exit 0.
#[test]
fn version_prints_crate_version_to_stdout() {
    maapp()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("maapp {}\n", env!("CARGO_PKG_VERSION")))
        .stderr(predicate::str::is_empty());
}

/// `--help` is normal output: stdout, exit 0, empty stderr (clig.dev).
#[test]
fn long_help_goes_to_stdout_and_exits_zero() {
    maapp()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: maapp <COMMAND>"))
        .stderr(predicate::str::is_empty());
}

/// A parse error (unknown subcommand) is diagnostics: stderr, exit 2,
/// nothing on stdout.
#[test]
fn unknown_subcommand_error_goes_to_stderr() {
    maapp()
        .arg("frobnicate")
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("error"));
}

/// assert_cmd pipes stdout (not a TTY): clap's anstream must emit no ANSI
/// escape sequences on a pipe.
#[test]
fn help_carries_no_ansi_escapes_on_a_pipe() {
    let assert = maapp().arg("--help").assert().success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        !out.contains('\u{1b}'),
        "`maapp --help` emitted ANSI escapes to a non-TTY pipe"
    );
}

/// `NO_COLOR=1` must also suppress ANSI escapes (informal standard clap
/// honours via anstream).
#[test]
fn no_color_env_suppresses_ansi_escapes() {
    let assert = maapp()
        .env("NO_COLOR", "1")
        .args(["validate", "--help"])
        .assert()
        .success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        !out.contains('\u{1b}'),
        "`maapp validate --help` emitted ANSI escapes despite NO_COLOR=1"
    );
}
