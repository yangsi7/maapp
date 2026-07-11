//! maapp engine core — the portable app-structure substrate.
//!
//! All engine logic lives in the library; `src/main.rs` is a thin clap shell.
//! The core (parse, model, validate, canonical serialize) is free of
//! `std::fs`/net/time so it stays wasm-clean. Native-only file loading is gated
//! behind a `cfg(not(wasm32))` boundary.
//!
//! Slice 1 surface: load a graph, validate it (faithful port of the Python
//! oracle), and emit byte-stable canonical JSON. Query/render verbs land in
//! later slices.

pub mod design;
pub mod diff;
/// Native-only (std::fs + std::process::Command git): THE ONLY disk-toucher
/// in the verb set (LIFECYCLE-VERBS-SPEC §3). cfg-gated so the core library
/// stays wasm-clean.
#[cfg(not(target_arch = "wasm32"))]
pub mod drift;
pub mod error;
pub mod export;
pub mod freshness;
pub mod graph;
/// Distribution wiring (`maapp init`): std::fs by nature, so CLI-only AND
/// native-only (the `cli` feature is in the default set the wasm gate builds
/// with, so the extra wasm cfg keeps the core wasm-clean like `drift`/`mutate`).
#[cfg(all(feature = "cli", not(target_arch = "wasm32")))]
pub mod init;
pub mod model;
/// Native-only (std::fs temp+rename writes): the mutation verbs rewrite the
/// named graph file on disk (LIFECYCLE-VERBS-SPEC §4). cfg-gated like `drift`;
/// the pure canonical projection they emit through lives wasm-clean in
/// `export`.
#[cfg(not(target_arch = "wasm32"))]
pub mod mutate;
pub mod query;
pub mod render;
pub mod schema;
pub mod validate;

pub use design::{DesignScore, run_design_lints};
pub use diff::{DiffReport, diff_graphs};
#[cfg(not(target_arch = "wasm32"))]
pub use drift::{DriftReport, check_drift, stamp};
pub use error::EngineError;
pub use export::{SliceSelector, canonical_doc, export_slice};
pub use freshness::{FreshnessScore, run_freshness};
pub use graph::Graph;
#[cfg(not(target_arch = "wasm32"))]
pub use mutate::{MutationOutcome, parse_kv};
pub use schema::{schema_bytes, schema_value};
pub use validate::{Finding, apply_waivers, validate};

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Parse a graph document from raw JSON bytes (wasm-clean: no filesystem).
///
/// A non-object top-level value or a malformed `edges`/edge element is a load
/// error, matching the oracle's `load_graph` + `Graph.__init__` exit-2 behavior.
pub fn load_graph_from_slice(bytes: &[u8]) -> Result<Graph, EngineError> {
    let doc: Value = serde_json::from_slice(bytes)?;
    if !doc.is_object() {
        return Err(EngineError::NotAnObject(json_type_name(&doc)));
    }
    Graph::from_doc(doc)
}

/// Load a graph from a file path (native-only convenience).
#[cfg(not(target_arch = "wasm32"))]
pub fn load_graph_from_path(path: &std::path::Path) -> Result<Graph, EngineError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(EngineError::FileNotFound(path.to_path_buf()));
        }
        Err(e) => return Err(EngineError::Io(e)),
    };
    let doc: Value =
        serde_json::from_slice(&bytes).map_err(|source| EngineError::MalformedJson {
            path: path.to_path_buf(),
            source,
        })?;
    if !doc.is_object() {
        return Err(EngineError::NotAnObject(json_type_name(&doc)));
    }
    Graph::from_doc(doc)
}

/// The `validate --json` report shape (ENGINE-CONTRACT §4).
///
/// `errors`/`warnings` are COUNTS; `findings` is the full ordered list.
/// `designScore` is the deterministic `{satisfied, applicable}` integer pair
/// (DESIGN-COMPLETENESS §4.2), present ONLY when the design-completeness lint
/// pack is enabled (absent otherwise, so the base 1.3 `--json` shape is
/// byte-unchanged). `freshness` is the Slice-B per-type `{fresh, reviewed}` map
/// (FRESHNESS-SUBSTRATE §4), emitted BESIDE `designScore`, NEVER merged into it
/// (so stale-vs-incomplete are not double-counted), present only when the lint
/// pack is enabled.
#[derive(Debug, Serialize)]
pub struct ValidateReport<'a> {
    pub file: &'a str,
    pub clean: bool,
    pub errors: usize,
    pub warnings: usize,
    /// Advisories waived via `meta.waivers` (RES-004 friction #9): counted
    /// OUT of `warnings`, visible here, never exit-code-relevant. Omitted
    /// when zero so the pre-waiver `--json` byte shape is unchanged.
    #[serde(skip_serializing_if = "is_zero")]
    pub waived: usize,
    /// Declared `meta.flows` count (schema 1.4, F2) — a passthrough so
    /// consumers/gates see the journey inventory without parsing the graph.
    /// Omitted when zero (same convention as `waived`): the pre-1.4 `--json`
    /// byte shape is unchanged.
    #[serde(skip_serializing_if = "is_zero")]
    pub flows: usize,
    pub findings: Vec<Finding>,
    /// `meta.provenance` passthrough (the authored object, or `null` when the
    /// graph carries none) so consumers/gates read the trust stamp without
    /// parsing the graph. ALWAYS present — part of the byte-stable `--json`
    /// shape since 1.5 (ROADMAP v2 move 2).
    pub provenance: Value,
    #[serde(rename = "designScore", skip_serializing_if = "Option::is_none")]
    pub design_score: Option<DesignScore>,
    /// `freshness?: {<type>:{fresh, reviewed}}` — standalone per-type integer pairs,
    /// never merged into `designScore` (FRESHNESS-SUBSTRATE.md §4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness: Option<BTreeMap<String, FreshnessScore>>,
}

impl<'a> ValidateReport<'a> {
    /// Build a base report from findings, splitting hard vs soft for the counts.
    /// `provenance` is `null`; `designScore` + `freshness` are absent.
    pub fn new(file: &'a str, findings: Vec<Finding>) -> Self {
        Self::full(file, findings, Value::Null, None, None)
    }

    /// Build a report with an optional `designScore` (provenance null, freshness
    /// absent). Retained for the design-only call sites; the lint-pack always
    /// co-emits both, so this is the partial constructor used by Slice-A tests.
    pub fn with_design(
        file: &'a str,
        findings: Vec<Finding>,
        design_score: Option<DesignScore>,
    ) -> Self {
        Self::full(file, findings, Value::Null, design_score, None)
    }

    /// Build a report with the `meta.provenance` passthrough (pass `Value::Null`
    /// when the graph carries none) plus an optional `designScore` AND optional
    /// `freshness` map. When either option is `Some`, the corresponding findings
    /// MUST already be appended to `findings` so the `warnings` count and the
    /// findings list agree.
    pub fn full(
        file: &'a str,
        findings: Vec<Finding>,
        provenance: Value,
        design_score: Option<DesignScore>,
        freshness: Option<BTreeMap<String, FreshnessScore>>,
    ) -> Self {
        let errors = findings.iter().filter(|f| f.hard()).count();
        let waived = findings.iter().filter(|f| f.waived()).count();
        let warnings = findings.len() - errors - waived;
        ValidateReport {
            file,
            clean: errors == 0,
            errors,
            warnings,
            waived,
            flows: 0,
            findings,
            provenance,
            design_score,
            freshness,
        }
    }

    /// True iff there is at least one hard-fail finding (drives exit code 1).
    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }
}

/// Run the full validator AND, when the `design-completeness` lint pack is opted in
/// via `meta.lints`, the design lint pack PLUS the freshness substrate. Returns the
/// (possibly design+freshness-augmented) findings list, the `designScore` (None when
/// the pack is off), and the per-type freshness map (None when off).
///
/// This is the single place the gate is applied: `validate(&g)` always runs;
/// `run_design_lints(&g)` + `run_freshness(&g)` run ONLY when
/// `g.lint_enabled("design-completeness")`. Order of appended advisory findings:
/// core (oracle-faithful) → design pack (D1-D5) → freshness (`W_*_STALE`, D6). All
/// additive; the design pack's order is preserved and the freshness findings follow.
pub fn validate_full(
    g: &Graph,
) -> (
    Vec<Finding>,
    Option<DesignScore>,
    Option<BTreeMap<String, FreshnessScore>>,
) {
    let mut findings = validate(g);
    if g.lint_enabled(design::LINT_NAME) {
        let (design_findings, score) = run_design_lints(g);
        findings.extend(design_findings);
        let (stale_findings, freshness) = run_freshness(g);
        findings.extend(stale_findings);
        // Re-apply waivers over the appended lint-pack advisories (idempotent
        // for the core findings validate() already waived).
        validate::apply_waivers(g, &mut findings);
        (findings, Some(score), Some(freshness))
    } else {
        (findings, None, None)
    }
}

/// `skip_serializing_if` helper for the `waived` count.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde requires the &T signature
fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Serialize any value to byte-stable canonical JSON: compact (no whitespace),
/// with one trailing newline. This is the maapp canonical form — deterministic
/// and byte-identical across runs (the `--json` contract).
///
/// Writes to a `Vec<u8>` via `serde_json::to_writer` (no intermediate `String`).
pub fn to_canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, EngineError> {
    let mut buf = Vec::new();
    serde_json::to_writer(&mut buf, value)?;
    buf.push(b'\n');
    Ok(buf)
}

/// A stable canonical view of a whole graph document, used for the parse →
/// canonical-serialize byte-stability test.
///
/// Edges are sorted by the total key `(from, to, type)` and re-serialized; nodes
/// emit through the `BTreeMap<Slug, Node>` index (slug-sorted), each tagged with
/// its layer. This is NOT the wire format the oracle reads back (layers are
/// flattened); it is a deterministic canonical projection whose only contract is
/// byte-stability across runs.
#[derive(Debug, Serialize)]
pub struct CanonicalGraph {
    pub schema: Option<Value>,
    pub version: Option<Value>,
    pub nodes: std::collections::BTreeMap<String, CanonicalNode>,
    pub edges: Vec<CanonicalEdge>,
}

/// A node in canonical projection: its layer plus the original node body.
#[derive(Debug, Serialize)]
pub struct CanonicalNode {
    pub layer: String,
    #[serde(flatten)]
    pub node: model::Node,
}

/// An edge in canonical projection: the structural triple plus key-ordered attrs.
#[derive(Debug, Serialize)]
pub struct CanonicalEdge {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(flatten)]
    pub attrs: std::collections::BTreeMap<String, Value>,
}

impl CanonicalGraph {
    /// Project a loaded graph into its deterministic canonical form.
    pub fn from_graph(g: &Graph) -> Self {
        let nodes: std::collections::BTreeMap<String, CanonicalNode> = g
            .nodes
            .iter()
            .map(|(slug, node)| {
                (
                    slug.clone(),
                    CanonicalNode {
                        layer: g.node_layer.get(slug).cloned().unwrap_or_default(),
                        node: node.clone(),
                    },
                )
            })
            .collect();

        let mut edges: Vec<CanonicalEdge> = g
            .edges
            .iter()
            .map(|e| CanonicalEdge {
                r#type: e.r#type.clone(),
                from: e.from.clone(),
                to: e.to.clone(),
                attrs: e.attrs.clone(),
            })
            .collect();
        // Total order: (from, to, type). None sorts before Some.
        edges.sort_by(|a, b| (&a.from, &a.to, &a.r#type).cmp(&(&b.from, &b.to, &b.r#type)));

        CanonicalGraph {
            schema: g.doc.get("schema").cloned(),
            version: g.doc.get("version").cloned(),
            nodes,
            edges,
        }
    }
}

/// Python-style JSON type name for top-level load errors.
fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "float",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}
