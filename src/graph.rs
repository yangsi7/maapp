//! Graph loading + indexing, plus the frozen-core vocabulary tables.
//!
//! This is the Rust analogue of the oracle's `Graph` class and its module-level
//! `CORE_NODE_KINDS` / `CORE_EDGES` / `REFS_KEYS` tables. The dual-axis legality
//! (frozen core OR instance registry) is resolved here; the lint passes in
//! `validate` consume the resolved views.

use crate::error::EngineError;
use crate::model::{Edge, Node, Slug};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// A normalized edge signature, unified across the frozen-core table and
/// `edgeRegistry` extension rows.
#[derive(Debug, Clone)]
pub struct EdgeSig {
    pub from: Vec<&'static str>,
    pub to: Vec<&'static str>,
    /// Required attribute names.
    pub req: Vec<&'static str>,
    /// Enum-checked attributes: attr name -> legal values.
    pub enums: Vec<(&'static str, Vec<&'static str>)>,
}

/// A signature resolved at runtime (may come from the registry, so values are
/// owned `String`s rather than `'static`).
#[derive(Debug, Clone)]
pub struct ResolvedSig {
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub req: Vec<String>,
    pub enums: Vec<(String, Vec<String>)>,
}

impl From<&EdgeSig> for ResolvedSig {
    fn from(s: &EdgeSig) -> Self {
        ResolvedSig {
            from: s.from.iter().map(|x| (*x).to_string()).collect(),
            to: s.to.iter().map(|x| (*x).to_string()).collect(),
            req: s.req.iter().map(|x| (*x).to_string()).collect(),
            enums: s
                .enums
                .iter()
                .map(|(k, v)| {
                    (
                        (*k).to_string(),
                        v.iter().map(|x| (*x).to_string()).collect(),
                    )
                })
                .collect(),
        }
    }
}

/// Frozen-core node kinds, by layer. Mirrors `CORE_NODE_KINDS` in the oracle.
pub fn core_node_kinds() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("Screen", "surface"),
        ("Component", "surface"),
        ("NavContainer", "surface"),
        ("Trigger", "logic"),
        ("NavAction", "logic"),
        ("MutationAction", "logic"),
        ("EffectAction", "logic"),
        ("ViewStateAction", "logic"),
        ("QueryAction", "logic"),
        ("ViewState", "logic"),
        ("Assertion", "logic"),
        ("StateStore", "substrate"),
        ("DataSource", "substrate"),
        ("BackendOp", "substrate"),
        ("PipelineStage", "substrate"),
        ("SideEffect", "substrate"),
        ("Policy", "policy"),
    ])
}

/// Frozen-core edge signature table. Mirrors `CORE_EDGES` in the oracle.
///
/// `from`/`to` empty means "any kind"; `req` lists required attrs; `enums` lists
/// attrs that are membership-checked.
pub fn core_edges() -> BTreeMap<&'static str, EdgeSig> {
    let mut m = BTreeMap::new();
    m.insert(
        "renders",
        EdgeSig {
            from: vec!["Screen", "NavContainer"],
            to: vec!["Component", "NavContainer"],
            req: vec![],
            enums: vec![],
        },
    );
    m.insert(
        "handles",
        EdgeSig {
            from: vec!["Screen", "Component"],
            to: vec!["Trigger"],
            req: vec!["event"],
            enums: vec![(
                "event",
                vec![
                    "tap",
                    "appear",
                    "submit",
                    "longPress",
                    "change",
                    "timer",
                    "doubleTap",
                    "swipe",
                    "pullToRefresh",
                    "endReached",
                    "dragDismiss",
                ],
            )],
        },
    );
    m.insert(
        "fires",
        EdgeSig {
            from: vec!["Trigger", "x-ext:ExternalSurface", "x-ext:PermissionGate"],
            to: vec![
                "NavAction",
                "MutationAction",
                "EffectAction",
                "ViewStateAction",
                "QueryAction",
                "Trigger",
            ],
            req: vec![],
            enums: vec![],
        },
    );
    m.insert(
        "navigates",
        EdgeSig {
            from: vec!["NavAction", "NavContainer", "MutationAction"],
            to: vec!["Screen", "NavContainer", "x-ext:ExternalSurface"],
            req: vec!["present"],
            enums: vec![(
                "present",
                vec![
                    "push",
                    "sheet",
                    "fullScreen",
                    "fullScreenCover",
                    "popover",
                    "tab-switch",
                    "replace-root",
                    "deep-link",
                ],
            )],
        },
    );
    m.insert(
        "dismisses",
        EdgeSig {
            from: vec!["NavAction", "MutationAction"],
            to: vec!["Screen", "NavContainer"],
            req: vec!["target"],
            enums: vec![("target", vec!["self", "root", "count"])],
        },
    );
    m.insert(
        "setsViewState",
        EdgeSig {
            from: vec!["ViewStateAction"],
            to: vec!["ViewState"],
            req: vec![],
            enums: vec![],
        },
    );
    m.insert(
        "writes",
        EdgeSig {
            from: vec!["MutationAction", "BackendOp", "PipelineStage"],
            to: vec!["StateStore", "DataSource"],
            req: vec!["mode"],
            enums: vec![("mode", vec!["set", "append", "delete", "toggle"])],
        },
    );
    m.insert(
        "invokes",
        EdgeSig {
            from: vec!["MutationAction", "BackendOp", "PipelineStage"],
            to: vec!["BackendOp", "PipelineStage"],
            req: vec!["awaits"],
            enums: vec![],
        },
    );
    m.insert(
        "reads",
        EdgeSig {
            from: vec!["QueryAction", "BackendOp", "PipelineStage"],
            to: vec!["StateStore", "DataSource"],
            req: vec![],
            enums: vec![(
                "cachePolicy",
                vec!["fresh", "cached", "stale-while-revalidate"],
            )],
        },
    );
    m.insert(
        "binds",
        EdgeSig {
            from: vec!["Screen", "Component", "ViewState"],
            to: vec!["ViewState", "StateStore", "DataSource"],
            req: vec![],
            enums: vec![],
        },
    );
    m.insert(
        "emits",
        EdgeSig {
            from: vec![
                "MutationAction",
                "EffectAction",
                "BackendOp",
                "PipelineStage",
                "x-ext:ExternalSurface",
                "x-ext:PermissionGate",
            ],
            to: vec!["SideEffect"],
            req: vec![],
            enums: vec![],
        },
    );
    m.insert(
        "produces",
        EdgeSig {
            from: vec!["PipelineStage", "BackendOp"],
            to: vec!["StateStore", "DataSource"],
            req: vec!["artifact"],
            // Domain-neutral core (schema 1.4, RES-003 2.13): the core table
            // carries only generic verification states. A graph needing a
            // domain-specific token declares it under "produces.verification" in
            // its attrEnumRegistry (data overlay, D-003).
            enums: vec![("verification", vec!["verified", "unverified"])],
        },
    );
    m.insert(
        "guardedBy",
        EdgeSig {
            from: vec![
                "Trigger",
                "NavAction",
                "MutationAction",
                "EffectAction",
                "ViewStateAction",
                "QueryAction",
                "PipelineStage",
                "BackendOp",
                "DataSource",
            ],
            to: vec!["Assertion", "Policy"],
            req: vec!["polarity"],
            enums: vec![("polarity", vec!["require", "forbid"])],
        },
    );
    m.insert(
        "derivesFrom",
        EdgeSig {
            // StateStore joined the from-set in schema 1.4 (RES-003 2.5): the
            // store-granularity linking convention — a StateStore mirroring a
            // persisted entity SHOULD `derivesFrom` its backing DataSource.
            // Additive widening; DELIBERATE oracle divergence (documented).
            from: vec!["Assertion", "StateStore", "ViewState"],
            to: vec!["StateStore", "DataSource", "PipelineStage", "ViewState"],
            req: vec![],
            enums: vec![],
        },
    );
    m
}

/// The closed `Trigger.attrs.cause` enum (schema 1.4, RES-003 1.5): what
/// summons a ROOT Trigger (one no `handles` edge points at) — webhooks, cron
/// ticks, callbacks, inbound messages, deep links, OS/system events. Open via
/// an `attrEnumRegistry` `"Trigger.cause"` row (D-003 data overlay). Shared by
/// the validator (`E_TRIGGER_CAUSE`) and the `maapp schema` emitter.
pub fn trigger_causes() -> [&'static str; 7] {
    [
        "webhook",
        "cron",
        "schedule",
        "callback",
        "message",
        "deep-link",
        "system",
    ]
}

/// Closed refs key set. Mirrors `REFS_KEYS` in the oracle, extended with the
/// core `source` anchor (ROADMAP v2 move 2; the vendored oracle predates it).
pub fn refs_keys() -> Vec<&'static str> {
    vec![
        "spec", "design", "decision", "slice", "tokens", "data", "source",
    ]
}

/// Legal keys of the `meta.provenance` object (closed shape). Shared between
/// the validator and the `maapp schema` emitter — single source of truth.
/// `asOf` (LIFECYCLE-VERBS-SPEC §6.2) is the source-repo SHA at last sync:
/// `check-drift`'s base rev, bumped by `maapp stamp` / lifecycle updates.
/// Distinct from `sourceCommit` (the ingest-time origin commit, never bumped).
pub fn provenance_keys() -> [&'static str; 5] {
    ["origin", "sourceCommit", "generatedAt", "fidelity", "asOf"]
}

/// The closed `meta.provenance.origin` enum: who/what the graph came from.
/// `generated` (LLM-authored from a brief), `ingested` (derived from a real
/// codebase), `ratified` (human-reviewed and signed off).
pub fn provenance_origins() -> [&'static str; 3] {
    ["generated", "ingested", "ratified"]
}

/// A loaded, indexed graph ready for validation and (future) query/render.
pub struct Graph {
    /// The full parsed document (passthrough fields: meta, registries, nodeKinds).
    pub doc: Value,
    /// id -> node, flattened across layers (first occurrence wins on dup).
    /// `BTreeMap` for O(log n) lookups; iteration order for findings comes from
    /// `node_order` (document order), NOT this map's sorted order.
    pub nodes: BTreeMap<Slug, Node>,
    /// Node ids in DOCUMENT order (the order the validator must iterate to match
    /// the oracle's finding order). First-seen wins; duplicates are not re-listed.
    pub node_order: Vec<Slug>,
    /// id -> layer it was filed under.
    pub node_layer: BTreeMap<Slug, String>,
    /// (id, [first_layer, dup_layer]) for ids appearing in more than one layer.
    pub dup_ids: Vec<(Slug, [String; 2])>,
    /// The flat edge array (source of truth), in document order.
    pub edges: Vec<Edge>,
    /// id -> outgoing edge indices.
    pub out_edges: HashMap<Slug, Vec<usize>>,
    /// id -> incoming edge indices.
    pub in_edges: HashMap<Slug, Vec<usize>>,
    /// `nodeKindRegistry` extension kinds.
    pub node_registry: BTreeMap<String, Value>,
    /// `edgeRegistry` extension verbs.
    pub edge_registry: BTreeMap<String, Value>,
    /// `refsRegistry` extension refs keys (open-with-registry, A3). Row shape is
    /// minimal-declarative: key → `{"description": "..."}` (data overlay, D-003;
    /// no code-plugin semantics). `BTreeMap` so keys iterate sorted anywhere
    /// they serialize or render.
    pub refs_registry: BTreeMap<String, Value>,
    /// `attrEnumRegistry` extension enum tokens (open-with-registry, RES-003
    /// 2.1). Row shape: `"<verb>.<attr>"` → `["x-<ns>:token", ...]`. An `x-`
    /// value in a closed-enum position is legal ONLY if declared here
    /// (declarative data overlay, D-003). `BTreeMap` for sorted iteration at
    /// any serialization boundary.
    pub attr_enum_registry: BTreeMap<String, Value>,
}

impl Graph {
    /// Build a graph from an already-parsed document value.
    ///
    /// Mirrors `Graph.__init__`: a non-list `edges` or a non-object edge element
    /// is a load error (`exit 2`), not a lint finding.
    pub fn from_doc(doc: Value) -> Result<Self, EngineError> {
        let node_registry = obj_map(doc.get("nodeKindRegistry"));
        let edge_registry = obj_map(doc.get("edgeRegistry"));
        let refs_registry = obj_map(doc.get("refsRegistry"));
        let attr_enum_registry = obj_map(doc.get("attrEnumRegistry"));

        let mut nodes = BTreeMap::new();
        let mut node_order: Vec<Slug> = Vec::new();
        let mut node_layer = BTreeMap::new();
        let mut dup_ids = Vec::new();
        // Iterate layers + nodes in DOCUMENT order (serde_json `preserve_order` is
        // on, so object iteration is insertion order). This matches the oracle's
        // `for layer, nmap in doc["nodes"].items()` so dup detection AND the
        // finding order downstream are document-faithful. `seen` enforces
        // first-occurrence-wins.
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        if let Some(layers) = doc.get("nodes").and_then(Value::as_object) {
            for (layer, nmap_val) in layers {
                let Some(nmap) = nmap_val.as_object() else {
                    continue;
                };
                for (nid, nval) in nmap {
                    if let Some(first_layer) = seen.get(nid) {
                        dup_ids.push((nid.clone(), [first_layer.clone(), layer.clone()]));
                    } else {
                        seen.insert(nid.clone(), layer.clone());
                        let node: Node = serde_json::from_value(nval.clone())?;
                        nodes.insert(nid.clone(), node);
                        node_order.push(nid.clone());
                        node_layer.insert(nid.clone(), layer.clone());
                    }
                }
            }
        }

        // `edges` validity: a non-array or non-object element is a load error.
        let edges: Vec<Edge> = match doc.get("edges") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(arr)) => {
                let mut out = Vec::with_capacity(arr.len());
                for (i, e) in arr.iter().enumerate() {
                    if !e.is_object() {
                        return Err(EngineError::EdgeNotObject {
                            index: i,
                            got: json_type_name(e),
                        });
                    }
                    out.push(serde_json::from_value(e.clone())?);
                }
                out
            }
            Some(other) => {
                return Err(EngineError::EdgesNotArray(json_type_name(other)));
            }
        };

        let mut out_edges: HashMap<Slug, Vec<usize>> = HashMap::new();
        let mut in_edges: HashMap<Slug, Vec<usize>> = HashMap::new();
        for (i, e) in edges.iter().enumerate() {
            if let Some(f) = e.from() {
                out_edges.entry(f.to_string()).or_default().push(i);
            }
            if let Some(t) = e.to() {
                in_edges.entry(t.to_string()).or_default().push(i);
            }
        }

        Ok(Graph {
            doc,
            nodes,
            node_order,
            node_layer,
            dup_ids,
            edges,
            out_edges,
            in_edges,
            node_registry,
            edge_registry,
            refs_registry,
            attr_enum_registry,
        })
    }

    /// The declared kind of a node, if it exists and has one.
    pub fn kind(&self, nid: &str) -> Option<&str> {
        self.nodes.get(nid).and_then(Node::kind)
    }

    /// True iff a node kind is a frozen-core token or a registered extension.
    pub fn known_node_kind(&self, k: &str) -> bool {
        core_node_kinds().contains_key(k) || self.node_registry.contains_key(k)
    }

    /// True iff an opt-in advisory lint is switched on via `meta.lints`.
    pub fn lint_enabled(&self, name: &str) -> bool {
        self.doc
            .get("meta")
            .and_then(Value::as_object)
            .and_then(|m| m.get("lints"))
            .and_then(Value::as_array)
            .is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some(name)))
    }

    /// Resolve the signature for any known edge type (core or registry), or
    /// `None` if unknown. Registry rows declare attrs but the spike does not
    /// force them (`req`/`enums` empty), matching the oracle.
    pub fn edge_sig(&self, etype: &str) -> Option<ResolvedSig> {
        if let Some(sig) = core_edges().get(etype) {
            return Some(ResolvedSig::from(sig));
        }
        if let Some(row) = self.edge_registry.get(etype) {
            return Some(ResolvedSig {
                from: str_vec(row.get("from_kinds")),
                to: str_vec(row.get("to_kinds")),
                req: vec![],
                enums: vec![],
            });
        }
        None
    }

    /// Whether an `x-` extension value is declared for `"<verb>.<attr>"` in
    /// the graph's `attrEnumRegistry`. Non-array rows declare nothing.
    pub fn attr_enum_declared(&self, etype: &str, attr: &str, val: &str) -> bool {
        self.attr_enum_registry
            .get(&format!("{etype}.{attr}"))
            .and_then(Value::as_array)
            .is_some_and(|arr| arr.iter().any(|v| v.as_str() == Some(val)))
    }

    /// For a registry (boundary/extension) node, its `legal_out_edges` list, or
    /// `None` if the node is core (no registry governance).
    pub fn legal_out_for_node(&self, nid: &str) -> Option<Vec<String>> {
        let k = self.kind(nid)?;
        let row = self.node_registry.get(k)?;
        // Key absent → `None` (no constraint; dual-axis passes). Present → the list.
        row.get("legal_out_edges").map(|v| str_vec(Some(v)))
    }

    /// For a registry node, its `legal_in_edges` list, or `None` if core.
    pub fn legal_in_for_node(&self, nid: &str) -> Option<Vec<String>> {
        let k = self.kind(nid)?;
        let row = self.node_registry.get(k)?;
        row.get("legal_in_edges").map(|v| str_vec(Some(v)))
    }

    /// The raw `meta.provenance` value, if the document carries one. Shape is
    /// validated by `validate` (E_PROVENANCE_*); this is the passthrough read
    /// used by the `--json` report and the W_ANCHORLESS gate.
    pub fn provenance(&self) -> Option<&Value> {
        self.doc.get("meta")?.get("provenance")
    }

    /// The raw `meta.slice_of` annotation stamped by `maapp export --slice`
    /// (LIFECYCLE-VERBS-SPEC §5). Presence puts `validate` in slice mode:
    /// `W_ORPHAN` is suppressed (slicing creates boundary orphans by
    /// construction). Shape is validated (E_SLICE_OF_MALFORMED).
    pub fn slice_of(&self) -> Option<&Value> {
        self.doc.get("meta")?.get("slice_of")
    }

    /// The raw `meta.waivers` list (the warning baseline: RES-004 friction
    /// #9). Shape is validated (E_WAIVER_*); matching advisory findings
    /// report as `waived` instead of `warning` and never affect exit codes.
    pub fn waivers(&self) -> Option<&Value> {
        self.doc.get("meta")?.get("waivers")
    }

    /// The raw `meta.flows` list (first-class named journeys, schema 1.4,
    /// RES-003 2.4). Shape is validated (E_FLOW_*); reachability surfaces as
    /// the advisory `W_FLOW_UNREACHABLE`.
    pub fn flows(&self) -> Option<&Value> {
        self.doc.get("meta")?.get("flows")
    }

    /// Number of declared flow entries (0 when `meta.flows` is absent or not
    /// an array) — the `validate --json` flows-count passthrough.
    pub fn flows_count(&self) -> usize {
        self.flows().and_then(Value::as_array).map_or(0, Vec::len)
    }

    /// True iff `nid` is a ROOT Trigger: kind `Trigger` with NO incoming
    /// `handles` edge (schema 1.4, RES-003 1.5). Root Triggers model
    /// system-initiated entry points and are legal flow entries.
    pub fn is_root_trigger(&self, nid: &str) -> bool {
        if self.kind(nid) != Some("Trigger") {
            return false;
        }
        !self
            .in_edges
            .get(nid)
            .into_iter()
            .flatten()
            .any(|&i| self.edges[i].edge_type() == Some("handles"))
    }

    /// The `attrs.outcome` enum declared by a boundary node's registry kind.
    pub fn outcome_enum(&self, nid: &str) -> Option<Vec<String>> {
        let k = self.kind(nid)?;
        let row = self.node_registry.get(k)?;
        row.get("attrs")
            .and_then(|a| a.get("outcome"))
            .map(|v| str_vec(Some(v)))
    }
}

/// Coerce a JSON value to an object map of owned key/value pairs (empty if absent
/// or not an object), mirroring the oracle's `doc.get(k, {}) or {}`.
fn obj_map(v: Option<&Value>) -> BTreeMap<String, Value> {
    v.and_then(Value::as_object)
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// Coerce a JSON value to a `Vec<String>` (string elements only), empty otherwise.
fn str_vec(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Python-style type name for a JSON value, for load-error messages.
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
