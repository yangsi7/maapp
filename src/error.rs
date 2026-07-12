//! The single typed error for the maapp engine library.
//!
//! Per `rust-conventions`: one `thiserror` enum, one variant per failure mode,
//! satisfying C-GOOD-ERR (`Error + Send + Sync`, lowercase Display). `anyhow`
//! never leaks here so downstream + wasm + eval can `match` on the variant.

use std::path::PathBuf;

/// Every way the engine can fail to load or process a graph.
///
/// Display strings are lowercase and context-rich; the CLI binary wraps them
/// with `anyhow` for exit messaging but the variants stay matchable.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The graph file could not be read from disk (CLI/native path only).
    #[error("file not found: {0}")]
    FileNotFound(PathBuf),

    /// The file was read but its bytes were not valid JSON.
    #[error("malformed json in {path}: {source}")]
    MalformedJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// The top-level JSON value was not an object.
    #[error("top-level json must be an object, got {0}")]
    NotAnObject(&'static str),

    /// `edges` was present but not a JSON array.
    #[error("edges must be an array, got {0}")]
    EdgesNotArray(&'static str),

    /// An element of `edges` was not a JSON object.
    #[error("edges[{index}] must be an object, got {got}")]
    EdgeNotObject { index: usize, got: &'static str },

    /// Two edges in one document share the same `(type, from, to)` identity.
    /// `diff` must detect this, not silently merge (LIFECYCLE-VERBS-SPEC §1).
    /// `role` names which input carried the duplicate (`"old"` / `"new"`).
    #[error("duplicate edge identity in {role} graph: {eid}")]
    DuplicateEdgeIdentity { role: &'static str, eid: String },

    /// `check-drift` has no base rev: the graph carries no
    /// `meta.provenance.asOf` and no `--since` was given (LIFECYCLE-VERBS-SPEC
    /// §3: missing provenance without `--since` = exit 2, pointed message).
    #[error(
        "graph carries no meta.provenance.asOf and no --since was given; stamp the graph first (maapp stamp <graph> --repo <repo>) or pass --since <rev>"
    )]
    NoDriftBase,

    /// A read-only git operation failed (not a git repo, unknown rev, git
    /// missing, …). `op` names the operation; `detail` carries git's stderr.
    #[error("git {op} failed in {repo}: {detail}")]
    GitOp {
        op: &'static str,
        repo: PathBuf,
        detail: String,
    },

    /// `stamp` requires an existing `meta.provenance` object to bump: `origin`
    /// is required authored data the engine must never invent (determinism
    /// rule; provenance is a closed shape with `origin` REQUIRED).
    #[error(
        "graph carries no meta.provenance object; author {{\"origin\": ...}} first — stamp only bumps asOf"
    )]
    NoProvenanceToStamp,

    /// A mutation named a node id the graph does not contain, or a slice
    /// selector named an unknown root (matches the query verbs' wording).
    #[error("unknown node id: {0}")]
    NodeNotFound(String),

    /// `add-node` named a slug that already exists (mutations must be
    /// well-defined: use `update-node` to change an existing node).
    #[error("node '{0}' already exists; use update-node to change it")]
    NodeExists(String),

    /// `add-node` was given a kind that is neither frozen-core nor declared in
    /// the graph's `nodeKindRegistry` — the layer cannot be derived (spec §4:
    /// layer placement is derived from kind, the agent never names a layer).
    #[error(
        "unknown kind '{0}': cannot derive a layer (not frozen-core and not in nodeKindRegistry)"
    )]
    UnknownKindNoLayer(String),

    /// `add-edge` would duplicate an existing `(type, from, to)` identity
    /// (LIFECYCLE-VERBS-SPEC §1: duplicate edge identities are an input error).
    #[error("edge already exists: {0}")]
    EdgeExists(String),

    /// `remove-edge` matched no edge by `(type, from, to)` identity.
    #[error("no edge matches identity: {0}")]
    EdgeNotFound(String),

    /// `update-node` with no `--intent`/`--ref`/`--attr` flags (spec §4:
    /// a no-op is an error, not a silent success).
    #[error("update-node {0}: no changes given (pass --intent, --ref and/or --attr)")]
    NoOpUpdate(String),

    /// A batch `--where key=value` selector (T3b) matched no node — an empty
    /// batch is a usage error, never a silent no-op write.
    #[error("no nodes match --where {key}={value}")]
    WhereMatchesNothing { key: String, value: String },

    /// `migrate` on a graph whose `version` is absent or not a MAJOR.MINOR
    /// string — there is no source schema to upgrade FROM (T5).
    #[error(
        "graph has no parseable \"version\" (MAJOR.MINOR) to migrate from; add one, e.g. \"version\": \"{0}\""
    )]
    MigrateNoVersion(String),

    /// `migrate` cannot cross a major boundary (only mechanical minor upgrades
    /// within a known major are additive/safe, T5).
    #[error(
        "cannot migrate across schema majors: graph is major {from_major}, this engine supports major {engine_major}"
    )]
    MigrateAcrossMajor { from_major: u64, engine_major: u64 },

    /// `migrate --to` named a minor this engine does not know (a future schema
    /// the engine cannot mechanically produce, T5).
    #[error(
        "cannot migrate to '{to}': unknown to this engine (supports {engine_major}.0–{engine_major}.{engine_minor_max}); upgrade maapp"
    )]
    MigrateUnknownTarget {
        to: String,
        engine_major: u64,
        engine_minor_max: u64,
    },

    /// `migrate --to <minor>` behind the graph's current minor — `migrate` is
    /// an upgrade verb, it never downgrades (T5).
    #[error(
        "cannot migrate: target {to} is behind the graph's current {from} (migrate never downgrades)"
    )]
    MigrateDowngrade { from: String, to: String },

    /// `migrate --to` value was not a MINOR (`4`) or MAJOR.MINOR (`1.4`) string.
    #[error("--to expects a minor ('4') or MAJOR.MINOR ('1.4') version, got '{0}'")]
    MigrateBadTarget(String),

    /// `remove-node` refused: the node has incident edges and `--cascade` was
    /// not given (spec §4). `eids` lists them in document order.
    #[error(
        "node '{node}' has {} incident edge(s): {}; re-run with --cascade to also remove them",
        eids.len(),
        eids.join(", ")
    )]
    RemoveNodeHasEdges { node: String, eids: Vec<String> },

    /// The mutated document would carry MORE hard validation errors than the
    /// input did — the atomic validated write refuses (spec §4). `findings`
    /// carries the would-be `E_*` findings for the CLI to print.
    #[error("mutation would raise hard validation errors {before} -> {after}; refusing to write")]
    ValidationRegression {
        before: usize,
        after: usize,
        findings: Vec<crate::validate::Finding>,
    },

    /// A `--ref`/`--attr` flag value was not in `K=V` form.
    #[error("expected K=V form, got '{0}'")]
    BadKeyValue(String),

    /// A structural document slot (e.g. `nodes`, a layer map) held a
    /// non-object where the mutation needed one.
    #[error("malformed document: {0}")]
    MalformedDocument(&'static str),

    /// `export --slice scope:<s>` matched no node (an empty slice is useless).
    #[error("no nodes match scope '{0}'")]
    ScopeMatchesNothing(String),

    /// `export --slice-tag <tag>` matched no node carrying that `refs.slice`
    /// tag (an empty slice is useless — the pilot app hit this planning
    /// `export --slice scope:S0` before slice-tag export existed, T6).
    #[error("no nodes carry refs.slice '{0}'")]
    SliceTagMatchesNothing(String),

    /// `init` could not merge the hooks block into an existing
    /// `.claude/settings.json` (invalid JSON, or a `hooks`/`PostToolUse` slot
    /// with the wrong shape). The file is left untouched: maapp never rewrites
    /// a settings file it cannot parse.
    #[error(
        "cannot merge hooks into {}: {detail}; fix the file by hand (maapp never rewrites a settings file it cannot parse)",
        path.display()
    )]
    SettingsMerge { path: PathBuf, detail: String },

    /// `init` could not patch the routing target (CLAUDE.md / AGENTS.md):
    /// unbalanced maapp markers or non-UTF-8 content.
    #[error("cannot patch {}: {detail}", path.display())]
    PatchTarget { path: PathBuf, detail: String },

    /// A read-from-bytes path failed to decode (wasm-clean entry point).
    #[error("could not parse graph: {0}")]
    Parse(#[from] serde_json::Error),

    /// An I/O failure not covered by a more specific variant (native only).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
