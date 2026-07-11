// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `maapp schema` — the canonical machine-readable contract (ROADMAP v2
//! Phase-1 move 2, feature 3; closes RES-003 finding 2.3-no-machine-schema,
//! RES-004 friction #8: agents burned ~270K tokens re-deriving gate contracts).
//!
//! The JSON Schema is GENERATED from the same Rust vocabulary tables `validate`
//! uses (core_node_kinds / core_edges / refs_keys / provenance shape / version
//! window) — never hand-authored in parallel. The committed artifact
//! `schema/maapp-graph.schema.json` is byte-locked to the emitter: drift
//! between code tables and the committed schema = red test.

use assert_cmd::Command;

const ARTIFACT: &str = "schema/maapp-graph.schema.json";

fn maapp() -> Command {
    Command::cargo_bin("maapp").expect("binary builds")
}

/// THE drift lock: `maapp schema` stdout is byte-identical to the committed
/// schema artifact. Editing the Rust tables without re-emitting the artifact
/// (or hand-editing the artifact) turns this red.
#[test]
fn schema_cli_output_matches_committed_artifact() {
    let committed = std::fs::read(ARTIFACT)
        .unwrap_or_else(|e| panic!("read {ARTIFACT} (commit the emitted schema): {e}"));
    let out = maapp().arg("schema").assert().success();
    let stdout = &out.get_output().stdout;
    assert!(
        stdout == &committed,
        "`maapp schema` output differs from committed {ARTIFACT} — re-emit it \
         (cargo run -- schema > {ARTIFACT})"
    );
}

/// The emitter is a pure function: two calls are byte-identical, parse as
/// JSON, and end in exactly one trailing newline.
#[test]
fn schema_emit_is_deterministic_and_parses() {
    let a = maapp::schema_bytes().expect("schema emits");
    let b = maapp::schema_bytes().expect("schema emits");
    assert_eq!(a, b, "schema emit must be byte-stable across calls");
    assert_eq!(a.last(), Some(&b'\n'), "one trailing newline");
    let v: serde_json::Value = serde_json::from_slice(&a).expect("schema parses as JSON");
    assert!(v.is_object());
}

/// The schema carries the engine tables: draft 2020-12 header, pinned
/// canonical `$id`, all 14 core edge signatures, the 17 core node kinds by
/// layer, the refs core keys incl. `source`, and the provenance shape.
#[test]
fn schema_carries_engine_tables() {
    let v = maapp::schema_value();

    // Header rules.
    assert_eq!(
        v["$schema"],
        serde_json::json!("https://json-schema.org/draft/2020-12/schema")
    );
    assert_eq!(v["$id"], serde_json::json!("maapp-graph"));
    let desc = v["description"].as_str().expect("description string");
    assert!(
        desc.contains("<app>-graph") && desc.contains("compat"),
        "description must note the <app>-graph suffix acceptance as a compat rule, got: {desc}"
    );

    // schema/version header property rules.
    assert_eq!(
        v["properties"]["schema"]["pattern"],
        serde_json::json!("^.+-graph$")
    );
    assert_eq!(
        v["properties"]["version"]["pattern"],
        serde_json::json!("^[0-9]+\\.[0-9]+$")
    );

    // All 14 core edge signatures, keyed by `const` type in the allOf table.
    let all_of = v["$defs"]["edge"]["allOf"].as_array().expect("edge allOf");
    assert_eq!(all_of.len(), 14, "one if/then signature per core verb");
    let verbs: Vec<&str> = all_of
        .iter()
        .map(|c| c["if"]["properties"]["type"]["const"].as_str().unwrap())
        .collect();
    for verb in [
        "binds",
        "derivesFrom",
        "dismisses",
        "emits",
        "fires",
        "guardedBy",
        "handles",
        "invokes",
        "navigates",
        "produces",
        "reads",
        "renders",
        "setsViewState",
        "writes",
    ] {
        assert!(verbs.contains(&verb), "missing edge signature for '{verb}'");
    }
    // Signatures carry required attrs + enums + from/to kind-set annotations.
    let navigates = all_of
        .iter()
        .find(|c| c["if"]["properties"]["type"]["const"] == "navigates")
        .unwrap();
    assert_eq!(
        navigates["then"]["required"],
        serde_json::json!(["present"])
    );
    assert!(navigates["then"]["properties"]["present"].is_object());
    assert!(
        navigates["then"]["x-maapp-fromKinds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k == "NavAction"),
        "navigates must annotate its from-kind set"
    );

    // Node kinds per layer: 17 core kinds distributed over the layer map.
    let layers = v["properties"]["nodes"]["properties"]
        .as_object()
        .expect("layer map");
    let mut kind_count = 0;
    for (_, layer_schema) in layers {
        kind_count += layer_schema["x-maapp-coreKinds"]
            .as_array()
            .map_or(0, Vec::len);
    }
    assert_eq!(
        kind_count, 17,
        "all 17 core node kinds distributed by layer"
    );
    assert!(
        layers["surface"]["x-maapp-coreKinds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k == "Screen")
    );

    // Refs core keys incl. source.
    let refs_props = v["$defs"]["refs"]["properties"].as_object().expect("refs");
    for key in [
        "data", "decision", "design", "slice", "source", "spec", "tokens",
    ] {
        assert!(
            refs_props.contains_key(key),
            "missing refs core key '{key}'"
        );
    }

    // Provenance shape: closed object, required origin, closed enum.
    let prov = &v["$defs"]["provenance"];
    assert_eq!(prov["required"], serde_json::json!(["origin"]));
    assert_eq!(prov["additionalProperties"], serde_json::json!(false));
    assert_eq!(
        prov["properties"]["origin"]["enum"],
        serde_json::json!(["generated", "ingested", "ratified"])
    );
    assert_eq!(
        prov["properties"]["fidelity"]["minimum"],
        serde_json::json!(0)
    );
    assert_eq!(
        prov["properties"]["fidelity"]["maximum"],
        serde_json::json!(1)
    );

    // Engine-read meta keys (closed shapes, cross-checked with validate):
    // slice_of (export --slice annotation) + waivers (warning baseline).
    let meta_props = v["properties"]["meta"]["properties"]
        .as_object()
        .expect("meta properties");
    for key in ["lints", "provenance", "slice_of", "waivers"] {
        assert!(
            meta_props.contains_key(key),
            "missing engine-read meta key '{key}'"
        );
    }
    let slice_of = &v["$defs"]["sliceOf"];
    assert_eq!(slice_of["required"], serde_json::json!(["selector"]));
    assert_eq!(slice_of["additionalProperties"], serde_json::json!(false));
    assert_eq!(
        slice_of["properties"]["depth"]["minimum"],
        serde_json::json!(0)
    );
    let waiver = &v["$defs"]["waiver"];
    assert_eq!(
        waiver["required"],
        serde_json::json!(["code", "node", "reason"])
    );
    assert_eq!(waiver["additionalProperties"], serde_json::json!(false));
    assert_eq!(
        waiver["properties"]["code"]["pattern"],
        serde_json::json!("^W_"),
        "waivable codes are W_* only (E_ never waivable)"
    );

    // Registries' shapes are described (data overlays, D-003).
    for reg in [
        "attrEnumRegistry",
        "edgeRegistry",
        "nodeKindRegistry",
        "refsRegistry",
    ] {
        assert!(
            v["properties"][reg].is_object(),
            "missing registry shape '{reg}'"
        );
    }

    // Closed-enum x- escape hatch is registry-gated (RES-003 2.1): the enum
    // attr schema names attrEnumRegistry, not an unconditional x- bypass.
    let writes = all_of
        .iter()
        .find(|c| c["if"]["properties"]["type"]["const"] == "writes")
        .unwrap();
    let mode_x_desc = writes["then"]["properties"]["mode"]["anyOf"][1]["description"]
        .as_str()
        .expect("x- arm description");
    assert!(
        mode_x_desc.contains("attrEnumRegistry"),
        "enum x- arm must teach the attrEnumRegistry gate, got: {mode_x_desc}"
    );

    // emits.effect is documented as explicitly UNCHECKED (open string hint),
    // not silently absent from the signature table.
    let emits = all_of
        .iter()
        .find(|c| c["if"]["properties"]["type"]["const"] == "emits")
        .unwrap();
    let effect_desc = emits["then"]["properties"]["effect"]["description"]
        .as_str()
        .expect("emits.effect description");
    assert!(
        effect_desc.contains("NOT validator-checked"),
        "emits.effect must be documented as explicitly unchecked, got: {effect_desc}"
    );
}

/// Deterministic key order: table-derived maps emit in sorted key order
/// (BTreeMap discipline at the serialization boundary).
#[test]
fn schema_table_maps_emit_sorted() {
    let v = maapp::schema_value();
    for (obj, what) in [
        (v["properties"].as_object().unwrap(), "top-level properties"),
        (v["$defs"].as_object().unwrap(), "$defs"),
        (
            v["$defs"]["refs"]["properties"].as_object().unwrap(),
            "refs properties",
        ),
        (
            v["properties"]["nodes"]["properties"].as_object().unwrap(),
            "layer map",
        ),
        (
            v["$defs"]["provenance"]["properties"].as_object().unwrap(),
            "provenance properties",
        ),
    ] {
        let keys: Vec<&String> = obj.keys().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "{what} keys must be sorted");
    }
    // The edge signature table iterates the BTreeMap: verbs in sorted order.
    let verbs: Vec<&str> = v["$defs"]["edge"]["allOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["if"]["properties"]["type"]["const"].as_str().unwrap())
        .collect();
    let mut sorted = verbs.clone();
    sorted.sort_unstable();
    assert_eq!(verbs, sorted, "edge signatures must be verb-sorted");
}

// ---------------------------------------------------------------------------
// Schema 1.4 vocabulary batch (D-009 Phase-3 step 9)
// ---------------------------------------------------------------------------

/// The `produces.verification` core enum is exactly {unverified, verified}
/// (sorted); any domain-specific token is out of core and lives in a per-graph
/// registry declaration.
#[test]
fn schema_produces_verification_enum_is_domain_neutral() {
    let v = maapp::schema_value();
    let all_of = v["$defs"]["edge"]["allOf"].as_array().expect("edge allOf");
    let produces = all_of
        .iter()
        .find(|c| c["if"]["properties"]["type"]["const"] == "produces")
        .expect("produces signature");
    assert_eq!(
        produces["then"]["properties"]["verification"]["anyOf"][0]["enum"],
        serde_json::json!(["unverified", "verified"]),
        "core verification enum must be domain-neutral"
    );
}

/// F1: the Trigger `attrs.cause` closed enum rides the node def as an
/// if/then on kind == Trigger (open via the `"Trigger.cause"` registry key).
#[test]
fn schema_carries_trigger_cause_enum() {
    let v = maapp::schema_value();
    let all_of = v["$defs"]["node"]["allOf"]
        .as_array()
        .expect("node allOf (Trigger cause signature)");
    let trigger = all_of
        .iter()
        .find(|c| c["if"]["properties"]["kind"]["const"] == "Trigger")
        .expect("Trigger node signature");
    assert_eq!(
        trigger["then"]["properties"]["attrs"]["properties"]["cause"]["anyOf"][0]["enum"],
        serde_json::json!([
            "callback",
            "cron",
            "deep-link",
            "message",
            "schedule",
            "system",
            "webhook"
        ]),
        "Trigger.cause core enum (sorted) must ride the schema"
    );
}

/// F2: `meta.flows` is an engine-read key with a closed per-entry $defs shape.
#[test]
fn schema_carries_meta_flows_shape() {
    let v = maapp::schema_value();
    let meta_props = v["properties"]["meta"]["properties"]
        .as_object()
        .expect("meta properties");
    assert!(
        meta_props.contains_key("flows"),
        "meta.flows must be an engine-read key"
    );
    let flow = &v["$defs"]["flow"];
    assert_eq!(
        flow["required"],
        serde_json::json!(["entry", "name", "terminals"])
    );
    assert_eq!(flow["additionalProperties"], serde_json::json!(false));
    assert!(flow["properties"]["via"].is_object(), "via is optional");
}

/// F3: `derivesFrom` from-kinds gains StateStore (the mirror-store linking
/// convention, RES-003 2.5).
#[test]
fn schema_derivesfrom_from_kinds_include_statestore() {
    let v = maapp::schema_value();
    let all_of = v["$defs"]["edge"]["allOf"].as_array().expect("edge allOf");
    let derives = all_of
        .iter()
        .find(|c| c["if"]["properties"]["type"]["const"] == "derivesFrom")
        .expect("derivesFrom signature");
    assert_eq!(
        derives["then"]["x-maapp-fromKinds"],
        serde_json::json!(["Assertion", "StateStore", "ViewState"])
    );
}

/// F4 policy: attrEnumRegistry rows may declare bare legacy tokens — the
/// emitted schema no longer pins the `^x-` pattern on declared values.
#[test]
fn schema_attr_enum_registry_admits_bare_tokens() {
    let v = maapp::schema_value();
    let items = &v["properties"]["attrEnumRegistry"]["additionalProperties"]["items"];
    assert_eq!(items["type"], serde_json::json!("string"));
    assert!(
        items.get("pattern").is_none(),
        "declared tokens are no longer pattern-locked to ^x- (1.4 policy)"
    );
}
