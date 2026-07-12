//! The validator: a faithful Rust port of the oracle's `validate()` + helpers.
//!
//! Fidelity is the contract here. Every finding's `code`, `ids`, `message`, and
//! `fix` match the oracle byte-for-byte (the mutation battery checks exact codes;
//! the ground-truth suite checks shapes). Finding ORDER also matches the oracle's
//! pass order: dup-ids, unknown-kind/layer, refs, per-edge passes (in document
//! order), pipeline DAG, dismisses, navigates branch sets, returnsTo branch sets,
//! orphans, slice-coverage.

use crate::graph::{
    Graph, core_edges, core_node_kinds, provenance_keys, provenance_origins, refs_keys,
    trigger_causes,
};
use crate::model::Edge;
use crate::query::{DEP_TYPES, TRACE_TYPES};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// One validator finding. `code` starting with `E_` is a hard-fail; `W_`
/// advisory. A `meta.waivers` entry matching an advisory turns its severity
/// to `"waived"` (visible, never silent, never exit-code-relevant).
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub code: String,
    pub severity: &'static str,
    pub ids: Vec<String>,
    pub message: String,
    pub fix: String,
    /// The waiver's `reason`, present ONLY on waived findings (the audit
    /// trail rides with the finding; absent = key omitted, byte-compat).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waived_reason: Option<String>,
}

impl Finding {
    fn new(code: &str, ids: Vec<String>, message: String, fix: String) -> Self {
        let severity = if code.starts_with("E_") {
            "error"
        } else {
            "warning"
        };
        Finding {
            code: code.to_string(),
            severity,
            ids,
            message,
            fix,
            waived_reason: None,
        }
    }

    /// Public constructor for findings produced outside the core validator pass
    /// (the design-completeness lint pack in `crate::design`). Severity is derived
    /// from the code prefix exactly as the core findings (`E_` ⇒ error, else
    /// warning), so a `W_DESIGN_*` code lands as `severity:"warning"` ⇒ exit 0.
    pub fn design(code: &str, ids: Vec<String>, message: String, fix: String) -> Self {
        Finding::new(code, ids, message, fix)
    }

    /// True for hard-fail (`E_*`) findings.
    pub fn hard(&self) -> bool {
        self.code.starts_with("E_")
    }

    /// True for findings waived via `meta.waivers`.
    pub fn waived(&self) -> bool {
        self.severity == "waived"
    }

    /// Human-readable single-finding render, matching the oracle's `Finding.render`.
    /// A waived finding renders `[WAIVE]` plus its reason line.
    pub fn render(&self) -> String {
        let sev = if self.hard() {
            "ERROR"
        } else if self.waived() {
            "WAIVE"
        } else {
            "WARN "
        };
        let idstr = if self.ids.is_empty() {
            "-".to_string()
        } else {
            self.ids.join(", ")
        };
        let mut line = format!("[{sev}] {}  ({idstr})\n        {}", self.code, self.message);
        if let Some(reason) = &self.waived_reason {
            line.push_str(&format!("\n        WAIVED: {reason}"));
        }
        if !self.fix.is_empty() {
            line.push_str(&format!("\n        FIX: {}", self.fix));
        }
        line
    }
}

/// Run the full validator over a loaded graph, returning findings in oracle order.
pub fn validate(g: &Graph) -> Vec<Finding> {
    let mut f: Vec<Finding> = Vec::new();

    // ---- schema/version gate (A4) — read FIRST, before any node/edge pass ----
    check_schema_header(g, &mut f);

    // ---- meta.provenance shape (trust stamp) — header-level, absent = legal ----
    check_provenance(g, &mut f);

    // ---- meta.slice_of shape (export --slice annotation) — absent = legal ----
    check_slice_of(g, &mut f);

    // ---- meta.waivers shape (warning baseline) — absent = legal ----------
    check_meta_waivers(g, &mut f);

    // ---- meta.flows shape + reachability (schema 1.4, F2) — absent = legal ----
    check_meta_flows(g, &mut f);

    // ---- duplicate node ids (across layers) ------------------------------
    for (nid, layers) in &g.dup_ids {
        f.push(Finding::new(
            "E_DUP_ID",
            vec![nid.clone()],
            format!(
                "node id '{nid}' appears in more than one layer ({} and {}).",
                layers[0], layers[1]
            ),
            "Each node id must be unique across all nodes.<layer> sections. Rename or remove the duplicate.".to_string(),
        ));
    }

    // ---- unknown node kind (closed-vocabulary) ---------------------------
    // Iterate in DOCUMENT order (`node_order`) so the finding order matches the
    // oracle's `for nid, n in g.nodes.items()` (Python dict insertion order).
    for nid in &g.node_order {
        let n = &g.nodes[nid];
        match n.kind() {
            None => {
                f.push(Finding::new(
                    "E_NODE_NO_KIND",
                    vec![nid.clone()],
                    format!("node '{nid}' has no 'kind'."),
                    "Every node needs a 'kind' from the frozen core or a registered x-<ns> kind."
                        .to_string(),
                ));
            }
            Some(k) if !g.known_node_kind(k) => {
                // Insertion order (oracle's `list(CORE_NODE_KINDS) + list(registry)`)
                // so `_closest` tie-breaks identically to the Python dict iteration.
                let candidates: Vec<String> = CORE_NODE_KIND_ORDER
                    .iter()
                    .map(|s| (*s).to_string())
                    .chain(g.node_registry.keys().cloned())
                    .collect();
                let sugg = closest(k, &candidates);
                let mut fix = "Use a frozen-core kind or register x-<ns>:Name in nodeKindRegistry."
                    .to_string();
                if let Some(s) = sugg {
                    fix.push_str(&format!(" Closest known: {s}."));
                }
                f.push(Finding::new(
                    "E_UNKNOWN_KIND",
                    vec![nid.clone()],
                    format!("node '{nid}' has unknown kind '{k}'."),
                    fix,
                ));
            }
            Some(k) => {
                // layer/kind consistency
                let exp_layer = core_node_kinds().get(k).map(|s| s.to_string()).or_else(|| {
                    g.node_registry
                        .get(k)
                        .and_then(|r| r.get("layer"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
                let act_layer = g.node_layer.get(nid).cloned();
                if let (Some(exp), Some(act)) = (exp_layer, act_layer)
                    && !exp.is_empty()
                    && !act.is_empty()
                    && exp != act
                {
                    f.push(Finding::new(
                        "E_LAYER_MISMATCH",
                        vec![nid.clone()],
                        format!(
                            "node '{nid}' kind '{k}' belongs in layer '{exp}' but is filed under '{act}'."
                        ),
                        format!("Move '{nid}' into nodes.{exp}."),
                    ));
                }
            }
        }
    }

    // ---- malformed refs --------------------------------------------------
    for nid in &g.node_order {
        let n = &g.nodes[nid];
        match &n.refs {
            None => {
                f.push(Finding::new(
                    "W_REFS_ABSENT",
                    vec![nid.clone()],
                    format!("node '{nid}' has no 'refs' block."),
                    "Every node should carry refs:{} (uniform shape) even when empty.".to_string(),
                ));
            }
            Some(refs) if !refs.is_object() => {
                f.push(Finding::new(
                    "E_REFS_MALFORMED",
                    vec![nid.clone()],
                    format!(
                        "node '{nid}' refs must be an object, got {}.",
                        py_type(refs)
                    ),
                    "refs is a closed-key string-valued object; use {} when empty.".to_string(),
                ));
            }
            Some(Value::Object(obj)) => {
                // Object: check each key/value. serde_json::Map preserves insertion
                // order; the oracle iterates dict items in insertion order too.
                for (rk, rv) in obj {
                    // Open-with-registry (A3): a key is legal iff core OR declared
                    // in the graph's `refsRegistry` (declarative data overlay,
                    // D-003). Core keys keep the string + form checks; a
                    // registry-declared key's VALUE shape is app-owned (mirrors
                    // edgeRegistry rows declaring attrs the engine does not
                    // force — upstream's refs.swift is object-valued).
                    if rk == "source" {
                        // Core source anchor: one string or an array of strings
                        // (multi-anchor); each is FORM-checked only — existence
                        // on disk is the future check-drift verb's job, so
                        // validate stays repo-independent + deterministic.
                        check_source_ref(nid, rv, &mut f);
                    } else if refs_keys().contains(&rk.as_str()) {
                        if let Value::String(s) = rv {
                            check_ref_form(nid, rk, s, &mut f);
                        } else {
                            f.push(Finding::new(
                                "E_REFS_MALFORMED",
                                vec![nid.clone()],
                                format!(
                                    "node '{nid}' refs.{rk} must be a string, got {}.",
                                    py_type(rv)
                                ),
                                "All refs values are strings (doc anchors / file refs / ids)."
                                    .to_string(),
                            ));
                        }
                    } else if !g.refs_registry.contains_key(rk.as_str()) {
                        let mut sorted: Vec<String> =
                            refs_keys().iter().map(|s| s.to_string()).collect();
                        sorted.sort_unstable();
                        f.push(Finding::new(
                            "E_REFS_KEY",
                            vec![nid.clone()],
                            format!("node '{nid}' refs has unknown key '{rk}'."),
                            format!(
                                "Declare '{rk}' in refsRegistry or use a core key: {}.",
                                py_str_list(&sorted)
                            ),
                        ));
                    }
                }
            }
            // `refs` present and an object handled above; the `!is_object()` arm
            // catches non-objects, so this is unreachable but keeps the match total.
            Some(_) => {}
        }
    }

    // ---- Trigger.attrs.cause enum (schema 1.4, F1: root triggers) --------
    check_trigger_causes(g, &mut f);

    // ---- per-edge passes (document order) --------------------------------
    for (i, e) in g.edges.iter().enumerate() {
        per_edge(g, i, e, &mut f);
    }

    // ---- pipeline DAG acyclicity (x-pipeline:feeds) ----------------------
    check_pipeline_dag(g, &mut f);

    // ---- dismisses must have a matching forward navigates ----------------
    check_dismisses(g, &mut f);

    // ---- XOR branch sets: navigates.when (Assertion-keyed) ---------------
    check_navigates_branch_sets(g, &mut f);

    // ---- XOR branch sets: returnsTo.when (outcome-keyed) -----------------
    check_returnsto_branch_sets(g, &mut f);

    // ---- ADVISORY: orphan nodes (no incident edge); Policy excluded ------
    // Suppressed wholesale in slice mode (meta.slice_of present): slicing
    // creates boundary orphans by construction (LIFECYCLE-VERBS-SPEC §5).
    if g.slice_of().is_none() {
        for nid in &g.node_order {
            if g.kind(nid) == Some("Policy") {
                continue;
            }
            let has_out = g.out_edges.get(nid).is_some_and(|v| !v.is_empty());
            let has_in = g.in_edges.get(nid).is_some_and(|v| !v.is_empty());
            if !has_out && !has_in {
                f.push(Finding::new(
                    "W_ORPHAN",
                    vec![nid.clone()],
                    format!("node '{nid}' has no incident edges (orphan)."),
                    "Wire it into the graph or confirm it is intentionally standalone (suppressible).".to_string(),
                ));
            }
        }
    }

    // ---- ADVISORY: Screen/Component/PipelineStage missing refs.slice (OPT-IN) ----
    if g.lint_enabled("slice-coverage") {
        for nid in &g.node_order {
            let n = &g.nodes[nid];
            let k = n.kind();
            if matches!(
                k,
                Some("Screen") | Some("Component") | Some("PipelineStage")
            ) {
                let has_slice = n
                    .refs
                    .as_ref()
                    .and_then(Value::as_object)
                    .is_some_and(|o| o.contains_key("slice"));
                let refs_is_obj_or_absent = n.refs.as_ref().is_none_or(Value::is_object);
                if refs_is_obj_or_absent && !has_slice {
                    f.push(Finding::new(
                        "W_NO_SLICE",
                        vec![nid.clone()],
                        format!(
                            "{} '{nid}' has no refs.slice.",
                            k.unwrap_or("")
                        ),
                        "Every Screen/Component/PipelineStage SHOULD carry refs.slice (S0-buildable-vs-pipeline join key).".to_string(),
                    ));
                }
            }
        }
    }

    // ---- ADVISORY: anchorless nodes in an INGESTED graph (W_ANCHORLESS) ----
    check_anchorless(g, &mut f);

    // ---- ADVISORY: unlinked mirror stores in an INGESTED graph (F3) ------
    check_unlinked_mirror(g, &mut f);

    // ---- waivers last: matching advisories report as `waived` ------------
    apply_waivers(g, &mut f);

    f
}

/// Known schema version window (A4): major must match; a minor beyond the known
/// window is advisory-only (additive-minor policy, ENGINE-CONTRACT §1). Minor 4
/// is KNOWN on this branch: the D-005/D-006 co-landed features (design
/// completeness + freshness substrate) are implemented here and the vendored
/// corpus carries version-1.4 graphs that must stay warning-free.
/// `pub` because the `maapp schema` emitter derives the version-window
/// description from the same constants (single source of truth).
pub const KNOWN_MAJOR: u64 = 1;
pub const KNOWN_MINOR_MAX: u64 = 4;

/// Node kinds that SHOULD carry a `refs.source` anchor when the graph is
/// ingested from a real codebase: the load-bearing structure (surfaces, state,
/// backend ops) plus Assertions (guards agents must locate in source).
pub const ANCHOR_REQUIRED_KINDS: [&str; 4] = ["Screen", "StateStore", "BackendOp", "Assertion"];

/// The A4 schema/version gate. Accepted schema ids: the canonical
/// `"maapp-graph"` and the per-app `<app>-graph` convention (render derives the
/// app title token from the prefix) — i.e. any id with a non-empty prefix and
/// the `-graph` suffix. Anything else (or a missing/non-string header) is a
/// hard fail; an unknown MINOR of a known MAJOR is advisory-only.
fn check_schema_header(g: &Graph, f: &mut Vec<Finding>) {
    match g.doc.get("schema").and_then(Value::as_str) {
        None => {
            f.push(Finding::new(
                "E_SCHEMA_MISSING",
                vec![],
                "graph has no top-level 'schema' (string id).".to_string(),
                "Add \"schema\": \"maapp-graph\" (a per-app \"<app>-graph\" id is also accepted)."
                    .to_string(),
            ));
        }
        Some(id) if id.strip_suffix("-graph").is_none_or(str::is_empty) => {
            f.push(Finding::new(
                "E_SCHEMA_UNKNOWN",
                vec![],
                format!("schema id '{id}' is not a maapp graph id."),
                "Use \"maapp-graph\" (a per-app '<app>-graph' id is also accepted).".to_string(),
            ));
        }
        Some(_) => {}
    }
    match g.doc.get("version").and_then(Value::as_str) {
        None => {
            f.push(Finding::new(
                "E_SCHEMA_MISSING",
                vec![],
                "graph has no top-level 'version' (string).".to_string(),
                format!(
                    "Add \"version\": \"{KNOWN_MAJOR}.{KNOWN_MINOR_MAX}\" (MAJOR.MINOR string)."
                ),
            ));
        }
        Some(v) => match parse_version(v) {
            None => {
                f.push(Finding::new(
                    "E_VERSION_UNKNOWN",
                    vec![],
                    format!("version '{v}' is not a MAJOR.MINOR string."),
                    format!("Use a \"{KNOWN_MAJOR}.{KNOWN_MINOR_MAX}\"-style version string."),
                ));
            }
            Some((major, _)) if major != KNOWN_MAJOR => {
                f.push(Finding::new(
                    "E_VERSION_UNKNOWN",
                    vec![],
                    format!(
                        "version '{v}' has unknown major {major}; this engine supports major {KNOWN_MAJOR}."
                    ),
                    "Regenerate the graph for a supported schema major, or upgrade maapp."
                        .to_string(),
                ));
            }
            Some((_, minor)) if minor > KNOWN_MINOR_MAX => {
                f.push(Finding::new(
                    "W_VERSION_MINOR",
                    vec![],
                    format!(
                        "version '{v}' has unknown minor {minor} (known: {KNOWN_MAJOR}.0–{KNOWN_MAJOR}.{KNOWN_MINOR_MAX}); validating with the {KNOWN_MAJOR}.{KNOWN_MINOR_MAX} ruleset."
                    ),
                    "Advisory only — upgrade maapp if newer-minor features are in use."
                        .to_string(),
                ));
            }
            Some((_, minor)) if minor < KNOWN_MINOR_MAX => {
                // The previously-silent behind-arm (T5): a graph trailing the
                // engine rots on an old schema (missing newer-minor features).
                // Advisory only — points at the `migrate` upgrade verb.
                f.push(Finding::new(
                    "W_VERSION_BEHIND",
                    vec![],
                    format!(
                        "version '{v}' is behind this engine's {KNOWN_MAJOR}.{KNOWN_MINOR_MAX}; newer-minor schema features are unavailable until you upgrade."
                    ),
                    format!(
                        "Run `maapp migrate` to mechanically upgrade the graph to {KNOWN_MAJOR}.{KNOWN_MINOR_MAX}."
                    ),
                ));
            }
            Some(_) => {}
        },
    }
}

/// Parse a `"MAJOR.MINOR"` version string; `None` for any other shape.
fn parse_version(v: &str) -> Option<(u64, u64)> {
    let (maj, min) = v.split_once('.')?;
    Some((maj.parse().ok()?, min.parse().ok()?))
}

/// Per-edge passes: membership, dangling, selectional typecheck, boundary
/// legality, required attrs, enum membership.
fn per_edge(g: &Graph, i: usize, e: &Edge, f: &mut Vec<Finding>) {
    let raw_type = e.edge_type();
    let frm_opt = e.from();
    let to_opt = e.to();
    let eid = e.eid();

    // PASS 0 — membership (closed-vocab). On success bind BOTH the signature and
    // the (now-known-Some) edge type, so no `expect()` is needed downstream.
    let Some((etype, sig)) = raw_type.and_then(|t| g.edge_sig(t).map(|sig| (t, sig))) else {
        // Insertion order (oracle's `list(CORE_EDGES)`) so `_closest` ties break
        // identically to the Python dict iteration (e.g. navigates before dismisses).
        let core: Vec<String> = CORE_EDGE_ORDER.iter().map(|s| (*s).to_string()).collect();
        let sugg = closest(raw_type.unwrap_or(""), &core);
        let mut fix = String::new();
        if let Some(s) = sugg {
            fix.push_str(&format!("Closest core verb: {s}. "));
        }
        fix.push_str("Use a core verb or register x-<ns>:verb in edgeRegistry (with from_kinds/to_kinds/family/subPropertyOf).");
        f.push(Finding::new(
            "E_UNKNOWN_TYPE",
            vec![eid],
            format!(
                "edge #{i} has unknown type '{}'.",
                raw_type.unwrap_or("None")
            ),
            fix,
        ));
        return;
    };

    // referential integrity — dangling endpoints
    let mut dangling = false;
    let frm_in = frm_opt.is_some_and(|x| g.nodes.contains_key(x));
    let to_in = to_opt.is_some_and(|x| g.nodes.contains_key(x));
    if !frm_in {
        dangling = true;
        f.push(Finding::new(
            "E_DANGLING",
            vec![eid.clone()],
            format!(
                "edge #{i} ({etype}) 'from' endpoint '{}' is not a node in any layer.",
                frm_opt.unwrap_or("None")
            ),
            format!(
                "Add node '{}' or fix the 'from' slug.",
                frm_opt.unwrap_or("None")
            ),
        ));
    }
    if !to_in {
        dangling = true;
        f.push(Finding::new(
            "E_DANGLING",
            vec![eid.clone()],
            format!(
                "edge #{i} ({etype}) 'to' endpoint '{}' is not a node in any layer.",
                to_opt.unwrap_or("None")
            ),
            format!(
                "Add node '{}' or fix the 'to' slug.",
                to_opt.unwrap_or("None")
            ),
        ));
    }
    if dangling {
        return;
    }
    // Not dangling ⇒ both endpoints resolved to real nodes; bind non-optionally.
    let (Some(frm), Some(to)) = (frm_opt, to_opt) else {
        return;
    };

    let fk = g.kind(frm).map(str::to_string);
    let tk = g.kind(to).map(str::to_string);
    let everb = short_verb(etype);

    // PASS 1 — selectional typecheck (dual-axis).
    let from_ext_ok = registry_endpoint_ok(g, frm, etype, everb, /*outgoing=*/ true);
    let to_ext_ok = registry_endpoint_ok(g, to, etype, everb, /*outgoing=*/ false);

    if !sig.from.is_empty()
        && fk
            .as_deref()
            .is_none_or(|k| !sig.from.iter().any(|s| s == k))
        && !from_ext_ok
    {
        let accept = accepting_verbs(g, fk.as_deref(), tk.as_deref());
        let mut sorted_from = sig.from.clone();
        sorted_from.sort();
        let mut fix = String::new();
        if !accept.is_empty() {
            fix.push_str(&format!("Did you mean: {}? ", accept.join(", ")));
        }
        fix.push_str(&format!(
            "Either retag '{frm}' to a legal kind or use a verb whose from_kinds includes {}.",
            fk.as_deref().unwrap_or("None")
        ));
        f.push(Finding::new(
            "E_KIND",
            vec![frm.to_string(), to.to_string()],
            format!(
                "{etype} illegal from-kind: got from={frm} ({}); {etype} requires from ∈ {}.",
                fk.as_deref().unwrap_or("None"),
                py_str_list(&sorted_from)
            ),
            fix,
        ));
    }
    if !sig.to.is_empty()
        && tk.as_deref().is_none_or(|k| !sig.to.iter().any(|s| s == k))
        && !to_ext_ok
    {
        let accept = accepting_verbs(g, fk.as_deref(), tk.as_deref());
        let mut sorted_to = sig.to.clone();
        sorted_to.sort();
        let mut fix = String::new();
        if !accept.is_empty() {
            fix.push_str(&format!("Did you mean: {}? ", accept.join(", ")));
        }
        fix.push_str(&format!(
            "Either retag '{to}' to a legal kind or pick a verb whose to_kinds includes {}.",
            tk.as_deref().unwrap_or("None")
        ));
        f.push(Finding::new(
            "E_KIND",
            vec![frm.to_string(), to.to_string()],
            format!(
                "{etype} illegal to-kind: got to={to} ({}); {etype} requires to ∈ {}.",
                tk.as_deref().unwrap_or("None"),
                py_str_list(&sorted_to)
            ),
            fix,
        ));
    }

    // boundary-node legal_out / legal_in enforcement
    if let Some(lo) = g.legal_out_for_node(frm)
        && !lo.iter().any(|x| x == everb)
        && !lo.iter().any(|x| x == etype)
    {
        f.push(Finding::new(
            "E_KIND",
            vec![frm.to_string()],
            format!(
                "boundary node '{frm}' ({}) may not originate '{etype}'; its legal_out_edges are {}.",
                fk.as_deref().unwrap_or("None"),
                py_str_list(&lo)
            ),
            format!(
                "Only {} are legal out of a {} node.",
                py_str_list(&lo),
                fk.as_deref().unwrap_or("None")
            ),
        ));
    }
    if let Some(li) = g.legal_in_for_node(to)
        && !li.iter().any(|x| x == everb)
        && !li.iter().any(|x| x == etype)
    {
        f.push(Finding::new(
            "E_KIND",
            vec![to.to_string()],
            format!(
                "boundary node '{to}' ({}) may not be the target of '{etype}'; its legal_in_edges are {}.",
                tk.as_deref().unwrap_or("None"),
                py_str_list(&li)
            ),
            format!(
                "Only {} may point INTO a {} node.",
                py_str_list(&li),
                tk.as_deref().unwrap_or("None")
            ),
        ));
    }

    // required attrs + enum membership
    for ra in &sig.req {
        if !e.attrs.contains_key(ra) {
            f.push(Finding::new(
                "E_ATTR_MISSING",
                vec![eid.clone()],
                format!("{etype} edge #{i} is missing required attr '{ra}'."),
                format!("{etype} requires '{ra}'."),
            ));
        }
    }
    for (an, enum_vals) in &sig.enums {
        let Some(Value::String(val)) = e.attrs.get(an) else {
            continue;
        };
        if val == "else" || enum_vals.iter().any(|v| v == val) {
            continue;
        }
        if val.starts_with("x-") {
            // Open-with-registry (RES-003 2.1): an x-<ns>:token extension
            // value is legal ONLY if declared under "<verb>.<attr>" in the
            // graph's `attrEnumRegistry` (declarative data overlay, D-003).
            // DELIBERATE oracle divergence: the oracle's enum check skips any
            // value starting "x-" unconditionally (graph.py:
            // `not e[an].startswith("x-")`), which made "x-" a silent bypass
            // of every closed enum. Hardened here; documented, not a bug.
            if !g.attr_enum_declared(etype, an, val) {
                f.push(Finding::new(
                    "E_ATTR_ENUM",
                    vec![eid.clone()],
                    format!(
                        "{etype} edge #{i} attr {an}='{val}' is an x- extension value not declared in attrEnumRegistry."
                    ),
                    format!(
                        "Declare '{val}' under '{etype}.{an}' in attrEnumRegistry (list of x-<ns>:token strings) or use a core value."
                    ),
                ));
            }
        } else if !g.attr_enum_declared(etype, an, val) {
            // Since 1.4 the registry admits BARE non-core tokens too (F4:
            // a graph can declare domain-specific vocabulary as data instead of
            // keeping it in core). `x-<ns>:` namespacing remains the documented
            // convention for NEW extensions (collision-safe against future core
            // tokens).
            let mut sorted = enum_vals.clone();
            sorted.sort();
            f.push(Finding::new(
                "E_ATTR_ENUM",
                vec![eid.clone()],
                format!(
                    "{etype} edge #{i} attr {an}='{val}' is not in the legal set {}.",
                    py_str_list(&sorted)
                ),
                format!(
                    "Use one of {} (or a registered x-<ns> value where open-with-registry).",
                    py_str_list(&sorted)
                ),
            ));
        }
    }
}

/// Dual-axis: a registry endpoint passes if it is a registry kind AND its
/// legal_in/out list is absent OR admits the verb (short or namespaced form).
fn registry_endpoint_ok(g: &Graph, nid: &str, etype: &str, everb: &str, outgoing: bool) -> bool {
    let Some(k) = g.kind(nid) else { return false };
    if !g.node_registry.contains_key(k) {
        return false;
    }
    let legal = if outgoing {
        g.legal_out_for_node(nid)
    } else {
        g.legal_in_for_node(nid)
    };
    match legal {
        None => true,
        Some(list) => list.iter().any(|x| x == everb) || list.iter().any(|x| x == etype),
    }
}

/// Verbs whose signature would accept a (from-kind, to-kind) pair. Core first
/// (sorted by the BTreeMap key order — matching the oracle's dict insertion is
/// not guaranteed, so we sort the SUGGESTION list deterministically), then
/// registry verbs.
fn accepting_verbs(g: &Graph, fk: Option<&str>, tk: Option<&str>) -> Vec<String> {
    // The oracle iterates CORE_EDGES in dict-insertion order then registry in
    // dict order. To stay deterministic and order-stable we emit core verbs in a
    // fixed canonical order matching the oracle's CORE_EDGES insertion order.
    let mut out = Vec::new();
    for name in CORE_EDGE_ORDER {
        if let Some(sig) = core_edges().get(name) {
            let from_ok = sig.from.is_empty() || fk.is_some_and(|k| sig.from.contains(&k));
            let to_ok = sig.to.is_empty() || tk.is_some_and(|k| sig.to.contains(&k));
            if from_ok && to_ok {
                out.push((*name).to_string());
            }
        }
    }
    for (name, row) in &g.edge_registry {
        let f: Vec<String> = row
            .get("from_kinds")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let t: Vec<String> = row
            .get("to_kinds")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let from_ok = f.is_empty() || fk.is_some_and(|k| f.iter().any(|s| s == k));
        let to_ok = t.is_empty() || tk.is_some_and(|k| t.iter().any(|s| s == k));
        if from_ok && to_ok {
            out.push(name.clone());
        }
    }
    out
}

/// CORE_NODE_KINDS insertion order from the oracle, so the `E_UNKNOWN_KIND`
/// suggestion tie-breaks identically to the Python dict iteration.
const CORE_NODE_KIND_ORDER: [&str; 17] = [
    "Screen",
    "Component",
    "NavContainer",
    "Trigger",
    "NavAction",
    "MutationAction",
    "EffectAction",
    "ViewStateAction",
    "QueryAction",
    "ViewState",
    "Assertion",
    "StateStore",
    "DataSource",
    "BackendOp",
    "PipelineStage",
    "SideEffect",
    "Policy",
];

/// CORE_EDGES insertion order from the oracle, so suggestion lists match exactly.
const CORE_EDGE_ORDER: [&str; 14] = [
    "renders",
    "handles",
    "fires",
    "navigates",
    "dismisses",
    "setsViewState",
    "writes",
    "invokes",
    "reads",
    "binds",
    "emits",
    "produces",
    "guardedBy",
    "derivesFrom",
];

/// Strip the `x-<ns>:` prefix from a namespaced verb.
fn short_verb(etype: &str) -> &str {
    if etype.starts_with("x-")
        && let Some((_, rest)) = etype.split_once(':')
    {
        return rest;
    }
    etype
}

/// Each ref kind must match its documented form.
fn check_ref_form(nid: &str, rk: &str, rv: &str, f: &mut Vec<Finding>) {
    match rk {
        "design" => {
            if !(rv.starts_with("file:") || rv.starts_with("figma:") || rv.starts_with("render:")) {
                f.push(Finding::new(
                    "E_REFS_FORM",
                    vec![nid.to_string()],
                    format!(
                        "node '{nid}' refs.design='{rv}' must start with file:|figma:|render:."
                    ),
                    "design names a kind: file:<path> | figma:<key/node> | render:<id>."
                        .to_string(),
                ));
            }
        }
        "slice" => {
            // S followed by >=1 alnum chars (S0..S5 form; Sn.. allowed)
            let ok = rv.len() >= 2
                && rv.starts_with('S')
                && rv[1..].chars().all(|c| c.is_alphanumeric());
            if !ok {
                f.push(Finding::new(
                    "E_REFS_FORM",
                    vec![nid.to_string()],
                    format!(
                        "node '{nid}' refs.slice='{rv}' is not a slice id (expected S0..S5 form)."
                    ),
                    "slice is the walking-skeleton slice id, e.g. 'S0'.".to_string(),
                ));
            }
        }
        "decision"
            if !(rv.starts_with("ADR-") || rv.starts_with("D-") || rv.starts_with("PLAN-")) =>
        {
            f.push(Finding::new(
                "W_REFS_FORM",
                vec![nid.to_string()],
                format!("node '{nid}' refs.decision='{rv}' is not an obvious ADR/D-/PLAN id."),
                "decision is a graph-internal id like 'ADR-008' (advisory).".to_string(),
            ));
        }
        _ => {}
    }
}

/// `refs.source` value check: one anchor string or an array of anchor strings.
/// Any other JSON shape is `E_REFS_MALFORMED`.
fn check_source_ref(nid: &str, rv: &Value, f: &mut Vec<Finding>) {
    const FIX: &str = "refs.source is one \"relative/path.ext\" string (optional '#L<start>', '#L<start>-L<end>' or '@<symbol>' anchor) or an array of them.";
    match rv {
        Value::String(s) => check_source_form(nid, s, f),
        Value::Array(items) => {
            for it in items {
                if let Value::String(s) = it {
                    check_source_form(nid, s, f);
                } else {
                    f.push(Finding::new(
                        "E_REFS_MALFORMED",
                        vec![nid.to_string()],
                        format!(
                            "node '{nid}' refs.source array element must be a string, got {}.",
                            py_type(it)
                        ),
                        FIX.to_string(),
                    ));
                }
            }
        }
        other => {
            f.push(Finding::new(
                "E_REFS_MALFORMED",
                vec![nid.to_string()],
                format!(
                    "node '{nid}' refs.source must be a string or array of strings, got {}.",
                    py_type(other)
                ),
                FIX.to_string(),
            ));
        }
    }
}

/// FORM check for one source anchor: a repo-relative path (no leading `/`, no
/// `..` segment) plus an optional well-formed fragment — `#L<start>`,
/// `#L<start>-L<end>`, or `@<symbol>`. Existence on disk is deliberately NOT
/// checked (validate is repo-independent; drift is the check-drift verb's job).
fn check_source_form(nid: &str, s: &str, f: &mut Vec<Finding>) {
    let (path, fragment_ok) = if let Some((p, frag)) = s.split_once('#') {
        (p, is_line_anchor(frag))
    } else if let Some((p, sym)) = s.split_once('@') {
        (p, !sym.is_empty())
    } else {
        (s, true)
    };
    let path_ok =
        !path.is_empty() && !path.starts_with('/') && !path.split('/').any(|seg| seg == "..");
    if !path_ok || !fragment_ok {
        f.push(Finding::new(
            "E_REFS_FORM",
            vec![nid.to_string()],
            format!(
                "node '{nid}' refs.source='{s}' is not a relative source anchor (\"relative/path.ext\" + optional '#L<start>', '#L<start>-L<end>' or '@<symbol>')."
            ),
            "source is repo-relative: no absolute paths, no '..' segments; anchors are '#L<n>', '#L<n>-L<m>' or '@<symbol>'. Existence on disk is checked by check-drift, not validate.".to_string(),
        ));
    }
}

/// `L<digits>` or `L<digits>-L<digits>`.
fn is_line_anchor(frag: &str) -> bool {
    let Some(rest) = frag.strip_prefix('L') else {
        return false;
    };
    let all_digits = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit());
    match rest.split_once("-L") {
        None => all_digits(rest),
        Some((start, end)) => all_digits(start) && all_digits(end),
    }
}

/// `meta.provenance` shape gate (trust stamp). Absent provenance is fully
/// legal (all pre-move-2 graphs unchanged). When present: it must be an
/// object; `origin` is REQUIRED and a member of the closed enum; unknown keys
/// are errors (closed shape); the optional fields are type-checked. Every
/// value is AUTHORED data — the engine never computes commits/timestamps
/// (determinism rule).
fn check_provenance(g: &Graph, f: &mut Vec<Finding>) {
    let Some(p) = g.provenance() else { return };
    let Some(obj) = p.as_object() else {
        f.push(Finding::new(
            "E_PROVENANCE_MALFORMED",
            vec![],
            format!("meta.provenance must be an object, got {}.", py_type(p)),
            "provenance is {origin, sourceCommit?, generatedAt?, fidelity?, asOf?}.".to_string(),
        ));
        return;
    };

    // Unknown keys (document order), closed shape.
    let mut legal: Vec<String> = provenance_keys().iter().map(|s| (*s).to_string()).collect();
    legal.sort_unstable();
    for k in obj.keys() {
        if !provenance_keys().contains(&k.as_str()) {
            f.push(Finding::new(
                "E_PROVENANCE_KEY",
                vec![],
                format!("meta.provenance has unknown key '{k}'."),
                format!(
                    "provenance is a closed shape; legal keys: {}.",
                    py_str_list(&legal)
                ),
            ));
        }
    }

    // origin: REQUIRED, closed enum.
    let origins: Vec<String> = provenance_origins()
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    match obj.get("origin") {
        None => {
            f.push(Finding::new(
                "E_PROVENANCE_ORIGIN",
                vec![],
                format!(
                    "meta.provenance is missing required 'origin'; legal set {}.",
                    py_str_list(&origins)
                ),
                format!("Add \"origin\": one of {}.", py_str_list(&origins)),
            ));
        }
        Some(Value::String(s)) if provenance_origins().contains(&s.as_str()) => {}
        Some(v) => {
            f.push(Finding::new(
                "E_PROVENANCE_ORIGIN",
                vec![],
                format!(
                    "meta.provenance.origin '{}' is not in the legal set {}.",
                    py_str_value(v),
                    py_str_list(&origins)
                ),
                format!("Use one of {}.", py_str_list(&origins)),
            ));
        }
    }

    // Optional fields: sourceCommit/generatedAt/asOf strings, fidelity number 0..=1.
    for key in ["sourceCommit", "generatedAt", "asOf"] {
        if let Some(v) = obj.get(key)
            && !v.is_string()
        {
            f.push(Finding::new(
                "E_PROVENANCE_MALFORMED",
                vec![],
                format!(
                    "meta.provenance.{key} must be a string, got {}.",
                    py_type(v)
                ),
                format!("{key} is authored data (never computed by the engine)."),
            ));
        }
    }
    if let Some(v) = obj.get("fidelity") {
        let in_range = v.as_f64().is_some_and(|x| (0.0..=1.0).contains(&x));
        if !in_range {
            f.push(Finding::new(
                "E_PROVENANCE_MALFORMED",
                vec![],
                format!(
                    "meta.provenance.fidelity must be a number in [0, 1], got {}.",
                    py_str_value(v)
                ),
                "fidelity is an authored 0..=1 estimate of ingest completeness.".to_string(),
            ));
        }
    }
}

/// `meta.slice_of` shape gate (the `export --slice` annotation, spec §5).
/// Absent = fully legal. Present: an object with required non-empty string
/// `selector`, optional non-negative integer `depth`, no other keys. One code
/// (`E_SLICE_OF_MALFORMED`) covers every shape violation — the annotation is
/// engine-stamped, so a malformed one is hand-tampering worth failing on.
fn check_slice_of(g: &Graph, f: &mut Vec<Finding>) {
    const FIX: &str = "slice_of is stamped by `maapp export --slice`: {\"selector\": \"<slug>|scope:<scope>\", \"depth\"?: <n>}.";
    let Some(s) = g.slice_of() else { return };
    let mut fail = |msg: String| {
        f.push(Finding::new(
            "E_SLICE_OF_MALFORMED",
            vec![],
            msg,
            FIX.to_string(),
        ));
    };
    let Some(obj) = s.as_object() else {
        fail(format!(
            "meta.slice_of must be an object, got {}.",
            py_type(s)
        ));
        return;
    };
    for k in obj.keys() {
        if k != "selector" && k != "depth" {
            fail(format!("meta.slice_of has unknown key '{k}'."));
        }
    }
    match obj.get("selector") {
        Some(Value::String(sel)) if !sel.is_empty() => {}
        Some(v) => fail(format!(
            "meta.slice_of.selector must be a non-empty string, got {}.",
            py_str_value(v)
        )),
        None => fail("meta.slice_of is missing required 'selector'.".to_string()),
    }
    if let Some(d) = obj.get("depth")
        && d.as_u64().is_none()
    {
        fail(format!(
            "meta.slice_of.depth must be a non-negative integer, got {}.",
            py_str_value(d)
        ));
    }
}

/// `meta.waivers` shape gate (the warning baseline, RES-004 friction #9).
/// Absent = fully legal. Present: an array of closed-shape objects
/// `{code, node, reason}` — all non-empty strings, `code` a `W_*` advisory.
/// A waiver naming an `E_*` code is `E_WAIVER_FORBIDDEN` (hard errors are
/// NEVER waivable); every other shape violation is `E_WAIVER_MALFORMED`.
fn check_meta_waivers(g: &Graph, f: &mut Vec<Finding>) {
    const FIX: &str = "waivers is a list of {\"code\": \"W_*\", \"node\": \"<slug>\", \"reason\": \"...\"} objects; E_ codes are never waivable.";
    let Some(w) = g.waivers() else { return };
    let Some(items) = w.as_array() else {
        f.push(Finding::new(
            "E_WAIVER_MALFORMED",
            vec![],
            format!("meta.waivers must be an array, got {}.", py_type(w)),
            FIX.to_string(),
        ));
        return;
    };
    for (i, item) in items.iter().enumerate() {
        let Some(obj) = item.as_object() else {
            f.push(Finding::new(
                "E_WAIVER_MALFORMED",
                vec![],
                format!(
                    "meta.waivers[{i}] must be an object, got {}.",
                    py_type(item)
                ),
                FIX.to_string(),
            ));
            continue;
        };
        for k in obj.keys() {
            if !["code", "node", "reason"].contains(&k.as_str()) {
                f.push(Finding::new(
                    "E_WAIVER_MALFORMED",
                    vec![],
                    format!("meta.waivers[{i}] has unknown key '{k}'."),
                    FIX.to_string(),
                ));
            }
        }
        for key in ["code", "node", "reason"] {
            match obj.get(key) {
                Some(Value::String(s)) if !s.is_empty() => {}
                Some(v) => f.push(Finding::new(
                    "E_WAIVER_MALFORMED",
                    vec![],
                    format!(
                        "meta.waivers[{i}].{key} must be a non-empty string, got {}.",
                        py_str_value(v)
                    ),
                    FIX.to_string(),
                )),
                None => f.push(Finding::new(
                    "E_WAIVER_MALFORMED",
                    vec![],
                    format!("meta.waivers[{i}] is missing required '{key}'."),
                    FIX.to_string(),
                )),
            }
        }
        if let Some(code) = obj.get("code").and_then(Value::as_str) {
            if code.starts_with("E_") {
                f.push(Finding::new(
                    "E_WAIVER_FORBIDDEN",
                    vec![],
                    format!(
                        "meta.waivers[{i}] attempts to waive hard error '{code}'; E_ errors are never waivable."
                    ),
                    "Fix the error — a hard-fail finding cannot be baselined away.".to_string(),
                ));
            } else if !code.starts_with("W_") {
                f.push(Finding::new(
                    "E_WAIVER_MALFORMED",
                    vec![],
                    format!("meta.waivers[{i}].code '{code}' is not a W_* advisory code."),
                    FIX.to_string(),
                ));
            }
        }
    }
}

/// `Trigger.attrs.cause` enum gate (schema 1.4, F1 — RES-003 1.5: root
/// triggers model system-initiated entry points: webhooks, cron ticks,
/// callbacks). A Trigger MAY declare what summons it; when present the value
/// is checked against the closed core set (`trigger_causes`), open via an
/// `attrEnumRegistry` `"Trigger.cause"` row (D-003 data overlay; `x-<ns>:`
/// namespacing recommended). A Trigger WITHOUT a cause keeps the pre-1.4
/// behavior untouched (the orphan/lint path still covers dead nodes); a
/// `cause` attr on a non-Trigger node is author-owned passthrough.
fn check_trigger_causes(g: &Graph, f: &mut Vec<Finding>) {
    let mut core: Vec<String> = trigger_causes().iter().map(|s| (*s).to_string()).collect();
    core.sort_unstable();
    for nid in &g.node_order {
        let n = &g.nodes[nid];
        if n.kind() != Some("Trigger") {
            continue;
        }
        let Some(cause) = n.attrs.as_ref().and_then(|a| a.get("cause")) else {
            continue;
        };
        match cause {
            Value::String(s)
                if trigger_causes().contains(&s.as_str())
                    || g.attr_enum_declared("Trigger", "cause", s) => {}
            Value::String(s) => {
                f.push(Finding::new(
                    "E_TRIGGER_CAUSE",
                    vec![nid.clone()],
                    format!(
                        "Trigger '{nid}' attrs.cause='{s}' is not in the legal set {} nor declared in attrEnumRegistry.",
                        py_str_list(&core)
                    ),
                    format!(
                        "Use one of {} or declare the token under 'Trigger.cause' in attrEnumRegistry (x-<ns>:token recommended).",
                        py_str_list(&core)
                    ),
                ));
            }
            other => {
                f.push(Finding::new(
                    "E_TRIGGER_CAUSE",
                    vec![nid.clone()],
                    format!(
                        "Trigger '{nid}' attrs.cause must be a string, got {}.",
                        py_type(other)
                    ),
                    format!(
                        "cause names what summons a root Trigger: one of {} or an attrEnumRegistry-declared token under 'Trigger.cause'.",
                        py_str_list(&core)
                    ),
                ));
            }
        }
    }
}

/// `meta.flows` gate (schema 1.4, F2 — RES-003 2.4: named journeys become a
/// first-class, queryable construct instead of judge-scored free text).
/// Absent = fully legal. Present: an array of closed-shape objects
/// `{name, entry, terminals, via?}` where
/// - `name` is a non-empty string, unique across flows (`E_FLOW_DUP_NAME`);
/// - `entry` is a non-empty string resolving to a Screen or a ROOT Trigger
///   (no `handles` in-edge — F1 synergy; else `E_FLOW_ENTRY`);
/// - `terminals` is a NON-empty array of non-empty strings;
/// - `via` is an optional array of non-empty strings.
///
/// Any other shape is `E_FLOW_MALFORMED`; a slug that does not resolve is
/// `E_FLOW_UNRESOLVED`. For a shape-clean flow whose entry resolves,
/// REACHABILITY is derived: following outgoing `TRACE_TYPES` edges (the same
/// family as `q_trace`), every resolvable terminal and via slug must be
/// reached — otherwise the advisory `W_FLOW_UNREACHABLE` lists the missed
/// slugs (exit stays 0; waivable like any advisory).
fn check_meta_flows(g: &Graph, f: &mut Vec<Finding>) {
    const FIX: &str = "meta.flows is a list of {\"name\": \"...\", \"entry\": \"<Screen or root-Trigger slug>\", \"terminals\": [\"<slug>\", ...], \"via\"?: [\"<slug>\", ...]} objects.";
    let Some(w) = g.flows() else { return };
    let Some(items) = w.as_array() else {
        f.push(Finding::new(
            "E_FLOW_MALFORMED",
            vec![],
            format!("meta.flows must be an array, got {}.", py_type(w)),
            FIX.to_string(),
        ));
        return;
    };
    let mut seen_names: Vec<String> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let Some(obj) = item.as_object() else {
            f.push(Finding::new(
                "E_FLOW_MALFORMED",
                vec![],
                format!("meta.flows[{i}] must be an object, got {}.", py_type(item)),
                FIX.to_string(),
            ));
            continue;
        };
        let mut shape_ok = true;
        for k in obj.keys() {
            if !["name", "entry", "terminals", "via"].contains(&k.as_str()) {
                shape_ok = false;
                f.push(Finding::new(
                    "E_FLOW_MALFORMED",
                    vec![],
                    format!("meta.flows[{i}] has unknown key '{k}'."),
                    FIX.to_string(),
                ));
            }
        }
        for key in ["name", "entry"] {
            match obj.get(key) {
                Some(Value::String(s)) if !s.is_empty() => {}
                Some(v) => {
                    shape_ok = false;
                    f.push(Finding::new(
                        "E_FLOW_MALFORMED",
                        vec![],
                        format!(
                            "meta.flows[{i}].{key} must be a non-empty string, got {}.",
                            py_str_value(v)
                        ),
                        FIX.to_string(),
                    ));
                }
                None => {
                    shape_ok = false;
                    f.push(Finding::new(
                        "E_FLOW_MALFORMED",
                        vec![],
                        format!("meta.flows[{i}] is missing required '{key}'."),
                        FIX.to_string(),
                    ));
                }
            }
        }
        let str_list = |v: &Value| -> Option<Vec<String>> {
            let arr = v.as_array()?;
            arr.iter()
                .map(|x| match x {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                })
                .collect()
        };
        let terminals: Vec<String> = match obj.get("terminals") {
            Some(v) => match str_list(v) {
                Some(list) if !list.is_empty() => list,
                _ => {
                    shape_ok = false;
                    f.push(Finding::new(
                        "E_FLOW_MALFORMED",
                        vec![],
                        format!(
                            "meta.flows[{i}].terminals must be a non-empty array of node slugs."
                        ),
                        FIX.to_string(),
                    ));
                    Vec::new()
                }
            },
            None => {
                shape_ok = false;
                f.push(Finding::new(
                    "E_FLOW_MALFORMED",
                    vec![],
                    format!("meta.flows[{i}] is missing required 'terminals'."),
                    FIX.to_string(),
                ));
                Vec::new()
            }
        };
        let via: Vec<String> = match obj.get("via") {
            None => Vec::new(),
            Some(v) => match str_list(v) {
                Some(list) => list,
                None => {
                    shape_ok = false;
                    f.push(Finding::new(
                        "E_FLOW_MALFORMED",
                        vec![],
                        format!("meta.flows[{i}].via must be an array of node slugs."),
                        FIX.to_string(),
                    ));
                    Vec::new()
                }
            },
        };

        // Display name: the declared name when well-formed, else the index.
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map_or_else(|| format!("#{i}"), str::to_string);
        if seen_names.contains(&name) {
            f.push(Finding::new(
                "E_FLOW_DUP_NAME",
                vec![],
                format!("meta.flows[{i}] duplicates flow name '{name}'."),
                "Flow names are unique across meta.flows; rename or merge the duplicates."
                    .to_string(),
            ));
        } else {
            seen_names.push(name.clone());
        }

        // Resolution: entry, terminals, via must all be node ids.
        let entry = obj
            .get("entry")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let entry_resolved = entry.is_some_and(|e| g.nodes.contains_key(e));
        if let Some(e) = entry
            && !entry_resolved
        {
            f.push(Finding::new(
                "E_FLOW_UNRESOLVED",
                vec![e.to_string()],
                format!("flow '{name}' entry '{e}' does not resolve to any node."),
                "Every flow slug (entry, terminals, via) must be a node id in some layer."
                    .to_string(),
            ));
        }
        let mut resolvable_targets: Vec<String> = Vec::new();
        for (field, slugs) in [("terminals", &terminals), ("via", &via)] {
            for s in slugs {
                if g.nodes.contains_key(s) {
                    resolvable_targets.push(s.clone());
                } else {
                    f.push(Finding::new(
                        "E_FLOW_UNRESOLVED",
                        vec![s.clone()],
                        format!("flow '{name}' {field} slug '{s}' does not resolve to any node."),
                        "Every flow slug (entry, terminals, via) must be a node id in some layer."
                            .to_string(),
                    ));
                }
            }
        }

        // Entry legality: a Screen, or a ROOT Trigger (no handles in-edge).
        if let Some(e) = entry
            && entry_resolved
        {
            let kind = g.kind(e).unwrap_or("None");
            let legal = kind == "Screen" || g.is_root_trigger(e);
            if !legal {
                let why = if kind == "Trigger" {
                    "a handles-owned Trigger".to_string()
                } else {
                    format!("a {kind}")
                };
                f.push(Finding::new(
                    "E_FLOW_ENTRY",
                    vec![e.to_string()],
                    format!(
                        "flow '{name}' entry '{e}' is {why}; a flow enters at a Screen or a root Trigger (no handles edge)."
                    ),
                    "Point entry at the Screen where the journey starts, or at a root Trigger (attrs.cause documents what summons it).".to_string(),
                ));
                continue;
            }

            // Reachability (advisory): only for a shape-clean flow with a legal
            // resolved entry; unresolved slugs already errored above.
            if shape_ok {
                let reached = trace_reachable(g, e);
                let missed: Vec<String> = resolvable_targets
                    .iter()
                    .filter(|s| !reached.contains(*s))
                    .cloned()
                    .collect();
                if !missed.is_empty() {
                    f.push(Finding::new(
                        "W_FLOW_UNREACHABLE",
                        missed.clone(),
                        format!(
                            "flow '{name}' cannot reach {} from entry '{e}' following trace edges.",
                            py_str_list(&missed)
                        ),
                        "Wire the journey (handles/fires/navigates/writes/... path) or fix the flow declaration (suppressible).".to_string(),
                    ));
                }
            }
        }
    }
}

/// Node set reachable from `entry` following outgoing `TRACE_TYPES` edges —
/// the same traversal family as `q_trace` (the flow-reachability contract).
fn trace_reachable(g: &Graph, entry: &str) -> std::collections::BTreeSet<String> {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    seen.insert(entry.to_string());
    let mut stack: Vec<String> = vec![entry.to_string()];
    while let Some(cur) = stack.pop() {
        for &i in g.out_edges.get(cur.as_str()).into_iter().flatten() {
            let e = &g.edges[i];
            if e.edge_type().is_some_and(|t| TRACE_TYPES.contains(&t))
                && let Some(t) = e.to()
                && !seen.contains(t)
            {
                seen.insert(t.to_string());
                stack.push(t.to_string());
            }
        }
    }
    seen
}

/// ADVISORY W_UNLINKED_MIRROR (schema 1.4, F3 — RES-003 2.5, the L-046
/// false-zero recurrence): a StateStore that mirrors a persisted entity but
/// carries no linking convention to it makes two faithful ingests disagree.
/// Trigger conditions (ALL required — deliberately conservative and
/// false-positive-averse; NO name/intent heuristics are used because they are
/// unreliable):
///   1. `meta.provenance.origin == "ingested"` (generated/ratified graphs and
///      graphs without provenance are silent);
///   2. the graph models ≥1 DataSource (nothing to mirror otherwise);
///   3. the StateStore's UNDIRECTED connected component over the
///      dependency-edge family (`DEP_TYPES`: writes, invokes, reads, produces,
///      binds, derivesFrom, x-pipeline:feeds/consumes, x-behavior:reconciles)
///      contains NO DataSource — i.e. not even a mutation/backend path ties
///      the store to any persisted entity.
fn check_unlinked_mirror(g: &Graph, f: &mut Vec<Finding>) {
    let ingested = g
        .provenance()
        .and_then(|p| p.get("origin"))
        .and_then(Value::as_str)
        == Some("ingested");
    if !ingested {
        return;
    }
    if !g
        .node_order
        .iter()
        .any(|nid| g.kind(nid) == Some("DataSource"))
    {
        return;
    }
    for nid in &g.node_order {
        if g.kind(nid) != Some("StateStore") {
            continue;
        }
        if !dep_component_reaches_datasource(g, nid) {
            f.push(Finding::new(
                "W_UNLINKED_MIRROR",
                vec![nid.clone()],
                format!(
                    "StateStore '{nid}' has no dependency-edge path to any DataSource (graph provenance is 'ingested')."
                ),
                "One DataSource per persisted entity, one StateStore per client-side slice; a StateStore mirroring a persisted entity SHOULD link to its backing DataSource (derivesFrom, or the reads/writes path of the op that syncs it). Waive if the store is genuinely client-only (suppressible).".to_string(),
            ));
        }
    }
}

/// True iff the undirected dependency-family component containing `start`
/// includes a DataSource node.
fn dep_component_reaches_datasource(g: &Graph, start: &str) -> bool {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    seen.insert(start.to_string());
    let mut stack: Vec<String> = vec![start.to_string()];
    while let Some(cur) = stack.pop() {
        let out = g.out_edges.get(cur.as_str()).into_iter().flatten();
        let inn = g.in_edges.get(cur.as_str()).into_iter().flatten();
        for &i in out.chain(inn) {
            let e = &g.edges[i];
            if !e.edge_type().is_some_and(|t| DEP_TYPES.contains(&t)) {
                continue;
            }
            for nxt in [e.from(), e.to()].into_iter().flatten() {
                if seen.contains(nxt) {
                    continue;
                }
                if g.kind(nxt) == Some("DataSource") {
                    return true;
                }
                seen.insert(nxt.to_string());
                stack.push(nxt.to_string());
            }
        }
    }
    false
}

/// One well-formed waiver row: `(code, node, reason)`.
fn valid_waivers(g: &Graph) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let Some(items) = g.waivers().and_then(Value::as_array) else {
        return out;
    };
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        if obj.len() != 3 {
            continue;
        }
        let (Some(code), Some(node), Some(reason)) = (
            obj.get("code").and_then(Value::as_str),
            obj.get("node").and_then(Value::as_str),
            obj.get("reason").and_then(Value::as_str),
        ) else {
            continue;
        };
        if code.starts_with("W_") && !node.is_empty() && !reason.is_empty() {
            out.push((code.to_string(), node.to_string(), reason.to_string()));
        }
    }
    out
}

/// Turn every advisory finding matched by a well-formed `meta.waivers` entry
/// (same `code` + the waiver's `node` among the finding's `ids`) into a
/// `waived` finding carrying the waiver's reason. `E_*` findings are never
/// touched (`E_WAIVER_FORBIDDEN` fires in `check_meta_waivers` instead).
/// Idempotent — `validate_full` re-applies it after appending the opt-in
/// lint-pack advisories so those are waivable too. Never affects exit codes:
/// the CLI exits on the hard count only.
pub fn apply_waivers(g: &Graph, findings: &mut [Finding]) {
    let waivers = valid_waivers(g);
    if waivers.is_empty() {
        return;
    }
    for finding in findings.iter_mut() {
        if finding.hard() || finding.waived() {
            continue;
        }
        if let Some((_, _, reason)) = waivers
            .iter()
            .find(|(code, node, _)| *code == finding.code && finding.ids.contains(node))
        {
            finding.severity = "waived";
            finding.waived_reason = Some(reason.clone());
        }
    }
}

/// ADVISORY W_ANCHORLESS: in an INGESTED graph
/// (`meta.provenance.origin == "ingested"`), every Screen/StateStore/BackendOp/
/// Assertion node lacking a `refs.source` anchor is flagged — an ingested node
/// without a source anchor cannot be traced back to the code it claims to
/// model. Silent for absent provenance or any other origin. An empty array
/// counts as lacking.
fn check_anchorless(g: &Graph, f: &mut Vec<Finding>) {
    let ingested = g
        .provenance()
        .and_then(|p| p.get("origin"))
        .and_then(Value::as_str)
        == Some("ingested");
    if !ingested {
        return;
    }
    for nid in &g.node_order {
        let n = &g.nodes[nid];
        let Some(k) = n.kind() else { continue };
        if !ANCHOR_REQUIRED_KINDS.contains(&k) {
            continue;
        }
        let has_source = n
            .refs
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|o| o.get("source"))
            .is_some_and(|v| match v {
                Value::String(s) => !s.is_empty(),
                Value::Array(a) => !a.is_empty(),
                _ => false,
            });
        if !has_source {
            f.push(Finding::new(
                "W_ANCHORLESS",
                vec![nid.clone()],
                format!(
                    "{k} '{nid}' has no refs.source anchor (graph provenance is 'ingested')."
                ),
                "Add refs.source (\"relative/path.ext\" + optional '#L<start>[-L<end>]' or '@<symbol>') so agents can jump from graph to code.".to_string(),
            ));
        }
    }
}

/// `x-pipeline:feeds` must be a DAG. Kahn toposort detects a cycle; a bounded
/// back-trace reconstructs a readable cycle for the finding (matches the oracle).
fn check_pipeline_dag(g: &Graph, f: &mut Vec<Finding>) {
    use std::collections::VecDeque;
    let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut indeg: BTreeMap<String, i64> = BTreeMap::new();
    let mut nodes: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in &g.edges {
        if e.edge_type() == Some("x-pipeline:feeds") {
            let (Some(from), Some(to)) = (e.from(), e.to()) else {
                continue;
            };
            adj.entry(from.to_string())
                .or_default()
                .push(to.to_string());
            *indeg.entry(to.to_string()).or_insert(0) += 1;
            nodes.insert(from.to_string());
            nodes.insert(to.to_string());
        }
    }
    for n in &nodes {
        indeg.entry(n.clone()).or_insert(0);
    }

    let mut indeg_work = indeg.clone();
    // queue seeded with zero-in-degree nodes, sorted (matches the oracle).
    let mut queue: VecDeque<String> = nodes
        .iter()
        .filter(|n| indeg_work.get(*n).copied().unwrap_or(0) == 0)
        .cloned()
        .collect();
    // BTreeSet iteration is already sorted, so `queue` is sorted; but new nodes
    // are appended in adjacency order, matching the oracle's `queue.append`.
    let mut removed = 0usize;
    while let Some(n) = queue.pop_front() {
        removed += 1;
        if let Some(succs) = adj.get(&n) {
            for m in succs {
                if let Some(d) = indeg_work.get_mut(m) {
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(m.clone());
                    }
                }
            }
        }
    }
    if removed == nodes.len() {
        return; // acyclic
    }

    // residual subgraph holds the cycle.
    let residual: std::collections::BTreeSet<String> = nodes
        .iter()
        .filter(|n| indeg_work.get(*n).copied().unwrap_or(0) > 0)
        .cloned()
        .collect();
    let mut radj: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for n in &residual {
        let succs: Vec<String> = adj
            .get(n)
            .map(|v| {
                v.iter()
                    .filter(|x| residual.contains(*x))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        radj.insert(n.clone(), succs);
    }
    let mut start: Vec<String> = residual
        .iter()
        .filter(|n| radj.get(*n).is_some_and(|v| !v.is_empty()))
        .cloned()
        .collect();
    start.sort();

    let mut cyc: Vec<String> = Vec::new();
    if let Some(first) = start.first() {
        let mut path: Vec<String> = Vec::new();
        let mut on_path: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut cur: Option<String> = Some(first.clone());
        while let Some(c) = cur.clone() {
            if on_path.contains(&c) {
                break;
            }
            path.push(c.clone());
            on_path.insert(c.clone());
            cur = radj.get(&c).and_then(|v| v.first()).cloned();
        }
        if let Some(c) = cur
            && on_path.contains(&c)
            && let Some(idx) = path.iter().position(|x| *x == c)
        {
            cyc = path[idx..].to_vec();
            cyc.push(c);
        }
    }
    if cyc.is_empty() {
        cyc = residual.iter().cloned().collect();
    }
    f.push(Finding::new(
        "E_PIPELINE_CYCLE",
        cyc.clone(),
        format!(
            "x-pipeline:feeds is not a DAG; cycle: {}.",
            cyc.join(" -> ")
        ),
        "Pipeline stage→stage flow must be acyclic. Remove the back-edge.".to_string(),
    ));
}

/// The set of edge types that OPEN a surface, derived from core nav verbs ∪ every
/// registry verb whose subPropertyOf chain reaches a core surface-opener.
fn surface_opener_verbs(g: &Graph) -> std::collections::BTreeSet<String> {
    let core_openers = ["navigates", "returnsTo"];
    let mut openers: std::collections::BTreeSet<String> =
        core_openers.iter().map(|s| s.to_string()).collect();
    for (vname, row) in &g.edge_registry {
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut cur: Option<String> = Some(vname.clone());
        while let Some(c) = cur.clone() {
            if seen.contains(&c) {
                break;
            }
            seen.insert(c.clone());
            if core_openers.contains(&short_verb(&c)) {
                openers.insert(vname.clone());
                break;
            }
            let mut nxt = g
                .edge_registry
                .get(&c)
                .and_then(|r| r.get("subPropertyOf"))
                .and_then(Value::as_str)
                .map(str::to_string);
            if nxt.is_none() && c == *vname {
                nxt = row
                    .get("subPropertyOf")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            cur = nxt;
        }
    }
    openers
}

/// A `dismisses` edge must target a surface some opener edge presents.
fn check_dismisses(g: &Graph, f: &mut Vec<Finding>) {
    let opener_verbs = surface_opener_verbs(g);
    let mut opened: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in &g.edges {
        if let Some(t) = e.edge_type()
            && opener_verbs.contains(t)
            && let Some(to) = e.to()
        {
            opened.insert(to.to_string());
        }
    }
    for e in &g.edges {
        if e.edge_type() == Some("dismisses") {
            let target = e.to();
            let in_opened = target.is_some_and(|t| opened.contains(t));
            if !in_opened {
                f.push(Finding::new(
                    "E_DISMISS_NO_NAV",
                    vec![
                        e.from().unwrap_or("None").to_string(),
                        target.unwrap_or("None").to_string(),
                    ],
                    format!(
                        "dismisses '{}' but no navigates/returnsTo ever opens it.",
                        target.unwrap_or("None")
                    ),
                    format!(
                        "Add a navigates edge that presents '{}', or remove this dismiss.",
                        target.unwrap_or("None")
                    ),
                ));
            }
        }
    }
}

/// Group edges of a given type by their `from` endpoint, preserving document order.
fn group_by_from<'a>(g: &'a Graph, etype: &str) -> Vec<(String, Vec<&'a Edge>)> {
    // Preserve first-seen `from` order (matches Python defaultdict insertion order
    // when iterating edges in document order).
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, Vec<&Edge>> = BTreeMap::new();
    for e in &g.edges {
        if e.edge_type() == Some(etype) {
            let from = e.from().unwrap_or("None").to_string();
            if !groups.contains_key(&from) {
                order.push(from.clone());
            }
            groups.entry(from).or_default().push(e);
        }
    }
    order
        .into_iter()
        .map(|k| {
            let v = groups.remove(&k).unwrap_or_default();
            (k, v)
        })
        .collect()
}

/// XOR branch-set checks for `navigates.when` (Assertion-keyed).
fn check_navigates_branch_sets(g: &Graph, f: &mut Vec<Finding>) {
    for (src, arms) in group_by_from(g, "navigates") {
        let whenful: Vec<&Edge> = arms
            .iter()
            .filter(|a| a.attr("when").is_some())
            .copied()
            .collect();
        let whenless: Vec<&Edge> = arms
            .iter()
            .filter(|a| a.attr("when").is_none())
            .copied()
            .collect();
        if whenful.is_empty() {
            continue;
        }
        if !whenless.is_empty() {
            f.push(Finding::new(
                "E_BRANCH_MIXED",
                vec![src.clone()],
                format!(
                    "Action '{src}' mixes {} unconditional navigates with {} when-bearing branch arm(s).",
                    whenless.len(),
                    whenful.len()
                ),
                "Either give every arm a 'when' (one may be when:\"else\"), or split the unconditional transition out. A branch set must be all-conditional.".to_string(),
            ));
        }
        // membership + overlap accounting, preserving first-seen `when` order.
        let mut seen_order: Vec<String> = Vec::new();
        let mut seen_when: BTreeMap<String, i64> = BTreeMap::new();
        let mut else_count = 0i64;
        for a in &whenful {
            let w = when_str(a);
            if !seen_when.contains_key(&w) {
                seen_order.push(w.clone());
            }
            *seen_when.entry(w.clone()).or_insert(0) += 1;
            if w == "else" {
                else_count += 1;
                continue;
            }
            if !g.nodes.contains_key(&w) {
                f.push(Finding::new(
                    "E_BRANCH_UNKNOWN_ASSERTION",
                    vec![src.clone(), w.clone()],
                    format!("Action '{src}' branch arm when='{w}' does not resolve to any node."),
                    "navigates.when must reference an Assertion id (or the literal \"else\")."
                        .to_string(),
                ));
            } else if g.kind(&w) != Some("Assertion") {
                f.push(Finding::new(
                    "E_BRANCH_UNKNOWN_ASSERTION",
                    vec![src.clone(), w.clone()],
                    format!(
                        "Action '{src}' branch arm when='{w}' refers to a {}, not an Assertion.",
                        g.kind(&w).unwrap_or("None")
                    ),
                    "navigates.when must reference an Assertion id (or \"else\").".to_string(),
                ));
            }
        }
        for w in &seen_order {
            let c = seen_when.get(w).copied().unwrap_or(0);
            if c > 1 {
                f.push(Finding::new(
                    "E_BRANCH_OVERLAP",
                    vec![src.clone()],
                    format!(
                        "Action '{src}' has {c} branch arms with the same when='{w}'; arms must be mutually exclusive."
                    ),
                    "Two arms cannot share a guard. Distinguish or merge them.".to_string(),
                ));
            }
        }
        if else_count > 1 {
            f.push(Finding::new(
                "E_BRANCH_OVERLAP",
                vec![src.clone()],
                format!(
                    "Action '{src}' has {else_count} when:\"else\" arms; at most one default arm is allowed."
                ),
                "Keep exactly one when:\"else\" default arm.".to_string(),
            ));
        }
        if else_count == 0 {
            f.push(Finding::new(
                "W_BRANCH_NONEXHAUSTIVE",
                vec![src.clone()],
                format!(
                    "Action '{src}' branch set has no when:\"else\" default arm; exhaustiveness cannot be proven."
                ),
                "Add a when:\"else\" arm if a non-match should still navigate (suppressible where a non-match legitimately no-ops).".to_string(),
            ));
        }
    }
}

/// XOR branch-set checks for `x-ext:returnsTo.when` (outcome-keyed).
fn check_returnsto_branch_sets(g: &Graph, f: &mut Vec<Finding>) {
    for (src, arms) in group_by_from(g, "x-ext:returnsTo") {
        let whenful: Vec<&Edge> = arms
            .iter()
            .filter(|a| a.attr("when").is_some())
            .copied()
            .collect();
        let whenless: Vec<&Edge> = arms
            .iter()
            .filter(|a| a.attr("when").is_none())
            .copied()
            .collect();
        if whenful.is_empty() {
            continue;
        }
        let outcome_enum = g.outcome_enum(&src);
        if !whenless.is_empty() {
            f.push(Finding::new(
                "E_BRANCH_MIXED",
                vec![src.clone()],
                format!(
                    "Boundary node '{src}' mixes {} unconditional returnsTo with {} when-bearing arm(s).",
                    whenless.len(),
                    whenful.len()
                ),
                "A returnsTo branch set must be all-conditional (one arm may be when:\"else\")."
                    .to_string(),
            ));
        }
        let mut seen_order: Vec<String> = Vec::new();
        let mut seen_when: BTreeMap<String, i64> = BTreeMap::new();
        let mut else_count = 0i64;
        for a in &whenful {
            let w = when_str(a);
            if !seen_when.contains_key(&w) {
                seen_order.push(w.clone());
            }
            *seen_when.entry(w.clone()).or_insert(0) += 1;
            if w == "else" {
                else_count += 1;
                continue;
            }
            match &outcome_enum {
                None => {
                    f.push(Finding::new(
                        "E_BRANCH_UNKNOWN_OUTCOME",
                        vec![src.clone(), w.clone()],
                        format!(
                            "Boundary node '{src}' has a when='{w}' returnsTo arm but its kind declares no outcome enum."
                        ),
                        "The boundary node's nodeKindRegistry kind must declare attrs.outcome."
                            .to_string(),
                    ));
                }
                Some(enum_vals) if !enum_vals.contains(&w) => {
                    f.push(Finding::new(
                        "E_BRANCH_UNKNOWN_OUTCOME",
                        vec![src.clone(), w.clone()],
                        format!(
                            "Boundary node '{src}' returnsTo when='{w}' is not in its outcome enum {}.",
                            py_str_list(enum_vals)
                        ),
                        format!(
                            "Use a member of {} or the literal \"else\".",
                            py_str_list(enum_vals)
                        ),
                    ));
                }
                Some(_) => {}
            }
        }
        for w in &seen_order {
            let c = seen_when.get(w).copied().unwrap_or(0);
            if c > 1 {
                f.push(Finding::new(
                    "E_BRANCH_OVERLAP",
                    vec![src.clone()],
                    format!(
                        "Boundary node '{src}' has {c} returnsTo arms with the same when='{w}'."
                    ),
                    "Outcome values are pairwise disjoint; a duplicated when is an overlap."
                        .to_string(),
                ));
            }
        }
        if else_count > 1 {
            f.push(Finding::new(
                "E_BRANCH_OVERLAP",
                vec![src.clone()],
                format!("Boundary node '{src}' has {else_count} when:\"else\" returnsTo arms."),
                "Keep exactly one when:\"else\" default arm.".to_string(),
            ));
        }
        if let Some(enum_vals) = &outcome_enum
            && else_count == 0
        {
            let covered: std::collections::BTreeSet<String> =
                seen_when.keys().filter(|w| *w != "else").cloned().collect();
            let missing: Vec<String> = enum_vals
                .iter()
                .filter(|o| !covered.contains(*o))
                .cloned()
                .collect();
            if !missing.is_empty() {
                let mut covered_sorted: Vec<String> = covered.iter().cloned().collect();
                covered_sorted.sort();
                f.push(Finding::new(
                    "W_BRANCH_NONEXHAUSTIVE",
                    vec![src.clone()],
                    format!(
                        "Boundary node '{src}' returnsTo arms cover {} but the outcome enum is {}; missing {}.",
                        py_str_list(&covered_sorted),
                        py_str_list(enum_vals),
                        py_str_list(&missing)
                    ),
                    format!(
                        "Add arms for {} or a single when:\"else\" default (suppressible).",
                        py_str_list(&missing)
                    ),
                ));
            }
        }
    }
}

/// The `when` attribute as a string. Non-string `when` values are rendered with
/// Python `str()` semantics for the message (the oracle does `str(w)`).
fn when_str(e: &Edge) -> String {
    match e.attr("when") {
        Some(Value::String(s)) => s.clone(),
        Some(v) => py_str_value(v),
        None => "None".to_string(),
    }
}

/// Cheap closest-token suggestion: max shared prefix, then closest length.
/// Mirrors the oracle's `_closest` (min by (-prefix, |len diff|)).
fn closest(s: &str, candidates: &[String]) -> Option<String> {
    if s.is_empty() || candidates.is_empty() {
        return None;
    }
    candidates
        .iter()
        .min_by_key(|c| {
            let mut p = 0i64;
            for (a, b) in s.chars().zip(c.chars()) {
                if a == b {
                    p += 1;
                } else {
                    break;
                }
            }
            (-p, (c.len() as i64 - s.len() as i64).abs())
        })
        .cloned()
}

/// Python `repr`-style list of strings: `['a', 'b']`, matching `sorted(set)` print.
fn py_str_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| format!("'{s}'")).collect();
    format!("[{}]", inner.join(", "))
}

/// Python `type(x).__name__` for a JSON value (refs/edge attr error messages).
fn py_type(v: &Value) -> &'static str {
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

/// Python `str(value)` for a non-string JSON scalar used in a `when=` message.
fn py_str_value(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
