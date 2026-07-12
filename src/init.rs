//! `maapp init` — wire a consumer repo for the graph loop.
//!
//! Writes the distribution package (drift-nudge hook, settings hooks block,
//! routing section, optional CI gate) into a target directory, idempotently:
//! re-running is all skips, user content is NEVER clobbered without `--force`,
//! and an unparseable `.claude/settings.json` aborts BEFORE any write.
//!
//! Design: two phases. `plan()` is read-only — it loads and parses everything
//! and produces the ordered action list; any parse error exits there with the
//! tree untouched. `run()` then applies the writes (skipped under `--dry-run`).
//! Output ordering is FIXED (`.maapp/` → hook → settings → routing → CI), so
//! stdout is deterministic across runs and machines.
//!
//! Assets are embedded via `include_str!` from `package/` — single source, the
//! copy-pasteable files in the repo ARE what the binary installs.
//!
//! Native-only by nature (std::fs): gated in `lib.rs` behind
//! `all(feature = "cli", not(target_arch = "wasm32"))` like `drift`/`mutate`.

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use crate::error::EngineError;

/// The PostToolUse drift-nudge hook (single source: `package/claude/hooks/`).
pub const HOOK_JS: &str = include_str!("../package/claude/hooks/maapp-drift-nudge.js");
/// The routing patch, WITH its authoring preamble (split at `---` before use).
const CLAUDE_MD_PATCH: &str = include_str!("../package/claude/CLAUDE-md-patch.md");
/// The GitHub Actions PR gate (single source: `package/ci/`).
pub const CI_GATE_YML: &str = include_str!("../package/ci/maapp-gate.yml");

/// Sentinel present in the CI gate template while its install step is an
/// unfilled fail-closed TODO. R1 (publish binaries + fill the install step)
/// removes it, which auto-silences the init-time warning in `run`.
const CI_GATE_TODO_SENTINEL: &str = "TODO(release)";
/// The exact `hooks` block to merge into `.claude/settings.json`.
pub const SETTINGS_SNIPPET: &str = include_str!("../package/claude/settings-snippet.json");

/// Marker pair wrapping the routing section in CLAUDE.md / AGENTS.md. The
/// region between them is OWNED by maapp (idempotent update path); everything
/// outside is user content and never touched.
pub const MARKER_BEGIN: &str = "<!-- maapp:begin -->";
pub const MARKER_END: &str = "<!-- maapp:end -->";

/// Relative paths of everything init touches (also the printed names).
const REL_MAAPP_DIR: &str = ".maapp/";
const REL_HOOK: &str = ".claude/hooks/maapp-drift-nudge.js";
const REL_SETTINGS: &str = ".claude/settings.json";
const REL_SETTINGS_BAK: &str = ".claude/settings.json.bak";
const REL_CI: &str = ".github/workflows/maapp-gate.yml";

/// Options for one `init` run. `graph_env` is `$MAAPP_GRAPH` as seen by the
/// CALLER (the lib never reads the environment — determinism + testability).
#[derive(Debug)]
pub struct InitOptions {
    /// Target directory (the consumer repo root).
    pub dir: PathBuf,
    /// Overwrite maapp-owned files whose content has been modified.
    pub force: bool,
    /// Plan only: print `would write`/`would update`, touch nothing.
    pub dry_run: bool,
    /// Also install the `.github/workflows/maapp-gate.yml` PR gate.
    pub ci: bool,
    /// The `MAAPP_GRAPH` env value, if set (graph-location convention).
    pub graph_env: Option<String>,
}

/// The deterministic, ordered action log of a run (one printed line each).
#[derive(Debug)]
pub struct InitOutcome {
    pub lines: Vec<String>,
}

/// One planned step: the line to print plus the write to apply (if any).
struct Step {
    line: String,
    op: Op,
}

/// The filesystem effect of a step. Everything is computed at plan time; apply
/// is a dumb executor.
enum Op {
    /// Create a directory (and parents).
    MkDir(PathBuf),
    /// Write `bytes` to `path`, creating parent dirs; `backup` (original
    /// bytes) is written first when replacing a parseable user file.
    Write {
        path: PathBuf,
        bytes: Vec<u8>,
        backup: Option<(PathBuf, Vec<u8>)>,
    },
    /// Print-only (skips, notes).
    None,
}

/// Run `maapp init`: plan (read-only, all parsing happens here), then apply
/// unless `--dry-run`. Returns the ordered action lines for the CLI to print.
pub fn run(opts: &InitOptions) -> Result<InitOutcome, EngineError> {
    let steps = plan(opts)?;
    if !opts.dry_run {
        for step in &steps {
            apply(&step.op)?;
        }
    }
    // The CI gate ships FAIL-CLOSED and stays RED until you pin release
    // binaries. Warn LOUDLY (stderr) whenever `--ci` installs the still-TODO
    // template, so an adopter is never surprised by a guaranteed-red gate. The
    // warning lives here (not the CLI) so it surfaces regardless of the caller,
    // and self-silences once that TODO is filled (the sentinel disappears).
    if opts.ci && CI_GATE_YML.contains(CI_GATE_TODO_SENTINEL) {
        eprintln!(
            "warning: {REL_CI} is FAIL-CLOSED and its \"Install maapp\" step is an \
             unfilled TODO — the gate will be RED on every PR until you pin maapp \
             release binaries (edit that step). Interim: build from a pinned rev \
             (`cargo install --git <maapp-repo> --rev <sha> maapp`); permanent: use \
             the published release URL + sha256 once available. See \
             docs/INTEGRATIONS.md."
        );
    }
    Ok(InitOutcome {
        lines: steps.into_iter().map(|s| s.line).collect(),
    })
}

/// Build the ordered step list without touching the filesystem (reads only).
fn plan(opts: &InitOptions) -> Result<Vec<Step>, EngineError> {
    let mut steps = Vec::new();

    // 1. `.maapp/` home dir (no graph is authored — that is the skill's job).
    steps.push(plan_maapp_dir(opts));
    if let Some(hint) = graph_hint(opts) {
        steps.push(Step {
            line: hint,
            op: Op::None,
        });
    }

    // 2. `.claude/` wiring — only when the dir already exists (never created).
    if opts.dir.join(".claude").is_dir() {
        steps.push(plan_asset(opts, REL_HOOK, HOOK_JS.as_bytes()));
        steps.extend(plan_settings(opts)?);
    } else {
        steps.push(Step {
            line: "note: .claude/ absent; hook + settings not wired (create .claude/ and re-run)"
                .to_string(),
            op: Op::None,
        });
    }

    // 3. Routing section in CLAUDE.md / AGENTS.md.
    steps.push(plan_routing(opts)?);

    // 4. Optional CI gate.
    if opts.ci {
        steps.push(plan_asset(opts, REL_CI, CI_GATE_YML.as_bytes()));
    }

    Ok(steps)
}

/// Execute one planned op. Parent dirs are created for every write.
fn apply(op: &Op) -> Result<(), EngineError> {
    match op {
        Op::MkDir(path) => fs::create_dir_all(path)?,
        Op::Write {
            path,
            bytes,
            backup,
        } => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            if let Some((bak_path, bak_bytes)) = backup {
                fs::write(bak_path, bak_bytes)?;
            }
            fs::write(path, bytes)?;
        }
        Op::None => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// step planners
// ---------------------------------------------------------------------------

/// `.maapp/`: create when absent, skip when present.
fn plan_maapp_dir(opts: &InitOptions) -> Step {
    let path = opts.dir.join(".maapp");
    if path.is_dir() {
        Step {
            line: format!("skip {REL_MAAPP_DIR} (exists)"),
            op: Op::None,
        }
    } else {
        Step {
            line: format!("{} {REL_MAAPP_DIR}", write_verb(opts)),
            op: Op::MkDir(path),
        }
    }
}

/// Graph-location convention: `$MAAPP_GRAPH` if set (absolute, or relative to
/// the target dir), else `.maapp/graph.json`. When no graph exists there,
/// return the next-step hint line.
fn graph_hint(opts: &InitOptions) -> Option<String> {
    let (label, path) = match &opts.graph_env {
        Some(g) => {
            let p = PathBuf::from(g);
            let abs = if p.is_absolute() {
                p
            } else {
                opts.dir.join(&p)
            };
            (format!("MAAPP_GRAPH={g}"), abs)
        }
        None => (
            ".maapp/graph.json".to_string(),
            opts.dir.join(".maapp/graph.json"),
        ),
    };
    if path.is_file() {
        None
    } else {
        Some(format!(
            "note: no graph at {label}; author or ingest one via the /maapp skill, \
             or point MAAPP_GRAPH at an existing graph"
        ))
    }
}

/// Skip/write/update semantics for a byte-exact managed asset (hook, CI gate):
/// absent → write; identical → skip; modified → skip with a warning unless
/// `--force`, which overwrites.
fn plan_asset(opts: &InitOptions, rel: &str, desired: &[u8]) -> Step {
    let path = opts.dir.join(rel);
    match fs::read(&path) {
        Err(_) => Step {
            line: format!("{} {rel}", write_verb(opts)),
            op: write_op(opts, path, desired.to_vec(), None),
        },
        Ok(existing) if existing == desired => Step {
            line: format!("skip {rel} (exists, identical)"),
            op: Op::None,
        },
        Ok(_) if !opts.force => Step {
            line: format!("skip {rel} (exists, modified; use --force to overwrite)"),
            op: Op::None,
        },
        Ok(_) => Step {
            line: format!("{} {rel}", update_verb(opts)),
            op: write_op(opts, path, desired.to_vec(), None),
        },
    }
}

/// Merge the snippet's `hooks.PostToolUse` matcher entry into
/// `.claude/settings.json`, preserving EVERY existing key (serde_json's
/// `preserve_order` keeps the user's key order). Absent file → write the
/// snippet verbatim. Unparseable file or wrong-shaped `hooks` slot → error
/// (exit 2 at the CLI), file untouched. First modification writes a `.bak`.
fn plan_settings(opts: &InitOptions) -> Result<Vec<Step>, EngineError> {
    let path = opts.dir.join(REL_SETTINGS);
    let snippet: Value = serde_json::from_str(SETTINGS_SNIPPET)?;

    let Ok(existing_bytes) = fs::read(&path) else {
        // No settings file: the snippet IS the file (byte-exact single source).
        return Ok(vec![Step {
            line: format!("{} {REL_SETTINGS}", write_verb(opts)),
            op: write_op(opts, path, SETTINGS_SNIPPET.as_bytes().to_vec(), None),
        }]);
    };

    let mut settings: Value =
        serde_json::from_slice(&existing_bytes).map_err(|e| EngineError::SettingsMerge {
            path: path.clone(),
            detail: format!("invalid json ({e})"),
        })?;

    // Our matcher entry, from the single-source snippet.
    let our_entry = snippet["hooks"]["PostToolUse"][0].clone();

    let Some(root) = settings.as_object_mut() else {
        return Err(EngineError::SettingsMerge {
            path,
            detail: "top-level value is not an object".to_string(),
        });
    };
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return Err(EngineError::SettingsMerge {
            path,
            detail: "\"hooks\" is not an object".to_string(),
        });
    };
    let post = hooks_obj
        .entry("PostToolUse")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(entries) = post.as_array_mut() else {
        return Err(EngineError::SettingsMerge {
            path,
            detail: "\"hooks\".\"PostToolUse\" is not an array".to_string(),
        });
    };

    // Idempotency probe: any entry already carrying our command substring.
    let already_wired = entries
        .iter()
        .any(|e| serde_json::to_string(e).is_ok_and(|s| s.contains("maapp-drift-nudge.js")));
    if already_wired {
        return Ok(vec![Step {
            line: format!("skip {REL_SETTINGS} (hook already wired)"),
            op: Op::None,
        }]);
    }
    entries.push(our_entry);

    let mut bytes = serde_json::to_vec_pretty(&settings)?;
    bytes.push(b'\n');
    let bak_path = opts.dir.join(REL_SETTINGS_BAK);
    Ok(vec![
        Step {
            line: format!("{} {REL_SETTINGS_BAK}", write_verb(opts)),
            op: Op::None, // the backup is written by the settings step below
        },
        Step {
            line: format!("{} {REL_SETTINGS}", update_verb(opts)),
            op: write_op(opts, path, bytes, Some((bak_path, existing_bytes))),
        },
    ])
}

/// Routing section: CLAUDE.md if present, else AGENTS.md if present, else
/// CREATE AGENTS.md. Append when the markers are absent; replace the region
/// between them when present (the idempotent update path).
fn plan_routing(opts: &InitOptions) -> Result<Step, EngineError> {
    let (rel, path) = if opts.dir.join("CLAUDE.md").is_file() {
        ("CLAUDE.md", opts.dir.join("CLAUDE.md"))
    } else if opts.dir.join("AGENTS.md").is_file() {
        ("AGENTS.md", opts.dir.join("AGENTS.md"))
    } else {
        let section = marker_block();
        return Ok(Step {
            line: format!("{} AGENTS.md", write_verb(opts)),
            op: write_op(opts, opts.dir.join("AGENTS.md"), section.into_bytes(), None),
        });
    };

    let bytes = fs::read(&path)?;
    let content = String::from_utf8(bytes).map_err(|_| EngineError::PatchTarget {
        path: path.clone(),
        detail: "not valid utf-8".to_string(),
    })?;
    let section = marker_block();

    let updated = match (content.find(MARKER_BEGIN), content.find(MARKER_END)) {
        (Some(begin), Some(end)) if begin < end => {
            let end = end + MARKER_END.len();
            format!(
                "{}{}{}",
                &content[..begin],
                section.trim_end(),
                &content[end..]
            )
        }
        (None, None) => {
            // Append, separated by exactly one blank line.
            let mut out = content.clone();
            while !out.is_empty() && !out.ends_with("\n\n") {
                out.push('\n');
            }
            out.push_str(&section);
            out
        }
        _ => {
            return Err(EngineError::PatchTarget {
                path,
                detail: format!("unbalanced {MARKER_BEGIN} / {MARKER_END} markers"),
            });
        }
    };

    if updated == content {
        Ok(Step {
            line: format!("skip {rel} (section up to date)"),
            op: Op::None,
        })
    } else {
        Ok(Step {
            line: format!("{} {rel}", update_verb(opts)),
            op: write_op(opts, path, updated.into_bytes(), None),
        })
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// The routing section: the patch content BELOW its `---` separator (the part
/// above is authoring instructions, never installed), wrapped in the markers.
fn marker_block() -> String {
    let body = CLAUDE_MD_PATCH
        .split_once("\n---\n")
        .map_or(CLAUDE_MD_PATCH, |(_, below)| below)
        .trim();
    format!("{MARKER_BEGIN}\n{body}\n{MARKER_END}\n")
}

/// Print verbs flip to the conditional under `--dry-run`.
fn write_verb(opts: &InitOptions) -> &'static str {
    if opts.dry_run { "would write" } else { "write" }
}

fn update_verb(opts: &InitOptions) -> &'static str {
    if opts.dry_run {
        "would update"
    } else {
        "update"
    }
}

/// A write op — degraded to `Op::None` under `--dry-run` (the plan/apply split
/// already skips apply, this keeps the ops honest too).
fn write_op(
    opts: &InitOptions,
    path: PathBuf,
    bytes: Vec<u8>,
    backup: Option<(PathBuf, Vec<u8>)>,
) -> Op {
    if opts.dry_run {
        Op::None
    } else {
        Op::Write {
            path,
            bytes,
            backup,
        }
    }
}
