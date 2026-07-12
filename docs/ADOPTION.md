# Adopting maapp into an existing project

Two starting points are common, and this guide covers both:

- **You already have bespoke app-graph or architecture tooling** — a hand-rolled
  dependency checker, an architecture-lint gate, a diagram generator — and you want to
  cut over to maapp without a risky big-bang rewrite. See [Cutover](#cutover-replacing-bespoke-graph-tooling).
- **You have an app but no graph yet** — you want to bootstrap one on a live codebase and
  keep it honest as the code moves. See [Cold start](#cold-start-bootstrapping-a-graph-on-an-existing-app).

Both paths end at the same living loop (`validate` → `check-drift` → `stamp`) documented in
[INTEGRATIONS.md](INTEGRATIONS.md). Read the [caveats](#adoption-caveats) before you start;
they are cheap to honor up front and expensive to discover late.

maapp is early (v0.1.1). Pin an exact version everywhere you install it — the schema minor,
the CLI surface, and the release layout can still shift between minors.

---

## Cutover: replacing bespoke graph tooling

The failure mode of a cutover is demanding that the new engine **absorb** the old one wholesale
— "maapp must reproduce every gate our custom tool enforced before we can delete it." That
criterion is ill-posed: maapp is a typed-graph data engine, and much of what a bespoke tool
enforces is **project policy** a data engine can never own. The cutover succeeds by separating
the two.

### First, classify every incumbent surface

Inventory what the incumbent tool actually does, and split it two ways.

1. **Enforcing vs non-enforcing.** An *enforcing* surface fails CI or blocks a merge (an
   architecture-lint gate, a dependency-boundary check, a pre-commit reject). A *non-enforcing*
   surface only informs (a visualization, a dashboard, a generated diagram, an ad-hoc query
   script). Nothing depends on a non-enforcing surface passing, so it carries no migration risk.
2. **Structural-data vs domain-policy.** Within each enforcing surface, separate the questions
   maapp answers directly (blast radius, reachability, nav topology, orphans, "what depends on
   this store") from the *policy* layered on top ("no screen may bind more than three stores",
   "every backend op must be reachable from a user action"). maapp answers the first; the second
   stays yours.

### The five phases

Run them in order. Each phase is independently revertible, and both tools coexist until the last.

| Phase | Do | Removes anything? |
|---|---|---|
| **0. Inventory** | Classify every incumbent surface (above). | No |
| **1. Wrap** | Install maapp additively. Ingest or author the graph, `maapp validate` to 0 errors. Both tools run side by side. | No |
| **2. Repoint** | Rewrite each domain gate to read `maapp … --json` instead of the incumbent's output. Point the agent's routing section at maapp for structural queries. | No |
| **3. Delete legacy** | Cut every non-enforcing incumbent surface immediately. Retire each *enforcing* incumbent surface only once its maapp-backed replacement is green in CI. | Yes |
| **4. Enforce** | Pin the maapp CI gate to a checksummed release binary, turn on the drift-baseline ratchet, run a bounded burn-in, then flip the gate to required. | No |

### Domain gates are a permanent layer, not a migration failure

The project-specific gates you rewrite in Phase 2 are the **intended permanent end state**, not
scaffolding to delete later. A thin script that reads maapp's stable, byte-deterministic `--json`
and asserts your architecture rules is exactly the right shape — the same shape as every mature
architecture tool:

- **ESLint** parses your source into an AST it owns; your `.eslintrc` rules are the policy layer.
- **dependency-cruiser** builds the module graph; your `depcruise` rules are the policy layer.
- **ArchUnit** reads the compiled type graph; your architecture tests are the policy layer.

maapp is the graph engine; your gates are the policy layer over its `--json`. Do not try to fold
those gates into maapp, and do not treat their continued existence as unfinished migration. Write
**per-surface** acceptance criteria ("gate X now reads maapp `--json` and reproduces its prior
verdicts on the last N PRs"), never a single monolithic "maapp replaces the whole incumbent" bar.

### Pin the binary, and run a bounded burn-in

CI must install maapp from a **pinned, content-verified** release, never `latest` and never an
unpinned `curl | sh`. Each release ships a `.sha256` per artifact plus a unified `sha256.sum`, so
the gate can verify by content before it installs:

```sh
MAAPP_VERSION=v0.1.1
curl -fsSL -o maapp.tar.xz \
  "https://github.com/<owner>/maapp/releases/download/${MAAPP_VERSION}/maapp-x86_64-unknown-linux-gnu.tar.xz"
echo "<pinned-sha256>  maapp.tar.xz" | sha256sum -c -   # fails closed on any mismatch
tar -xf maapp.tar.xz
```

Bump `MAAPP_VERSION` and the pinned checksum together, deliberately, in a reviewed PR.

Keep the maapp gate **non-required** through a bounded burn-in with an explicit decommission
trigger written down in advance — for example, *"10 green PRs or two weeks, whichever comes first,
with maapp's own `validate` + `check-drift` green and zero false-positive noise."* Only then flip
the gate to required and retire the last incumbent enforcing surface. The burn-in measures maapp's
own checks; it is never a re-litigation of "does maapp absorb the incumbent's domain gates" (it
does not, by design — see above).

---

## Cold start: bootstrapping a graph on an existing app

Bootstrapping onto a live codebase is a **noise-before-signal** problem: the drift machinery
(`check-drift`, the drift-nudge hook, the CI gate) works off source *anchors*, and a freshly
ingested graph has few or none. Adopt the graph the same way you would adopt a linter on a large
legacy repo — ratchet, do not big-bang.

### The path

```sh
# 1. Ingest. The /maapp skill authors .maapp/graph.json from the existing app.
maapp validate .maapp/graph.json          # drive this to 0 errors before anything else

# 2. Establish the ratchet floor. Snapshot today's unmapped debt as tolerated.
maapp check-drift .maapp/graph.json --repo . --write-baseline .maapp/drift-baseline.json

# 3. Stamp provenance and commit the graph + baseline together.
maapp stamp .maapp/graph.json --repo .
```

From there the floor rises **one feature-slice at a time**. When you touch a slice of the app:

1. Anchor the nodes that slice touches — set each node's `refs.source` to the file(s) it maps to
   (via the CRUD verbs; never hand-edit the JSON).
2. `maapp validate` to 0, then `maapp stamp .maapp/graph.json --repo .`.
3. Refresh the baseline so the newly anchored paths leave the tolerated set:
   `maapp check-drift .maapp/graph.json --repo . --write-baseline .maapp/drift-baseline.json`.

The gate now blocks a **new** unmapped path, any **stale** anchor, or any **rot**, while the
still-unanchored legacy debt stays tolerated. Each slice you anchor shrinks the tolerated set —
the floor ratchets up, and the graph never regresses.

### Footguns (v0.1.1)

- **`validate` does not check anchor paths on disk — deliberately.** It is a structural, env-free,
  deterministic lint: it never opens the repo to confirm a `refs.source` path exists. On-disk
  anchor reconciliation is `check-drift`'s job, not `validate`'s. A graph can validate 0/0 and
  still point every anchor at a deleted file — `check-drift` is what catches that (as rot).
- **`check-drift` sees committed history only** (`asOf..HEAD`). Uncommitted, in-session edits are
  invisible to it; the Claude Code drift-nudge hook is what flags those between commits.
- **`--write-baseline` also reads committed history only.** Refresh the baseline **as the branch's
  last commit** — after the slice's anchors and `stamp` are already committed. Write it earlier and
  every commit you add afterward re-appears as a new unmapped path against a stale baseline.
- **Never refresh the baseline *before* anchoring.** A blind `--write-baseline` snapshots whatever
  is currently unmapped and silently re-absorbs new debt into the tolerated set, defeating the
  ratchet. Anchor first, then raise the floor.
- The graph file itself and everything under `.maapp/` are excluded from the unmapped computation
  automatically, so committing a freshly stamped graph does not report itself as unmapped forever.

---

## Adoption caveats

**Never pre-brand an incumbent's artifacts with maapp's name.** During a cutover it is tempting to
rename the old tool's outputs — its visualization, its report, its config directory — to the new
name before maapp actually produces them. Don't. A legacy artifact wearing the new tool's name
reads as maapp output to the next person and to the next agent, and the wrong file gets trusted, or
worse, deleted as "the old one" when it is the only one. Keep the incumbent's artifacts under the
incumbent's name until maapp genuinely owns each surface; rename only at the moment ownership
actually transfers (Phase 3).

**Pin an exact version.** maapp is pre-1.0. The graph schema minor, the CLI flag surface, and the
release artifact layout can change between minors. Pin `v0.1.1` (or whatever you adopt) in your
installer, your CI gate, and your contributor docs, and upgrade deliberately — read the
[CHANGELOG](../CHANGELOG.md) and run `maapp validate` (and `maapp migrate` if the schema minor
moved) before you commit the bump.
