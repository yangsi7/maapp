//! Query verbs — Slice 2: `node`, `orphans`, `blast-radius`, `trace`,
//! `neighbors`, `view`.
//!
//! Every function returns a serializable result struct with a byte-stable
//! `--json` form. ONE documented behavior worth noting: blast-radius counts
//! `guardedBy` in-edges in its DEPENDENTS traversal (see `is_dependent_edge`).
//! Determinism rules: `BTreeMap`/sorted `Vec` at every emit boundary; no
//! `HashMap`/`HashSet` in serialized output.

use crate::graph::Graph;
use crate::model::Edge;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

// ---------------------------------------------------------------------------
// q_node
// ---------------------------------------------------------------------------

/// Result of `query node <id>`.
///
/// JSON shape: `{id, layer, node, out_edges[], in_edges[]}`.
/// Mirrors oracle `q_node` return dict.
#[derive(Debug, Serialize)]
pub struct NodeResult<'a> {
    pub id: &'a str,
    pub layer: Option<&'a str>,
    /// The raw node body — all fields that were parsed (`kind`, `intent`, `refs`, `attrs`, extras).
    pub node: &'a crate::model::Node,
    pub out_edges: Vec<&'a Edge>,
    pub in_edges: Vec<&'a Edge>,
}

/// Look up a single node by id. Returns `None` when the id is unknown
/// (the CLI converts this to exit 2 + an error message).
pub fn q_node<'a>(g: &'a Graph, nid: &'a str) -> Option<NodeResult<'a>> {
    if !g.nodes.contains_key(nid) {
        return None;
    }
    let layer = g.node_layer.get(nid).map(String::as_str);
    let node = g.nodes.get(nid)?;
    let out_edges = g
        .out_edges
        .get(nid)
        .map(|idxs| idxs.iter().map(|&i| &g.edges[i]).collect())
        .unwrap_or_default();
    let in_edges = g
        .in_edges
        .get(nid)
        .map(|idxs| idxs.iter().map(|&i| &g.edges[i]).collect())
        .unwrap_or_default();
    Some(NodeResult {
        id: nid,
        layer,
        node,
        out_edges,
        in_edges,
    })
}

// ---------------------------------------------------------------------------
// q_orphans
// ---------------------------------------------------------------------------

/// Result of `query orphans`.
///
/// JSON shape: `{orphans: [slug, ...]}` — sorted ascending.
#[derive(Debug, Serialize)]
pub struct OrphansResult {
    pub orphans: Vec<String>,
}

/// Nodes that have no incident edges (no out AND no in). Sorted.
pub fn q_orphans(g: &Graph) -> OrphansResult {
    let mut orphans: Vec<String> = g
        .nodes
        .keys()
        .filter(|nid| {
            g.out_edges.get(*nid).is_none_or(|v| v.is_empty())
                && g.in_edges.get(*nid).is_none_or(|v| v.is_empty())
        })
        .cloned()
        .collect();
    orphans.sort();
    OrphansResult { orphans }
}

// ---------------------------------------------------------------------------
// q_blast_radius
// ---------------------------------------------------------------------------

/// Dependency-bearing edge types — mirrors oracle `DEP_TYPES`. `pub` because
/// the validator's `W_UNLINKED_MIRROR` connectivity check (schema 1.4, F3)
/// reuses the SAME dependency family (single source of truth).
pub static DEP_TYPES: &[&str] = &[
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

fn is_dep(etype: &str) -> bool {
    DEP_TYPES.contains(&etype)
}

/// Edge types that carry dependency in the DEPENDENTS traversal of
/// blast-radius: `DEP_TYPES` plus `guardedBy`.
///
/// `X guardedBy A` means X's behavior depends on assertion/policy A, so a
/// change to A's semantics affects X — X is a dependent of A. Treating
/// `guardedBy` as a dependent edge answers the "who is affected?" question
/// correctly when a guard's semantics change.
///
/// This is intentionally asymmetric, dependents-side only: the DEPENDENCIES
/// traversal keeps the narrower `is_dep` set (a guard is not listed among the
/// things its target depends ON). `q_view("dependency")` and `render deps`
/// likewise keep `DEP_TYPES` unchanged for that direction.
fn is_dependent_edge(etype: &str) -> bool {
    is_dep(etype) || etype == "guardedBy"
}

/// Result of `query blast-radius <id>`.
///
/// JSON shape: `{id, dependents[], dependencies[]}` — both sorted.
#[derive(Debug, Serialize)]
pub struct BlastRadiusResult {
    pub id: String,
    pub dependents: Vec<String>,
    pub dependencies: Vec<String>,
}

/// Compute the transitive blast-radius for `nid`.
///
/// `dependents` = nodes that depend ON `nid` (follow incoming dep edges).
/// `dependencies` = nodes that `nid` depends on (follow outgoing dep edges).
/// Both sets are sorted and exclude `nid` itself.
pub fn q_blast_radius(g: &Graph, nid: &str) -> Option<BlastRadiusResult> {
    if !g.nodes.contains_key(nid) {
        return None;
    }

    // Dependents: follow INCOMING dependency edges transitively.
    // `is_dependent_edge` (not `is_dep`): guardedBy in-edges count as
    // dependency-bearing here — see its doc comment for the oracle divergence.
    let mut dependents: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = vec![nid.to_string()];
    while let Some(cur) = stack.pop() {
        for &i in g.in_edges.get(cur.as_str()).into_iter().flatten() {
            let e = &g.edges[i];
            if e.edge_type().is_some_and(is_dependent_edge)
                && let Some(f) = e.from()
                && !dependents.contains(f)
                && f != nid
            {
                dependents.insert(f.to_string());
                stack.push(f.to_string());
            }
        }
    }

    // Dependencies: follow OUTGOING dependency edges transitively.
    let mut dependencies: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = vec![nid.to_string()];
    while let Some(cur) = stack.pop() {
        for &i in g.out_edges.get(cur.as_str()).into_iter().flatten() {
            let e = &g.edges[i];
            if e.edge_type().is_some_and(is_dep)
                && let Some(t) = e.to()
                && !dependencies.contains(t)
                && t != nid
            {
                dependencies.insert(t.to_string());
                stack.push(t.to_string());
            }
        }
    }

    Some(BlastRadiusResult {
        id: nid.to_string(),
        dependents: dependents.into_iter().collect(),
        dependencies: dependencies.into_iter().collect(),
    })
}

// ---------------------------------------------------------------------------
// q_trace + q_trace_terminals
// ---------------------------------------------------------------------------

/// Edge types followed by the trace traversal — mirrors oracle `TRACE_TYPES`.
/// `pub` because the validator's `W_FLOW_UNREACHABLE` check (schema 1.4, F2)
/// uses the SAME traversal family as `q_trace` (single source of truth).
pub static TRACE_TYPES: &[&str] = &[
    "binds",
    "reads",
    "invokes",
    "writes",
    "produces",
    "x-pipeline:feeds",
    "x-pipeline:consumes",
    "handles",
    "fires",
    "navigates",
    "setsViewState",
    "emits",
];

fn is_trace(etype: &str) -> bool {
    TRACE_TYPES.contains(&etype)
}

/// A single edge triple used in trace paths (owned, so paths can outlive the
/// stack frame — the oracle's paths are lists of edge dicts).
#[derive(Debug, Clone, Serialize)]
pub struct TraceEdge {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Key-ordered extra attrs, mirroring the edge body.
    #[serde(flatten)]
    pub attrs: BTreeMap<String, serde_json::Value>,
}

impl From<&Edge> for TraceEdge {
    fn from(e: &Edge) -> Self {
        TraceEdge {
            r#type: e.r#type.clone(),
            from: e.from.clone(),
            to: e.to.clone(),
            attrs: e.attrs.clone(),
        }
    }
}

/// Result of `query trace <id>`.
///
/// JSON shape: `{id, paths: [[edge, ...], ...]}`.
#[derive(Debug, Serialize)]
pub struct TraceResult {
    pub id: String,
    pub paths: Vec<Vec<TraceEdge>>,
}

/// Iterative DFS trace (mirrors the oracle's R2 explicit-stack rewrite).
///
/// The oracle semantics:
/// - Stack frames are `(cur_node, path_so_far)`.
/// - Per-frame: collect outgoing trace-type edges not yet visited.
/// - If no progress (all edges visited): record the path if non-empty.
/// - Else: push frames in REVERSED order so the first out-edge is processed
///   first (LIFO stack — mirrors the recursive variant's emission order).
/// - Global visited_edges set shared across all frames.
pub fn q_trace(g: &Graph, nid: &str) -> Option<TraceResult> {
    if !g.nodes.contains_key(nid) {
        return None;
    }

    let mut paths: Vec<Vec<TraceEdge>> = Vec::new();
    let mut visited_edges: BTreeSet<(String, String, String)> = BTreeSet::new();

    // Stack: (current_node, path_so_far)
    let mut stack: Vec<(String, Vec<TraceEdge>)> = vec![(nid.to_string(), Vec::new())];

    while let Some((cur, path)) = stack.pop() {
        // Collect unvisited outgoing trace edges.
        let outs: Vec<&Edge> = g
            .out_edges
            .get(cur.as_str())
            .into_iter()
            .flatten()
            .map(|&i| &g.edges[i])
            .filter(|e| e.edge_type().is_some_and(is_trace))
            .collect();

        let mut progressed = false;
        let mut to_push: Vec<(String, Vec<TraceEdge>)> = Vec::new();

        for e in &outs {
            let key = (
                e.r#type.clone().unwrap_or_default(),
                e.from.clone().unwrap_or_default(),
                e.to.clone().unwrap_or_default(),
            );
            if visited_edges.contains(&key) {
                continue;
            }
            visited_edges.insert(key);
            progressed = true;
            let next_node = e.to().unwrap_or("").to_string();
            let mut new_path = path.clone();
            new_path.push(TraceEdge::from(*e));
            to_push.push((next_node, new_path));
        }

        if !progressed {
            if !path.is_empty() {
                paths.push(path);
            }
        } else {
            // Push in reversed order so first out-edge is processed first (LIFO).
            for frame in to_push.into_iter().rev() {
                stack.push(frame);
            }
        }
    }

    Some(TraceResult {
        id: nid.to_string(),
        paths,
    })
}

/// Result of `query trace <id> --terminals`.
///
/// With `--terminals`, the oracle emits a PLAIN JSON ARRAY of sorted terminal
/// slugs (NOT the `{id, terminals}` dict). The Rust `--json` output must match
/// this exactly: a JSON array, not an object.
#[derive(Debug, Serialize)]
pub struct TraceTerminals {
    pub terminals: Vec<String>,
}

/// Unique, sorted terminal nodes reached by trace.
pub fn q_trace_terminals(g: &Graph, nid: &str) -> Option<TraceTerminals> {
    let res = q_trace(g, nid)?;
    let mut terms: BTreeSet<String> = BTreeSet::new();
    for path in &res.paths {
        if let Some(last) = path.last()
            && let Some(t) = &last.to
        {
            terms.insert(t.clone());
        }
    }
    Some(TraceTerminals {
        terminals: terms.into_iter().collect(),
    })
}

// ---------------------------------------------------------------------------
// q_neighbors
// ---------------------------------------------------------------------------

/// Result of `query neighbors <id> [--depth N]`.
///
/// JSON shape: `{root, depth, nodes{id: layer}, edges[]}`.
/// `nodes` is a map id→layer (or null if unknown layer). Oracle uses a plain
/// Python dict; Rust must emit a JSON object with sorted keys.
#[derive(Debug, Serialize)]
pub struct NeighborsResult {
    pub root: String,
    pub depth: usize,
    /// id → layer (BTreeMap for sorted key order).
    pub nodes: BTreeMap<String, Option<String>>,
    pub edges: Vec<TraceEdge>,
}

/// BFS neighborhood up to `depth` hops, following both out and in edges.
/// Deduplicates edges by `(type, from, to)`.
pub fn q_neighbors(g: &Graph, nid: &str, depth: usize) -> Option<NeighborsResult> {
    if !g.nodes.contains_key(nid) {
        return None;
    }

    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    seen.insert(nid.to_string(), 0);
    let mut frontier: VecDeque<(String, usize)> = VecDeque::new();
    frontier.push_back((nid.to_string(), 0));

    let mut used_edges: Vec<TraceEdge> = Vec::new();

    while let Some((cur, d)) = frontier.pop_front() {
        if d >= depth {
            continue;
        }
        // out + in edges, mirroring oracle's `g.out_edges.get(cur,[]) + g.in_edges.get(cur,[])`
        let out_idxs = g.out_edges.get(cur.as_str()).cloned().unwrap_or_default();
        let in_idxs = g.in_edges.get(cur.as_str()).cloned().unwrap_or_default();
        for idx in out_idxs.iter().chain(in_idxs.iter()) {
            let e = &g.edges[*idx];
            used_edges.push(TraceEdge::from(e));
            for nxt in [e.from(), e.to()].into_iter().flatten() {
                if !seen.contains_key(nxt) {
                    seen.insert(nxt.to_string(), d + 1);
                    frontier.push_back((nxt.to_string(), d + 1));
                }
            }
        }
    }

    // Deduplicate edges by (type, from, to) preserving first-seen order.
    let mut deduped: Vec<TraceEdge> = Vec::new();
    let mut edge_keys: BTreeSet<(String, String, String)> = BTreeSet::new();
    for e in used_edges {
        let key = (
            e.r#type.clone().unwrap_or_default(),
            e.from.clone().unwrap_or_default(),
            e.to.clone().unwrap_or_default(),
        );
        if !edge_keys.contains(&key) {
            edge_keys.insert(key);
            deduped.push(e);
        }
    }

    // nodes map: id -> layer (oracle uses `g.node_layer.get(k)` which may be None
    // for extension nodes not in any layer).
    let nodes: BTreeMap<String, Option<String>> = seen
        .into_keys()
        .map(|id| {
            let layer = g.node_layer.get(&id).cloned();
            (id, layer)
        })
        .collect();

    Some(NeighborsResult {
        root: nid.to_string(),
        depth,
        nodes,
        edges: deduped,
    })
}

// ---------------------------------------------------------------------------
// q_view
// ---------------------------------------------------------------------------

/// The three named views and their edge families.
///
/// `nav` = navigates | dismisses | x-ext:returnsTo
/// `dependency` = DEP_TYPES
/// `assertion` = guardedBy | derivesFrom
static VIEW_NAV: &[&str] = &["navigates", "dismisses", "x-ext:returnsTo"];
static VIEW_ASSERTION: &[&str] = &["guardedBy", "derivesFrom"];

fn view_family(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "nav" => Some(VIEW_NAV),
        "dependency" => Some(DEP_TYPES),
        "assertion" => Some(VIEW_ASSERTION),
        _ => None,
    }
}

/// Result of `query view <nav|dependency|assertion>`.
///
/// JSON shape: `{view, nodes[], edges[]}`.
/// `nodes` is sorted. `edges` is in document order (oracle iterates `g.edges`).
#[derive(Debug, Serialize)]
pub struct ViewResult {
    pub view: String,
    pub nodes: Vec<String>,
    pub edges: Vec<TraceEdge>,
}

/// Filter edges to the named view family. Returns `None` for an unknown name.
pub fn q_view(g: &Graph, name: &str) -> Option<ViewResult> {
    let fam = view_family(name)?;
    let edges: Vec<TraceEdge> = g
        .edges
        .iter()
        .filter(|e| e.edge_type().is_some_and(|t| fam.contains(&t)))
        .map(TraceEdge::from)
        .collect();

    // nodes = sorted union of all from/to ids in the filtered edges.
    let mut node_set: BTreeSet<String> = BTreeSet::new();
    for e in &edges {
        if let Some(f) = &e.from {
            node_set.insert(f.clone());
        }
        if let Some(t) = &e.to {
            node_set.insert(t.clone());
        }
    }
    let nodes: Vec<String> = node_set.into_iter().collect();

    Some(ViewResult {
        view: name.to_string(),
        nodes,
        edges,
    })
}
