//! `maapp fmt [--check]` (T3a) — the canonicality verb.
//!
//! **Motivating theme (ADOPTION-HARDENING theme 3, "unenforced conventions"):**
//! "never hand-edit the graph" was prose with no detector. The mutation verbs
//! already write canonical form (`crate::export::canonical_doc` — spec §1), but
//! a hand edit rarely matches it and was indistinguishable from a verb-written
//! change. `fmt` makes canonicality checkable:
//!
//! - `fmt <file>` rewrites the graph to canonical form atomically (temp +
//!   rename), exactly as the mutation verbs do.
//! - `fmt <file> --check` writes NOTHING and exits 1 iff the on-disk bytes are
//!   not already canonical (a linter gate, `gofmt -l` style — the CLI names the
//!   offending file path, "paths only", never a content dump).
//!
//! Canonicality is a pure BYTE comparison of the file against its canonical
//! projection, so it catches every non-canonical form (key order, layer order,
//! edge order, whitespace) — differences the SEMANTIC `diff` is blind to by
//! design. `fmt` is about FORM, not validity: it canonicalizes any *loadable*
//! graph and never runs the validator (a load error is still exit 2).
//!
//! Native-only (std::fs temp+rename writes), cfg-gated like `mutate`/`drift`;
//! the pure canonical projection it emits through lives wasm-clean in `export`.

use crate::error::EngineError;
use crate::export::canonical_doc;
use crate::graph::Graph;
use std::path::Path;

/// What `fmt` did / would do (the CLI maps this to an exit code + message).
#[derive(Debug, PartialEq, Eq)]
pub enum FmtOutcome {
    /// The file was already canonical: nothing to do (exit 0, both modes).
    AlreadyCanonical,
    /// `fmt` (no `--check`) rewrote the file to canonical form (exit 0).
    Rewrote,
    /// `fmt --check`: the file is non-canonical; nothing written (exit 1).
    NeedsFormat,
}

/// The canonical bytes for a loaded graph: pretty two-space JSON + one trailing
/// newline — byte-identical to what the mutation verbs write (spec §1).
fn canonical_bytes(g: &Graph) -> Result<Vec<u8>, EngineError> {
    let mut bytes = serde_json::to_vec_pretty(&canonical_doc(g))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Canonicalize `path` (or, with `check`, only report whether it is canonical).
///
/// Loads the graph (a malformed/unreadable file is a load error → the CLI maps
/// it to exit 2), compares the on-disk bytes to the canonical projection, and:
/// - equal → [`FmtOutcome::AlreadyCanonical`] (no write);
/// - differ + `check` → [`FmtOutcome::NeedsFormat`] (no write);
/// - differ + `!check` → atomic temp+rename rewrite → [`FmtOutcome::Rewrote`].
pub fn fmt(path: &Path, check: bool) -> Result<FmtOutcome, EngineError> {
    let g = crate::load_graph_from_path(path)?;
    let want = canonical_bytes(&g)?;
    let have = std::fs::read(path)?;
    if want == have {
        return Ok(FmtOutcome::AlreadyCanonical);
    }
    if check {
        return Ok(FmtOutcome::NeedsFormat);
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &want)?;
    std::fs::rename(&tmp, path)?;
    Ok(FmtOutcome::Rewrote)
}
