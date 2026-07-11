//! `maapp diff` — semantic set-diff of two graph documents.
//!
//! LIFECYCLE-VERBS-SPEC §1 (shared contracts) + §2. A pure function of the two
//! loaded documents: order-insensitive, whitespace-insensitive, repo-independent
//! (this module never touches the filesystem — wasm-clean).
//!
//! Contracts:
//! - Node identity = slug; edge identity = the `(type, from, to)` triple. Two
//!   edges sharing an identity in ONE document are an input error
//!   (`EngineError::DuplicateEdgeIdentity`) — detected, never silently merged.
//! - A node present in both documents with a changed `kind` is `nodes_changed`
//!   (slug identity wins), never remove+add.
//! - Output shape is stable: every collection key is always present;
//!   `meta_changed`/`registry_changed` are `null` when unchanged.
//! - Every emitted list/map has a total order (`BTreeMap` keys; edges sorted by
//!   `(type, from, to)`), and every embedded JSON value is canonicalized
//!   (object keys sorted recursively) so the `--json` bytes are independent of
//!   on-disk key order — the byte-stability contract.

use crate::error::EngineError;
use crate::graph::Graph;
use crate::model::{Edge, Node};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// A single field-level change: the old and new values (`null` = absent).
#[derive(Debug, Serialize)]
pub struct FieldChange {
    pub from: Value,
    pub to: Value,
}

/// A full edge object as emitted in `edges_added` / `edges_removed`:
/// the identity triple first, then all attrs in sorted key order.
#[derive(Debug, Serialize)]
pub struct DiffEdge {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(flatten)]
    pub attrs: BTreeMap<String, Value>,
}

impl From<&Edge> for DiffEdge {
    fn from(e: &Edge) -> Self {
        DiffEdge {
            r#type: e.r#type.clone(),
            from: e.from.clone(),
            to: e.to.clone(),
            attrs: e.attrs.iter().map(|(k, v)| (k.clone(), canon(v))).collect(),
        }
    }
}

/// The `(type, from, to)` identity of a changed edge.
#[derive(Debug, Serialize)]
pub struct EdgeIdentity {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// An edge present in both documents whose attrs differ.
#[derive(Debug, Serialize)]
pub struct EdgeChange {
    pub identity: EdgeIdentity,
    pub attrs: BTreeMap<String, FieldChange>,
}

/// The `maapp diff --json` report — exactly the spec §2 shape, keys in spec
/// order. Empty collections are present (stable shape); `meta_changed` /
/// `registry_changed` serialize as `null` when nothing changed.
#[derive(Debug, Serialize)]
pub struct DiffReport {
    pub nodes_added: Vec<String>,
    pub nodes_removed: Vec<String>,
    pub nodes_changed: BTreeMap<String, BTreeMap<String, FieldChange>>,
    pub edges_added: Vec<DiffEdge>,
    pub edges_removed: Vec<DiffEdge>,
    pub edges_changed: Vec<EdgeChange>,
    pub meta_changed: Option<BTreeMap<String, FieldChange>>,
    pub registry_changed: Option<BTreeMap<String, FieldChange>>,
}

impl DiffReport {
    /// True iff the two documents are semantically identical (drives exit 0).
    pub fn is_empty(&self) -> bool {
        self.nodes_added.is_empty()
            && self.nodes_removed.is_empty()
            && self.nodes_changed.is_empty()
            && self.edges_added.is_empty()
            && self.edges_removed.is_empty()
            && self.edges_changed.is_empty()
            && self.meta_changed.is_none()
            && self.registry_changed.is_none()
    }

    /// Human-readable rendering: the same content as the `--json` report, one
    /// line per delta (`+ node …` / `- node …` / `~ node … <field>` /
    /// `+ edge …` / `~ edge … seq: 0 → 1` / `~ meta …` / `~ registry …`).
    pub fn render(&self) -> String {
        if self.is_empty() {
            return "no differences".to_string();
        }
        let mut lines: Vec<String> = Vec::new();
        for slug in &self.nodes_added {
            lines.push(format!("+ node {slug}"));
        }
        for slug in &self.nodes_removed {
            lines.push(format!("- node {slug}"));
        }
        for (slug, fields) in &self.nodes_changed {
            for field in fields.keys() {
                lines.push(format!("~ node {slug} {field}"));
            }
        }
        for e in &self.edges_added {
            lines.push(format!("+ edge {}", diff_eid(&e.r#type, &e.from, &e.to)));
        }
        for e in &self.edges_removed {
            lines.push(format!("- edge {}", diff_eid(&e.r#type, &e.from, &e.to)));
        }
        for c in &self.edges_changed {
            let eid = diff_eid(&c.identity.r#type, &c.identity.from, &c.identity.to);
            for (attr, ch) in &c.attrs {
                lines.push(format!(
                    "~ edge {eid} {attr}: {} → {}",
                    render_value(&ch.from),
                    render_value(&ch.to)
                ));
            }
        }
        if let Some(meta) = &self.meta_changed {
            for key in meta.keys() {
                lines.push(format!("~ meta {key}"));
            }
        }
        if let Some(reg) = &self.registry_changed {
            for key in reg.keys() {
                lines.push(format!("~ registry {key}"));
            }
        }
        lines.join("\n")
    }
}

/// Compute the semantic diff old → new (LIFECYCLE-VERBS-SPEC §2).
///
/// Errors with `EngineError::DuplicateEdgeIdentity` when either document holds
/// two edges with the same `(type, from, to)` (input error, CLI exit 2).
pub fn diff_graphs(old: &Graph, new: &Graph) -> Result<DiffReport, EngineError> {
    let old_edges = edge_index(old, "old")?;
    let new_edges = edge_index(new, "new")?;

    // --- nodes (BTreeMap keys are already slug-sorted) ---
    let mut nodes_added: Vec<String> = Vec::new();
    let mut nodes_removed: Vec<String> = Vec::new();
    let mut nodes_changed: BTreeMap<String, BTreeMap<String, FieldChange>> = BTreeMap::new();

    for slug in new.nodes.keys() {
        if !old.nodes.contains_key(slug) {
            nodes_added.push(slug.clone());
        }
    }
    for (slug, old_node) in &old.nodes {
        let Some(new_node) = new.nodes.get(slug) else {
            nodes_removed.push(slug.clone());
            continue;
        };
        let changes = node_field_changes(
            old_node,
            new_node,
            old.node_layer.get(slug),
            new.node_layer.get(slug),
        );
        if !changes.is_empty() {
            nodes_changed.insert(slug.clone(), changes);
        }
    }

    // --- edges (identity maps are (type,from,to)-sorted BTreeMaps) ---
    let mut edges_added: Vec<DiffEdge> = Vec::new();
    let mut edges_removed: Vec<DiffEdge> = Vec::new();
    let mut edges_changed: Vec<EdgeChange> = Vec::new();

    for (key, edge) in &new_edges {
        if !old_edges.contains_key(key) {
            edges_added.push(DiffEdge::from(*edge));
        }
    }
    for (key, old_edge) in &old_edges {
        let Some(new_edge) = new_edges.get(key) else {
            edges_removed.push(DiffEdge::from(*old_edge));
            continue;
        };
        let attrs = attr_changes(&old_edge.attrs, &new_edge.attrs);
        if !attrs.is_empty() {
            edges_changed.push(EdgeChange {
                identity: EdgeIdentity {
                    r#type: old_edge.r#type.clone(),
                    from: old_edge.from.clone(),
                    to: old_edge.to.clone(),
                },
                attrs,
            });
        }
    }

    // --- meta (per top-level key; provenance bumps surface here, not buried) ---
    let meta_changed = keyed_object_diff(old.doc.get("meta"), new.doc.get("meta"), "");

    // --- registries (all four, keyed "<registry>.<entry>") ---
    let mut registry_changed: BTreeMap<String, FieldChange> = BTreeMap::new();
    for reg in [
        "nodeKindRegistry",
        "edgeRegistry",
        "refsRegistry",
        "attrEnumRegistry",
    ] {
        if let Some(map) = keyed_object_diff(old.doc.get(reg), new.doc.get(reg), reg) {
            registry_changed.extend(map);
        }
    }

    Ok(DiffReport {
        nodes_added,
        nodes_removed,
        nodes_changed,
        edges_added,
        edges_removed,
        edges_changed,
        meta_changed,
        registry_changed: if registry_changed.is_empty() {
            None
        } else {
            Some(registry_changed)
        },
    })
}

/// Index a graph's edges by `(type, from, to)` identity, erroring on duplicates
/// (spec §1: detect, never silently merge). The `BTreeMap` key IS the canonical
/// `(type, from, to)` emit order; `None` components sort first.
type EdgeKey = (Option<String>, Option<String>, Option<String>);

fn edge_index<'g>(
    g: &'g Graph,
    role: &'static str,
) -> Result<BTreeMap<EdgeKey, &'g Edge>, EngineError> {
    let mut index: BTreeMap<EdgeKey, &Edge> = BTreeMap::new();
    for e in &g.edges {
        let key = (e.r#type.clone(), e.from.clone(), e.to.clone());
        if index.insert(key, e).is_some() {
            return Err(EngineError::DuplicateEdgeIdentity { role, eid: e.eid() });
        }
    }
    Ok(index)
}

/// Field-level changes between two versions of the same node.
///
/// Reported fields (spec §2): `kind`, `intent`, `refs.*`, `attrs.*`, plus
/// `layer` (the node moved section — semantic, unlike physical position) and
/// any author-supplied extra keys. `refs`/`attrs` diff per sub-key when both
/// sides are objects-or-absent; a present non-object degrades to a whole-field
/// change under the bare field name.
fn node_field_changes(
    old: &Node,
    new: &Node,
    old_layer: Option<&String>,
    new_layer: Option<&String>,
) -> BTreeMap<String, FieldChange> {
    let mut out: BTreeMap<String, FieldChange> = BTreeMap::new();

    if old.kind != new.kind {
        out.insert(
            "kind".to_string(),
            FieldChange {
                from: opt_str(&old.kind),
                to: opt_str(&new.kind),
            },
        );
    }
    if old.intent != new.intent {
        out.insert(
            "intent".to_string(),
            FieldChange {
                from: opt_str(&old.intent),
                to: opt_str(&new.intent),
            },
        );
    }
    if old_layer != new_layer {
        out.insert(
            "layer".to_string(),
            FieldChange {
                from: old_layer.map_or(Value::Null, |l| Value::String(l.clone())),
                to: new_layer.map_or(Value::Null, |l| Value::String(l.clone())),
            },
        );
    }
    sub_field_changes("refs", old.refs.as_ref(), new.refs.as_ref(), &mut out);
    sub_field_changes("attrs", old.attrs.as_ref(), new.attrs.as_ref(), &mut out);

    // Extra author-supplied top-level keys, per key.
    let keys: std::collections::BTreeSet<&String> =
        old.extra.keys().chain(new.extra.keys()).collect();
    for k in keys {
        let f = old.extra.get(k);
        let t = new.extra.get(k);
        if f != t {
            out.insert(
                k.clone(),
                FieldChange {
                    from: f.map_or(Value::Null, canon),
                    to: t.map_or(Value::Null, canon),
                },
            );
        }
    }
    out
}

/// Diff a node's `refs`/`attrs` sub-object per key (`"<field>.<key>"`). When a
/// present side is not an object, fall back to one whole-field change under
/// the bare field name (never suppress a difference).
fn sub_field_changes(
    field: &str,
    old: Option<&Value>,
    new: Option<&Value>,
    out: &mut BTreeMap<String, FieldChange>,
) {
    let old_v = old.unwrap_or(&Value::Null);
    let new_v = new.unwrap_or(&Value::Null);
    if old_v == new_v {
        return;
    }
    let both_objectish =
        (old_v.is_object() || old_v.is_null()) && (new_v.is_object() || new_v.is_null());
    if !both_objectish {
        out.insert(
            field.to_string(),
            FieldChange {
                from: canon(old_v),
                to: canon(new_v),
            },
        );
        return;
    }
    let empty = serde_json::Map::new();
    let old_map = old_v.as_object().unwrap_or(&empty);
    let new_map = new_v.as_object().unwrap_or(&empty);
    let keys: std::collections::BTreeSet<&String> = old_map.keys().chain(new_map.keys()).collect();
    for k in keys {
        let f = old_map.get(k);
        let t = new_map.get(k);
        if f != t {
            out.insert(
                format!("{field}.{k}"),
                FieldChange {
                    from: f.map_or(Value::Null, canon),
                    to: t.map_or(Value::Null, canon),
                },
            );
        }
    }
}

/// Per-attr changes between two versions of the same edge.
fn attr_changes(
    old: &BTreeMap<String, Value>,
    new: &BTreeMap<String, Value>,
) -> BTreeMap<String, FieldChange> {
    let mut out = BTreeMap::new();
    let keys: std::collections::BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    for k in keys {
        let f = old.get(k);
        let t = new.get(k);
        if f != t {
            out.insert(
                k.clone(),
                FieldChange {
                    from: f.map_or(Value::Null, canon),
                    to: t.map_or(Value::Null, canon),
                },
            );
        }
    }
    out
}

/// Per-key diff of a top-level object (meta or a registry). `prefix` (e.g.
/// `"edgeRegistry"`) namespaces the keys; empty prefix emits bare keys.
/// Returns `None` when the two values are equal (absent counts as `null`).
/// A present non-object degrades to one whole-value change under the prefix
/// (or `""` for meta) — degenerate but never suppressed.
fn keyed_object_diff(
    old: Option<&Value>,
    new: Option<&Value>,
    prefix: &str,
) -> Option<BTreeMap<String, FieldChange>> {
    let old_v = old.unwrap_or(&Value::Null);
    let new_v = new.unwrap_or(&Value::Null);
    if old_v == new_v {
        return None;
    }
    let mut out: BTreeMap<String, FieldChange> = BTreeMap::new();
    let both_objectish =
        (old_v.is_object() || old_v.is_null()) && (new_v.is_object() || new_v.is_null());
    if !both_objectish {
        out.insert(
            prefix.to_string(),
            FieldChange {
                from: canon(old_v),
                to: canon(new_v),
            },
        );
        return Some(out);
    }
    let empty = serde_json::Map::new();
    let old_map = old_v.as_object().unwrap_or(&empty);
    let new_map = new_v.as_object().unwrap_or(&empty);
    let keys: std::collections::BTreeSet<&String> = old_map.keys().chain(new_map.keys()).collect();
    for k in keys {
        let f = old_map.get(k);
        let t = new_map.get(k);
        if f != t {
            let key = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            out.insert(
                key,
                FieldChange {
                    from: f.map_or(Value::Null, canon),
                    to: t.map_or(Value::Null, canon),
                },
            );
        }
    }
    Some(out)
}

/// Canonicalize a JSON value for emission: recursively sort object keys.
/// `serde_json` runs with `preserve_order`, so parsed objects carry on-disk
/// key order; re-inserting in sorted order makes the diff output independent
/// of how either input file was formatted (the order-invariance contract).
fn canon(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), canon(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canon).collect()),
        other => other.clone(),
    }
}

fn opt_str(v: &Option<String>) -> Value {
    v.as_ref().map_or(Value::Null, |s| Value::String(s.clone()))
}

/// `{type}:{from}->{to}` display id for human diff lines (missing → `None`,
/// matching `Edge::eid`).
fn diff_eid(t: &Option<String>, f: &Option<String>, to: &Option<String>) -> String {
    format!(
        "{}:{}->{}",
        t.as_deref().unwrap_or("None"),
        f.as_deref().unwrap_or("None"),
        to.as_deref().unwrap_or("None"),
    )
}

/// Compact-JSON rendering of an attr value for `~ edge … x: from → to` lines
/// (numbers/bools bare, strings quoted, absent as `null`).
fn render_value(v: &Value) -> String {
    v.to_string()
}
