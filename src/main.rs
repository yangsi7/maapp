//! Thin clap shell for the maapp engine.
//!
//! Responsibilities: parse args, call a public lib fn, map `Result` to an exit
//! code. NO engine logic here (rust-conventions: all logic in the lib). `anyhow`
//! is used only at this boundary for context-rich exit messages.
//!
//! Exit codes (ENGINE-CONTRACT §4): 0 clean, 1 validate found ≥1 `E_*`,
//! 2 usage / load error.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use maapp::{ValidateReport, query, render, to_canonical_json, validate_full};

#[cfg(not(target_arch = "wasm32"))]
use maapp::load_graph_from_path;

/// wasm32 shim: the lib keeps its core free of `std::fs`, so
/// `load_graph_from_path` is native-only. The CLI bin still has to COMPILE for
/// the wasm gate (`cargo build --target wasm32-unknown-unknown`, default
/// features include `cli`); it is never RUN there, so every load reports the
/// file as not found (exit 2), no panic path.
#[cfg(target_arch = "wasm32")]
fn load_graph_from_path(path: &std::path::Path) -> Result<maapp::Graph, maapp::EngineError> {
    Err(maapp::EngineError::FileNotFound(path.to_path_buf()))
}

/// maapp — app-spec graph engine.
#[derive(Parser)]
#[command(name = "maapp", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Lint a graph file (hard-fail E_* + advisory W_*).
    #[command(after_long_help = "\
Examples:
  # Lint a graph: exit 0 clean, 1 on any hard E_* finding, 2 on load error
  maapp validate examples/checkout.json

  # Machine-readable findings as canonical JSON on stdout
  maapp validate examples/checkout.json --json
")]
    Validate {
        /// Path to the graph JSON file.
        file: PathBuf,
        /// Emit machine-readable findings as canonical JSON.
        #[arg(long)]
        json: bool,
    },
    /// Semantic diff of two graph files (exit 0 identical, 1 differences, 2 error).
    #[command(after_long_help = "\
Examples:
  # Edit a working copy, then diff it against the original
  cp examples/checkout.json /tmp/checkout.json
  maapp remove-edge fires trig:cart/PromoTap act:cart/PresentPromoCode /tmp/checkout.json
  maapp diff examples/checkout.json /tmp/checkout.json

  # Machine-readable change report as canonical JSON
  maapp diff examples/checkout.json /tmp/checkout.json --json
")]
    Diff {
        /// Path to the OLD graph JSON file.
        old: PathBuf,
        /// Path to the NEW graph JSON file.
        new: PathBuf,
        /// Emit the machine-readable report as canonical JSON.
        #[arg(long)]
        json: bool,
    },
    /// Check graph anchors against the source repo for drift since
    /// meta.provenance.asOf (exit 0 fresh, 1 drift, 2 error). The ONLY
    /// disk-touching verb: reads the graph + the named repo (read-only git).
    #[command(after_long_help = "\
Examples:
  # Compare graph anchors against the repo since the stamped asOf rev
  maapp check-drift .maapp/graph.json --repo .

  # Override the base rev instead of reading meta.provenance.asOf
  maapp check-drift .maapp/graph.json --repo . --since HEAD~5

  # Machine-readable drift report (exit 0 fresh, 1 drift, 2 error)
  maapp check-drift .maapp/graph.json --repo . --json
")]
    CheckDrift {
        /// Path to the graph JSON file.
        file: PathBuf,
        /// Path to the source repository the anchors name files in.
        #[arg(long)]
        repo: PathBuf,
        /// Base rev override (defaults to meta.provenance.asOf).
        #[arg(long)]
        since: Option<String>,
        /// Emit the machine-readable report as canonical JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stamp meta.provenance.asOf to a source-repo rev (default HEAD),
    /// closing the anchor → stamp → check-drift loop (spec §6.3).
    #[command(after_long_help = "\
Examples:
  # Stamp meta.provenance.asOf to the repo's current HEAD
  maapp stamp .maapp/graph.json --repo .

  # Stamp an explicit rev instead of HEAD
  maapp stamp .maapp/graph.json --repo . --as-of HEAD~1

The graph must already carry a meta.provenance object ({\"origin\": ...});
stamp only bumps its asOf field.
")]
    Stamp {
        /// Path to the graph JSON file (rewritten atomically).
        file: PathBuf,
        /// Path to the source repository to resolve the rev in.
        #[arg(long)]
        repo: PathBuf,
        /// Rev to stamp (defaults to the repo's HEAD).
        #[arg(long)]
        as_of: Option<String>,
    },
    /// Add a node (atomic validated write; layer derived from --kind).
    #[command(after_long_help = "\
Examples:
  # Add a screen (layer derived from --kind)
  maapp add-node screen:settings/Language examples/settings-account.json \\
      --kind Screen --intent \"App language picker\"

  # Add a store with a source anchor and an attr in the same atomic write
  maapp add-node store:chat/DraftStore examples/chat.json \\
      --kind StateStore --intent \"Unsent drafts per conversation\" \\
      --ref source=src/stores/drafts.ts --attr persistence=disk
")]
    AddNode {
        /// The new node's slug, e.g. ds:home/Feed.
        slug: String,
        /// Path to the graph JSON file (rewritten atomically, canonical form).
        file: PathBuf,
        /// Node kind (frozen-core or a nodeKindRegistry x- kind).
        #[arg(long)]
        kind: String,
        /// One-line human intent.
        #[arg(long)]
        intent: String,
        /// refs entry K=V (repeatable; V parses as JSON when it can).
        #[arg(long = "ref", value_name = "K=V")]
        refs: Vec<String>,
        /// attrs entry K=V (repeatable; V parses as JSON when it can).
        #[arg(long = "attr", value_name = "K=V")]
        attrs: Vec<String>,
        /// Bump meta.provenance.asOf to this SHA in the same atomic write.
        #[arg(long = "as-of", value_name = "SHA")]
        as_of: Option<String>,
    },
    /// Update a node's intent / refs / attrs (no flags = error, exit 2).
    #[command(after_long_help = "\
Examples:
  # Replace the one-line intent
  maapp update-node screen:cart/Bag examples/checkout.json \\
      --intent \"Cart with promo entry and qty steppers\"

  # Set a source anchor and an attr in the same atomic write
  maapp update-node store:CartStore examples/checkout.json \\
      --ref source=src/state/cart.ts --attr persistence=memory
")]
    UpdateNode {
        /// The node's slug.
        slug: String,
        /// Path to the graph JSON file (rewritten atomically, canonical form).
        file: PathBuf,
        /// Replace the one-line intent.
        #[arg(long)]
        intent: Option<String>,
        /// refs entry K=V to set (repeatable).
        #[arg(long = "ref", value_name = "K=V")]
        refs: Vec<String>,
        /// attrs entry K=V to set (repeatable).
        #[arg(long = "attr", value_name = "K=V")]
        attrs: Vec<String>,
        /// Bump meta.provenance.asOf to this SHA in the same atomic write.
        #[arg(long = "as-of", value_name = "SHA")]
        as_of: Option<String>,
    },
    /// Remove a node; refuses when incident edges exist unless --cascade.
    #[command(after_long_help = "\
Examples:
  # Refuses while incident edges exist (lists them; file untouched)
  maapp remove-node comp:chat/TypingIndicator examples/chat.json

  # Cascade: remove the node plus its incident edges
  maapp remove-node comp:chat/TypingIndicator examples/chat.json --cascade
")]
    RemoveNode {
        /// The node's slug.
        slug: String,
        /// Path to the graph JSON file (rewritten atomically, canonical form).
        file: PathBuf,
        /// Also remove incident edges (prints exactly what was removed).
        #[arg(long)]
        cascade: bool,
        /// Bump meta.provenance.asOf to this SHA in the same atomic write.
        #[arg(long = "as-of", value_name = "SHA")]
        as_of: Option<String>,
    },
    /// Add a typed edge (signature-checked at add time via the validator).
    #[command(after_long_help = "\
Examples:
  # Add a binds edge from a component to its backing store
  maapp add-edge binds comp:checkout/PlaceOrderButton store:CheckoutStore examples/checkout.json

  # A writes edge with a mode attr (signature-checked before the write)
  maapp add-edge writes act:cart/UpdateQty store:CheckoutStore examples/checkout.json --attr mode=set
")]
    AddEdge {
        /// Edge verb, e.g. fires / writes / guardedBy.
        r#type: String,
        /// Source node slug.
        from: String,
        /// Target node slug.
        to: String,
        /// Path to the graph JSON file (rewritten atomically, canonical form).
        file: PathBuf,
        /// Edge attr K=V (repeatable; V parses as JSON when it can).
        #[arg(long = "attr", value_name = "K=V")]
        attrs: Vec<String>,
        /// Bump meta.provenance.asOf to this SHA in the same atomic write.
        #[arg(long = "as-of", value_name = "SHA")]
        as_of: Option<String>,
    },
    /// Remove the edge with the exact (type, from, to) identity.
    #[command(after_long_help = "\
Examples:
  # Remove a fires edge by its exact (type, from, to) identity
  maapp remove-edge fires trig:cart/PromoTap act:cart/PresentPromoCode examples/checkout.json

  # Remove a writes edge (the removal is validated before the write)
  maapp remove-edge writes act:cart/UpdateQty store:CartStore examples/checkout.json
")]
    RemoveEdge {
        /// Edge verb.
        r#type: String,
        /// Source node slug.
        from: String,
        /// Target node slug.
        to: String,
        /// Path to the graph JSON file (rewritten atomically, canonical form).
        file: PathBuf,
        /// Bump meta.provenance.asOf to this SHA in the same atomic write.
        #[arg(long = "as-of", value_name = "SHA")]
        as_of: Option<String>,
    },
    /// Export a slice as a complete, valid, dispatchable graph document
    /// (scoped read: a cold agent reads a slice, never the whole file).
    #[command(after_long_help = "\
Examples:
  # BFS neighborhood around one node (depth 1)
  maapp export examples/checkout.json --slice screen:checkout/Payment

  # Deeper neighborhood as canonical JSON for agent dispatch
  maapp export examples/checkout.json --slice screen:checkout/Payment --depth 2 --json

  # Every node in a scope (all *:checkout/* slugs)
  maapp export examples/checkout.json --slice scope:checkout
")]
    Export {
        /// Path to the graph JSON file (read-only).
        file: PathBuf,
        /// Slice selector: a node slug (BFS neighborhood) or scope:<scope>
        /// (all *:<scope>/* nodes).
        #[arg(long, value_name = "SELECTOR")]
        slice: String,
        /// Neighborhood depth for the slug form (default 1); invalid with a
        /// scope: selector.
        #[arg(long, value_name = "N")]
        depth: Option<usize>,
        /// Emit the slice document as canonical JSON (the machine surface).
        #[arg(long)]
        json: bool,
    },
    /// Scoped reads + traversals.
    #[command(after_long_help = "\
Examples:
  # One node card: intent, refs, attrs, in/out edges
  maapp query node screen:checkout/Payment examples/checkout.json

  # What breaks if this store changes?
  maapp query blast-radius store:CartStore examples/checkout.json --json

  # Follow a trigger to its terminal effects
  maapp query trace trig:review/PlaceOrderTap examples/checkout.json

  # Nodes with no incident edges
  maapp query orphans examples/checkout.json
")]
    Query {
        /// Query verb: node | neighbors | blast-radius | trace | view | orphans
        verb: String,
        /// Remaining args (id?, file, --depth N, --terminals, --json).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },
    /// Generate hub / deps / storyboard / spine / html views.
    #[command(after_long_help = "\
Examples:
  # Hub table: the whole graph at a glance
  maapp render hub examples/checkout.json

  # ASCII storyboard of screens and navigation
  maapp render storyboard examples/checkout.json

  # Self-contained interactive HTML view (requires --out)
  maapp render html examples/checkout.json --out checkout.html
")]
    Render {
        /// Render target.
        what: String,
        /// Remaining args (file, …).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },
    /// Print the canonical maapp-graph JSON Schema (draft 2020-12), generated
    /// from the engine's vocabulary tables (the committed copy lives at
    /// schema/maapp-graph.schema.json).
    #[command(after_long_help = "\
Examples:
  # Print the canonical JSON Schema to stdout
  maapp schema

  # Refresh the committed copy
  maapp schema > schema/maapp-graph.schema.json
")]
    Schema,
    /// Wire a project for the graph loop: .maapp/ home, the drift-nudge hook
    /// and settings merge (only when .claude/ exists), the CLAUDE.md/AGENTS.md
    /// routing section, and optionally the CI gate. Idempotent; never
    /// overwrites modified files without --force.
    #[command(after_long_help = "\
Examples:
  # Wire the current project (idempotent, safe to re-run)
  maapp init

  # Preview without touching the filesystem
  maapp init --dry-run

  # Also write the .github/workflows/maapp-gate.yml PR gate
  maapp init --ci

  # Target another directory
  maapp init --dir ../my-app --dry-run
")]
    Init {
        /// Overwrite maapp-owned files whose content has been modified.
        #[arg(long)]
        force: bool,
        /// Print what would change without touching the filesystem.
        #[arg(long)]
        dry_run: bool,
        /// Also write the .github/workflows/maapp-gate.yml PR gate.
        #[arg(long)]
        ci: bool,
        /// Target directory (defaults to the current directory).
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Validate { file, json } => run_validate(&file, json),
        Command::Diff { old, new, json } => run_diff(&old, &new, json),
        Command::CheckDrift {
            file,
            repo,
            since,
            json,
        } => run_check_drift(&file, &repo, since.as_deref(), json),
        Command::Stamp { file, repo, as_of } => run_stamp(&file, &repo, as_of.as_deref()),
        #[cfg(not(target_arch = "wasm32"))]
        Command::AddNode {
            slug,
            file,
            kind,
            intent,
            refs,
            attrs,
            as_of,
        } => emit_mutation((|| {
            let refs = parse_kvs(&refs)?;
            let attrs = parse_kvs(&attrs)?;
            maapp::mutate::add_node(
                &file,
                &slug,
                &kind,
                &intent,
                &refs,
                &attrs,
                as_of.as_deref(),
            )
        })()),
        #[cfg(not(target_arch = "wasm32"))]
        Command::UpdateNode {
            slug,
            file,
            intent,
            refs,
            attrs,
            as_of,
        } => emit_mutation((|| {
            let refs = parse_kvs(&refs)?;
            let attrs = parse_kvs(&attrs)?;
            maapp::mutate::update_node(
                &file,
                &slug,
                intent.as_deref(),
                &refs,
                &attrs,
                as_of.as_deref(),
            )
        })()),
        #[cfg(not(target_arch = "wasm32"))]
        Command::RemoveNode {
            slug,
            file,
            cascade,
            as_of,
        } => emit_mutation(maapp::mutate::remove_node(
            &file,
            &slug,
            cascade,
            as_of.as_deref(),
        )),
        #[cfg(not(target_arch = "wasm32"))]
        Command::AddEdge {
            r#type,
            from,
            to,
            file,
            attrs,
            as_of,
        } => emit_mutation((|| {
            let attrs = parse_kvs(&attrs)?;
            maapp::mutate::add_edge(&file, &r#type, &from, &to, &attrs, as_of.as_deref())
        })()),
        #[cfg(not(target_arch = "wasm32"))]
        Command::RemoveEdge {
            r#type,
            from,
            to,
            file,
            as_of,
        } => emit_mutation(maapp::mutate::remove_edge(
            &file,
            &r#type,
            &from,
            &to,
            as_of.as_deref(),
        )),
        #[cfg(target_arch = "wasm32")]
        Command::AddNode { .. }
        | Command::UpdateNode { .. }
        | Command::RemoveNode { .. }
        | Command::AddEdge { .. }
        | Command::RemoveEdge { .. } => {
            eprintln!("maapp: mutation verbs are native-only (they rewrite the graph on disk)");
            ExitCode::from(2)
        }
        Command::Export {
            file,
            slice,
            depth,
            json,
        } => run_export(&file, &slice, depth, json),
        Command::Query { verb, rest } => run_query(&verb, &rest),
        Command::Render { what, rest } => run_render(&what, &rest),
        Command::Schema => run_schema(),
        #[cfg(not(target_arch = "wasm32"))]
        Command::Init {
            force,
            dry_run,
            ci,
            dir,
        } => run_init(force, dry_run, ci, dir),
        #[cfg(target_arch = "wasm32")]
        Command::Init { .. } => {
            eprintln!("maapp: init is native-only (it writes project files to disk)");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------
// init (native-only; the wasm32 match arm above answers exit 2)
// ---------------------------------------------------------------------------

/// Run `init` and print its deterministic action log. Exit 0 on success
/// (including all-skips), 2 on any usage/IO/parse error. `$MAAPP_GRAPH` is
/// read HERE (the lib never touches the environment) and only steers the
/// graph-location hint.
#[cfg(not(target_arch = "wasm32"))]
fn run_init(force: bool, dry_run: bool, ci: bool, dir: PathBuf) -> ExitCode {
    let opts = maapp::init::InitOptions {
        dir,
        force,
        dry_run,
        ci,
        graph_env: std::env::var("MAAPP_GRAPH").ok(),
    };
    match maapp::init::run(&opts) {
        Ok(outcome) => {
            for line in &outcome.lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("maapp: {e}");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------
// mutation verbs (native-only; the wasm32 match arm above answers exit 2)
// ---------------------------------------------------------------------------

/// Parse repeated `--ref`/`--attr` K=V flags into typed pairs.
#[cfg(not(target_arch = "wasm32"))]
fn parse_kvs(raw: &[String]) -> Result<Vec<(String, serde_json::Value)>, maapp::EngineError> {
    raw.iter().map(|r| maapp::parse_kv(r)).collect()
}

/// Print a mutation outcome (or its refusal) and map to the §1 exit codes:
/// 0 = written, 2 = usage/input error or validation regression (spec §4: the
/// would-be `E_*` findings print before the refusal line; file untouched).
#[cfg(not(target_arch = "wasm32"))]
fn emit_mutation(result: Result<maapp::MutationOutcome, maapp::EngineError>) -> ExitCode {
    match result {
        Ok(outcome) => {
            println!("{}", outcome.summary);
            if let Some(sha) = outcome.stamped {
                println!("STAMPED meta.provenance.asOf = {sha}");
            }
            ExitCode::SUCCESS
        }
        Err(maapp::EngineError::ValidationRegression {
            before,
            after,
            findings,
        }) => {
            for f in &findings {
                eprintln!("{}", f.render());
            }
            eprintln!(
                "maapp: mutation would raise hard validation errors {before} -> {after}; refusing to write"
            );
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("maapp: {e}");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------
// export --slice (pure compute over the loaded graph; wasm-clean lib fn)
// ---------------------------------------------------------------------------

/// Run `export --slice`: emit the slice as a complete graph document
/// (canonical JSON with `--json`, a human summary otherwise). Exit 0 on
/// success, 2 on any usage/input error.
fn run_export(file: &std::path::Path, slice: &str, depth: Option<usize>, json: bool) -> ExitCode {
    let g = match load_graph_from_path(file) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("maapp: {e}");
            return ExitCode::from(2);
        }
    };
    let selector = if let Some(scope) = slice.strip_prefix("scope:") {
        if depth.is_some() {
            eprintln!("maapp: --depth is not valid with a scope: selector (a scope has no depth)");
            return ExitCode::from(2);
        }
        maapp::SliceSelector::Scope(scope)
    } else {
        maapp::SliceSelector::Node {
            slug: slice,
            depth: depth.unwrap_or(1),
        }
    };
    let doc = match maapp::export_slice(&g, selector) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("maapp: {e}");
            return ExitCode::from(2);
        }
    };
    if json {
        match to_canonical_json(&doc) {
            Ok(bytes) => {
                if std::io::stdout().write_all(&bytes).is_err() {
                    return ExitCode::from(2);
                }
            }
            Err(e) => {
                eprintln!("maapp: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        // Human summary: selector + sorted slugs + edge count.
        let mut slugs: Vec<&String> = Vec::new();
        if let Some(layers) = doc.get("nodes").and_then(serde_json::Value::as_object) {
            for layer in layers.values() {
                if let Some(map) = layer.as_object() {
                    slugs.extend(map.keys());
                }
            }
        }
        slugs.sort();
        let edge_count = doc
            .get("edges")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        println!(
            "SLICE {slice}: {} node(s), {edge_count} edge(s)",
            slugs.len()
        );
        for s in slugs {
            println!("  {s}");
        }
    }
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

/// Run `diff` and map the report to the LIFECYCLE-VERBS-SPEC §1 exit codes:
/// 0 = no differences, 1 = differences found, 2 = usage/input error.
fn run_diff(old: &std::path::Path, new: &std::path::Path, json: bool) -> ExitCode {
    let (g_old, g_new) = match (load_graph_from_path(old), load_graph_from_path(new)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("maapp: {e}");
            return ExitCode::from(2);
        }
    };
    let report = match maapp::diff_graphs(&g_old, &g_new) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("maapp: {e}");
            return ExitCode::from(2);
        }
    };
    if json {
        match to_canonical_json(&report) {
            Ok(bytes) => {
                if std::io::stdout().write_all(&bytes).is_err() {
                    return ExitCode::from(2);
                }
            }
            Err(e) => {
                eprintln!("maapp: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        println!("{}", report.render());
    }
    if report.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

// ---------------------------------------------------------------------------
// check-drift + stamp (native-only disk IO; wasm shims below)
// ---------------------------------------------------------------------------

/// Run `check-drift` and map to the spec §1/§3 exit codes: 0 = no drift,
/// 1 = drift found (stale / unmapped / rot), 2 = usage or input error.
/// `unanchored_nodes` is advisory and never drives the exit code.
#[cfg(not(target_arch = "wasm32"))]
fn run_check_drift(
    file: &std::path::Path,
    repo: &std::path::Path,
    since: Option<&str>,
    json: bool,
) -> ExitCode {
    let g = match load_graph_from_path(file) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("maapp: {e}");
            return ExitCode::from(2);
        }
    };
    let report = match maapp::check_drift(&g, repo, since) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("maapp: {e}");
            return ExitCode::from(2);
        }
    };
    if json {
        match to_canonical_json(&report) {
            Ok(bytes) => {
                if std::io::stdout().write_all(&bytes).is_err() {
                    return ExitCode::from(2);
                }
            }
            Err(e) => {
                eprintln!("maapp: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        println!("{}", report.render());
    }
    if report.has_drift() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Run `stamp`: bump meta.provenance.asOf to the resolved rev (default HEAD)
/// in an atomic write. Exit 0 on success, 2 on any error.
#[cfg(not(target_arch = "wasm32"))]
fn run_stamp(file: &std::path::Path, repo: &std::path::Path, as_of: Option<&str>) -> ExitCode {
    match maapp::stamp(file, repo, as_of) {
        Ok(sha) => {
            println!("STAMPED {} meta.provenance.asOf = {sha}", file.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("maapp: {e}");
            ExitCode::from(2)
        }
    }
}

/// wasm32 shims: check-drift/stamp are native-only (they read the source repo
/// from disk and shell out to git). The CLI bin still has to COMPILE for the
/// wasm gate; it is never RUN there, so both report native-only (exit 2).
#[cfg(target_arch = "wasm32")]
fn run_check_drift(
    _file: &std::path::Path,
    _repo: &std::path::Path,
    _since: Option<&str>,
    _json: bool,
) -> ExitCode {
    eprintln!("maapp: check-drift is native-only (reads the source repo from disk)");
    ExitCode::from(2)
}

#[cfg(target_arch = "wasm32")]
fn run_stamp(_file: &std::path::Path, _repo: &std::path::Path, _as_of: Option<&str>) -> ExitCode {
    eprintln!("maapp: stamp is native-only (rewrites the graph file on disk)");
    ExitCode::from(2)
}

/// Emit the canonical JSON Schema derived from the engine tables (pure
/// compute, no filesystem — wasm-clean).
fn run_schema() -> ExitCode {
    match maapp::schema_bytes() {
        Ok(bytes) => {
            if std::io::stdout().write_all(&bytes).is_err() {
                return ExitCode::from(2);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("maapp: {e}");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

/// Run `validate`, print findings, and return the contract exit code.
fn run_validate(file: &std::path::Path, json: bool) -> ExitCode {
    let g = match load_graph_from_path(file) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("maapp: {e}");
            return ExitCode::from(2);
        }
    };

    // `validate_full` runs the core validator always, plus the design-completeness
    // lint pack + the freshness substrate ONLY when opted in via
    // `meta.lints:["design-completeness"]`. When the pack is off, `design_score` and
    // `freshness` are both None ⇒ the base `--json` shape is byte-unchanged (no
    // `designScore`, no `freshness` key).
    let (findings, design_score, freshness) = validate_full(&g);
    let file_str = file.to_string_lossy();
    // `provenance` passes through verbatim (object or null) so gates read the
    // trust stamp without parsing the graph.
    let provenance = g.provenance().cloned().unwrap_or(serde_json::Value::Null);
    let mut report = ValidateReport::full(&file_str, findings, provenance, design_score, freshness);
    // Flows-count passthrough (schema 1.4, F2): omitted when zero so the
    // pre-1.4 `--json` byte shape is unchanged.
    report.flows = g.flows_count();

    if json {
        match to_canonical_json(&report) {
            Ok(bytes) => {
                if std::io::stdout().write_all(&bytes).is_err() {
                    return ExitCode::from(2);
                }
            }
            Err(e) => {
                eprintln!("maapp: {e}");
                return ExitCode::from(2);
            }
        }
    } else if report.findings.is_empty() {
        println!(
            "CLEAN — {file_str}: 0 errors, 0 warnings. ({} nodes, {} edges)",
            g.nodes.len(),
            g.edges.len()
        );
    } else {
        for f in &report.findings {
            println!("{}", f.render());
        }
        let verdict = if report.has_errors() { "FAIL" } else { "CLEAN" };
        let waived = if report.waived > 0 {
            format!(", {} waived", report.waived)
        } else {
            String::new()
        };
        println!(
            "\n{verdict} — {} error(s), {} warning(s){waived} ({} nodes, {} edges)",
            report.errors,
            report.warnings,
            g.nodes.len(),
            g.edges.len()
        );
    }

    if report.has_errors() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

// ---------------------------------------------------------------------------
// query — arg parsing
// ---------------------------------------------------------------------------

/// Parse and dispatch a `query` subcommand.
///
/// The oracle's argparse layout for `query`:
///   graph query <verb> [id] <file> [--depth N] [--terminals] [--json]
///
/// Special case: `orphans` takes no id, so its positional is the file.
/// With `trailing_var_arg`, clap gives us `verb` plus `rest` = all remaining
/// tokens; we parse them manually to stay oracle-faithful.
fn run_query(verb: &str, rest: &[String]) -> ExitCode {
    // Pull flags out of `rest` first, leaving positionals in order.
    let mut json_flag = false;
    let mut terminals_flag = false;
    let mut depth: usize = 1;
    let mut positionals: Vec<&str> = Vec::new();
    let mut rest_iter = rest.iter().peekable();
    while let Some(tok) = rest_iter.next() {
        match tok.as_str() {
            "--json" => json_flag = true,
            "--terminals" => terminals_flag = true,
            "--depth" => {
                if let Some(n) = rest_iter.next() {
                    match n.parse::<usize>() {
                        Ok(v) => depth = v,
                        Err(_) => {
                            eprintln!("maapp: --depth requires a positive integer, got {n:?}");
                            return ExitCode::from(2);
                        }
                    }
                } else {
                    eprintln!("maapp: --depth requires an argument");
                    return ExitCode::from(2);
                }
            }
            other => positionals.push(other),
        }
    }

    // For `orphans`: positional[0] = file (no id needed).
    // For all other verbs: positional[0] = id, positional[1] = file.
    // Two code paths avoid carrying Option<&str> past this point.
    if verb == "orphans" {
        let file_str = match positionals.as_slice() {
            [file] => *file,
            [] => {
                eprintln!("maapp: query orphans requires a <file> argument");
                return ExitCode::from(2);
            }
            _ => {
                eprintln!("maapp: query orphans takes no id argument");
                return ExitCode::from(2);
            }
        };
        let g = match load_graph_from_path(std::path::Path::new(file_str)) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("maapp: {e}");
                return ExitCode::from(2);
            }
        };
        let res = query::q_orphans(&g);
        return emit_json_or_text(json_flag, &res, || render_orphans(&res));
    }

    // All non-orphan verbs require an id.
    let (nid, file_str) = match positionals.as_slice() {
        [id, file] => (*id, *file),
        [_] | [] => {
            eprintln!("maapp: query {verb} requires <id> and <file> arguments");
            return ExitCode::from(2);
        }
        _ => {
            eprintln!("maapp: too many positional arguments for query {verb}");
            return ExitCode::from(2);
        }
    };

    let file_path = std::path::Path::new(file_str);
    let g = match load_graph_from_path(file_path) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("maapp: {e}");
            return ExitCode::from(2);
        }
    };

    match verb {
        "node" => match query::q_node(&g, nid) {
            None => {
                eprintln!("maapp: unknown node id: {nid}");
                ExitCode::from(2)
            }
            Some(res) => emit_json_or_text(json_flag, &res, || render_node(&res)),
        },
        "blast-radius" => match query::q_blast_radius(&g, nid) {
            None => {
                eprintln!("maapp: unknown node id: {nid}");
                ExitCode::from(2)
            }
            Some(res) => emit_json_or_text(json_flag, &res, || render_blast(&res)),
        },
        "trace" => {
            if terminals_flag {
                match query::q_trace_terminals(&g, nid) {
                    None => {
                        eprintln!("maapp: unknown node id: {nid}");
                        ExitCode::from(2)
                    }
                    Some(res) => {
                        if json_flag {
                            // --terminals --json: bare JSON array (not an object).
                            match to_canonical_json(&res.terminals) {
                                Ok(bytes) => {
                                    if std::io::stdout().write_all(&bytes).is_err() {
                                        return ExitCode::from(2);
                                    }
                                    ExitCode::SUCCESS
                                }
                                Err(e) => {
                                    eprintln!("maapp: {e}");
                                    ExitCode::from(2)
                                }
                            }
                        } else {
                            for t in &res.terminals {
                                println!("{t}");
                            }
                            ExitCode::SUCCESS
                        }
                    }
                }
            } else {
                match query::q_trace(&g, nid) {
                    None => {
                        eprintln!("maapp: unknown node id: {nid}");
                        ExitCode::from(2)
                    }
                    Some(res) => emit_json_or_text(json_flag, &res, || render_trace(&res)),
                }
            }
        }
        "neighbors" => match query::q_neighbors(&g, nid, depth) {
            None => {
                eprintln!("maapp: unknown node id: {nid}");
                ExitCode::from(2)
            }
            Some(res) => emit_json_or_text(json_flag, &res, || render_neighbors(&res)),
        },
        "view" => match query::q_view(&g, nid) {
            None => {
                eprintln!("maapp: unknown view '{nid}'; choose from assertion, dependency, nav");
                ExitCode::from(2)
            }
            Some(res) => emit_json_or_text(json_flag, &res, || render_view(&res)),
        },
        other => {
            eprintln!("maapp: unknown query verb: {other}");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------
// emit helpers
// ---------------------------------------------------------------------------

/// Emit JSON (compact + newline) or call the text renderer.
fn emit_json_or_text<T: serde::Serialize>(
    json: bool,
    val: &T,
    text_fn: impl FnOnce() -> String,
) -> ExitCode {
    if json {
        match to_canonical_json(val) {
            Ok(bytes) => {
                if std::io::stdout().write_all(&bytes).is_err() {
                    return ExitCode::from(2);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("maapp: {e}");
                ExitCode::from(2)
            }
        }
    } else {
        println!("{}", text_fn());
        ExitCode::SUCCESS
    }
}

// ---------------------------------------------------------------------------
// text renderers — mirrors oracle's `_render_*` functions
// ---------------------------------------------------------------------------

fn edge_line(e: &query::TraceEdge) -> String {
    let from = e.from.as_deref().unwrap_or("None");
    let etype = e.r#type.as_deref().unwrap_or("None");
    let to = e.to.as_deref().unwrap_or("None");
    let extras: Vec<String> = e
        .attrs
        .iter()
        .map(|(k, v)| {
            let vstr = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{k}={vstr}")
        })
        .collect();
    let attrs = if extras.is_empty() {
        String::new()
    } else {
        format!("{{{}}}", extras.join(", "))
    };
    format!("{from} --{etype}{attrs}--> {to}")
}

fn render_node(res: &query::NodeResult<'_>) -> String {
    let n = res.node;
    let kind = n.kind().unwrap_or("None");
    let layer = res.layer.unwrap_or("None");
    let mut lines = vec![
        format!("NODE {}  [{} / {}]", res.id, kind, layer),
        format!("  intent: {}", n.intent.as_deref().unwrap_or("")),
    ];
    if let Some(refs) = &n.refs
        && let Some(obj) = refs.as_object()
        && !obj.is_empty()
    {
        let kvs: Vec<String> = obj
            .iter()
            .map(|(k, v)| {
                let vstr = v.as_str().map_or_else(|| v.to_string(), str::to_string);
                format!("{k}={vstr}")
            })
            .collect();
        lines.push(format!("  refs: {}", kvs.join(", ")));
    }
    if let Some(attrs) = &n.attrs
        && let Some(obj) = attrs.as_object()
        && !obj.is_empty()
    {
        let kvs: Vec<String> = obj
            .iter()
            .map(|(k, v)| {
                let vstr = v.as_str().map_or_else(|| v.to_string(), str::to_string);
                format!("{k}={vstr}")
            })
            .collect();
        lines.push(format!("  attrs: {}", kvs.join(", ")));
    }
    lines.push(format!("  OUT ({}):", res.out_edges.len()));
    for e in &res.out_edges {
        let te = query::TraceEdge::from(*e);
        lines.push(format!("    {}", edge_line(&te)));
    }
    lines.push(format!("  IN ({}):", res.in_edges.len()));
    for e in &res.in_edges {
        let te = query::TraceEdge::from(*e);
        lines.push(format!("    {}", edge_line(&te)));
    }
    lines.join("\n")
}

fn render_orphans(res: &query::OrphansResult) -> String {
    if res.orphans.is_empty() {
        "ORPHANS: none".to_string()
    } else {
        let body: Vec<String> = res.orphans.iter().map(|o| format!("  {o}")).collect();
        format!("ORPHANS (no incident edges):\n{}", body.join("\n"))
    }
}

fn render_blast(res: &query::BlastRadiusResult) -> String {
    let mut lines = vec![
        format!("BLAST-RADIUS {}", res.id),
        format!(
            "  DEPENDENTS (depend ON it; break if it changes) [{}]:",
            res.dependents.len()
        ),
    ];
    for d in &res.dependents {
        lines.push(format!("    {d}"));
    }
    lines.push(format!(
        "  DEPENDENCIES (it depends on) [{}]:",
        res.dependencies.len()
    ));
    for d in &res.dependencies {
        lines.push(format!("    {d}"));
    }
    lines.join("\n")
}

fn render_trace(res: &query::TraceResult) -> String {
    let mut lines = vec![format!(
        "TRACE from {}  ({} path(s) to terminal):",
        res.id,
        res.paths.len()
    )];
    for (i, path) in res.paths.iter().enumerate() {
        let mut chain = res.id.clone();
        for e in path {
            let extras: Vec<String> = e
                .attrs
                .iter()
                .map(|(k, v)| {
                    let vstr = v.as_str().map_or_else(|| v.to_string(), str::to_string);
                    format!("{k}={vstr}")
                })
                .collect();
            let tail = if extras.is_empty() {
                String::new()
            } else {
                format!("{{{}}}", extras.join(","))
            };
            let etype = e.r#type.as_deref().unwrap_or("None");
            let to = e.to.as_deref().unwrap_or("None");
            chain += &format!("\n      --{etype}{tail}--> {to}");
        }
        lines.push(format!("  [{}] {}", i + 1, chain));
    }
    lines.join("\n")
}

fn render_neighbors(res: &query::NeighborsResult) -> String {
    let mut lines = vec![format!(
        "NEIGHBORS of {} (depth {}): {} node(s)",
        res.root,
        res.depth,
        res.nodes.len()
    )];
    // Oracle sorts nodes by key (BTreeMap iteration is sorted).
    for nid in res.nodes.keys() {
        lines.push(format!("  {nid}"));
    }
    lines.push(format!("  edges ({}):", res.edges.len()));
    for e in &res.edges {
        lines.push(format!("    {}", edge_line(e)));
    }
    lines.join("\n")
}

fn render_view(res: &query::ViewResult) -> String {
    let mut lines = vec![format!(
        "VIEW '{}': {} edge(s), {} node(s)",
        res.view,
        res.edges.len(),
        res.nodes.len()
    )];
    // Oracle sorts edges by (from, type, to).
    let mut sorted = res.edges.clone();
    sorted.sort_by(|a, b| {
        let af = a.from.as_deref().unwrap_or("");
        let at = a.r#type.as_deref().unwrap_or("");
        let ato = a.to.as_deref().unwrap_or("");
        let bf = b.from.as_deref().unwrap_or("");
        let bt = b.r#type.as_deref().unwrap_or("");
        let bto = b.to.as_deref().unwrap_or("");
        (af, at, ato).cmp(&(bf, bt, bto))
    });
    for e in &sorted {
        lines.push(format!("  {}", edge_line(e)));
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------

/// Parse and dispatch a `render` subcommand.
///
/// Usage: maapp render <verb> [--ascii] [--out <path>] <file>
/// Verbs: hub | deps | storyboard | spine | html
/// --ascii: accepted for storyboard/spine (implied); ignored for others.
/// --out <path>: REQUIRED for html (write file); ignored for others.
fn run_render(verb: &str, rest: &[String]) -> ExitCode {
    // Parse flags and positionals.
    let mut out_path: Option<&str> = None;
    let mut positionals: Vec<&str> = Vec::new();
    let mut iter = rest.iter().peekable();
    while let Some(tok) = iter.next() {
        match tok.as_str() {
            "--ascii" | "--ascii-off" => {}
            "--out" => {
                if let Some(p) = iter.next() {
                    out_path = Some(p.as_str());
                } else {
                    eprintln!("maapp: --out requires a path argument");
                    return ExitCode::from(2);
                }
            }
            other => positionals.push(other),
        }
    }

    let file_str = match positionals.first() {
        Some(f) => *f,
        None => {
            eprintln!("maapp: render {verb} requires a <file> argument");
            return ExitCode::from(2);
        }
    };

    // html requires --out (oracle: `--out OUT (REQUIRED for html)`).
    if verb == "html" && out_path.is_none() {
        eprintln!(
            "maapp: render html requires --out <path> (write the self-contained HTML to this path)"
        );
        return ExitCode::from(2);
    }

    let g = match load_graph_from_path(std::path::Path::new(file_str)) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("maapp: {e}");
            return ExitCode::from(2);
        }
    };

    let output = match verb {
        "hub" => render::render_hub(&g),
        "deps" => render::render_deps(&g),
        "storyboard" => render::render_storyboard_ascii(&g),
        "spine" => render::render_spine_ascii(&g),
        "html" => render::render_html(&g),
        other => {
            eprintln!(
                "maapp: unknown render verb: {other}; choose from hub, deps, storyboard, spine, html"
            );
            return ExitCode::from(2);
        }
    };

    if let Some(path) = out_path {
        // Write to file (used for html).
        if let Err(e) = std::fs::write(path, &output) {
            eprintln!("maapp: failed to write {path}: {e}");
            return ExitCode::from(2);
        }
    } else {
        println!("{output}");
    }
    ExitCode::SUCCESS
}
