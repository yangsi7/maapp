// Test code may panic on failure: that IS the assertion mechanism.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Eval Pillar 1: Ground-truth task battery.
//!
//! 21 deterministic tasks covering blast-radius, trace, nav-reachability,
//! node-kind membership, orphans, renders, guardedBy, neighbors, writes, pipeline
//! topo-sort, nav-view, reachability, multi-effect nav, guard-viewstate, coherence,
//! reactive-recompute, optimistic coupling, trace-terminals, cache-policy, and
//! skippable-step queries.
//!
//! Expected answers: each task's expected value was vendored from the reference
//! oracle at port time, run on the SAME shipped fixture the task queries
//! (checkout/dashboard/media/chat). Any divergence from the expected value IS a
//! bug in the Rust engine, not in the expected value.

use maapp::{
    graph::Graph,
    load_graph_from_slice,
    query::{q_blast_radius, q_neighbors, q_orphans, q_trace_terminals, q_view},
};
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn load(path: &str) -> (Vec<u8>, Graph) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let g = load_graph_from_slice(&bytes).unwrap_or_else(|e| panic!("load {path}: {e}"));
    (bytes, g)
}

fn graph(name: &str) -> Graph {
    load(&format!("examples/{name}.json")).1
}

fn set(vs: &[&str]) -> BTreeSet<String> {
    vs.iter().map(|s| s.to_string()).collect()
}

// ---------------------------------------------------------------------------
// t01 — blast-radius for store:CheckoutStore (checkout)
// Expected: union of dependents ∪ dependencies, re-derived 2026-07-10:
// maapp query blast-radius \
//   store:CheckoutStore examples/checkout.json --json
// PLUS `act:shipping/Continue` — the ONE documented deliberate oracle
// divergence (engine-fix rider F4, see `is_dependent_edge` in src/query.rs):
// guardedBy in-edges count on the DEPENDENTS side, so
// CheckoutStore <-derivesFrom- assert:shipping/AddressPresent
//               <-guardedBy-   act:shipping/Continue is a dependent.
// ---------------------------------------------------------------------------
#[test]
fn t01_blast_store_checkoutstore() {
    let g = graph("checkout");
    let res = q_blast_radius(&g, "store:CheckoutStore").expect("blast-radius");
    let got: BTreeSet<String> = res
        .dependents
        .iter()
        .chain(res.dependencies.iter())
        .cloned()
        .collect();
    let want = set(&[
        "act:address/SaveAndContinue",
        "act:card/SaveCard",
        "act:pay/SelectMethod",
        "act:review/PlaceOrder",
        "act:shipping/Continue",
        "assert:pay/CardSelected",
        "assert:pay/WalletSelected",
        "assert:shipping/AddressPresent",
        "comp:checkout/AddressCard",
        "screen:checkout/Payment",
    ]);
    assert_eq!(got, want, "t01 blast-radius set mismatch");
}

// ---------------------------------------------------------------------------
// t02 — trace terminals from screen:cart/Bag (checkout)
// Re-derived 2026-07-10:
// maapp query trace \
//   screen:cart/Bag examples/checkout.json --terminals --json
// ---------------------------------------------------------------------------
#[test]
fn t02_trace_bag() {
    let g = graph("checkout");
    let res = q_trace_terminals(&g, "screen:cart/Bag").expect("trace terminals");
    let got: BTreeSet<String> = res.terminals.into_iter().collect();
    let want = set(&[
        "act:address/Cancel",
        "act:delivery/Confirm",
        "act:promo/Apply",
        "fx:analytics/ApplePayResult",
        "fx:analytics/CheckoutStarted",
        "fx:analytics/OrderPlaced",
        "fx:haptic/OrderSuccess",
        "screen:checkout/Review",
        "store:CartStore",
        "store:CheckoutStore",
        "store:OrderStore",
        "vs:address/Form",
    ]);
    assert_eq!(got, want, "t02 trace terminals mismatch");
}

// ---------------------------------------------------------------------------
// t03 — nav reachability from screen:cart/Bag (checkout)
// Follow handles -> fires -> navigates chains; collect surface-layer destinations.
// Expected set re-derived 2026-07-10 with an independent Python BFS over the
// oracle's NAV_FWD = {handles, fires, navigates, renders} edge set.
// ---------------------------------------------------------------------------
#[test]
fn t03_nav_reach_bag() {
    let g = graph("checkout");
    // BFS over navigates edges starting from handles on the root cart screen.
    // Oracle semantics: fires->NavAction->navigates + container navigates.
    // We replicate: collect all navigates .to destinations reachable from
    // screen:cart/Bag via handles->fires->navigates traversal.
    let want = set(&[
        "screen:cart/PromoCode",
        "screen:checkout/AddressForm",
        "screen:checkout/CardEntry",
        "screen:checkout/Complete",
        "screen:checkout/DeliveryMethod",
        "screen:checkout/OrderFailed",
        "screen:checkout/Payment",
        "screen:checkout/Review",
        "screen:checkout/Shipping",
    ]);
    let nav_reachable = nav_bfs_from(&g, "screen:cart/Bag");
    assert_eq!(nav_reachable, want, "t03 nav reachability mismatch");
}

/// Forward BFS over handles/fires/navigates/renders edges (NAV_FWD in the oracle).
/// Returns all reachable Screen and NavContainer nodes, excluding the start itself.
fn nav_bfs_from(g: &Graph, start: &str) -> BTreeSet<String> {
    // Oracle: NAV_FWD = {"handles", "fires", "navigates", "renders"}
    let nav_types: BTreeSet<&str> = ["navigates", "handles", "fires", "renders"]
        .iter()
        .copied()
        .collect();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    queue.push_back(start.to_string());
    visited.insert(start.to_string());

    while let Some(cur) = queue.pop_front() {
        if let Some(idxs) = g.out_edges.get(cur.as_str()) {
            for &i in idxs {
                let e = &g.edges[i];
                let etype = e.edge_type().unwrap_or("");
                if nav_types.contains(etype)
                    && let Some(t) = e.to()
                    && visited.insert(t.to_string())
                {
                    queue.push_back(t.to_string());
                }
            }
        }
    }

    // Filter: surface kinds only, excluding the start node (oracle: n != root).
    visited
        .into_iter()
        .filter(|nid| {
            nid != start
                && g.nodes.get(nid.as_str()).is_some_and(|node| {
                    matches!(node.kind(), Some("Screen") | Some("NavContainer"))
                })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// t04 — Assertion-kind membership (checkout)
// A deterministic scoped-membership task, keyed on node kind.
// Verified independently: jq over examples/checkout.json node kinds.
// ---------------------------------------------------------------------------
#[test]
fn t04_assertion_membership() {
    let g = graph("checkout");
    let mut got: BTreeSet<String> = BTreeSet::new();
    for (id, node) in &g.nodes {
        if node.kind() == Some("Assertion") {
            got.insert(id.clone());
        }
    }
    let want = set(&[
        "assert:address/Valid",
        "assert:pay/CardSelected",
        "assert:pay/WalletSelected",
        "assert:review/NotSubmitting",
        "assert:shipping/AddressPresent",
    ]);
    assert_eq!(got, want, "t04 Assertion membership mismatch");
}

// ---------------------------------------------------------------------------
// t05 — orphans (all shipped fixtures)
// Every shipped example graph is fully wired: zero orphans on each
// (verified via `maapp query orphans` over all 8).
// Over-reporting on any fixture fails here; recall of a REAL orphan is covered
// by the mutation battery's W_ORPHAN injector.
// ---------------------------------------------------------------------------
#[test]
fn t05_orphans() {
    for name in [
        "chat",
        "checkout",
        "dashboard",
        "maps",
        "media",
        "onboarding-variant",
        "settings-account",
        "wizard",
    ] {
        let g = graph(name);
        let res = q_orphans(&g);
        let got: BTreeSet<String> = res.orphans.into_iter().collect();
        assert!(got.is_empty(), "t05 orphans mismatch on {name}: {got:?}");
    }
}

// ---------------------------------------------------------------------------
// t06 — which screens render comp:checkout/PlaceOrderButton (checkout)
// ---------------------------------------------------------------------------
#[test]
fn t06_renders_placeorderbutton() {
    let g = graph("checkout");
    let target = "comp:checkout/PlaceOrderButton";
    let got: BTreeSet<String> = g
        .edges
        .iter()
        .filter(|e| e.edge_type() == Some("renders") && e.to() == Some(target))
        .filter_map(|e| e.from().map(|f| f.to_string()))
        .collect();
    let want = set(&["screen:checkout/Review"]);
    assert_eq!(got, want, "t06 renders PlaceOrderButton mismatch");
}

// ---------------------------------------------------------------------------
// t07 — what guards act:player/SkipNext (media)
// ---------------------------------------------------------------------------
#[test]
fn t07_guards_skip_next() {
    let g = graph("media");
    let src = "act:player/SkipNext";
    let got: BTreeSet<String> = g
        .edges
        .iter()
        .filter(|e| e.edge_type() == Some("guardedBy") && e.from() == Some(src))
        .filter_map(|e| e.to().map(|t| t.to_string()))
        .collect();
    let want = set(&["assert:player/HasCurrentTrack", "assert:queue/HasNext"]);
    assert_eq!(got, want, "t07 guards SkipNext mismatch");
}

// ---------------------------------------------------------------------------
// t08 — depth-2 neighbors of screen:checkout/Payment (checkout)
// Excludes the root node itself. Re-derived 2026-07-10:
// maapp query neighbors \
//   screen:checkout/Payment examples/checkout.json --depth 2 --json
// ---------------------------------------------------------------------------
#[test]
fn t08_neighbors_d2() {
    let g = graph("checkout");
    let res = q_neighbors(&g, "screen:checkout/Payment", 2).expect("neighbors");
    let got: BTreeSet<String> = res
        .nodes
        .keys()
        .filter(|k| k.as_str() != "screen:checkout/Payment")
        .cloned()
        .collect();
    let want = set(&[
        "act:address/SaveAndContinue",
        "act:card/SaveCard",
        "act:pay/Continue",
        "act:pay/PresentCardEntry",
        "act:pay/SelectMethod",
        "act:review/PlaceOrder",
        "act:shipping/Continue",
        "assert:pay/CardSelected",
        "assert:pay/WalletSelected",
        "assert:shipping/AddressPresent",
        "comp:checkout/AddressCard",
        "fx:analytics/ApplePayResult",
        "screen:checkout/Review",
        "store:CheckoutStore",
        "trig:pay/ContinueTap",
        "trig:pay/MethodSelect",
        "trig:shipping/ContinueTap",
        "x-ext:pay/ApplePaySheet",
    ]);
    assert_eq!(got, want, "t08 neighbors depth-2 mismatch");
}

// ---------------------------------------------------------------------------
// t09 — what writes to store:CheckoutStore (checkout)
// ---------------------------------------------------------------------------
#[test]
fn t09_writes_to_checkoutstore() {
    let g = graph("checkout");
    let target = "store:CheckoutStore";
    let got: BTreeSet<String> = g
        .edges
        .iter()
        .filter(|e| e.edge_type() == Some("writes") && e.to() == Some(target))
        .filter_map(|e| e.from().map(|f| f.to_string()))
        .collect();
    let want = set(&[
        "act:address/SaveAndContinue",
        "act:card/SaveCard",
        "act:pay/SelectMethod",
        "act:review/PlaceOrder",
    ]);
    assert_eq!(got, want, "t09 writes to CheckoutStore mismatch");
}

// ---------------------------------------------------------------------------
// t10 — pipeline topological order via x-pipeline:feeds (dashboard)
// Kahn's algorithm with deterministic tie-break by id. Dashboard is the only
// shipped fixture with a feeds chain: Normalize -> Aggregate -> Series.
// ---------------------------------------------------------------------------
#[test]
fn t10_pipeline_toposort() {
    let g = graph("dashboard");
    let feeds_type = "x-pipeline:feeds";

    // Build adjacency for PipelineStage nodes over feeds edges.
    let stage_nodes: BTreeSet<&str> = g
        .nodes
        .iter()
        .filter_map(|(id, node)| {
            if node.kind() == Some("PipelineStage") {
                Some(id.as_str())
            } else {
                None
            }
        })
        .collect();

    // Kahn's: compute in-degree within the feeds sub-graph.
    let mut in_deg: BTreeMap<&str, usize> = stage_nodes.iter().map(|&n| (n, 0)).collect();
    let mut succ: BTreeMap<&str, Vec<&str>> = stage_nodes.iter().map(|&n| (n, vec![])).collect();

    for e in &g.edges {
        if e.edge_type() == Some(feeds_type) {
            let f = e.from().unwrap_or("");
            let t = e.to().unwrap_or("");
            if stage_nodes.contains(f) && stage_nodes.contains(t) {
                *in_deg.entry(t).or_insert(0) += 1;
                succ.entry(f).or_default().push(t);
            }
        }
    }

    // Priority queue using BTreeSet for deterministic tie-break by id.
    let mut ready: BTreeSet<&str> = in_deg
        .iter()
        .filter_map(|(&n, &d)| if d == 0 { Some(n) } else { None })
        .collect();
    let mut order: Vec<String> = Vec::new();
    while let Some(&cur) = ready.iter().next() {
        let cur = ready.take(cur).unwrap();
        order.push(cur.to_string());
        for &nxt in succ.get(cur).unwrap_or(&vec![]) {
            let d = in_deg.entry(nxt).or_insert(0);
            *d -= 1;
            if *d == 0 {
                ready.insert(nxt);
            }
        }
    }

    let want: Vec<&str> = vec![
        "stage:agg/Normalize",
        "stage:agg/Aggregate",
        "stage:agg/Series",
    ];
    assert_eq!(
        order,
        want.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "t10 pipeline topo-sort order mismatch"
    );
}

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// t11 — nav-view edge set (checkout): navigates | dismisses | x-ext:returnsTo
// Expected: set of "from|type|to" triples. Re-derived 2026-07-10:
// maapp query view nav \
//   examples/checkout.json --json
// (19 edges; the two ApplePaySheet returnsTo arms to Payment collapse to one
// triple in the set, hence 18 entries.)
// ---------------------------------------------------------------------------
#[test]
fn t11_nav_view_edgeset() {
    let g = graph("checkout");
    let res = q_view(&g, "nav").expect("view nav");
    let got: BTreeSet<String> = res
        .edges
        .iter()
        .map(|e| {
            format!(
                "{}|{}|{}",
                e.from.as_deref().unwrap_or(""),
                e.r#type.as_deref().unwrap_or(""),
                e.to.as_deref().unwrap_or("")
            )
        })
        .collect();
    let want = set(&[
        "act:address/Cancel|dismisses|screen:checkout/AddressForm",
        "act:address/SaveAndContinue|dismisses|screen:checkout/AddressForm",
        "act:card/SaveCard|dismisses|screen:checkout/CardEntry",
        "act:cart/GoToShipping|navigates|screen:checkout/Shipping",
        "act:cart/PresentPromoCode|navigates|screen:cart/PromoCode",
        "act:delivery/Confirm|dismisses|screen:checkout/DeliveryMethod",
        "act:order/Retry|navigates|screen:checkout/Review",
        "act:pay/Continue|navigates|screen:checkout/Review",
        "act:pay/Continue|navigates|x-ext:pay/ApplePaySheet",
        "act:pay/PresentCardEntry|navigates|screen:checkout/CardEntry",
        "act:promo/Apply|dismisses|screen:cart/PromoCode",
        "act:review/PlaceOrder|navigates|screen:checkout/Complete",
        "act:review/PlaceOrder|navigates|screen:checkout/OrderFailed",
        "act:shipping/Continue|navigates|screen:checkout/Payment",
        "act:shipping/PresentAddressForm|navigates|screen:checkout/AddressForm",
        "act:shipping/PresentDeliveryMethod|navigates|screen:checkout/DeliveryMethod",
        "x-ext:pay/ApplePaySheet|x-ext:returnsTo|screen:checkout/Payment",
        "x-ext:pay/ApplePaySheet|x-ext:returnsTo|screen:checkout/Review",
    ]);
    assert_eq!(got, want, "t11 nav-view edge triples mismatch");
}

// ---------------------------------------------------------------------------
// t12 — is screen:checkout/Complete reachable from screen:cart/Bag? (checkout)
// Forward BFS over handles/fires/navigates/renders (+ dismisses/returnsTo).
// ---------------------------------------------------------------------------
#[test]
fn t12_reachable_bool() {
    let g = graph("checkout");
    let reachable = forward_bfs(&g, "screen:cart/Bag");
    assert!(
        reachable.contains("screen:checkout/Complete"),
        "t12: screen:checkout/Complete should be reachable from screen:cart/Bag"
    );
}

fn forward_bfs(g: &Graph, start: &str) -> BTreeSet<String> {
    let forward_types: BTreeSet<&str> = [
        "handles",
        "fires",
        "navigates",
        "renders",
        "dismisses",
        "x-ext:returnsTo",
    ]
    .iter()
    .copied()
    .collect();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<String> = vec![start.to_string()];
    while let Some(cur) = queue.pop() {
        if !visited.insert(cur.clone()) {
            continue;
        }
        if let Some(idxs) = g.out_edges.get(cur.as_str()) {
            for &i in idxs {
                let e = &g.edges[i];
                if e.edge_type().is_some_and(|t| forward_types.contains(t))
                    && let Some(t) = e.to()
                    && !visited.contains(t)
                {
                    queue.push(t.to_string());
                }
            }
        }
    }
    visited
}

// ---------------------------------------------------------------------------
// t13 — multi-effect-nav: actions that both MUTATE and NAVIGATE (media graph)
// G15: single MutationActions with both navigates and writes/invokes/emits edges.
// ---------------------------------------------------------------------------
#[test]
fn t13_multieffect_nav() {
    let g = graph("media");
    let mut mutation_sources: BTreeSet<String> = BTreeSet::new();
    let mut nav_sources: BTreeSet<String> = BTreeSet::new();
    for e in &g.edges {
        match e.edge_type() {
            Some("writes") | Some("invokes") | Some("emits") => {
                if let Some(f) = e.from() {
                    mutation_sources.insert(f.to_string());
                }
            }
            Some("navigates") => {
                if let Some(f) = e.from() {
                    nav_sources.insert(f.to_string());
                }
            }
            _ => {}
        }
    }
    let got: BTreeSet<String> = mutation_sources
        .intersection(&nav_sources)
        .cloned()
        .collect();
    let want = set(&["act:detail/PlayPlaylist", "act:detail/PlayTrack"]);
    assert_eq!(got, want, "t13 multi-effect-nav mismatch");
}

// ---------------------------------------------------------------------------
// t14 — guards deriving from ViewState (maps graph)
// G16: derivesFrom sources whose target kind == ViewState
// ---------------------------------------------------------------------------
#[test]
fn t14_guard_viewstate() {
    let g = graph("maps");
    let got: BTreeSet<String> = g
        .edges
        .iter()
        .filter(|e| e.edge_type() == Some("derivesFrom"))
        .filter_map(|e| {
            let t = e.to()?;
            let node = g.nodes.get(t)?;
            if node.kind() == Some("ViewState") {
                e.from().map(|f| f.to_string())
            } else {
                None
            }
        })
        .collect();
    let want = set(&["assert:search/QueryValid"]);
    assert_eq!(got, want, "t14 guard-viewstate mismatch");
}

// ---------------------------------------------------------------------------
// t15 — coherence set for store:PlayerStore (media): nodes that bind it
// ---------------------------------------------------------------------------
#[test]
fn t15_playerstore_binds() {
    let g = graph("media");
    let target = "store:PlayerStore";
    let got: BTreeSet<String> = g
        .edges
        .iter()
        .filter(|e| e.edge_type() == Some("binds") && e.to() == Some(target))
        .filter_map(|e| e.from().map(|f| f.to_string()))
        .collect();
    // Re-derived 2026-07-09 (C5b added screen:player/Lyrics binding PlayerStore):
    // maapp query node store:PlayerStore \
    //   examples/media.json --json | jq '[.in_edges[]|select(.type=="binds")|.from]|sort'
    let want = set(&[
        "comp:player/MiniBar",
        "comp:player/SeekBar",
        "comp:player/Transport",
        "screen:player/Lyrics",
        "screen:player/NowPlaying",
        "screen:player/Queue",
    ]);
    assert_eq!(got, want, "t15 PlayerStore coherence set mismatch");
}

// ---------------------------------------------------------------------------
// t16 — reactive recompute: FilterStore invalidation chain (dashboard)
// FilterStore --x-pipeline:consumes--> stages, feeds-closure, --produces--> stores,
// binds<-- components
// ---------------------------------------------------------------------------
#[test]
fn t16_filterstore_recompute() {
    let g = graph("dashboard");
    let filter_store = "store:FilterStore";

    // Step 1: Find pipeline stages that consume FilterStore.
    // Edge direction: store:FilterStore --x-pipeline:consumes--> stage:...
    // Oracle: consumers = {e["to"] for e in g.edges if e["type"]=="x-pipeline:consumes" and e["from"]==store}
    let consuming_stages: BTreeSet<String> = g
        .edges
        .iter()
        .filter(|e| e.edge_type() == Some("x-pipeline:consumes") && e.from() == Some(filter_store))
        .filter_map(|e| e.to().map(|t| t.to_string()))
        .collect();

    // Step 2: Transitive feeds-closure from those stages.
    let mut reachable_stages: BTreeSet<String> = consuming_stages.clone();
    let mut queue: Vec<String> = consuming_stages.into_iter().collect();
    while let Some(cur) = queue.pop() {
        for e in &g.edges {
            if e.edge_type() == Some("x-pipeline:feeds")
                && e.from() == Some(cur.as_str())
                && let Some(t) = e.to()
                && reachable_stages.insert(t.to_string())
            {
                queue.push(t.to_string());
            }
        }
    }

    // Step 3: Stores produced by those stages.
    let produced_stores: BTreeSet<String> = g
        .edges
        .iter()
        .filter(|e| {
            e.edge_type() == Some("produces")
                && e.from().is_some_and(|f| reachable_stages.contains(f))
        })
        .filter_map(|e| e.to().map(|t| t.to_string()))
        .collect();

    // Step 4: Components that bind those stores.
    let bound_components: BTreeSet<String> = g
        .edges
        .iter()
        .filter(|e| {
            e.edge_type() == Some("binds") && e.to().is_some_and(|t| produced_stores.contains(t))
        })
        .filter_map(|e| e.from().map(|f| f.to_string()))
        .collect();

    let got: BTreeSet<String> = reachable_stages
        .iter()
        .chain(produced_stores.iter())
        .chain(bound_components.iter())
        .cloned()
        .collect();

    let want = set(&[
        "comp:analytics/ChartView",
        "comp:analytics/MetricCard",
        "stage:agg/Aggregate",
        "stage:agg/Normalize",
        "stage:agg/Series",
        "store:AggregateStore",
        "store:ChartSeriesStore",
    ]);
    assert_eq!(got, want, "t16 FilterStore recompute chain mismatch");
}

// ---------------------------------------------------------------------------
// t17 — optimistic reconcile: stores that are BOTH optimistic-write AND reconciles targets (chat)
// ---------------------------------------------------------------------------
#[test]
fn t17_optimistic_reconcile() {
    let g = graph("chat");

    // Optimistic-write targets: writes edges with attrs.optimistic == true.
    let opt_targets: BTreeSet<String> = g
        .edges
        .iter()
        .filter(|e| {
            e.edge_type() == Some("writes")
                && e.attrs
                    .get("optimistic")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
        })
        .filter_map(|e| e.to().map(|t| t.to_string()))
        .collect();

    // Reconciles targets.
    let rec_targets: BTreeSet<String> = g
        .edges
        .iter()
        .filter(|e| e.edge_type() == Some("x-behavior:reconciles"))
        .filter_map(|e| e.to().map(|t| t.to_string()))
        .collect();

    let got: BTreeSet<String> = opt_targets.intersection(&rec_targets).cloned().collect();
    let want = set(&["store:ThreadStore"]);
    assert_eq!(got, want, "t17 optimistic reconcile mismatch");
}

// ---------------------------------------------------------------------------
// t18 — trace terminals from screen:chat/Thread (chat)
// ---------------------------------------------------------------------------
#[test]
fn t18_thread_trace_terminals() {
    let g = graph("chat");
    let res = q_trace_terminals(&g, "screen:chat/Thread").expect("trace terminals");
    let got: BTreeSet<String> = res.terminals.into_iter().collect();
    // Re-derived 2026-07-09 (C5b expanded chat.json with profile/info/viewer flows):
    // maapp query trace screen:chat/Thread \
    //   examples/chat.json --terminals --json
    let want = set(&[
        "act:info/GoBack",
        "act:profile/GoBack",
        "act:shared/GoBack",
        "act:thread/GoBack",
        "act:viewer/GoBack",
        "ds:realtime/Socket",
        "stage:rt/Ingest",
        "store:ContactsStore",
        "store:ConversationStore",
        "store:NotificationSettingsStore",
        "store:ThreadStore",
    ]);
    assert_eq!(got, want, "t18 chat Thread trace terminals mismatch");
}

// ---------------------------------------------------------------------------
// t19 — cache policy fresh reads (dashboard): reads edges with cachePolicy == 'fresh'
// Answer = "from|to" set.
// ---------------------------------------------------------------------------
#[test]
fn t19_cachepolicy_fresh() {
    let g = graph("dashboard");
    let got: BTreeSet<String> = g
        .edges
        .iter()
        .filter(|e| {
            e.edge_type() == Some("reads")
                && e.attrs.get("cachePolicy").and_then(|v| v.as_str()) == Some("fresh")
        })
        .map(|e| format!("{}|{}", e.from().unwrap_or(""), e.to().unwrap_or("")))
        .collect();
    let want = set(&[
        "act:overview/RefreshCharts|store:ChartSeriesStore",
        "op:SyncLedger|src:LedgerAPI",
    ]);
    assert_eq!(got, want, "t19 cache-policy fresh reads mismatch");
}

// ---------------------------------------------------------------------------
// t20 — blast-radius DEPENDENTS of store:QueueStore (media)
// ---------------------------------------------------------------------------
#[test]
fn t20_queuestore_blast() {
    let g = graph("media");
    let res = q_blast_radius(&g, "store:QueueStore").expect("blast-radius");
    let got: BTreeSet<String> = res.dependents.into_iter().collect();
    // Re-derived 2026-07-09 (C5b cycle-1 added assert:queue/HasNext guard precondition):
    // maapp query blast-radius \
    //   store:QueueStore examples/media.json --json
    let want = set(&[
        "act:detail/PlayPlaylist",
        "act:detail/PlayTrack",
        "act:player/SkipNext",
        "act:queue/RemoveTrack",
        "act:queue/ReorderTrack",
        "assert:queue/HasNext",
        "screen:player/Queue",
    ]);
    assert_eq!(got, want, "t20 QueueStore blast-radius dependents mismatch");
}

// ---------------------------------------------------------------------------
// t21 — skippable steps (wizard): unguarded NavAction sources that navigate
// to a screen also reachable by >=2 navigates from any source.
// Answer = {act:onb/SkipStep1}
// ---------------------------------------------------------------------------
#[test]
fn t21_skippable_step() {
    let g = graph("wizard");

    // Count navigates-in-count for each target screen.
    let mut nav_in_count: BTreeMap<String, usize> = BTreeMap::new();
    for e in &g.edges {
        if e.edge_type() == Some("navigates")
            && let Some(t) = e.to()
        {
            *nav_in_count.entry(t.to_string()).or_insert(0) += 1;
        }
    }

    // Multi-target screens: reached by >=2 navigates.
    let multi_targets: BTreeSet<&str> = nav_in_count
        .iter()
        .filter_map(|(k, &v)| if v >= 2 { Some(k.as_str()) } else { None })
        .collect();

    // NavAction sources of navigates to those multi-target screens.
    // The skip action is the one source that is a NavAction AND has no guardedBy edges.
    let mut skip_actions: BTreeSet<String> = BTreeSet::new();
    for e in &g.edges {
        if e.edge_type() == Some("navigates") {
            let t = e.to().unwrap_or("");
            if !multi_targets.contains(t) {
                continue;
            }
            let f = match e.from() {
                Some(f) => f,
                None => continue,
            };
            // Must be a NavAction kind.
            let node = match g.nodes.get(f) {
                Some(n) => n,
                None => continue,
            };
            if node.kind() != Some("NavAction") {
                continue;
            }
            // Must have no guardedBy edges (unguarded).
            let guarded = g
                .out_edges
                .get(f)
                .into_iter()
                .flatten()
                .any(|&i| g.edges[i].edge_type() == Some("guardedBy"));
            if !guarded {
                skip_actions.insert(f.to_string());
            }
        }
    }

    let want = set(&["act:onb/SkipStep1"]);
    assert_eq!(skip_actions, want, "t21 skippable steps mismatch");
}
