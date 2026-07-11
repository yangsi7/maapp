//! Data model for a maapp-graph document.
//!
//! Design notes (rust-conventions / ENGINE-CONTRACT):
//! - `kind` and edge `type` are `String`, NOT closed enums: the frozen-core
//!   vocabulary is closed but `nodeKindRegistry` / `edgeRegistry` add namespaced
//!   `x-ns:` extensions, so validity is a membership check (see `validate`), not
//!   a deserialization constraint.
//! - Node `attrs` and edge attrs are arbitrary JSON maps captured with
//!   `#[serde(flatten)]` / a `serde_json::Map`, so round-tripping preserves
//!   author content the validator does not model.
//! - `refs` is a closed-key string map; we keep it as a `BTreeMap<String, Value>`
//!   so malformed refs (non-string values, unknown keys) survive parsing and are
//!   caught by the validator rather than rejected by serde.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A node slug, e.g. `screen:chat/ConversationList`.
pub type Slug = String;

/// The five layers a node can be filed under.
pub const LAYERS: [&str; 5] = ["surface", "logic", "substrate", "policy", "boundary"];

/// A single node as it appears under `nodes.<layer>.<slug>`.
///
/// Unknown top-level keys are captured in `extra` so a parse → serialize round
/// trip is lossless. `kind` may be absent (the validator emits `E_NODE_NO_KIND`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Closed-key string map; kept loose so malformed refs reach the validator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refs: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Value>,
    /// Any other author-supplied keys, preserved for lossless round-trip.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Node {
    /// The node's declared kind, if any.
    pub fn kind(&self) -> Option<&str> {
        self.kind.as_deref()
    }
}

/// An edge from the flat `edges[]` array.
///
/// `type`, `from`, `to` are the structural triple; all remaining attrs (event,
/// present, mode, when, …) land in `attrs` so the validator can inspect any of
/// them and a round trip is lossless. `attrs` is a `BTreeMap` so re-serialization
/// is deterministic regardless of input key order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// All remaining edge attributes, key-ordered for deterministic emit.
    #[serde(flatten)]
    pub attrs: BTreeMap<String, Value>,
}

impl Edge {
    pub fn edge_type(&self) -> Option<&str> {
        self.r#type.as_deref()
    }
    pub fn from(&self) -> Option<&str> {
        self.from.as_deref()
    }
    pub fn to(&self) -> Option<&str> {
        self.to.as_deref()
    }
    /// Look up an edge attribute by name.
    pub fn attr(&self, name: &str) -> Option<&Value> {
        self.attrs.get(name)
    }
    /// The `eid` display key used in findings: `{type}:{from}->{to}`.
    ///
    /// A missing endpoint renders as the literal `None` to match the oracle,
    /// whose `eid = f"{etype}:{frm}->{to}"` interpolates Python's `None`
    /// (still true in the live oracle as of the 2026-07-09 re-vendor,
    /// graph.py:368). COUPLING: finding-parity tests depend on these exact
    /// bytes — do NOT swap in "?" or similar until the oracle itself changes.
    pub fn eid(&self) -> String {
        format!(
            "{}:{}->{}",
            self.r#type.as_deref().unwrap_or("None"),
            self.from.as_deref().unwrap_or("None"),
            self.to.as_deref().unwrap_or("None"),
        )
    }
}
