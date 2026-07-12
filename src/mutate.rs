//! Mutation verbs (LIFECYCLE-VERBS-SPEC §4, P1): `add-node` / `update-node` /
//! `remove-node` / `add-edge` / `remove-edge`.
//!
//! **Motivating episode (F5):** one logical change = 3–6 hand Edit anchors,
//! each needing a prior region-read + placement guesses at unwritten narrative
//! conventions. Here one logical delta is one command that cannot produce an
//! invalid or non-canonical document.
//!
//! ## The atomic validated write (every verb)
//!
//! parse → apply → run the FULL validator in-memory → write (temp + rename)
//! ONLY if the hard-error count did not increase; otherwise return the
//! would-be `E_*` findings ([`EngineError::ValidationRegression`], CLI exit 2)
//! leaving the file byte-untouched. `add-edge` therefore enforces the
//! signature table (endpoint kinds + required attrs) at add time for free —
//! the same checks validate would raise, surfaced immediately.
//!
//! ## Canonical emission
//!
//! Every write goes through [`crate::export::canonical_doc`] (spec §1): edges
//! sorted `(type, from, to)`, slugs sorted within layer, layers in canonical
//! order, pretty two-space JSON + one trailing newline (the `stamp`
//! convention). The first mutation of a narratively-ordered file canonicalizes
//! it wholesale — expected and documented; `diff` is order-insensitive, so the
//! churn is semantically invisible.
//!
//! ## Scope discipline (spec §4 must-not)
//!
//! No file other than the named graph is touched; the source repo is never
//! read; counterpart nodes/edges are never inferred or auto-created ("smart"
//! completion is refused with a message — the agent owns modeling decisions).
//!
//! ## §6.3: `--as-of <sha>`
//!
//! Every verb accepts `--as-of` to bump `meta.provenance.asOf` in the SAME
//! atomic write. Like `stamp`, it requires an existing `meta.provenance`
//! object (`origin` is authored data the engine must never invent).
//!
//! Native-only: these verbs rewrite the graph file on disk, so the module is
//! cfg-gated off wasm32 like `drift` (the pure canonical projection they emit
//! through lives wasm-clean in `crate::export`).

use crate::error::EngineError;
use crate::export::canonical_doc;
use crate::graph::{Graph, core_node_kinds};
use crate::model::Node;
use crate::validate::validate;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::Path;

/// What a successful mutation did, for the CLI to print.
#[derive(Debug)]
pub struct MutationOutcome {
    /// One human line (plus cascade detail lines) describing the delta.
    pub summary: String,
    /// The `asOf` value stamped in the same write, when `--as-of` was given.
    pub stamped: Option<String>,
}

/// How a batch mutation (`update-node` / `remove-node`, T3b) names its targets:
/// an explicit slug list, or a `--where key=value` filter. A single slug is
/// simply `Slugs(&[slug])` — the single-slug behavior is untouched.
#[derive(Debug, Clone, Copy)]
pub enum NodeSelector<'a> {
    /// One or more explicit slugs (deduped in order; EVERY slug must exist, or
    /// the whole batch fails naming the first unknown — all-or-nothing).
    Slugs(&'a [String]),
    /// `--where key=value`: match nodes on `kind`, `refs.<k>`, or `attrs.<k>`
    /// (string-compared). Matching nothing is a usage error, never a no-op.
    Where { key: &'a str, value: &'a str },
}

/// The human display of a selector (used in the no-op / audit messages). One
/// slug renders as the bare slug, so the single-slug messages are unchanged.
fn selector_display(selector: &NodeSelector) -> String {
    match selector {
        NodeSelector::Slugs(slugs) => slugs.join(", "),
        NodeSelector::Where { key, value } => format!("--where {key}={value}"),
    }
}

/// Parse one `--ref`/`--attr` flag value in `K=V` form. `V` parses as JSON
/// when it can (`true` → boolean, `1` → number, `[...]` → array) and falls
/// back to a plain string otherwise — so `--attr seq=1` is the number 1 and
/// `--attr mode=set` is the string "set". Deterministic either way.
pub fn parse_kv(raw: &str) -> Result<(String, Value), EngineError> {
    let Some((k, v)) = raw.split_once('=') else {
        return Err(EngineError::BadKeyValue(raw.to_string()));
    };
    if k.is_empty() {
        return Err(EngineError::BadKeyValue(raw.to_string()));
    }
    let value = serde_json::from_str::<Value>(v).unwrap_or_else(|_| Value::String(v.to_string()));
    Ok((k.to_string(), value))
}

// ---------------------------------------------------------------------------
// verbs
// ---------------------------------------------------------------------------

/// `maapp add-node <slug> <graph> --kind K --intent I [--ref k=v]... [--attr k=v]...`
///
/// Layer placement is derived from `kind` (frozen core, else the registry
/// row's `layer`) — the agent never names a layer. The new node always
/// carries a `refs` block (`{}` when no `--ref` given), per the uniform-shape
/// advisory.
pub fn add_node(
    path: &Path,
    slug: &str,
    kind: &str,
    intent: &str,
    refs: &[(String, Value)],
    attrs: &[(String, Value)],
    as_of: Option<&str>,
) -> Result<MutationOutcome, EngineError> {
    let g = crate::load_graph_from_path(path)?;
    if g.nodes.contains_key(slug) {
        return Err(EngineError::NodeExists(slug.to_string()));
    }
    let layer = core_node_kinds()
        .get(kind)
        .map(|l| (*l).to_string())
        .or_else(|| {
            g.node_registry
                .get(kind)?
                .get("layer")?
                .as_str()
                .map(str::to_string)
        })
        .ok_or_else(|| EngineError::UnknownKindNoLayer(kind.to_string()))?;
    let before = hard_count(&g);

    let mut node = Map::new();
    node.insert("kind".to_string(), Value::String(kind.to_string()));
    node.insert("intent".to_string(), Value::String(intent.to_string()));
    node.insert("refs".to_string(), pairs_object(refs));
    if !attrs.is_empty() {
        node.insert("attrs".to_string(), pairs_object(attrs));
    }

    let mut doc = g.doc;
    let root = doc
        .as_object_mut()
        .ok_or(EngineError::MalformedDocument("top level is not an object"))?;
    let nodes = root
        .entry("nodes")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or(EngineError::MalformedDocument("'nodes' is not an object"))?;
    let layer_map = nodes
        .entry(layer.clone())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or(EngineError::MalformedDocument("layer map is not an object"))?;
    layer_map.insert(slug.to_string(), Value::Object(node));

    let stamped = finish(path, before, doc, as_of)?;
    Ok(MutationOutcome {
        summary: format!("ADDED node {slug} ({layer})"),
        stamped,
    })
}

/// `maapp update-node <slug> <graph> [--intent I] [--ref k=v]... [--attr k=v]...`
///
/// Single-slug convenience over [`update_nodes`]: no flags =
/// [`EngineError::NoOpUpdate`] (exit 2, a no-op is an error), `--ref`/`--attr`
/// pairs merge into the existing blocks (creating them when absent).
pub fn update_node(
    path: &Path,
    slug: &str,
    intent: Option<&str>,
    refs: &[(String, Value)],
    attrs: &[(String, Value)],
    as_of: Option<&str>,
) -> Result<MutationOutcome, EngineError> {
    let slugs = [slug.to_string()];
    update_nodes(
        path,
        NodeSelector::Slugs(&slugs),
        intent,
        refs,
        attrs,
        as_of,
    )
}

/// `maapp update-node <slug>... | --where k=v <graph> [--intent I] [--ref ...] [--attr ...]`
///
/// Batch update (T3b): apply the SAME intent/refs/attrs delta to every selected
/// node in ONE atomic validated write. No flags = [`EngineError::NoOpUpdate`]
/// (a no-op is an error). An unknown slug (Slugs form) or an empty match (Where
/// form) fails the WHOLE batch before any write — all-or-nothing.
pub fn update_nodes(
    path: &Path,
    selector: NodeSelector<'_>,
    intent: Option<&str>,
    refs: &[(String, Value)],
    attrs: &[(String, Value)],
    as_of: Option<&str>,
) -> Result<MutationOutcome, EngineError> {
    if intent.is_none() && refs.is_empty() && attrs.is_empty() {
        return Err(EngineError::NoOpUpdate(selector_display(&selector)));
    }
    let g = crate::load_graph_from_path(path)?;
    let targets = resolve_targets(&g, &selector)?;
    // Capture (slug, layer) before moving the doc out of the graph.
    let target_layers: Vec<(String, String)> = targets
        .iter()
        .map(|s| (s.clone(), g.node_layer.get(s).cloned().unwrap_or_default()))
        .collect();
    let before = hard_count(&g);

    let mut doc = g.doc;
    let mut summaries: Vec<String> = Vec::new();
    for (slug, layer) in &target_layers {
        let node = doc
            .get_mut("nodes")
            .and_then(|n| n.get_mut(layer))
            .and_then(|l| l.get_mut(slug))
            .and_then(Value::as_object_mut)
            .ok_or(EngineError::MalformedDocument("node body is not an object"))?;
        let mut changed: Vec<String> = Vec::new();
        if let Some(text) = intent {
            node.insert("intent".to_string(), Value::String(text.to_string()));
            changed.push("intent".to_string());
        }
        merge_pairs(node, "refs", refs, &mut changed)?;
        merge_pairs(node, "attrs", attrs, &mut changed)?;
        summaries.push(format!("UPDATED node {slug} ({})", changed.join(", ")));
    }

    let stamped = finish(path, before, doc, as_of)?;
    Ok(MutationOutcome {
        summary: summaries.join("\n"),
        stamped,
    })
}

/// `maapp remove-node <slug> <graph> [--cascade]`
///
/// Single-slug convenience over [`remove_nodes`]: with incident edges and no
/// `--cascade` it refuses, listing them (spec §4); with `--cascade` it removes
/// the node + its incident edges and reports exactly what was removed.
pub fn remove_node(
    path: &Path,
    slug: &str,
    cascade: bool,
    as_of: Option<&str>,
) -> Result<MutationOutcome, EngineError> {
    let slugs = [slug.to_string()];
    remove_nodes(path, NodeSelector::Slugs(&slugs), cascade, as_of)
}

/// `maapp remove-node <slug>... | --where k=v <graph> [--cascade]`
///
/// Batch remove (T3b): remove every selected node in ONE atomic validated
/// write. All-or-nothing — an unknown slug / empty match fails before any
/// write, and without `--cascade` the FIRST selected node that still has
/// incident edges refuses the whole batch (naming it + its edges, exit 2). With
/// `--cascade` the union of all incident edges is removed once.
pub fn remove_nodes(
    path: &Path,
    selector: NodeSelector<'_>,
    cascade: bool,
    as_of: Option<&str>,
) -> Result<MutationOutcome, EngineError> {
    let g = crate::load_graph_from_path(path)?;
    let targets = resolve_targets(&g, &selector)?;
    let before = hard_count(&g);

    // Per-target (slug, layer, incident edge indices in doc order), computed
    // against the loaded graph BEFORE the doc is moved out for mutation.
    let mut per_node: Vec<(String, String, Vec<usize>)> = Vec::new();
    for slug in &targets {
        let layer = g.node_layer.get(slug).cloned().unwrap_or_default();
        let mut incident: Vec<usize> = g
            .out_edges
            .get(slug.as_str())
            .into_iter()
            .chain(g.in_edges.get(slug.as_str()))
            .flatten()
            .copied()
            .collect();
        incident.sort_unstable();
        incident.dedup();
        per_node.push((slug.clone(), layer, incident));
    }

    // Refuse the whole batch on the FIRST node that still has incident edges
    // (all-or-nothing), naming it + its own edges exactly as the single verb.
    if !cascade
        && let Some((slug, _, incident)) = per_node.iter().find(|(_, _, inc)| !inc.is_empty())
    {
        let eids: Vec<String> = incident.iter().map(|&i| g.edges[i].eid()).collect();
        return Err(EngineError::RemoveNodeHasEdges {
            node: slug.clone(),
            eids,
        });
    }

    // Per-node summary lines + the union of incident edges to drop (each once).
    let summaries: Vec<String> = per_node
        .iter()
        .map(|(slug, layer, incident)| {
            let mut s = format!("REMOVED node {slug} ({layer})");
            if !incident.is_empty() {
                s.push_str(&format!(" + {} edge(s):", incident.len()));
                for &i in incident {
                    s.push_str(&format!("\n  {}", g.edges[i].eid()));
                }
            }
            s
        })
        .collect();
    let mut union_edges: Vec<usize> = per_node
        .iter()
        .flat_map(|(_, _, inc)| inc.iter().copied())
        .collect();
    union_edges.sort_unstable();
    union_edges.dedup();

    let mut doc = g.doc;
    for (slug, layer, _) in &per_node {
        if let Some(layer_map) = doc
            .get_mut("nodes")
            .and_then(|n| n.get_mut(layer))
            .and_then(Value::as_object_mut)
        {
            layer_map.remove(slug);
        }
    }
    if !union_edges.is_empty()
        && let Some(edges) = doc.get_mut("edges").and_then(Value::as_array_mut)
    {
        // g.edges indices map 1:1 onto the parsed doc's edges array; remove in
        // reverse so earlier indices stay valid.
        for &i in union_edges.iter().rev() {
            edges.remove(i);
        }
    }

    let stamped = finish(path, before, doc, as_of)?;
    Ok(MutationOutcome {
        summary: summaries.join("\n"),
        stamped,
    })
}

/// `maapp add-edge <type> <from> <to> <graph> [--attr k=v]...`
///
/// A duplicate `(type, from, to)` identity is refused up front (§1); endpoint
/// kinds, dangling endpoints and required attrs are enforced by the in-memory
/// validate pass (the same `E_*` checks, surfaced immediately).
pub fn add_edge(
    path: &Path,
    etype: &str,
    from: &str,
    to: &str,
    attrs: &[(String, Value)],
    as_of: Option<&str>,
) -> Result<MutationOutcome, EngineError> {
    let g = crate::load_graph_from_path(path)?;
    let eid = format!("{etype}:{from}->{to}");
    if g.edges
        .iter()
        .any(|e| e.edge_type() == Some(etype) && e.from() == Some(from) && e.to() == Some(to))
    {
        return Err(EngineError::EdgeExists(eid));
    }
    let before = hard_count(&g);

    let mut edge = Map::new();
    edge.insert("type".to_string(), Value::String(etype.to_string()));
    edge.insert("from".to_string(), Value::String(from.to_string()));
    edge.insert("to".to_string(), Value::String(to.to_string()));
    for (k, v) in attrs {
        edge.insert(k.clone(), v.clone());
    }

    let mut doc = g.doc;
    let root = doc
        .as_object_mut()
        .ok_or(EngineError::MalformedDocument("top level is not an object"))?;
    let edges = root
        .entry("edges")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or(EngineError::MalformedDocument("'edges' is not an array"))?;
    edges.push(Value::Object(edge));

    let stamped = finish(path, before, doc, as_of)?;
    Ok(MutationOutcome {
        summary: format!("ADDED edge {eid}"),
        stamped,
    })
}

/// `maapp remove-edge <type> <from> <to> <graph>`
///
/// Removes every edge matching the identity (a legal document has at most
/// one — duplicates are the §1 input-error class).
pub fn remove_edge(
    path: &Path,
    etype: &str,
    from: &str,
    to: &str,
    as_of: Option<&str>,
) -> Result<MutationOutcome, EngineError> {
    let g = crate::load_graph_from_path(path)?;
    let eid = format!("{etype}:{from}->{to}");
    let matching: Vec<usize> = g
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.edge_type() == Some(etype) && e.from() == Some(from) && e.to() == Some(to)
        })
        .map(|(i, _)| i)
        .collect();
    if matching.is_empty() {
        return Err(EngineError::EdgeNotFound(eid));
    }
    let before = hard_count(&g);

    let mut doc = g.doc;
    let edges = doc
        .get_mut("edges")
        .and_then(Value::as_array_mut)
        .ok_or(EngineError::MalformedDocument("'edges' is not an array"))?;
    for &i in matching.iter().rev() {
        edges.remove(i);
    }

    let stamped = finish(path, before, doc, as_of)?;
    Ok(MutationOutcome {
        summary: format!("REMOVED edge {eid}"),
        stamped,
    })
}

// ---------------------------------------------------------------------------
// shared plumbing
// ---------------------------------------------------------------------------

/// Hard (`E_*`) finding count of a loaded graph.
fn hard_count(g: &Graph) -> usize {
    validate(g).iter().filter(|f| f.hard()).count()
}

/// Resolve a batch selector to a deterministic list of existing slugs (T3b).
///
/// - `Slugs`: preserve first-seen order, dedupe, and fail the whole batch on
///   the FIRST unknown slug ([`EngineError::NodeNotFound`]) — all-or-nothing,
///   before any write.
/// - `Where`: every node matching the filter, in slug-sorted order (`g.nodes`
///   is a `BTreeMap`); an empty match is [`EngineError::WhereMatchesNothing`].
fn resolve_targets(g: &Graph, selector: &NodeSelector) -> Result<Vec<String>, EngineError> {
    match selector {
        NodeSelector::Slugs(slugs) => {
            let mut seen: BTreeSet<&str> = BTreeSet::new();
            let mut out: Vec<String> = Vec::new();
            for s in slugs.iter() {
                if !g.node_layer.contains_key(s) {
                    return Err(EngineError::NodeNotFound(s.clone()));
                }
                if seen.insert(s.as_str()) {
                    out.push(s.clone());
                }
            }
            Ok(out)
        }
        NodeSelector::Where { key, value } => {
            let out: Vec<String> = g
                .nodes
                .iter()
                .filter(|(_, n)| where_matches(n, key, value))
                .map(|(slug, _)| slug.clone())
                .collect();
            if out.is_empty() {
                return Err(EngineError::WhereMatchesNothing {
                    key: (*key).to_string(),
                    value: (*value).to_string(),
                });
            }
            Ok(out)
        }
    }
}

/// Does a node match a `--where key=value` filter? `key` is `kind`, `refs.<k>`,
/// or `attrs.<k>`; the compared value is the field's scalar rendered as a string
/// (a JSON string is its inner text, `true`/`1` render as-is). Any other key
/// shape matches nothing (the caller surfaces the empty-match error).
fn where_matches(n: &Node, key: &str, value: &str) -> bool {
    if key == "kind" {
        return n.kind() == Some(value);
    }
    if let Some(k) = key.strip_prefix("refs.") {
        return node_field_scalar(n.refs.as_ref(), k).as_deref() == Some(value);
    }
    if let Some(k) = key.strip_prefix("attrs.") {
        return node_field_scalar(n.attrs.as_ref(), k).as_deref() == Some(value);
    }
    false
}

/// A node `refs`/`attrs` sub-field rendered as a comparable string, or `None`
/// when the block/field is absent or JSON `null`.
fn node_field_scalar(block: Option<&Value>, field: &str) -> Option<String> {
    match block?.as_object()?.get(field)? {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// Build a JSON object from CLI `K=V` pairs (later duplicates win).
fn pairs_object(pairs: &[(String, Value)]) -> Value {
    let mut out = Map::new();
    for (k, v) in pairs {
        out.insert(k.clone(), v.clone());
    }
    Value::Object(out)
}

/// Merge CLI pairs into a node's `refs`/`attrs` sub-object (created when
/// absent), recording `"<field>.<key>"` into `changed`.
fn merge_pairs(
    node: &mut Map<String, Value>,
    field: &str,
    pairs: &[(String, Value)],
    changed: &mut Vec<String>,
) -> Result<(), EngineError> {
    if pairs.is_empty() {
        return Ok(());
    }
    let slot = node
        .entry(field.to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or(EngineError::MalformedDocument(
            "refs/attrs block is not an object",
        ))?;
    for (k, v) in pairs {
        slot.insert(k.clone(), v.clone());
        changed.push(format!("{field}.{k}"));
    }
    Ok(())
}

/// The atomic validated write shared by every mutation verb: optional
/// `--as-of` bump → [`commit_canonical`] (validate + canonical temp+rename).
fn finish(
    path: &Path,
    before_errors: usize,
    mut doc: Value,
    as_of: Option<&str>,
) -> Result<Option<String>, EngineError> {
    if let Some(rev) = as_of {
        let prov = doc
            .get_mut("meta")
            .and_then(|m| m.get_mut("provenance"))
            .and_then(Value::as_object_mut)
            .ok_or(EngineError::NoProvenanceToStamp)?;
        prov.insert("asOf".to_string(), Value::String(rev.to_string()));
    }
    commit_canonical(path, before_errors, doc)?;
    Ok(as_of.map(str::to_string))
}

/// Reload → full validate → refuse on hard-error regression → canonical
/// emission → temp+rename. The write core shared by the mutation verbs (via
/// [`finish`]) and `migrate` (T5): a write may never RAISE the hard-error
/// count, and the on-disk result is always canonical form (spec §1).
pub(crate) fn commit_canonical(
    path: &Path,
    before_errors: usize,
    doc: Value,
) -> Result<(), EngineError> {
    let g = Graph::from_doc(doc)?;
    let would_be: Vec<crate::validate::Finding> = validate(&g)
        .into_iter()
        .filter(crate::validate::Finding::hard)
        .collect();
    if would_be.len() > before_errors {
        return Err(EngineError::ValidationRegression {
            before: before_errors,
            after: would_be.len(),
            findings: would_be,
        });
    }

    let mut bytes = serde_json::to_vec_pretty(&canonical_doc(&g))?;
    bytes.push(b'\n');
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Hard (`E_*`) finding count of a loaded graph — the `before` baseline a
/// write must not exceed. Shared with `migrate` (T5).
pub(crate) fn hard_error_count(g: &Graph) -> usize {
    hard_count(g)
}

#[cfg(test)]
// Test code may panic on failure: that IS the assertion mechanism.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// `V` parses as JSON when it can, string otherwise; missing/empty key errs.
    #[test]
    fn parse_kv_types_values_as_json_with_string_fallback() {
        assert_eq!(parse_kv("seq=1").unwrap(), ("seq".into(), Value::from(1)));
        assert_eq!(
            parse_kv("awaits=true").unwrap(),
            ("awaits".into(), Value::Bool(true))
        );
        assert_eq!(
            parse_kv("mode=set").unwrap(),
            ("mode".into(), Value::String("set".into()))
        );
        assert_eq!(
            parse_kv("source=src/a.ts@Save").unwrap(),
            ("source".into(), Value::String("src/a.ts@Save".into()))
        );
        assert!(matches!(
            parse_kv("no-equals"),
            Err(EngineError::BadKeyValue(_))
        ));
        assert!(matches!(parse_kv("=v"), Err(EngineError::BadKeyValue(_))));
    }
}
