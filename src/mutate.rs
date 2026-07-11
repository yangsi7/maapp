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
use crate::validate::validate;
use serde_json::{Map, Value};
use std::path::Path;

/// What a successful mutation did, for the CLI to print.
#[derive(Debug)]
pub struct MutationOutcome {
    /// One human line (plus cascade detail lines) describing the delta.
    pub summary: String,
    /// The `asOf` value stamped in the same write, when `--as-of` was given.
    pub stamped: Option<String>,
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
/// No flags = [`EngineError::NoOpUpdate`] (exit 2): a no-op is an error, not a
/// silent success. `--ref`/`--attr` pairs merge into the existing blocks
/// (creating them when absent).
pub fn update_node(
    path: &Path,
    slug: &str,
    intent: Option<&str>,
    refs: &[(String, Value)],
    attrs: &[(String, Value)],
    as_of: Option<&str>,
) -> Result<MutationOutcome, EngineError> {
    if intent.is_none() && refs.is_empty() && attrs.is_empty() {
        return Err(EngineError::NoOpUpdate(slug.to_string()));
    }
    let g = crate::load_graph_from_path(path)?;
    let layer = g
        .node_layer
        .get(slug)
        .cloned()
        .ok_or_else(|| EngineError::NodeNotFound(slug.to_string()))?;
    let before = hard_count(&g);

    let mut changed: Vec<String> = Vec::new();
    let mut doc = g.doc;
    let node = doc
        .get_mut("nodes")
        .and_then(|n| n.get_mut(&layer))
        .and_then(|l| l.get_mut(slug))
        .and_then(Value::as_object_mut)
        .ok_or(EngineError::MalformedDocument("node body is not an object"))?;
    if let Some(text) = intent {
        node.insert("intent".to_string(), Value::String(text.to_string()));
        changed.push("intent".to_string());
    }
    merge_pairs(node, "refs", refs, &mut changed)?;
    merge_pairs(node, "attrs", attrs, &mut changed)?;

    let stamped = finish(path, before, doc, as_of)?;
    Ok(MutationOutcome {
        summary: format!("UPDATED node {slug} ({})", changed.join(", ")),
        stamped,
    })
}

/// `maapp remove-node <slug> <graph> [--cascade]`
///
/// With incident edges and no `--cascade`: refuses, listing them (spec §4).
/// With `--cascade`: removes node + incident edges and reports exactly what
/// was removed.
pub fn remove_node(
    path: &Path,
    slug: &str,
    cascade: bool,
    as_of: Option<&str>,
) -> Result<MutationOutcome, EngineError> {
    let g = crate::load_graph_from_path(path)?;
    let layer = g
        .node_layer
        .get(slug)
        .cloned()
        .ok_or_else(|| EngineError::NodeNotFound(slug.to_string()))?;
    let before = hard_count(&g);

    // Incident edge indices in document order (out + in, deduped, sorted).
    let mut incident: Vec<usize> = g
        .out_edges
        .get(slug)
        .into_iter()
        .chain(g.in_edges.get(slug))
        .flatten()
        .copied()
        .collect();
    incident.sort_unstable();
    incident.dedup();
    let eids: Vec<String> = incident.iter().map(|&i| g.edges[i].eid()).collect();
    if !cascade && !eids.is_empty() {
        return Err(EngineError::RemoveNodeHasEdges {
            node: slug.to_string(),
            eids,
        });
    }

    let mut doc = g.doc;
    let layer_map = doc
        .get_mut("nodes")
        .and_then(|n| n.get_mut(&layer))
        .and_then(Value::as_object_mut)
        .ok_or(EngineError::MalformedDocument("layer map is not an object"))?;
    layer_map.remove(slug);
    if !incident.is_empty()
        && let Some(edges) = doc.get_mut("edges").and_then(Value::as_array_mut)
    {
        // g.edges indices map 1:1 onto the parsed doc's edges array.
        for &i in incident.iter().rev() {
            edges.remove(i);
        }
    }

    let stamped = finish(path, before, doc, as_of)?;
    let mut summary = format!("REMOVED node {slug} ({layer})");
    if !eids.is_empty() {
        summary.push_str(&format!(" + {} edge(s):", eids.len()));
        for eid in &eids {
            summary.push_str(&format!("\n  {eid}"));
        }
    }
    Ok(MutationOutcome { summary, stamped })
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

/// The atomic validated write shared by every verb: optional `--as-of` bump →
/// reload → full validate → refuse on hard-error regression → canonical
/// emission → temp + rename.
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
    Ok(as_of.map(str::to_string))
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
