//! `maapp migrate <file> [--to <minor>]` (T5) — mechanical schema upgrade.
//!
//! **Motivating theme (ADOPTION-HARDENING theme 5, "invisible version drift"):**
//! adopter graphs rot on an old schema because `validate` was silent when a
//! graph trailed the engine (the pilot app stuck at 1.3, missing `meta.flows`
//! and `Trigger.attrs.cause`) and there was no upgrade verb. `migrate` is the
//! upgrade path; the companion `W_VERSION_BEHIND` advisory (in `validate`)
//! points at it.
//!
//! The 1.3 -> 1.4 delta is entirely **additive and optional** — `meta.flows`,
//! `Trigger.attrs.cause`, and `attrEnumRegistry` namespacing are all new OPT-IN
//! structures a 1.3 graph simply lacks. So the mechanical minor upgrade is a
//! `version` bump: an existing document is already a valid target-minor
//! document once its stamp catches up. The write is a MINIMAL in-place bump —
//! it preserves the document's existing key/node/edge order (a version upgrade
//! must never wholesale-reorder the user's authored file; run `fmt` for that)
//! — guarded by the same no-regression check as the mutation verbs, so a bad
//! graph never gets worse. A canonical graph stays canonical (only the version
//! value changes); a narratively-ordered one keeps its narrative order.
//!
//! Only same-major minor upgrades are mechanical/safe: a downgrade, a
//! cross-major migration, an unknown-future target, or a graph with no
//! parseable version are all refused (exit 2), file untouched.
//!
//! Native-only (rewrites the graph file on disk), cfg-gated like `mutate`.

use crate::error::EngineError;
use crate::graph::Graph;
use crate::validate::{Finding, KNOWN_MAJOR, KNOWN_MINOR_MAX, validate};
use serde_json::Value;
use std::path::Path;

/// What `migrate` did: the from/to minors, and whether it wrote (a graph
/// already at the target is a no-op).
#[derive(Debug)]
pub struct MigrateOutcome {
    /// The graph's version before migration, e.g. `"1.3"`.
    pub from: String,
    /// The target version, e.g. `"1.4"`.
    pub to: String,
    /// `false` when the graph was already at the target (nothing written).
    pub changed: bool,
}

/// Parse a `MAJOR.MINOR` version string into `(major, minor)`.
fn parse_version(v: &str) -> Option<(u64, u64)> {
    let (maj, min) = v.split_once('.')?;
    Some((maj.parse().ok()?, min.parse().ok()?))
}

/// Parse a `--to` value: a bare MINOR (`"4"` → engine major) or a MAJOR.MINOR
/// string (`"1.4"`).
fn parse_target(to: &str) -> Option<(u64, u64)> {
    if to.contains('.') {
        parse_version(to)
    } else {
        Some((KNOWN_MAJOR, to.parse().ok()?))
    }
}

/// Upgrade `path`'s schema to the engine's latest minor (or `--to`). The
/// mechanical additive upgrade bumps `version` and rewrites in canonical form
/// via the shared no-regression commit. Errors (all exit 2, file untouched):
/// no parseable version, cross-major, unknown-future target, or downgrade.
pub fn migrate(path: &Path, to: Option<&str>) -> Result<MigrateOutcome, EngineError> {
    let g = crate::load_graph_from_path(path)?;

    // Source version — must be a parseable MAJOR.MINOR of the known major.
    let (from_major, from_minor) = g
        .doc
        .get("version")
        .and_then(Value::as_str)
        .and_then(parse_version)
        .ok_or_else(|| EngineError::MigrateNoVersion(format!("{KNOWN_MAJOR}.{KNOWN_MINOR_MAX}")))?;
    if from_major != KNOWN_MAJOR {
        return Err(EngineError::MigrateAcrossMajor {
            from_major,
            engine_major: KNOWN_MAJOR,
        });
    }

    // Target version — engine latest by default; `--to` must be a known minor.
    let (to_major, to_minor) = match to {
        Some(s) => parse_target(s).ok_or_else(|| EngineError::MigrateBadTarget(s.to_string()))?,
        None => (KNOWN_MAJOR, KNOWN_MINOR_MAX),
    };
    if to_major != KNOWN_MAJOR || to_minor > KNOWN_MINOR_MAX {
        return Err(EngineError::MigrateUnknownTarget {
            to: format!("{to_major}.{to_minor}"),
            engine_major: KNOWN_MAJOR,
            engine_minor_max: KNOWN_MINOR_MAX,
        });
    }

    let from = format!("{from_major}.{from_minor}");
    let to = format!("{to_major}.{to_minor}");
    if from_minor > to_minor {
        return Err(EngineError::MigrateDowngrade { from, to });
    }
    if from_minor == to_minor {
        // Already at the target: no write.
        return Ok(MigrateOutcome {
            from,
            to,
            changed: false,
        });
    }

    // Mechanical additive upgrade: bump `version` in place (preserving the
    // document's existing order), guarded by the mutation verbs' no-regression
    // rule — a write may never RAISE the hard-error count.
    let before = crate::mutate::hard_error_count(&g);
    let mut doc = g.doc;
    doc.as_object_mut()
        .ok_or(EngineError::MalformedDocument("top level is not an object"))?
        .insert("version".to_string(), Value::String(to.clone()));

    let check = Graph::from_doc(doc.clone())?;
    let would_be: Vec<Finding> = validate(&check).into_iter().filter(Finding::hard).collect();
    if would_be.len() > before {
        return Err(EngineError::ValidationRegression {
            before,
            after: would_be.len(),
            findings: would_be,
        });
    }

    // Minimal in-place write (pretty two-space + one trailing newline, the
    // file convention) via temp + rename, preserving authored order.
    let mut bytes = serde_json::to_vec_pretty(&doc)?;
    bytes.push(b'\n');
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;

    Ok(MigrateOutcome {
        from,
        to,
        changed: true,
    })
}
