//! Render verbs: hub, deps, storyboard (ASCII), spine (ASCII), html.
//!
//! All render logic is a faithful port of the Python oracle's `render_hub`,
//! `render_deps`, `render_screens._emit_storyboard_ascii`, and
//! `render_screens._emit_spine_ascii` functions. Output must be byte-identical
//! to the oracle for identical inputs (the oracle differential contract).
//!
//! DETERMINISM CONTRACT: never iterate a HashMap/HashSet into output. All
//! ordering is imposed explicitly (BTreeMap iteration or .sort()).
//!
//! Several internal helpers carry invariant-guaranteed unwraps (keys inserted
//! in the same block; closures operate on map entries that were just constructed)
//! and internal nested functions with many arguments (direct port of oracle).
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use crate::graph::{Graph, core_edges};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// True when a `produces.verification` value is review-flagged: any NON-CORE
/// token (registry-declared) marks an artifact needing human review.
/// Derived from the SAME core enum table `validate` enforces — never a
/// hardcoded vendor string (schema 1.4 F4, RES-003 2.13: a domain-specific
/// review token is per-graph data now, and its render cue is this generic
/// non-core rule).
fn is_review_verification(v: &str) -> bool {
    let core = core_edges();
    let Some(sig) = core.get("produces") else {
        return false;
    };
    !sig.enums
        .iter()
        .any(|(an, vals)| *an == "verification" && vals.contains(&v))
}

// ---------------------------------------------------------------------------
// DEP_TYPES — mirrors oracle's `DEP_TYPES` set
// ---------------------------------------------------------------------------

const DEP_TYPES: &[&str] = &[
    "writes",
    "invokes",
    "reads",
    "produces",
    "binds",
    "derivesFrom",
    "x-pipeline:feeds",
    "x-pipeline:consumes",
    "x-behavior:reconciles",
];

fn is_dep_type(t: &str) -> bool {
    DEP_TYPES.contains(&t)
}

// ---------------------------------------------------------------------------
// KEY_OUT — mirrors oracle's `KEY_OUT` tuple
// ---------------------------------------------------------------------------

const KEY_OUT: &[&str] = &[
    "navigates",
    "dismisses",
    "renders",
    "writes",
    "invokes",
    "produces",
    "x-pipeline:feeds",
    "guardedBy",
];

// ---------------------------------------------------------------------------
// LAYER_ORDER — matches oracle's LAYER_ORDER list
// ---------------------------------------------------------------------------

const LAYER_ORDER: &[&str] = &["surface", "logic", "substrate", "policy", "boundary"];

// ---------------------------------------------------------------------------
// render_hub — markdown BLUEPRINT table
// ---------------------------------------------------------------------------

pub fn render_hub(g: &Graph) -> String {
    let schema = g.doc.get("schema").and_then(Value::as_str).unwrap_or("");
    let version = g.doc.get("version").and_then(Value::as_str).unwrap_or("");
    let mut lines = vec![
        "# App-Spec Graph — BLUEPRINT hub (generated; do NOT hand-edit)".to_string(),
        String::new(),
        format!(
            "> schema `{}` v`{}` · {} nodes · {} edges. Regenerate with `graph render hub`.",
            schema,
            version,
            g.nodes.len(),
            g.edges.len()
        ),
        String::new(),
    ];

    for &layer in LAYER_ORDER {
        let mut ids: Vec<&str> = g
            .node_layer
            .iter()
            .filter(|(_, l)| l.as_str() == layer)
            .map(|(id, _)| id.as_str())
            .collect();
        if ids.is_empty() {
            continue;
        }
        ids.sort_unstable();

        lines.push(format!("## {layer}"));
        lines.push(String::new());
        lines.push("| id | intent | slice | key out-edges |".to_string());
        lines.push("|---|---|---|---|".to_string());

        for nid in ids {
            let n = &g.nodes[nid];
            let intent = n.intent.as_deref().unwrap_or("").replace('|', "\\|");
            let slice = n
                .refs
                .as_ref()
                .and_then(Value::as_object)
                .and_then(|r| r.get("slice"))
                .and_then(Value::as_str)
                .unwrap_or("");

            let mut outs: Vec<(&str, &str)> = g
                .out_edges
                .get(nid)
                .map(|indices| {
                    indices
                        .iter()
                        .filter_map(|&i| {
                            let e = &g.edges[i];
                            let etype = e.edge_type()?;
                            let to = e.to()?;
                            if KEY_OUT.contains(&etype) {
                                Some((etype, to))
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            outs.sort_unstable();
            let out_str = if outs.is_empty() {
                "\u{2014}".to_string()
            } else {
                outs.iter()
                    .map(|(t, to)| format!("{t}\u{2192}{to}"))
                    .collect::<Vec<_>>()
                    .join("; ")
                    .replace('|', "\\|")
            };
            lines.push(format!("| `{nid}` | {intent} | {slice} | {out_str} |"));
        }
        lines.push(String::new());
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// render_deps — dependency view markdown + ASCII adjacency
// ---------------------------------------------------------------------------

pub fn render_deps(g: &Graph) -> String {
    // Use raw edge objects from the document to preserve JSON key insertion order
    // for attrs, matching the oracle's `{k: v for k, v in e.items() if k not in ...}`.
    let raw_edges: &[Value] = g
        .doc
        .get("edges")
        .and_then(Value::as_array)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);

    // Build (from, type, to, raw_obj) for dep edges, then sort by (from, type, to).
    let mut dep_edges: Vec<(&str, &str, &str, &Value)> = raw_edges
        .iter()
        .filter_map(|e| {
            let etype = e.get("type").and_then(Value::as_str)?;
            if !is_dep_type(etype) {
                return None;
            }
            let from = e.get("from").and_then(Value::as_str).unwrap_or("");
            let to = e.get("to").and_then(Value::as_str).unwrap_or("");
            Some((from, etype, to, e))
        })
        .collect();
    dep_edges.sort_by_key(|&(from, etype, to, _)| (from, etype, to));

    let dep_types_str = {
        let mut v: Vec<&str> = DEP_TYPES.to_vec();
        v.sort_unstable();
        let parts: Vec<String> = v.iter().map(|s| format!("'{s}'")).collect();
        format!("[{}]", parts.join(", "))
    };

    let mut lines = vec![
        "# App-Spec Graph — dependency view (generated; do NOT hand-edit)".to_string(),
        String::new(),
        format!(
            "> The dependency-edge subgraph (filter `.edges[]` to {}). {} edge(s). Regenerate with `graph render deps`.",
            dep_types_str,
            dep_edges.len()
        ),
        String::new(),
        "## Edge list".to_string(),
        String::new(),
        "| from | edge | to | attrs |".to_string(),
        "|---|---|---|---|".to_string(),
    ];

    for (from, etype, to, raw) in &dep_edges {
        let astr = raw_edge_attrs_str(raw);
        lines.push(format!("| `{from}` | {etype} | `{to}` | {astr} |"));
    }

    lines.push(String::new());
    lines.push("## ASCII adjacency (dependency out-edges)".to_string());
    lines.push(String::new());
    lines.push("```".to_string());

    let mut adj: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (src, etype, to, _) in &dep_edges {
        adj.entry(src).or_default().push((etype, to));
    }
    for (src, targets) in &adj {
        lines.push(src.to_string());
        for (etype, dst) in targets {
            lines.push(format!(
                "  \u{2500}\u{2500}{etype}\u{2500}\u{2500}\u{25b6} {dst}"
            ));
        }
    }
    lines.push("```".to_string());

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// render_storyboard_ascii
// ---------------------------------------------------------------------------

pub fn render_storyboard_ascii(g: &Graph) -> String {
    let proj = project_storyboard(g);
    let lanes = layout_storyboard(&proj, g);
    emit_storyboard_ascii(g, &proj, &lanes)
}

// ---------------------------------------------------------------------------
// render_spine_ascii
// ---------------------------------------------------------------------------

pub fn render_spine_ascii(g: &Graph) -> String {
    let proj = project_spine(g);
    emit_spine_ascii(g, &proj)
}

// ---------------------------------------------------------------------------
// render_html
//
// Mechanical: read the vendored static template, replace the literal placeholder
// `/*__GRAPH_JSON__*/null` with the compact source-order JSON of the document,
// breakout-escaped. Byte-identical to the Python oracle:
//   json.dumps(doc, separators=(",",":"))  → compact, ensure_ascii=True
//     .replace("</","<\\/").replace("<!--","<\\!--")
// ---------------------------------------------------------------------------

/// The vendored client template (src/assets/app-graph-template.html). Generic,
/// graph-driven, no app branding (see render_html.py docstring). Contains the
/// placeholder the inlined graph JSON replaces.
const HTML_TEMPLATE: &str = include_str!("assets/app-graph-template.html");
const HTML_PLACEHOLDER: &str = "/*__GRAPH_JSON__*/null";

pub fn render_html(g: &Graph) -> String {
    let graph_json = inline_graph_json(&g.doc);
    // `str::replace` is literal (no backref interpretation), matching Python's
    // str.replace. The placeholder occurs exactly once in the template.
    HTML_TEMPLATE.replace(HTML_PLACEHOLDER, &graph_json)
}

/// Serialize the document compactly and make it safe to embed inside a `<script>`:
/// neutralize `</` (closes the element) and `<!--` (opens an HTML comment), and
/// match Python `json.dumps`'s `ensure_ascii=True` (every non-ASCII codepoint
/// becomes a lowercase `\uXXXX` escape; codepoints > 0xFFFF become surrogate pairs).
fn inline_graph_json(doc: &Value) -> String {
    // serde_json compact output == Python separators=(",",":"). preserve_order is
    // enabled, so `doc` keeps the source file's key order (matches Python dict order).
    let compact = serde_json::to_string(doc).unwrap_or_default();
    let ascii = escape_non_ascii(&compact);
    ascii.replace("</", "<\\/").replace("<!--", "<\\!--")
}

/// Escape every codepoint > 0x7F to Python-compatible `\uXXXX` (lowercase hex).
/// Codepoints above the BMP (> 0xFFFF) are encoded as a UTF-16 surrogate pair,
/// exactly as Python's `json.dumps(ensure_ascii=True)` does.
fn escape_non_ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if cp <= 0x7F {
            out.push(ch);
        } else if cp <= 0xFFFF {
            out.push_str(&format!("\\u{cp:04x}"));
        } else {
            // Encode as a UTF-16 surrogate pair.
            let v = cp - 0x1_0000;
            let high = 0xD800 + (v >> 10);
            let low = 0xDC00 + (v & 0x3FF);
            out.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn short(nid: &str) -> &str {
    let after_colon = nid.split(':').nth(1).unwrap_or(nid);
    after_colon.split('/').next_back().unwrap_or(after_colon)
}

fn ns_of(nid: &str) -> &str {
    let after_colon = nid.split(':').nth(1).unwrap_or(nid);
    after_colon.split('/').next().unwrap_or(after_colon)
}

fn cat_of(nid: &str) -> &str {
    ns_of(nid)
}

fn natkey_string(s: &str) -> Vec<(u8, u64, String)> {
    let mut keys: Vec<(u8, u64, String)> = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        if rest.starts_with(|c: char| c.is_ascii_digit()) {
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            let num: u64 = rest[..end].parse().unwrap_or(0);
            keys.push((1, num, String::new()));
            rest = &rest[end..];
        } else {
            let end = rest
                .find(|c: char| c.is_ascii_digit())
                .unwrap_or(rest.len());
            keys.push((0, 0, rest[..end].to_string()));
            rest = &rest[end..];
        }
    }
    keys
}

fn cmp_natkey(a: &str, b: &str) -> std::cmp::Ordering {
    natkey_string(a).cmp(&natkey_string(b))
}

/// Emit attrs from a raw JSON edge Value in insertion order (preserves JSON key order).
/// Mirrors oracle: `{k: v for k, v in e.items() if k not in ("type", "from", "to")}`.
fn raw_edge_attrs_str(raw: &Value) -> String {
    let obj = match raw.as_object() {
        Some(o) => o,
        None => return String::new(),
    };
    let parts: Vec<String> = obj
        .iter()
        .filter(|(k, _)| k.as_str() != "type" && k.as_str() != "from" && k.as_str() != "to")
        .map(|(k, v)| {
            let vstr = match v {
                Value::String(s) => s.clone(),
                Value::Bool(b) => {
                    if *b {
                        "True".to_string()
                    } else {
                        "False".to_string()
                    }
                }
                other => other.to_string(),
            };
            format!("{k}={vstr}")
        })
        .collect();
    parts.join(", ")
}

fn edges_by_type(g: &Graph) -> HashMap<&str, Vec<&crate::model::Edge>> {
    let mut by: HashMap<&str, Vec<&crate::model::Edge>> = HashMap::new();
    for e in &g.edges {
        if let Some(t) = e.edge_type() {
            by.entry(t).or_default().push(e);
        }
    }
    by
}

fn app_title(g: &Graph) -> String {
    let generic_schemas: &[&str] = &["appui-graph", "app-graph", "appspec-graph", "graph", ""];
    if let Some(app) = g
        .doc
        .get("meta")
        .and_then(Value::as_object)
        .and_then(|m| m.get("app"))
        .and_then(Value::as_str)
    {
        let trimmed = app.trim();
        if !trimmed.is_empty() {
            return trimmed
                .split_whitespace()
                .next()
                .unwrap_or(trimmed)
                .to_string();
        }
    }
    let schema = g.doc.get("schema").and_then(Value::as_str).unwrap_or("");
    if generic_schemas.contains(&schema) {
        return "App-Spec Graph".to_string();
    }
    let token = schema.split('-').next().unwrap_or("").trim();
    if token.is_empty() {
        return "App-Spec Graph".to_string();
    }
    let mut chars = token.chars();
    match chars.next() {
        None => "App-Spec Graph".to_string(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn mode_short(mode: Option<&str>) -> &str {
    match mode {
        Some("push") => "push",
        Some("sheet") => "sheet",
        Some("fullScreen") => "full",
        Some("fullScreenCover") => "full",
        Some("popover") => "popover",
        Some("tab-switch") => "tab",
        Some("replace-root") => "root",
        Some("deep-link") => "deep",
        _ => "",
    }
}

fn center_str(s: &str, width: usize) -> String {
    let char_len = s.chars().count();
    if char_len >= width {
        return s.to_string();
    }
    let pad = width - char_len;
    let left = pad / 2;
    let right = pad - left;
    format!("{}{}{}", " ".repeat(left), s, " ".repeat(right))
}

fn ljust(s: &str, width: usize) -> String {
    let char_len = s.chars().count();
    if char_len >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - char_len))
    }
}

fn trunc_ljust(s: &str, width: usize) -> String {
    let truncated: String = s.chars().take(width).collect();
    ljust(&truncated, width)
}

// ---------------------------------------------------------------------------
// Storyboard projection
// ---------------------------------------------------------------------------

struct StoryboardProj {
    screens: Vec<String>,
    navs: Vec<String>,
    boundary_nodes: Vec<String>,
    nav_arrows: Vec<(String, String, String, Option<String>, Vec<String>)>,
    return_arrows: Vec<(String, String, String)>,
    returnsto_arrows: Vec<(String, String, String, Option<String>)>,
    footers: HashMap<String, Footer>,
    tail: Vec<TailStage>,
    renders: HashMap<String, Vec<(String, bool)>>,
    n_nodes: usize,
    n_edges: usize,
    schema: String,
    version: String,
    title: String,
}

#[derive(Default)]
struct Footer {
    binds: Vec<String>,
    has_vs: bool,
    writes: Vec<(String, bool)>,
}

struct TailStage {
    label: String,
    llm: bool,
    produces: Vec<(String, Option<String>, Option<String>)>,
}

fn project_storyboard(g: &Graph) -> StoryboardProj {
    let by_type = edges_by_type(g);

    let mut screens: Vec<String> = g
        .nodes
        .iter()
        .filter(|(_, n)| n.kind.as_deref() == Some("Screen"))
        .map(|(id, _)| id.clone())
        .collect();
    screens.sort_unstable();

    let mut navs: Vec<String> = g
        .nodes
        .iter()
        .filter(|(_, n)| n.kind.as_deref() == Some("NavContainer"))
        .map(|(id, _)| id.clone())
        .collect();
    navs.sort_unstable();

    let mut boundary_nodes: Vec<String> = g
        .node_layer
        .iter()
        .filter(|(_, l)| l.as_str() == "boundary")
        .map(|(id, _)| id.clone())
        .collect();
    boundary_nodes.sort_unstable();

    let screen_set: BTreeSet<&str> = screens.iter().map(String::as_str).collect();

    let mut renders: HashMap<String, Vec<(String, bool)>> = HashMap::new();
    for e in by_type.get("renders").map(|v| v.as_slice()).unwrap_or(&[]) {
        let from = e.from().unwrap_or("");
        let to = e.to().unwrap_or("");
        if from.starts_with("screen:") && to.starts_with("comp:") {
            let repeat = e
                .attrs
                .get("repeat")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            renders
                .entry(from.to_string())
                .or_default()
                .push((to.to_string(), repeat));
        }
    }
    for v in renders.values_mut() {
        v.sort_unstable();
    }

    let mut handles: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in by_type.get("handles").map(|v| v.as_slice()).unwrap_or(&[]) {
        handles
            .entry(e.from().unwrap_or(""))
            .or_default()
            .push(e.to().unwrap_or(""));
    }

    let mut fires_map: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in by_type.get("fires").map(|v| v.as_slice()).unwrap_or(&[]) {
        fires_map
            .entry(e.from().unwrap_or(""))
            .or_default()
            .push(e.to().unwrap_or(""));
    }

    let mut navtarget: HashMap<&str, (&str, Option<&str>)> = HashMap::new();
    for e in by_type
        .get("navigates")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        navtarget.insert(
            e.from().unwrap_or(""),
            (
                e.to().unwrap_or(""),
                e.attrs.get("present").and_then(Value::as_str),
            ),
        );
    }

    let mut dismisstarget: HashMap<&str, (&str, Option<&str>)> = HashMap::new();
    for e in by_type
        .get("dismisses")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        dismisstarget.insert(
            e.from().unwrap_or(""),
            (
                e.to().unwrap_or(""),
                e.attrs.get("target").and_then(Value::as_str),
            ),
        );
    }

    let mut stackroot: HashMap<&str, &str> = HashMap::new();
    for e in by_type
        .get("navigates")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        let from = e.from().unwrap_or("");
        let to = e.to().unwrap_or("");
        if from.starts_with("nav:")
            && e.attrs.get("present").and_then(Value::as_str) == Some("replace-root")
        {
            stackroot.insert(from, to);
        }
    }

    // resolve: if tid is a nav: node with a replace-root edge, return the root screen.
    // We use a macro-free helper to avoid closure lifetime issues.
    fn do_resolve<'r>(tid: &'r str, stackroot: &HashMap<&'r str, &'r str>) -> &'r str {
        if tid.starts_with("nav:")
            && let Some(&root) = stackroot.get(tid)
        {
            return root;
        }
        tid
    }

    let mut guards: HashMap<&str, Vec<String>> = HashMap::new();
    for e in by_type
        .get("guardedBy")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        let from = e.from().unwrap_or("");
        let to = e.to().unwrap_or("");
        if to.starts_with("assert:") {
            guards.entry(from).or_default().push(short(to).to_string());
        }
    }

    let mut nav_arrows_set: BTreeSet<(String, String, String, Option<String>, Vec<String>)> =
        BTreeSet::new();
    let mut dismiss_pairs: BTreeSet<(String, String)> = BTreeSet::new();

    // Oracle iterates ALL handles (not just screens): `for screen, trigs in handles.items()`.
    // Component-owned triggers that navigate are also included.
    let mut all_handle_nodes: Vec<&str> = handles.keys().copied().collect();
    all_handle_nodes.sort_unstable();
    for node in &all_handle_nodes {
        for &trig in handles.get(node).into_iter().flatten() {
            let ctrl = short(trig).to_string();
            for &act in fires_map.get(trig).into_iter().flatten() {
                if let Some(&(tgt, mode)) = navtarget.get(act) {
                    let resolved = do_resolve(tgt, &stackroot);
                    let mut gs: Vec<String> = guards.get(act).cloned().unwrap_or_default();
                    gs.sort_unstable();
                    nav_arrows_set.insert((
                        node.to_string(),
                        resolved.to_string(),
                        ctrl.clone(),
                        mode.map(str::to_string),
                        gs,
                    ));
                } else if let Some(&(tgt, _)) = dismisstarget.get(act) {
                    dismiss_pairs.insert((do_resolve(tgt, &stackroot).to_string(), ctrl.clone()));
                }
            }
        }
    }
    let nav_arrows: Vec<_> = nav_arrows_set.into_iter().collect();

    let mut presenter: HashMap<&str, &str> = HashMap::new();
    for (src, dst, _, _, _) in &nav_arrows {
        presenter.insert(dst.as_str(), src.as_str());
    }
    // Oracle deduplicates return_arrows by (dismissed, pres): first ctrl wins.
    // Python: `for dismissed, ctrl in sorted(set(dismiss_pairs)): if (dismissed, pres) not in seen`
    let mut return_set: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut seen_return: BTreeSet<(String, String)> = BTreeSet::new();
    let mut dismiss_sorted: Vec<_> = dismiss_pairs.iter().collect();
    dismiss_sorted.sort();
    for (dismissed, ctrl) in dismiss_sorted {
        if let Some(&pres) = presenter.get(dismissed.as_str())
            && pres != dismissed.as_str()
        {
            let key = (dismissed.clone(), pres.to_string());
            if seen_return.insert(key) {
                return_set.insert((dismissed.clone(), pres.to_string(), ctrl.clone()));
            }
        }
    }
    let return_arrows: Vec<_> = return_set.into_iter().collect();

    let mut returnsto_set: BTreeSet<(String, String, String, Option<String>)> = BTreeSet::new();
    for &etype in &["x-ext:returnsTo", "returnsTo"] {
        for e in by_type.get(etype).map(|v| v.as_slice()).unwrap_or(&[]) {
            let from = e.from().unwrap_or("");
            let to = e.to().unwrap_or("");
            let when = e.attrs.get("when").and_then(Value::as_str).unwrap_or("");
            let mode = e.attrs.get("present").and_then(Value::as_str);
            returnsto_set.insert((
                from.to_string(),
                do_resolve(to, &stackroot).to_string(),
                when.to_string(),
                mode.map(str::to_string),
            ));
        }
    }
    let returnsto_arrows: Vec<_> = returnsto_set.into_iter().collect();

    let mut binds_map: HashMap<&str, Vec<(&str, Option<&str>)>> = HashMap::new();
    for e in by_type.get("binds").map(|v| v.as_slice()).unwrap_or(&[]) {
        binds_map.entry(e.from().unwrap_or("")).or_default().push((
            e.to().unwrap_or(""),
            e.attrs.get("scopedBy").and_then(Value::as_str),
        ));
    }

    let mut act_writes: HashMap<&str, Vec<(&str, Option<&str>, bool)>> = HashMap::new();
    for e in by_type.get("writes").map(|v| v.as_slice()).unwrap_or(&[]) {
        let from = e.from().unwrap_or("");
        if from.starts_with("act:") {
            act_writes.entry(from).or_default().push((
                e.to().unwrap_or(""),
                e.attrs.get("mode").and_then(Value::as_str),
                e.attrs
                    .get("irreversible")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ));
        }
    }

    let mut act_invokes: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in by_type.get("invokes").map(|v| v.as_slice()).unwrap_or(&[]) {
        let from = e.from().unwrap_or("");
        if from.starts_with("act:") {
            act_invokes
                .entry(from)
                .or_default()
                .push(e.to().unwrap_or(""));
        }
    }

    let mut by_src_prod: HashMap<&str, Vec<(&str, Option<&str>, Option<&str>)>> = HashMap::new();
    for e in by_type.get("produces").map(|v| v.as_slice()).unwrap_or(&[]) {
        by_src_prod
            .entry(e.from().unwrap_or(""))
            .or_default()
            .push((
                e.to().unwrap_or(""),
                e.attrs.get("artifact").and_then(Value::as_str),
                e.attrs.get("verification").and_then(Value::as_str),
            ));
    }
    for v in by_src_prod.values_mut() {
        v.sort_unstable();
    }

    let footer_for = |screen: &str| -> Footer {
        let comps: Vec<&str> = renders
            .get(screen)
            .map(|v| v.iter().map(|(c, _)| c.as_str()).collect())
            .unwrap_or_default();
        let owners: Vec<&str> = std::iter::once(screen)
            .chain(comps.iter().copied())
            .collect();

        let mut bnd: Vec<String> = Vec::new();
        let mut seen_b: BTreeSet<&str> = BTreeSet::new();
        let mut has_vs = false;

        for &o in &owners {
            let mut b_sorted: Vec<(&str, Option<&str>)> =
                binds_map.get(o).cloned().unwrap_or_default();
            b_sorted.sort_unstable();
            for (tgt, _scope) in b_sorted {
                let k = g.kind(tgt);
                if k == Some("ViewState") || tgt.starts_with("vs:") {
                    has_vs = true;
                    continue;
                }
                if k == Some("StateStore") && !seen_b.contains(tgt) {
                    seen_b.insert(tgt);
                    bnd.push(short(tgt).to_string());
                }
            }
        }

        let mut wr: Vec<(String, bool)> = Vec::new();
        let mut seen_w: BTreeSet<String> = BTreeSet::new();

        for &trig in handles.get(screen).into_iter().flatten() {
            for &act in fires_map.get(trig).into_iter().flatten() {
                for &(store, _mode, irr) in act_writes.get(act).into_iter().flatten() {
                    let key = short(store).to_string();
                    if !seen_w.contains(&key) {
                        seen_w.insert(key.clone());
                        wr.push((key, irr));
                    }
                }
                for &op in act_invokes.get(act).into_iter().flatten() {
                    for &(store, _art, _v) in by_src_prod.get(op).into_iter().flatten() {
                        let key = short(store).to_string();
                        if !seen_w.contains(&key) {
                            seen_w.insert(key.clone());
                            wr.push((key, false));
                        }
                    }
                }
            }
        }

        Footer {
            binds: bnd,
            has_vs,
            writes: wr,
        }
    };

    let mut footers: HashMap<String, Footer> = HashMap::new();
    for s in &screens {
        footers.insert(s.clone(), footer_for(s));
    }

    let tail = pipeline_tail(g);
    let schema = g
        .doc
        .get("schema")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let version = g
        .doc
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let title = app_title(g);

    let _ = screen_set; // used via closure
    StoryboardProj {
        screens,
        navs,
        boundary_nodes,
        nav_arrows,
        return_arrows,
        returnsto_arrows,
        footers,
        tail,
        renders,
        n_nodes: g.nodes.len(),
        n_edges: g.edges.len(),
        schema,
        version,
        title,
    }
}

// ---------------------------------------------------------------------------
// Pipeline tail
// ---------------------------------------------------------------------------

fn pipeline_order_from_graph(g: &Graph) -> Vec<String> {
    let by_type = edges_by_type(g);
    let mut feeds: HashMap<&str, &str> = HashMap::new();
    for e in by_type
        .get("x-pipeline:feeds")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        feeds.insert(e.from().unwrap_or(""), e.to().unwrap_or(""));
    }
    let mut stages: Vec<String> = g
        .nodes
        .iter()
        .filter(|(_, n)| n.kind.as_deref() == Some("PipelineStage"))
        .map(|(id, _)| id.clone())
        .collect();
    stages.sort_unstable();
    if stages.is_empty() {
        return vec![];
    }

    let targets: BTreeSet<&str> = feeds.values().copied().collect();
    let mut heads: Vec<&str> = feeds
        .keys()
        .copied()
        .filter(|s| !targets.contains(s))
        .collect();
    heads.sort_unstable();

    let mut order: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let start = heads
        .first()
        .copied()
        .or_else(|| stages.first().map(String::as_str));
    let mut cur: Option<String> = start.map(str::to_string);
    while let Some(ref c) = cur.clone() {
        if seen.contains(c) {
            break;
        }
        order.push(c.clone());
        seen.insert(c.clone());
        cur = feeds.get(c.as_str()).map(|s| s.to_string());
    }
    for s in &stages {
        if !seen.contains(s) {
            order.push(s.clone());
            seen.insert(s.clone());
        }
    }
    order
}

fn pipeline_tail(g: &Graph) -> Vec<TailStage> {
    let by_type = edges_by_type(g);
    let order = pipeline_order_from_graph(g);
    let mut by_src_prod: HashMap<&str, Vec<(&str, Option<&str>, Option<&str>)>> = HashMap::new();
    for e in by_type.get("produces").map(|v| v.as_slice()).unwrap_or(&[]) {
        by_src_prod
            .entry(e.from().unwrap_or(""))
            .or_default()
            .push((
                e.to().unwrap_or(""),
                e.attrs.get("artifact").and_then(Value::as_str),
                e.attrs.get("verification").and_then(Value::as_str),
            ));
    }
    for v in by_src_prod.values_mut() {
        v.sort_unstable();
    }

    let mut out: Vec<TailStage> = Vec::new();
    for sid in &order {
        let n = g.nodes.get(sid.as_str());
        let attrs = n.and_then(|n| n.attrs.as_ref()).and_then(Value::as_object);
        let stage_label = attrs
            .and_then(|a| a.get("stage"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| short(sid))
            .to_string();
        let llm = attrs
            .and_then(|a| a.get("llm"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let prod: Vec<(String, Option<String>, Option<String>)> = by_src_prod
            .get(sid.as_str())
            .map(|v| {
                v.iter()
                    .map(|(st, art, verif)| {
                        (
                            short(st).to_string(),
                            art.map(str::to_string),
                            verif.map(str::to_string),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(TailStage {
            label: stage_label,
            llm,
            produces: prod,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Storyboard layout (derive lanes)
// ---------------------------------------------------------------------------

struct Lane {
    label: String,
    screens: Vec<String>,
}

fn layout_storyboard(proj: &StoryboardProj, g: &Graph) -> Vec<Lane> {
    let lanes = derive_lanes(g, &proj.screens);
    let placed: BTreeSet<&str> = lanes
        .iter()
        .flat_map(|l| l.screens.iter().map(String::as_str))
        .collect();
    let mut leftover: Vec<String> = proj
        .screens
        .iter()
        .filter(|s| !placed.contains(s.as_str()))
        .cloned()
        .collect();
    leftover.sort_unstable();
    let mut result = lanes;
    if !leftover.is_empty() {
        result.push(Lane {
            label: "other".to_string(),
            screens: leftover,
        });
    }
    let adj = screen_nav_adj(g, &proj.screens);
    for lane in &mut result {
        lane.screens = flow_order_within_lane(&lane.screens, &adj);
    }
    result
}

fn derive_lanes(g: &Graph, screens: &[String]) -> Vec<Lane> {
    if screens.is_empty() {
        return vec![];
    }
    if let Some(lanes) = lanes_by_slice(g, screens) {
        return lanes;
    }
    if let Some(lanes) = lanes_by_namespace(g, screens) {
        return lanes;
    }
    lanes_by_bfs_depth(g, screens)
}

fn lanes_by_slice(g: &Graph, screens: &[String]) -> Option<Vec<Lane>> {
    let slices: Vec<Option<&str>> = screens
        .iter()
        .map(|s| {
            g.nodes.get(s.as_str()).and_then(|n| {
                n.refs
                    .as_ref()
                    .and_then(Value::as_object)
                    .and_then(|r| r.get("slice"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
            })
        })
        .collect();
    if screens.is_empty() || slices.iter().any(|s| s.is_none()) {
        return None;
    }
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (s, sl) in screens.iter().zip(slices.iter()) {
        groups
            .entry(sl.unwrap().to_string())
            .or_default()
            .push(s.clone());
    }
    let mut sorted_slice_ids: Vec<String> = groups.keys().cloned().collect();
    sorted_slice_ids.sort_by(|a, b| cmp_natkey(a, b));
    let lanes = sorted_slice_ids
        .into_iter()
        .map(|slice_id| {
            let mut members = groups[&slice_id].clone();
            members.sort_by(|a, b| ns_of(a).cmp(ns_of(b)).then_with(|| short(a).cmp(short(b))));
            Lane {
                label: slice_id,
                screens: members,
            }
        })
        .collect();
    Some(lanes)
}

fn lanes_by_namespace(g: &Graph, screens: &[String]) -> Option<Vec<Lane>> {
    let mut ns_first: Vec<String> = Vec::new();
    let mut screens_sorted = screens.to_vec();
    screens_sorted.sort_by(|a, b| cmp_natkey(a, b));
    let mut seen_ns: BTreeSet<String> = BTreeSet::new();
    for s in &screens_sorted {
        let n = ns_of(s).to_string();
        if seen_ns.insert(n.clone()) {
            ns_first.push(n);
        }
    }
    if ns_first.len() < 2 {
        return None;
    }

    let adj = screen_nav_adj(g, screens);
    let mut ns_dag: HashMap<&str, BTreeSet<String>> = HashMap::new();
    for s in screens {
        for d in adj.get(s.as_str()).into_iter().flatten() {
            if ns_of(s) != ns_of(d) {
                ns_dag
                    .entry(ns_of(s))
                    .or_default()
                    .insert(ns_of(d).to_string());
            }
        }
    }

    let order = topo_order_namespaces(&ns_first, &ns_dag);
    let mut by_ns: HashMap<&str, Vec<String>> = HashMap::new();
    for s in screens {
        by_ns.entry(ns_of(s)).or_default().push(s.clone());
    }

    let lanes = order
        .into_iter()
        .map(|ns_val| {
            let mut members = by_ns.get(ns_val.as_str()).cloned().unwrap_or_default();
            members.sort_by(|a, b| cmp_natkey(short(a), short(b)));
            Lane {
                label: ns_val,
                screens: members,
            }
        })
        .collect();
    Some(lanes)
}

fn topo_order_namespaces(
    namespaces_in_order: &[String],
    ns_dag: &HashMap<&str, BTreeSet<String>>,
) -> Vec<String> {
    let order_idx: HashMap<&str, usize> = namespaces_in_order
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    let mut indeg: HashMap<&str, usize> = namespaces_in_order
        .iter()
        .map(|s| (s.as_str(), 0usize))
        .collect();
    let mut succ: HashMap<&str, BTreeSet<&str>> = namespaces_in_order
        .iter()
        .map(|s| (s.as_str(), BTreeSet::new()))
        .collect();

    for ns_from in namespaces_in_order {
        if let Some(nbrs) = ns_dag.get(ns_from.as_str()) {
            for nb in nbrs {
                let nb_str = nb.as_str();
                if ns_from.as_str() == nb_str || !indeg.contains_key(nb_str) {
                    continue;
                }
                if succ.get_mut(ns_from.as_str()).unwrap().insert(nb_str) {
                    *indeg.get_mut(nb_str).unwrap() += 1;
                }
            }
        }
    }

    let mut ready: Vec<&str> = namespaces_in_order
        .iter()
        .filter(|s| indeg[s.as_str()] == 0)
        .map(String::as_str)
        .collect();
    ready.sort_by_key(|s| order_idx.get(s).copied().unwrap_or(usize::MAX));

    let mut out: Vec<String> = Vec::new();
    let mut placed: BTreeSet<&str> = BTreeSet::new();

    while !ready.is_empty() {
        ready.sort_by_key(|s| order_idx.get(s).copied().unwrap_or(usize::MAX));
        let cur = ready.remove(0);
        if placed.contains(cur) {
            continue;
        }
        placed.insert(cur);
        out.push(cur.to_string());
        let succs: Vec<&str> = succ[cur].iter().copied().collect();
        for nxt in succs {
            if let Some(v) = indeg.get_mut(nxt) {
                if *v > 0 {
                    *v -= 1;
                }
                if *v == 0 && !placed.contains(nxt) {
                    ready.push(nxt);
                }
            }
        }
    }
    for ns_val in namespaces_in_order {
        if !placed.contains(ns_val.as_str()) {
            out.push(ns_val.clone());
        }
    }
    out
}

fn lanes_by_bfs_depth(g: &Graph, screens: &[String]) -> Vec<Lane> {
    if screens.is_empty() {
        return vec![];
    }
    let adj = screen_nav_adj(g, screens);
    let screen_set: BTreeSet<&str> = screens.iter().map(String::as_str).collect();

    let bfs_depth = |start: &str| -> HashMap<String, usize> {
        let mut depth: HashMap<String, usize> = HashMap::new();
        depth.insert(start.to_string(), 0);
        let mut frontier: Vec<String> = vec![start.to_string()];
        loop {
            let mut sorted_f = frontier.clone();
            sorted_f.sort_by(|a, b| cmp_natkey(a, b));
            let mut nxt: Vec<String> = Vec::new();
            for u in &sorted_f {
                if let Some(dsts) = adj.get(u.as_str()) {
                    let mut sd: Vec<_> = dsts.iter().map(String::as_str).collect();
                    sd.sort_by(|a, b| cmp_natkey(a, b));
                    for &v in &sd {
                        if !depth.contains_key(v) {
                            depth.insert(v.to_string(), depth[u.as_str()] + 1);
                            nxt.push(v.to_string());
                        }
                    }
                }
            }
            if nxt.is_empty() {
                break;
            }
            frontier = nxt;
        }
        depth
    };

    let mut indeg: HashMap<&str, usize> = screen_set.iter().map(|&s| (s, 0usize)).collect();
    for s in screens {
        for d in adj.get(s.as_str()).into_iter().flatten() {
            if screen_set.contains(d.as_str()) {
                *indeg.entry(d.as_str()).or_default() += 1;
            }
        }
    }
    let sources: Vec<&str> = screens
        .iter()
        .filter(|s| indeg.get(s.as_str()).copied().unwrap_or(0) == 0)
        .map(String::as_str)
        .collect();

    let roots = nav_roots(g, screens);
    let mut candidates: Vec<String> = Vec::new();
    let mut seen_c: BTreeSet<String> = BTreeSet::new();
    for r in roots
        .iter()
        .chain(
            sources
                .iter()
                .map(|&s| s.into())
                .collect::<Vec<String>>()
                .iter(),
        )
        .chain(screens.iter())
    {
        if seen_c.insert(r.clone()) {
            candidates.push(r.clone());
        }
    }
    candidates.sort_by(|a, b| cmp_natkey(a, b));

    let start = candidates
        .iter()
        .max_by_key(|c| bfs_depth(c).len())
        .cloned()
        .unwrap_or_else(|| screens[0].clone());
    let depth = bfs_depth(&start);

    let mut bands: BTreeMap<Option<usize>, Vec<String>> = BTreeMap::new();
    for s in screens {
        bands
            .entry(depth.get(s.as_str()).copied())
            .or_default()
            .push(s.clone());
    }

    let mut lanes: Vec<Lane> = Vec::new();
    let mut depth_keys: Vec<usize> = bands.keys().filter_map(|k| *k).collect();
    depth_keys.sort_unstable();
    for d in depth_keys {
        let mut members = bands[&Some(d)].clone();
        members.sort_by(|a, b| cmp_natkey(a, b));
        lanes.push(Lane {
            label: format!("depth {d}"),
            screens: members,
        });
    }
    if let Some(unreached) = bands.get(&None) {
        let mut members = unreached.clone();
        members.sort_by(|a, b| cmp_natkey(a, b));
        lanes.push(Lane {
            label: "unreached".to_string(),
            screens: members,
        });
    }
    lanes
}

fn nav_roots(g: &Graph, screens: &[String]) -> Vec<String> {
    let by_type = edges_by_type(g);
    let screen_set: BTreeSet<&str> = screens.iter().map(String::as_str).collect();

    let mut stackroot: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in by_type
        .get("navigates")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        let from = e.from().unwrap_or("");
        let to = e.to().unwrap_or("");
        if from.starts_with("nav:")
            && e.attrs.get("present").and_then(Value::as_str) == Some("replace-root")
        {
            stackroot.entry(from).or_default().push(to);
        }
    }
    let mut nav_renders_map: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in by_type.get("renders").map(|v| v.as_slice()).unwrap_or(&[]) {
        let from = e.from().unwrap_or("");
        let to = e.to().unwrap_or("");
        if from.starts_with("nav:") && to.starts_with("nav:") {
            nav_renders_map.entry(from).or_default().push(to);
        }
    }

    fn resolve_container<'a>(
        nid: &'a str,
        seen: &mut BTreeSet<&'a str>,
        stackroot: &'a HashMap<&'a str, Vec<&'a str>>,
        nav_renders: &'a HashMap<&'a str, Vec<&'a str>>,
        screen_set: &BTreeSet<&'a str>,
    ) -> Vec<&'a str> {
        if seen.contains(nid) {
            return vec![];
        }
        seen.insert(nid);
        let mut out = Vec::new();
        for &r in stackroot.get(nid).into_iter().flatten() {
            if screen_set.contains(r) {
                out.push(r);
            } else if r.starts_with("nav:") {
                out.extend(resolve_container(
                    r,
                    seen,
                    stackroot,
                    nav_renders,
                    screen_set,
                ));
            }
        }
        for &child in nav_renders.get(nid).into_iter().flatten() {
            out.extend(resolve_container(
                child,
                seen,
                stackroot,
                nav_renders,
                screen_set,
            ));
        }
        out
    }

    let rendered_navs: BTreeSet<&str> = nav_renders_map.values().flatten().copied().collect();
    let top_navs: Vec<&str> = stackroot
        .keys()
        .copied()
        .filter(|n| !rendered_navs.contains(n))
        .collect();
    let top_navs = if top_navs.is_empty() {
        stackroot.keys().copied().collect()
    } else {
        top_navs
    };

    let mut roots: Vec<String> = Vec::new();
    let mut seen_roots: BTreeSet<String> = BTreeSet::new();
    for n in top_navs {
        for r in resolve_container(
            n,
            &mut BTreeSet::new(),
            &stackroot,
            &nav_renders_map,
            &screen_set,
        ) {
            if seen_roots.insert(r.to_string()) {
                roots.push(r.to_string());
            }
        }
    }
    roots.sort_by(|a, b| cmp_natkey(a, b));
    roots
}

fn screen_nav_adj(g: &Graph, screens: &[String]) -> HashMap<String, Vec<String>> {
    let by_type = edges_by_type(g);
    let screen_set: BTreeSet<&str> = screens.iter().map(String::as_str).collect();

    let mut handles: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in by_type.get("handles").map(|v| v.as_slice()).unwrap_or(&[]) {
        handles
            .entry(e.from().unwrap_or(""))
            .or_default()
            .push(e.to().unwrap_or(""));
    }
    let mut fires_m: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in by_type.get("fires").map(|v| v.as_slice()).unwrap_or(&[]) {
        fires_m
            .entry(e.from().unwrap_or(""))
            .or_default()
            .push(e.to().unwrap_or(""));
    }
    let mut navtarget: HashMap<&str, &str> = HashMap::new();
    for e in by_type
        .get("navigates")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        navtarget.insert(e.from().unwrap_or(""), e.to().unwrap_or(""));
    }
    let mut stackroot: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in by_type
        .get("navigates")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        let from = e.from().unwrap_or("");
        let to = e.to().unwrap_or("");
        if from.starts_with("nav:")
            && e.attrs.get("present").and_then(Value::as_str) == Some("replace-root")
        {
            stackroot.entry(from).or_default().push(to);
        }
    }
    let mut nav_renders_map: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in by_type.get("renders").map(|v| v.as_slice()).unwrap_or(&[]) {
        let from = e.from().unwrap_or("");
        let to = e.to().unwrap_or("");
        if from.starts_with("nav:") && to.starts_with("nav:") {
            nav_renders_map.entry(from).or_default().push(to);
        }
    }

    fn resolve_adj<'a>(
        tid: &'a str,
        screen_set: &BTreeSet<&'a str>,
        stackroot: &'a HashMap<&'a str, Vec<&'a str>>,
        nav_renders: &'a HashMap<&'a str, Vec<&'a str>>,
    ) -> Vec<&'a str> {
        if screen_set.contains(tid) {
            return vec![tid];
        }
        if !tid.starts_with("nav:") {
            return vec![];
        }
        fn rc<'b>(
            nid: &'b str,
            seen: &mut BTreeSet<&'b str>,
            stackroot: &'b HashMap<&'b str, Vec<&'b str>>,
            nav_renders: &'b HashMap<&'b str, Vec<&'b str>>,
            screen_set: &BTreeSet<&'b str>,
        ) -> Vec<&'b str> {
            if seen.contains(nid) {
                return vec![];
            }
            seen.insert(nid);
            let mut out = Vec::new();
            for &r in stackroot.get(nid).into_iter().flatten() {
                if screen_set.contains(r) {
                    out.push(r);
                } else if r.starts_with("nav:") {
                    out.extend(rc(r, seen, stackroot, nav_renders, screen_set));
                }
            }
            for &child in nav_renders.get(nid).into_iter().flatten() {
                out.extend(rc(child, seen, stackroot, nav_renders, screen_set));
            }
            out
        }
        rc(
            tid,
            &mut BTreeSet::new(),
            stackroot,
            nav_renders,
            screen_set,
        )
    }

    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for screen in screens {
        if !screen_set.contains(screen.as_str()) {
            continue;
        }
        for &trig in handles.get(screen.as_str()).into_iter().flatten() {
            for &act in fires_m.get(trig).into_iter().flatten() {
                if let Some(&tgt) = navtarget.get(act) {
                    for dst in resolve_adj(tgt, &screen_set, &stackroot, &nav_renders_map) {
                        if screen_set.contains(dst) && dst != screen.as_str() {
                            adj.entry(screen.clone()).or_default().push(dst.to_string());
                        }
                    }
                }
            }
        }
    }
    adj
}

fn flow_order_within_lane(members: &[String], adj: &HashMap<String, Vec<String>>) -> Vec<String> {
    let member_set: BTreeSet<&str> = members.iter().map(String::as_str).collect();
    let order_idx: HashMap<&str, usize> = members
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();

    let mut indeg: HashMap<&str, usize> = members.iter().map(|s| (s.as_str(), 0usize)).collect();
    let mut succ: HashMap<&str, Vec<&str>> =
        members.iter().map(|s| (s.as_str(), Vec::new())).collect();

    for s in members {
        for d in adj.get(s).into_iter().flatten() {
            if member_set.contains(d.as_str()) && d != s && !succ[s.as_str()].contains(&d.as_str())
            {
                succ.get_mut(s.as_str()).unwrap().push(d.as_str());
                *indeg.get_mut(d.as_str()).unwrap() += 1;
            }
        }
    }

    let mut ready: Vec<&str> = members
        .iter()
        .filter(|s| indeg[s.as_str()] == 0)
        .map(String::as_str)
        .collect();
    ready.sort_by_key(|s| order_idx[s]);

    let mut out: Vec<String> = Vec::new();
    let mut placed: BTreeSet<&str> = BTreeSet::new();

    while !ready.is_empty() {
        let cur = ready.remove(0);
        if placed.contains(cur) {
            continue;
        }
        placed.insert(cur);
        out.push(cur.to_string());
        let succs: Vec<&str> = succ[cur].to_vec();
        let mut newly: Vec<&str> = Vec::new();
        for d in succs {
            let v = indeg.get_mut(d).unwrap();
            if *v > 0 {
                *v -= 1;
            }
            if *v == 0 && !placed.contains(d) {
                newly.push(d);
            }
        }
        if !newly.is_empty() {
            ready.extend(newly);
            ready.sort_by_key(|s| order_idx.get(s).copied().unwrap_or(usize::MAX));
        }
    }
    for s in members {
        if !placed.contains(s.as_str()) {
            out.push(s.clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Storyboard ASCII emission
// ---------------------------------------------------------------------------

fn sb_blocks(
    screen: &str,
    renders: &HashMap<String, Vec<(String, bool)>>,
) -> Vec<(String, bool, bool)> {
    let comps = renders.get(screen);
    if comps.map(|c| c.is_empty()).unwrap_or(true) {
        return vec![("(view body)".to_string(), false, true)];
    }
    comps
        .unwrap()
        .iter()
        .map(|(cid, rep)| (short(cid).to_string(), *rep, false))
        .collect()
}

fn emit_storyboard_ascii(g: &Graph, proj: &StoryboardProj, lanes: &[Lane]) -> String {
    let w = 80usize;
    let mut l: Vec<String> = Vec::new();

    l.push("=".repeat(w));
    l.push(center_str(
        &format!("STORYBOARD — {} screen flow", proj.title),
        w,
    ));
    l.push(center_str("(screen-first projection of the app-graph)", w));
    l.push("=".repeat(w));

    let navlabel = if proj.navs.is_empty() {
        "(none)".to_string()
    } else {
        proj.navs
            .iter()
            .map(|n| short(n))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    l.push(format!("nav containers (swimlane, not frames): {navlabel}"));
    l.push(format!(
        "schema {} v{}  ·  {} nodes / {} edges",
        proj.schema, proj.version, proj.n_nodes, proj.n_edges
    ));
    l.push(String::new());
    l.push(format!(
        "JOURNEY LANES (left -> right):  {}",
        lanes
            .iter()
            .enumerate()
            .map(|(i, ln)| format!("L{i} {}", ln.label))
            .collect::<Vec<_>>()
            .join("  ->  ")
    ));
    l.push(String::new());

    let frame_box = |screen: &str| -> Vec<String> {
        let iw = 30usize;
        let ft = proj.footers.get(screen).expect("footer must exist");
        let mut out: Vec<String> = vec![
            format!("  +{}+", "-".repeat(iw)),
            format!("  |{}|", ljust(&format!(" {}", short(screen)), iw)),
            format!("  |{}|", ljust(&format!("  [{}]", cat_of(screen)), iw)),
            format!("  +{}+", "-".repeat(iw)),
        ];
        for (label, rep, _generic) in sb_blocks(screen, &proj.renders) {
            if rep {
                out.push(format!("  |{}|", ljust(&format!("  : {label} xN :"), iw)));
            } else {
                out.push(format!("  |{}|", ljust(&format!("  [ {label} ]"), iw)));
            }
        }
        out.push(format!("  +{}+", "=".repeat(iw)));
        let bnd = {
            let mut s = ft.binds.join(", ");
            if ft.has_vs {
                if !s.is_empty() {
                    s += ", ";
                }
                s += "vs:*";
            }
            s = s.trim_matches(|c| c == ' ' || c == ',').to_string();
            if s.is_empty() { "-".to_string() } else { s }
        };
        let ws = if ft.writes.is_empty() {
            "-".to_string()
        } else {
            ft.writes
                .iter()
                .map(|(s, irr)| if *irr { format!("{s}!") } else { s.clone() })
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push(format!(
            "  |{}|",
            trunc_ljust(&format!(" binds: {bnd}"), iw)
        ));
        out.push(format!(
            "  |{}|",
            trunc_ljust(&format!(" writes: {ws}"), iw)
        ));
        out.push(format!("  +{}+", "-".repeat(iw)));
        out
    };

    for (i, ln) in lanes.iter().enumerate() {
        l.push("-".repeat(w));
        l.push(format!("LANE L{i}: {}", ln.label));
        l.push("-".repeat(w));
        if ln.screens.is_empty() {
            l.push("  (no screens)".to_string());
        }
        for s in &ln.screens {
            l.extend(frame_box(s));
            l.push(String::new());
        }
    }

    if !proj.boundary_nodes.is_empty() {
        l.push("-".repeat(w));
        l.push("LANE [boundary]: OS / external handoff surfaces".to_string());
        l.push("-".repeat(w));
        for b in &proj.boundary_nodes {
            let n = g.nodes.get(b.as_str());
            let kind_str = n.and_then(|n| n.kind.as_deref()).unwrap_or("");
            let outcome = n
                .and_then(|n| n.attrs.as_ref())
                .and_then(Value::as_object)
                .and_then(|a| a.get("outcome"));
            let iw = 30usize;
            l.push(format!("  +{}+", "~".repeat(iw)));
            l.push(format!(
                "  |{}|",
                trunc_ljust(&format!(" >> {}", short(b)), iw)
            ));
            l.push(format!(
                "  |{}|",
                trunc_ljust(&format!("  [{kind_str}]"), iw)
            ));
            if let Some(oc) = outcome {
                let oc_str = match oc {
                    Value::String(s) => s.clone(),
                    Value::Array(a) => a
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", "),
                    other => other.to_string(),
                };
                l.push(format!(
                    "  |{}|",
                    trunc_ljust(&format!("  outcome: {oc_str}"), iw)
                ));
            }
            l.push(format!("  +{}+", "~".repeat(iw)));
            l.push(String::new());
        }
    }

    l.push("=".repeat(w));
    l.push("NAVIGATION ARROWS (collapsed handles->fires->navigates)".to_string());
    l.push("=".repeat(w));
    l.push(format!(
        "  {:16} {:2} {:16} {:20} GUARD",
        "FROM", "->", "TO", "CONTROL/MODE"
    ));
    l.push(format!("  {}", "-".repeat(w - 4)));
    for (src, dst, ctrl, mode, guards) in &proj.nav_arrows {
        let ms = mode_short(mode.as_deref());
        let cm = if ms.is_empty() {
            ctrl.clone()
        } else {
            format!("{ctrl}/{ms}")
        };
        let lock = if guards.is_empty() {
            String::new()
        } else {
            format!("LOCK[{}]", guards.join(","))
        };
        l.push(format!(
            "  {:16} -> {:16} {:20} {lock}",
            short(src),
            short(dst),
            cm
        ));
    }
    l.push(String::new());
    l.push(
        "RETURN ARROWS (dismiss = thin return to presenter; returnsTo = boundary outcome-branch):"
            .to_string(),
    );
    for (frm, to, ctrl) in &proj.return_arrows {
        l.push(format!(
            "  {:16} ~> {:16} {} (dismiss)",
            short(frm),
            short(to),
            ctrl
        ));
    }
    for (frm, to, when, mode) in &proj.returnsto_arrows {
        let ms = mode_short(mode.as_deref());
        let tag = if when.is_empty() {
            "(returnsTo)".to_string()
        } else {
            format!("when={when}")
        };
        let modestr = if ms.is_empty() {
            String::new()
        } else {
            format!("/{ms}")
        };
        l.push(format!(
            "  {:16} ~> {:16} {tag}{modestr} (returnsTo)",
            short(frm),
            short(to)
        ));
    }
    l.push(String::new());

    let tail = &proj.tail;
    if !tail.is_empty() {
        let has_llm = tail.iter().any(|st| st.llm);
        // Generic review legend (F4): raised by a NON-CORE verification value,
        // never by the core {verified, unverified} tokens.
        let has_review = tail.iter().any(|st| {
            st.produces
                .iter()
                .any(|(_, _, v)| v.as_deref().is_some_and(is_review_verification))
        });
        let mut legend_bits = vec!["x-pipeline:feeds"];
        if has_llm {
            legend_bits.push("llm:true = *");
        }
        if has_review {
            legend_bits.push("! = review-required");
        }
        l.push("=".repeat(w));
        l.push(format!("PIPELINE TAIL ({})", legend_bits.join("; ")));
        l.push("=".repeat(w));
        let lane_str = tail
            .iter()
            .map(|st| {
                if st.llm {
                    format!("[{}*]", st.label)
                } else {
                    format!("[{}]", st.label)
                }
            })
            .collect::<Vec<_>>()
            .join(" -> ");
        l.push(format!("  {lane_str}"));
        l.push("  produces:".to_string());
        for st in tail {
            for (store, art, verif) in &st.produces {
                let cue = match verif.as_deref() {
                    Some(v) if is_review_verification(v) => "  [!REVIEW]".to_string(),
                    Some(v) => format!("  [{v}]"),
                    None => String::new(),
                };
                l.push(format!(
                    "    [{}] -> {:22} ({}){cue}",
                    st.label,
                    store,
                    art.as_deref().unwrap_or("?")
                ));
            }
        }
        l.push(String::new());
    }
    l.push("LEGEND:".to_string());
    l.push("  [ X ]    component block (renders edge)   : X xN :  repeat:true".to_string());
    l.push("  binds:   StateStores the frame reads-through   writes: mutations it owns (! irreversible)".to_string());
    l.push("  ->       navigates    ~>  dismiss/return    LOCK[..]  guard assertion".to_string());
    l.push("=".repeat(w));

    l.join("\n")
}

// ---------------------------------------------------------------------------
// Spine projection
// ---------------------------------------------------------------------------

struct SpineProj {
    consumers: Vec<String>,
    spines: HashMap<String, SpineData>,
    pipeline_order: Vec<String>,
    backend_order: Vec<String>,
    n_nodes: usize,
    n_edges: usize,
    schema: String,
    version: String,
    /// Any produces edge carries a NON-CORE (review-flagged) verification
    /// value — raises the generic `[!] = review-required` legend (F4).
    has_review: bool,
    // store consumer kinds for the column codes legend
    consumer_kinds: HashMap<String, String>,
}

struct SpineData {
    logic_stores: Vec<String>,
    view_states: Vec<String>,
    deep_stores: BTreeSet<String>,
    stages: BTreeSet<String>,
    backend_ops: BTreeSet<String>,
    verif_nodes: BTreeSet<String>,
    fan_in: usize,
    touches_floor: bool,
}

fn project_spine(g: &Graph) -> SpineProj {
    let by_type = edges_by_type(g);

    let mut binds_map: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in by_type
        .get("binds")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
        .iter()
        .chain(
            by_type
                .get("reads")
                .map(|v| v.as_slice())
                .unwrap_or(&[])
                .iter(),
        )
    {
        binds_map
            .entry(e.from().unwrap_or(""))
            .or_default()
            .push(e.to().unwrap_or(""));
    }

    let mut produced_by: HashMap<String, Vec<(String, Option<String>)>> = HashMap::new();
    for e in by_type.get("produces").map(|v| v.as_slice()).unwrap_or(&[]) {
        let verif = e
            .attrs
            .get("verification")
            .and_then(Value::as_str)
            .map(str::to_string);
        produced_by
            .entry(e.to().unwrap_or("").to_string())
            .or_default()
            .push((e.from().unwrap_or("").to_string(), verif));
    }

    let mut feeds_pred: HashMap<String, Vec<String>> = HashMap::new();
    for e in by_type
        .get("x-pipeline:feeds")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        feeds_pred
            .entry(e.to().unwrap_or("").to_string())
            .or_default()
            .push(e.from().unwrap_or("").to_string());
    }

    let mut invokes_pred: HashMap<String, Vec<String>> = HashMap::new();
    for e in by_type.get("invokes").map(|v| v.as_slice()).unwrap_or(&[]) {
        invokes_pred
            .entry(e.to().unwrap_or("").to_string())
            .or_default()
            .push(e.from().unwrap_or("").to_string());
    }

    let mut store_consumes: HashMap<String, Vec<String>> = HashMap::new();
    for e in by_type
        .get("x-pipeline:consumes")
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        store_consumes
            .entry(e.to().unwrap_or("").to_string())
            .or_default()
            .push(e.from().unwrap_or("").to_string());
    }

    fn walk_ops(
        op: &str,
        g: &Graph,
        ops: &mut BTreeSet<String>,
        invokes_pred: &HashMap<String, Vec<String>>,
        seen: &mut BTreeSet<String>,
    ) {
        if seen.contains(op) {
            return;
        }
        seen.insert(op.to_string());
        ops.insert(op.to_string());
        for src in invokes_pred.get(op).into_iter().flatten() {
            if g.kind(src) == Some("BackendOp") {
                walk_ops(src, g, ops, invokes_pred, seen);
            }
        }
    }

    fn dive_pipeline(
        stage: &str,
        g: &Graph,
        stages: &mut BTreeSet<String>,
        deep_stores: &mut BTreeSet<String>,
        ops: &mut BTreeSet<String>,
        seen: &mut BTreeSet<String>,
        produced_by: &HashMap<String, Vec<(String, Option<String>)>>,
        feeds_pred: &HashMap<String, Vec<String>>,
        invokes_pred: &HashMap<String, Vec<String>>,
        store_consumes: &HashMap<String, Vec<String>>,
    ) {
        if seen.contains(stage) {
            return;
        }
        seen.insert(stage.to_string());
        stages.insert(stage.to_string());
        for st in store_consumes.get(stage).into_iter().flatten() {
            deep_stores.insert(st.clone());
            for (p, _v) in produced_by.get(st).into_iter().flatten() {
                if g.kind(p) == Some("PipelineStage") {
                    dive_pipeline(
                        p,
                        g,
                        stages,
                        deep_stores,
                        ops,
                        seen,
                        produced_by,
                        feeds_pred,
                        invokes_pred,
                        store_consumes,
                    );
                }
            }
        }
        for up in feeds_pred.get(stage).into_iter().flatten() {
            dive_pipeline(
                up,
                g,
                stages,
                deep_stores,
                ops,
                seen,
                produced_by,
                feeds_pred,
                invokes_pred,
                store_consumes,
            );
        }
        for src in invokes_pred.get(stage).into_iter().flatten() {
            if g.kind(src) == Some("BackendOp") {
                walk_ops(src, g, ops, invokes_pred, &mut BTreeSet::new());
            }
        }
    }

    // Select consumers
    let all_consumers: Vec<String> = {
        let c: Vec<String> = g
            .nodes
            .iter()
            .filter(|(nid, n)| {
                let k = n.kind.as_deref();
                if k != Some("Screen") && k != Some("Component") {
                    return false;
                }
                let bound = binds_map.get(nid.as_str());
                let store_binds: usize = bound
                    .into_iter()
                    .flatten()
                    .filter(|&&t| g.kind(t) == Some("StateStore"))
                    .count();
                if k == Some("Component") && store_binds == 0 {
                    return false;
                }
                if k == Some("Screen") {
                    return bound.map(|b| !b.is_empty()).unwrap_or(false);
                }
                true
            })
            .map(|(id, _)| id.clone())
            .collect();

        let screens: Vec<String> = g
            .nodes
            .iter()
            .filter(|(_, n)| n.kind.as_deref() == Some("Screen"))
            .map(|(id, _)| id.clone())
            .collect();
        let lanes = derive_lanes(g, &screens);
        let mut rank: HashMap<String, usize> = HashMap::new();
        for (ci, ln) in lanes.iter().enumerate() {
            for s in &ln.screens {
                rank.entry(ns_of(s).to_string()).or_insert(ci);
            }
        }
        let fallback = rank.len() + 1;
        let mut sorted = c;
        sorted.sort_by(|a, b| {
            let ra = rank.get(cat_of(a)).copied().unwrap_or(fallback);
            let rb = rank.get(cat_of(b)).copied().unwrap_or(fallback);
            ra.cmp(&rb).then_with(|| a.cmp(b))
        });
        sorted
    };

    let mut spines: HashMap<String, SpineData> = HashMap::new();
    for c in &all_consumers {
        let mut logic_stores: Vec<String> = Vec::new();
        let mut view_states: Vec<String> = Vec::new();
        let mut deep_stores: BTreeSet<String> = BTreeSet::new();
        let mut stages: BTreeSet<String> = BTreeSet::new();
        let mut ops_set: BTreeSet<String> = BTreeSet::new();
        let mut verif_nodes: BTreeSet<String> = BTreeSet::new();

        for &tgt in binds_map.get(c.as_str()).into_iter().flatten() {
            let k = g.kind(tgt);
            if k == Some("ViewState") {
                if !view_states.contains(&tgt.to_string()) {
                    view_states.push(tgt.to_string());
                }
                continue;
            }
            if k != Some("StateStore") {
                continue;
            }
            if !logic_stores.contains(&tgt.to_string()) {
                logic_stores.push(tgt.to_string());
            }
            let producers = produced_by.get(tgt).cloned().unwrap_or_default();
            let stage_p: Vec<_> = producers
                .iter()
                .filter(|(p, _)| g.kind(p) == Some("PipelineStage"))
                .collect();
            let op_p: Vec<_> = producers
                .iter()
                .filter(|(p, _)| g.kind(p) == Some("BackendOp"))
                .collect();
            if !stage_p.is_empty() {
                deep_stores.insert(tgt.to_string());
                for (stage, verif) in &stage_p {
                    if verif.as_deref().is_some_and(is_review_verification) {
                        verif_nodes.insert(tgt.to_string());
                    }
                    dive_pipeline(
                        stage,
                        g,
                        &mut stages,
                        &mut deep_stores,
                        &mut ops_set,
                        &mut BTreeSet::new(),
                        &produced_by,
                        &feeds_pred,
                        &invokes_pred,
                        &store_consumes,
                    );
                }
            }
            if !op_p.is_empty() {
                deep_stores.insert(tgt.to_string());
                for (op, verif) in &op_p {
                    if verif.as_deref().is_some_and(is_review_verification) {
                        verif_nodes.insert(tgt.to_string());
                    }
                    walk_ops(op, g, &mut ops_set, &invokes_pred, &mut BTreeSet::new());
                }
            }
        }

        let all_stores_count: usize = {
            let mut s: BTreeSet<String> = logic_stores.iter().cloned().collect();
            for st in &deep_stores {
                if g.kind(st) == Some("StateStore") {
                    s.insert(st.clone());
                }
            }
            s.len()
        };

        spines.insert(
            c.clone(),
            SpineData {
                logic_stores,
                view_states,
                deep_stores,
                stages,
                backend_ops: ops_set,
                verif_nodes,
                fan_in: all_stores_count,
                touches_floor: !spines.get(c).map(|_| false).unwrap_or(false),
            },
        );
    }
    // Fix touches_floor (can't set it inline above due to borrow)
    for c in &all_consumers {
        let sp = spines.get_mut(c).unwrap();
        sp.touches_floor = !sp.backend_ops.is_empty();
    }

    let pipeline_order = pipeline_order_from_graph(g);
    let backend_order: Vec<String> = {
        let mut all_ops: BTreeSet<String> = BTreeSet::new();
        for c in &all_consumers {
            all_ops.extend(spines[c].backend_ops.iter().cloned());
        }
        all_ops.into_iter().collect()
    };

    let has_review = produced_by.values().any(|lst| {
        lst.iter()
            .any(|(_, v)| v.as_deref().is_some_and(is_review_verification))
    });
    let schema = g
        .doc
        .get("schema")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let version = g
        .doc
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let consumer_kinds: HashMap<String, String> = all_consumers
        .iter()
        .filter_map(|c| g.kind(c).map(|k| (c.clone(), k.to_string())))
        .collect();

    SpineProj {
        consumers: all_consumers,
        spines,
        pipeline_order,
        backend_order,
        n_nodes: g.nodes.len(),
        n_edges: g.edges.len(),
        schema,
        version,
        has_review,
        consumer_kinds,
    }
}

// ---------------------------------------------------------------------------
// Spine ASCII emission
// ---------------------------------------------------------------------------

fn emit_spine_ascii(g: &Graph, proj: &SpineProj) -> String {
    let consumers = &proj.consumers;
    let spines = &proj.spines;
    let pipeline_order = &proj.pipeline_order;
    let backend_order = &proj.backend_order;
    let has_review = proj.has_review;

    let intro_tail = if has_review {
        "BackendOps (floor). Spine length = pull depth. [!] = review-required."
    } else {
        "BackendOps (floor). Spine length = pull depth."
    };

    let mut lines: Vec<String> = vec![
        "BACKEND PULL SPINE — STALACTITE VIEW  (downward data-pull only)".to_string(),
        "=".repeat(80),
        "Each column = one consumer surface. Read DOWN: surface chip -> bound".to_string(),
        "stores/viewstates (LOGIC) -> produced stores -> pipeline stages ->".to_string(),
        intro_tail.to_string(),
        format!(
            "schema {} v{}  ·  {} nodes / {} edges",
            proj.schema, proj.version, proj.n_nodes, proj.n_edges
        ),
        String::new(),
    ];

    lines.push("PER-COLUMN PULL SUMMARY".to_string());
    lines.push("-".repeat(80));
    lines.push(format!(
        "{:24} {:6} {:5} {:5}  ticks",
        "surface", "depth", "fanin", "floor"
    ));
    for c in consumers {
        let sp = &spines[c];
        let depth = sp.logic_stores.len()
            + sp.view_states.len()
            + sp.deep_stores.len()
            + sp.stages.len()
            + sp.backend_ops.len();
        let floor = if sp.touches_floor { "YES" } else { "." };
        let verif = if sp.verif_nodes.is_empty() { " " } else { "!" };
        lines.push(format!(
            "{:24} {:<6} {:<5} {:<5}{} {}",
            short(c),
            depth,
            sp.fan_in,
            floor,
            verif,
            "#".repeat(sp.fan_in.min(6))
        ));
    }
    lines.push(String::new());

    lines.push("STALACTITE GRID  (shared substrate rows; | = spine, o = tick,".to_string());
    if has_review {
        lines.push("                  # = stage/op tick, ! = review-required notch)".to_string());
    } else {
        lines.push("                  # = stage/op tick)".to_string());
    }
    lines.push("-".repeat(80));

    let cw = 3usize;
    // Only StateStore nodes in deep_stores
    let deep_store_ids: Vec<String> = {
        let mut s: BTreeSet<String> = BTreeSet::new();
        for c in consumers {
            for st in &spines[c].deep_stores {
                if g.kind(st) == Some("StateStore") {
                    s.insert(st.clone());
                }
            }
        }
        s.into_iter().collect()
    };

    let rows: Vec<(String, String, String)> = deep_store_ids
        .iter()
        .map(|s| ("STORE".to_string(), short(s).to_string(), s.clone()))
        .chain(pipeline_order.iter().map(|s| {
            (
                "STAGE".to_string(),
                format!("stage:{}", short(s)),
                s.clone(),
            )
        }))
        .chain(
            backend_order
                .iter()
                .map(|o| ("OP".to_string(), format!("op:{}", short(o)), o.clone())),
        )
        .collect();

    fn make_code(nid: &str) -> String {
        let s = short(nid);
        let ups: Vec<char> = s.chars().filter(|c| c.is_uppercase()).collect();
        if ups.len() >= 2 {
            format!("{}{}", ups[0], ups[1]).to_uppercase()
        } else {
            s[..s.len().min(2)].to_uppercase()
        }
    }

    let mut codes: Vec<String> = consumers.iter().map(|c| make_code(c)).collect();
    let mut seen_codes: HashMap<String, usize> = HashMap::new();
    for code in &mut codes {
        let cd = code.clone();
        if let Some(cnt) = seen_codes.get_mut(&cd) {
            *code = format!("{}{}", &cd[..1], cnt);
            *cnt += 1;
        } else {
            seen_codes.insert(cd, 1);
        }
    }

    let label_w = 20usize;
    lines.push(format!(
        "{}|{}",
        " ".repeat(label_w),
        codes
            .iter()
            .map(|cd| format!("{cd:^cw$}"))
            .collect::<Vec<_>>()
            .join("")
    ));
    lines.push(format!(
        "{}+{}",
        " ".repeat(label_w),
        "-".repeat(cw * consumers.len())
    ));

    // Build grid rows
    let mut grid_rows: Vec<(String, Vec<GridCell>)> = Vec::new();

    // SURFACE row
    grid_rows.push((
        "SURFACE".to_string(),
        consumers.iter().map(|_| GridCell::Surface).collect(),
    ));

    let max_logic = consumers
        .iter()
        .map(|c| spines[c].logic_stores.len() + spines[c].view_states.len())
        .max()
        .unwrap_or(0)
        .min(4);
    for r in 0..max_logic {
        let cells: Vec<GridCell> = consumers
            .iter()
            .map(|c| {
                let sp = &spines[c];
                if r < sp.logic_stores.len() + sp.view_states.len() {
                    GridCell::Char('o')
                } else {
                    GridCell::Empty
                }
            })
            .collect();
        grid_rows.push((format!("  bind[{r}]"), cells));
    }

    for (lane, label, node) in &rows {
        let cells: Vec<GridCell> = consumers
            .iter()
            .map(|c| {
                let sp = &spines[c];
                match lane.as_str() {
                    "STORE" => {
                        if sp.deep_stores.contains(node) {
                            if sp.verif_nodes.contains(node) {
                                GridCell::Char('!')
                            } else {
                                GridCell::Char('o')
                            }
                        } else {
                            GridCell::Empty
                        }
                    }
                    "STAGE" => {
                        if sp.stages.contains(node) {
                            GridCell::Char('#')
                        } else {
                            GridCell::Empty
                        }
                    }
                    _ => {
                        if sp.backend_ops.contains(node) {
                            GridCell::Char('#')
                        } else {
                            GridCell::Empty
                        }
                    }
                }
            })
            .collect();
        let row_label = if lane == "STORE" {
            format!("  {label}")
        } else {
            label.clone()
        };
        grid_rows.push((row_label, cells));
    }

    let mut last_active: Vec<usize> = vec![0; consumers.len()];
    for (ri, (_, cells)) in grid_rows.iter().enumerate() {
        for (ci, ch) in cells.iter().enumerate() {
            if !matches!(ch, GridCell::Empty) {
                last_active[ci] = ri;
            }
        }
    }

    for (ri, (lab, cells)) in grid_rows.iter().enumerate() {
        let rowstr = format!(
            "{:<label_w$}|{}",
            lab,
            cells
                .iter()
                .enumerate()
                .map(|(ci, ch)| {
                    let s = match ch {
                        GridCell::Surface => "[]".to_string(),
                        GridCell::Char(c) => c.to_string(),
                        GridCell::Empty => {
                            if ri < last_active[ci] {
                                "|".to_string()
                            } else {
                                " ".to_string()
                            }
                        }
                    };
                    format!("{s:^cw$}")
                })
                .collect::<Vec<_>>()
                .join("")
        );
        lines.push(rowstr);
    }
    lines.push(format!(
        "{}+{}  <- floor (BackendOps)",
        " ".repeat(label_w),
        "=".repeat(cw * consumers.len())
    ));
    lines.push(String::new());

    lines.push("COLUMN CODES".to_string());
    lines.push("-".repeat(80));
    for (cd, c) in codes.iter().zip(consumers.iter()) {
        let kind = proj.consumer_kinds.get(c).map(String::as_str).unwrap_or("");
        lines.push(format!("  {:4} = {:22} ({kind})", cd, short(c)));
    }
    lines.push(String::new());

    lines.push("SHARED SUBSTRATE NODES (convergence — multiple spines land here)".to_string());
    lines.push("-".repeat(80));

    let mut reach: BTreeMap<(String, String), usize> = BTreeMap::new();
    for c in consumers {
        let sp = &spines[c];
        for s in &sp.deep_stores {
            *reach.entry(("store".to_string(), s.clone())).or_default() += 1;
        }
        for s in &sp.stages {
            *reach.entry(("stage".to_string(), s.clone())).or_default() += 1;
        }
        for o in &sp.backend_ops {
            *reach.entry(("op".to_string(), o.clone())).or_default() += 1;
        }
    }
    let mut reach_sorted: Vec<((String, String), usize)> = reach.into_iter().collect();
    // Sort by (-count, node_id) ascending — matches oracle's `key=lambda kv: (-kv[1], kv[0][1])`.
    // Descending count, then ascending node_id (not tag+node).
    reach_sorted.sort_by(|((_, na), ca), ((_, nb), cb)| cb.cmp(ca).then_with(|| na.cmp(nb)));
    for ((tag, node), cnt) in &reach_sorted {
        if *cnt >= 2 {
            lines.push(format!("  {cnt}x  {:5} {}", tag, short(node)));
        }
    }

    lines.join("\n")
}

#[derive(Debug)]
enum GridCell {
    Surface,
    Char(char),
    Empty,
}
