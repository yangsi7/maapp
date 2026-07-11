//! Canonical document projection + `maapp export --slice` (LIFECYCLE-VERBS-SPEC
//! §5 — scoped read, P2).
//!
//! Two responsibilities, one home:
//!
//! 1. **Canonical emission** (spec §1): the physical order of a maapp-graph
//!    file is NOT semantic — the canonical projection rebuilds `nodes` (layers
//!    in the canonical `LAYERS` order, slugs sorted within layer, node bodies
//!    in `kind`/`intent`/`refs`/`attrs` order with inner objects key-sorted)
//!    and `edges` (sorted by the `(type, from, to)` identity, attrs
//!    key-sorted). Every other top-level key passes through verbatim in parsed
//!    order. The mutation verbs (`crate::mutate`) emit through this projection,
//!    so the first mutation of a narratively-ordered file canonicalizes it
//!    wholesale — expected and documented (`diff` is order-insensitive, so the
//!    change is semantically invisible).
//! 2. **`export --slice`** (§5): produce a *complete, valid* maapp-graph
//!    document for a neighborhood (`<slug>` + `--depth N`, BFS over ALL edge
//!    types including guardedBy and derivesFrom — the F4 lesson) or a scope
//!    (`scope:<s>` → every node whose slug domain is `<s>`). Closure rule:
//!    selected nodes + all edges whose BOTH endpoints are selected. Never
//!    invents stub nodes (out-of-slice edges are dropped), never mutates the
//!    source graph. The result carries every header/registry key of the source
//!    plus a `meta.slice_of` annotation — validate reads that annotation as
//!    slice mode and suppresses `W_ORPHAN` (slicing creates boundary orphans
//!    by construction).
//!
//! Everything here is pure compute over a loaded [`Graph`] — wasm-clean, no
//! filesystem.

use crate::error::EngineError;
use crate::graph::Graph;
use crate::model::{Edge, LAYERS, Node};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The slice selector forms of `maapp export --slice` (spec §5).
#[derive(Debug, Clone, Copy)]
pub enum SliceSelector<'a> {
    /// `--slice <slug> [--depth N]`: the BFS neighborhood of one node.
    Node { slug: &'a str, depth: usize },
    /// `--slice scope:<scope>`: every node whose slug domain is `<scope>`
    /// (e.g. `scope:webhook` → all `*:webhook/*` nodes).
    Scope(&'a str),
}

// ---------------------------------------------------------------------------
// canonical projection
// ---------------------------------------------------------------------------

/// Project a loaded graph into its canonical document form (spec §1).
///
/// Top-level keys keep the parsed order (deterministic per input file);
/// `nodes` and `edges` are rebuilt canonically. An authored EMPTY layer
/// section disappears (layers are derived from the nodes present) — semantic
/// content is unchanged.
pub fn canonical_doc(g: &Graph) -> Value {
    let mut out = Map::new();
    let Some(obj) = g.doc.as_object() else {
        // Load guarantees an object top level; keep total anyway.
        return g.doc.clone();
    };
    for (k, v) in obj {
        match k.as_str() {
            "nodes" => {
                out.insert(k.clone(), canonical_nodes(g, None));
            }
            "edges" => {
                out.insert(k.clone(), canonical_edges(g, None));
            }
            _ => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// Rank a layer for canonical ordering: the semantic `LAYERS` order first,
/// then unknown layers alphabetically after.
fn layer_rank(layer: &str) -> (usize, &str) {
    LAYERS
        .iter()
        .position(|l| *l == layer)
        .map_or((LAYERS.len(), layer), |i| (i, ""))
}

/// The canonical `nodes` value: layers in canonical order, slugs sorted within
/// each layer. `keep` restricts to a selected slug set (slice mode).
fn canonical_nodes(g: &Graph, keep: Option<&BTreeSet<String>>) -> Value {
    // layer -> sorted slugs (g.nodes is a BTreeMap, iteration is slug-sorted).
    let mut by_layer: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for slug in g.nodes.keys() {
        if keep.is_some_and(|set| !set.contains(slug)) {
            continue;
        }
        let layer = g.node_layer.get(slug).cloned().unwrap_or_default();
        by_layer.entry(layer).or_default().push(slug);
    }
    let mut layers: Vec<&String> = by_layer.keys().collect();
    layers.sort_by_key(|l| layer_rank(l));

    let mut out = Map::new();
    for layer in layers {
        let mut layer_map = Map::new();
        for slug in &by_layer[layer.as_str()] {
            layer_map.insert((*slug).to_string(), canonical_node_value(&g.nodes[*slug]));
        }
        out.insert(layer.clone(), Value::Object(layer_map));
    }
    Value::Object(out)
}

/// One node body in canonical form: struct field order (`kind`, `intent`,
/// `refs`, `attrs`, then extras key-sorted), every value canonicalized
/// (inner object keys sorted).
fn canonical_node_value(n: &Node) -> Value {
    // Node's serde order is kind/intent/refs/attrs + flattened extras
    // (BTreeMap ⇒ sorted); `preserve_order` keeps that insertion order.
    let v = serde_json::to_value(n).unwrap_or(Value::Null);
    let Value::Object(map) = v else { return v };
    let mut out = Map::new();
    for (k, v) in map {
        out.insert(k, canon(&v));
    }
    Value::Object(out)
}

/// One edge in canonical form: the `(type, from, to)` identity first, then
/// attrs key-sorted with canonicalized values.
fn canonical_edge_value(e: &Edge) -> Value {
    let mut out = Map::new();
    if let Some(t) = &e.r#type {
        out.insert("type".to_string(), Value::String(t.clone()));
    }
    if let Some(f) = &e.from {
        out.insert("from".to_string(), Value::String(f.clone()));
    }
    if let Some(t) = &e.to {
        out.insert("to".to_string(), Value::String(t.clone()));
    }
    for (k, v) in &e.attrs {
        out.insert(k.clone(), canon(v));
    }
    Value::Object(out)
}

/// The canonical `edges` array: sorted by `(type, from, to)` (`None` sorts
/// first). `keep` restricts to edges whose BOTH endpoints are selected — the
/// §5 closure rule (out-of-slice edges are dropped, never stubbed).
fn canonical_edges(g: &Graph, keep: Option<&BTreeSet<String>>) -> Value {
    let mut edges: Vec<&Edge> = g
        .edges
        .iter()
        .filter(|e| {
            keep.is_none_or(|set| {
                e.from().is_some_and(|f| set.contains(f)) && e.to().is_some_and(|t| set.contains(t))
            })
        })
        .collect();
    edges.sort_by(|a, b| (&a.r#type, &a.from, &a.to).cmp(&(&b.r#type, &b.from, &b.to)));
    Value::Array(edges.into_iter().map(canonical_edge_value).collect())
}

/// Recursively sort object keys so emitted values are independent of on-disk
/// key order (mirrors `diff`'s canonicalization).
fn canon(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), canon(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canon).collect()),
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// export --slice
// ---------------------------------------------------------------------------

/// Build the §5 slice document: a complete, valid maapp-graph document holding
/// the selected nodes + the closure edge set, all header/registry keys carried
/// over, and `meta.slice_of` stamped.
pub fn export_slice(g: &Graph, selector: SliceSelector<'_>) -> Result<Value, EngineError> {
    let (selected, slice_of) = match selector {
        SliceSelector::Node { slug, depth } => {
            if !g.nodes.contains_key(slug) {
                return Err(EngineError::NodeNotFound(slug.to_string()));
            }
            let mut annotation = Map::new();
            annotation.insert(
                "depth".to_string(),
                Value::Number(serde_json::Number::from(depth)),
            );
            annotation.insert("selector".to_string(), Value::String(slug.to_string()));
            (neighborhood(g, slug, depth), Value::Object(annotation))
        }
        SliceSelector::Scope(scope) => {
            let selected: BTreeSet<String> = g
                .nodes
                .keys()
                .filter(|slug| slug_domain(slug) == Some(scope))
                .cloned()
                .collect();
            if selected.is_empty() {
                return Err(EngineError::ScopeMatchesNothing(scope.to_string()));
            }
            let mut annotation = Map::new();
            annotation.insert(
                "selector".to_string(),
                Value::String(format!("scope:{scope}")),
            );
            (selected, Value::Object(annotation))
        }
    };

    // Carry every top-level key; rebuild nodes/edges to the slice; stamp
    // meta.slice_of (creating meta when the source has none — appended last,
    // deterministic under `preserve_order`).
    let mut out = Map::new();
    let mut meta_seen = false;
    if let Some(obj) = g.doc.as_object() {
        for (k, v) in obj {
            match k.as_str() {
                "nodes" => {
                    out.insert(k.clone(), canonical_nodes(g, Some(&selected)));
                }
                "edges" => {
                    out.insert(k.clone(), canonical_edges(g, Some(&selected)));
                }
                "meta" => {
                    meta_seen = true;
                    let mut meta = v
                        .as_object()
                        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_else(Map::new);
                    meta.insert("slice_of".to_string(), slice_of.clone());
                    out.insert(k.clone(), Value::Object(meta));
                }
                _ => {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
    }
    if !meta_seen {
        let mut meta = Map::new();
        meta.insert("slice_of".to_string(), slice_of);
        out.insert("meta".to_string(), Value::Object(meta));
    }
    Ok(Value::Object(out))
}

/// Depth-bounded BFS over ALL edge types, both directions (mirrors
/// `query::q_neighbors`'s visit set — including guardedBy/derivesFrom).
fn neighborhood(g: &Graph, root: &str, depth: usize) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    seen.insert(root.to_string());
    let mut frontier: VecDeque<(String, usize)> = VecDeque::new();
    frontier.push_back((root.to_string(), 0));
    while let Some((cur, d)) = frontier.pop_front() {
        if d >= depth {
            continue;
        }
        let out_idxs = g.out_edges.get(cur.as_str()).cloned().unwrap_or_default();
        let in_idxs = g.in_edges.get(cur.as_str()).cloned().unwrap_or_default();
        for idx in out_idxs.iter().chain(in_idxs.iter()) {
            let e = &g.edges[*idx];
            for nxt in [e.from(), e.to()].into_iter().flatten() {
                if !seen.contains(nxt) {
                    seen.insert(nxt.to_string());
                    frontier.push_back((nxt.to_string(), d + 1));
                }
            }
        }
    }
    seen
}

/// The domain part of a `kind:domain/Name` slug (`op:webhook/CreateOrder` →
/// `webhook`), or `None` when the slug does not follow that form.
fn slug_domain(slug: &str) -> Option<&str> {
    let (_, rest) = slug.split_once(':')?;
    let (domain, _) = rest.split_once('/')?;
    Some(domain)
}

#[cfg(test)]
// Test code may panic on failure: that IS the assertion mechanism.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn slug_domain_parses_kind_domain_name_form() {
        assert_eq!(slug_domain("op:webhook/CreateOrder"), Some("webhook"));
        assert_eq!(slug_domain("screen:checkout/Form"), Some("checkout"));
        assert_eq!(slug_domain("no-colon"), None);
        assert_eq!(slug_domain("kind:no-slash"), None);
    }

    #[test]
    fn layer_rank_orders_semantic_layers_before_unknown() {
        let mut layers = vec!["boundary", "zz-custom", "surface", "logic"];
        layers.sort_by_key(|l| layer_rank(l));
        assert_eq!(layers, vec!["surface", "logic", "boundary", "zz-custom"]);
    }
}
