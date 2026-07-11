# Benchmarks

updated: 2026-07-10

maapp's claim is narrow and testable: an AI agent answering structural questions about an app
(what breaks if I change X, how does this event reach the backend, which screens are orphaned)
does better with a typed, validated graph than without one. This page reports every measurement
we have, including the negative one, with the exact protocol, sample size, reproduce command,
and limitations for each. The in-crate reproduce commands below were re-run against this repo
before publishing; the full test gate was green (`cargo nextest run --all-features --locked`:
338/338 passed). Some measurements were produced in the maintainers' research repository; those
are marked as such, with their methodology and limitations preserved here.

Two honesty rules govern this page:

1. Numbers from the pre-Rust prototype era are labeled as such and kept separate from the
   current reproducible evals.
2. Negative results are published with the same prominence as positive ones. The real-codebase
   pilot below did not go the way we hoped, and that is the most informative result on this page.

## Results at a glance

| Result | Value | n | Status |
|---|---|---|---|
| [Held-out generation parity](#held-out-generation-parity-spec-to-graph) | 0.9306 (bar: 0.90, PASS) | 3 held-out apps, 6 seeded runs | Measured, artifacts committed |
| [Validator mutation recall / false positives](#validator-mutation-battery) | 25/25 recall, 0/7 FP | 25 injectors, 7 valid graphs | Reproducible in CI |
| [Deterministic ground-truth battery](#deterministic-ground-truth-battery) | 21/21 | 21 tasks | Reproducible in CI |
| [Rust vs prototype oracle differential](#rust-vs-prototype-oracle-differential) | 0 divergences | 19 committed differential snapshots | Reproducible in CI |
| [Real-codebase ingest pilot](#real-codebase-ingest-pilot) | K1 PASS, K2 FAIL (ceiling caveat) | 12 tasks x 2 arms x 2 seeds = 48 runs | Measured, transcripts committed |
| [The historical 3.7x figure](#the-historical-37x-figure) | 3.76x (prototype era) | 12-task battery, 1 app, 1 run | Historical, superseded, not a hero claim |

## Held-out generation parity (spec to graph)

**What was measured.** How faithfully the unedited `/maapp` skill turns a written product spec
(PRD) into a graph, on apps it never saw during development. The metric is P2: a weighted mean
of set-F1 over 6 structural classes (screens, nav_edges, flows, stores, side_effects, guards),
computed by a frozen deterministic scorer with a logged, replayable LLM alignment layer for
semantic matches (`p2_aligned`). Because the reference graphs themselves only score ~0.59
against independently authored refspecs (the cross-author agreement ceiling), the reported
number is a parity ratio: skill score divided by that ceiling.

**Protocol.** 3 frozen held-out apps (onboarding-variant, media, settings-account), never used
for skill iteration. Generation runs are headless `claude -p` (sonnet driver); scoring uses the
3-model panel judge, not the cheaper in-loop judge. Pass bar, fixed in advance: parity >= 0.90
AND every held-out graph validates with 0 errors / 0 warnings. Spec: `docs/spec/SKILL-EVAL-SPEC.md`.

**Result (2026-06-27, iteration-0 unedited skill).**

| Held-out app | Skill p2_aligned (seeds) | Ceiling | Per-app parity |
|---|---|---|---|
| onboarding-variant | 0.5962 (3 seeds) | 0.6687 | 0.892 |
| media | 0.4206 (1 seed) | 0.4219 | 0.997 |
| settings-account | 0.6360 (2 seeds) | 0.6854 | 0.928 |
| **Aggregate** | **0.5509** | **0.5920** | **0.9306 >= 0.90, PASS** |

All 3 held-out graphs validate 0 errors / 0 warnings.

**Reproduce.** *Reproduced in the maintainers' research repository; the in-crate batteries below are fully reproducible here.*

**Limitations.** Small n (3 apps, 1 to 3 seeds each). The refspecs and PRDs were authored by
us. Semantic alignment uses LLM judges (panel-checked, decisions logged and replayable, but
still judges). Single model family for generation. Parity is relative to a ~0.59 cross-author
ceiling, not an absolute accuracy; a parity of 0.93 does not mean 93% of the app is captured.

## Validator mutation battery

**What was measured.** Whether `maapp validate` actually catches broken graphs, and whether it
stays quiet on valid ones. 25 distinct defect classes are injected into known-good graphs
(dangling edges, illegal kinds, duplicate ids, branch-contract violations, layer mismatches,
containment-illegal renders, and so on); each must be flagged with the exact expected error
code, and a catch with the wrong code counts as a miss. 7 mutated-but-still-valid graphs guard
against false positives: none may hard-fail.

**Result.** Recall 25/25 = 1.00, false positives 0/7 = 0.00. Ported from the Python prototype's
battery and re-verified against it; runs as 36 Rust tests (25 injectors, 7 FP guards, plus 4
freshness-detector cases added later).

**Reproduce.**

```
cargo nextest run --test eval_mutations
```

Verified 2026-07-10: all tests pass. This battery mutates graph DATA to test the validator; it
is orthogonal to source-level mutation testing of the Rust code itself.

**Limitations.** The injectors were authored by us, from the validator's own spec; 25 classes
cover every rule the validator implements but cannot prove the rule SET is complete. A defect
class nobody thought of is caught by neither.

## Deterministic ground-truth battery

**What was measured.** Whether the query engine answers structural questions correctly. 21
tasks (blast-radius, trace-to-terminals, depth-N neighborhoods, orphans, writes-to-store,
pipeline topological order, nav reachability, and others) whose answers are computed
independently from the graph fixtures, never trusted from the CLI's own output and never
LLM-judged.

**Result.** 21/21 tasks pass against the committed answer key.

**Reproduce.**

```
cargo nextest run --test eval_ground_truth
```

Verified 2026-07-10: all tests pass.

**Limitations.** Fixtures are 6 example apps authored by us (chat, checkout, dashboard, maps,
media, wizard). This battery proves the engine computes graph semantics correctly; it says
nothing about whether an agent using it completes tasks better (that is the pilot below).

## Rust vs prototype oracle differential

**What was measured.** The Rust engine is a port of a proven Python prototype. The differential
contract: on identical input bytes, both engines must produce identical output.

**Result.** Zero divergences. At port time the differential was run live against the Python
prototype: a 93-case validate differential, 9 query-verb differentials, and 30/30 byte-identical
render diffs covering all 5 render verbs across all 6 example apps, each independently
re-verified. In this repository the prototype's byte-exact output is frozen as committed
snapshots (`tests/snapshots/`): the query `--json` and render (storyboard/spine/hub/deps)
differentials assert the engine reproduces those frozen artifacts byte-for-byte. One exception:
the `render html` check is a structural smoke test, because the HTML template intentionally
diverged from the prototype; the `--json`, query, storyboard, and spine differentials remain
frozen byte-exact.

**Reproduce.**

```
cargo nextest run --test query --test render
```

Verified: 79/79 tests pass, including the frozen differentials. (These run against the committed
snapshots and need no external oracle.)

**Limitations.** A differential proves equivalence with the prototype, not that both are
correct (correctness rests on the mutation and ground-truth batteries above). Coverage is over
the 6 example fixtures, not arbitrary graphs.

## Real-codebase ingest pilot

**What was measured.** The utility question that matters: does an agent with a maapp graph of
an existing codebase answer real structural, impact, trace, and change-planning tasks better or
cheaper than an agent with source + grep alone? This was a pre-registered A/B pilot on a real
private production codebase (a checkout slice, pinned to a fixed SHA), with
kill thresholds frozen before any priced run.

**Protocol.** 12 tasks, frozen before the first scored run. Arm (i):
source + grep/read tools only. Arm (ii): the same, plus scoped `maapp query`/`render` calls
against the skill-ingested, uncorrected graph (62 nodes / 85 edges, validates 0/0); whole-graph
reads forbidden. Headless `claude -p` (sonnet), fresh context per run, 2 seeds per cell:
12 tasks x 2 arms x 2 seeds = 48 runs, all executed and scored. Accuracy comes from mechanical
rubrics applied by an arm-blind judge. A third arm (maintainer-corrected
graph) is pending.

**Result (2026-07-09).**

| Metric | Arm (i) source+grep | Arm (ii) + maapp graph |
|---|---|---|
| Mean accuracy (24 runs each) | 0.9124 | 0.8913 (-2.11 pp) |
| Mean tokens (billed weight) | 274,698 | 294,792 (+7.3%) |
| Mean cost per run | $1.10 | $1.38 |

Against the pre-registered thresholds: **K1 (battery validity) PASS; K2 (ingest utility) FAIL**
for this slice. The graph arm was neither more accurate nor cheaper overall.

What the per-task data shows:

- **Ceiling caveat, disclosed up front:** the baseline scored 0.91 with 8/12 tasks at a perfect
  1.0, so the pre-registered +15 pp bar was unreachable by construction (max attainable delta:
  +8.76 pp). The battery was too easy to detect a lift even if one exists; thresholds stayed
  frozen rather than tuned after the fact, and a harder ceiling-free v2 battery supersedes it.
- The graph won exactly the two tasks the baseline was weakest on: +20.0 pp (auth-guard
  enumeration) and +25.0 pp (multi-file change sweep).
- The graph was substantially cheaper on traversal-shaped tasks: -57%, -39%, and -23% billed
  tokens on the blast-radius, trace, and change-planning tasks it tied on.
- The graph showed reproducible harm on two fine-granularity tasks (-25.0 and -20.0 pp,
  consistent on both seeds): the agent anchored on coarse graph nodes and under-verified source
  details the 62-node graph did not carry.

**Reproduce.** *Reproduced in the maintainers' research repository; the in-crate batteries above are fully reproducible here.* All 48 run transcripts, judge verdicts, and harness scripts are retained there; full re-execution requires access to the private source repository plus roughly $60 of model runs.

**Limitations.** One codebase slice, 12 tasks, 2 seeds per cell (enough to gate flukes, not to
power significance tests), single model, tasks authored by us, and an uncorrected iteration-0
ingest graph. The baseline ceiling means this pilot cannot rule utility IN or OUT for harder
tasks; it does rule out "the graph helps on tasks this easy", and it identified where the graph
pays (weak-baseline and traversal-shaped tasks) and where it hurts (fine-grained detail).

## The historical 3.7x figure

Earlier internal material cited a "~3.7x task-accuracy lift". We traced it to its source and
retired it from headline use. The provenance, in full:

**Source.** The maintainers' evaluation of the Python prototype inside the app it was first
built in (a medical app), before maapp existed as a standalone tool.
Lane 2 of that report: a fresh agent with the graph + CLI + skill scored 16/17 = 0.94 on a
structural-question battery; a fresh agent with only the app's prose product docs scored
3/12 = 0.25. The ratio 0.94 / 0.25 = 3.76 is the 3.7x. The ground-truth answer key was computed
deterministically by the CLI; mapping the agents' prose answers onto canonical ids was
LLM-judge-assisted. The same report also measured a 52.6x per-question token advantage for
scoped graph queries (median 173 tokens per structural answer) at roughly 1.0x bulk-load parity.

**Why it is not a hero claim.** The comparison was one app, one two-arm run, a 12-task battery,
and, critically, a baseline of prose documentation rather than source + grep, which is what an
agent actually has today. It was never measured on an ingested graph of a real codebase. When
we ran that stronger comparison under a pre-registered protocol (the pilot above), the measured
lift on that slice was zero to negative overall, with real wins only on weak-baseline tasks.

**Status: historical internal result from the prototype era, superseded by the measured
results above.** We keep it documented because it motivated the project, but we do not use it
in marketing or README hero copy, and neither should you.

## Benchmark roadmap

What is planned, in order of readiness. No dates are promised.

1. **Pilot arm (iii) to resolve K3 (quality vs concept).** Same protocol and battery as the
   pilot, third arm with a maintainer-corrected graph of the same slice. Separates "the ingest
   produced a weak graph" from "the graph concept adds nothing here". Pre-registered.
2. **Ceiling-free battery v2.** Already drafted and registered: 12 harder tasks (completeness
   enumerations, cross-module traces, multi-hop impact chains, counterfactuals) authored blind
   to the graph's content, awaiting ratification before any priced run. Exists precisely because
   v1's baseline ceiling made the pre-registered bar unreachable.
3. **The full WITH/WITHOUT-graph agent eval.** The benchmark this page ultimately owes its
   readers, specified now so it cannot be quietly weakened later:
   - a frozen task set with stable task IDs, committed before any run;
   - pinned model and temperature (or the documented equivalent for runners without a
     temperature knob), recorded per run;
   - both arms fully defined in advance: WITHOUT = source + grep/read tools; WITH = the same
     plus scoped graph queries, whole-graph reads forbidden;
   - at least 10 runs per arm per task, so per-task pass rates carry a binomial confidence
     interval and the headline lift carries a ratio CI, instead of 2-seed direction checks;
   - every transcript committed, every aggregate re-derivable from committed artifacts;
   - one command (`run_eval.sh`) to execute the whole thing.

Until item 3 exists, the honest summary of maapp's measured utility is: generation from a spec
holds 0.93 of the human agreement ceiling on held-out apps; the engine is validated, oracle-
matched, and deterministic; and on one real codebase with easy tasks, the graph did not beat
grep. That is the baseline we intend to move.
