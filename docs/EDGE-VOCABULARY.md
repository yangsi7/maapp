# Edge vocabulary

The 14 core edge types, generated from `maapp schema` (the same tables `maapp validate`
enforces — never hand-edit this table out of sync with the engine; regenerate with
`maapp schema` if the two diverge). Attrs marked **required** fail validation
(`E_ATTR_MISSING`) when absent; enum-valued attrs accept an extension token only when it
is declared in the graph's `attrEnumRegistry` (open-with-registry — see `maapp schema`'s
`attrEnumRegistry` description). `from`/`to` kind sets are cross-node constraints the
validator enforces as `E_KIND`; an empty set means any kind.

| Type | From kinds | To kinds | Attrs | Example |
|---|---|---|---|---|
| `binds` | Component, Screen, ViewState | DataSource, StateStore, ViewState | — | `comp:chat/ConversationRow -> store:ConversationStore` |
| `derivesFrom` | Assertion, StateStore, ViewState | DataSource, PipelineStage, StateStore, ViewState | — | `assert:thread/DraftSendable -> vs:thread/Compose` |
| `dismisses` | MutationAction, NavAction | NavContainer, Screen | **target** (enum: `count`, `root`, `self`, or `else`) | `act:thread/GoBack -> screen:chat/Thread {target: self}` |
| `emits` | BackendOp, EffectAction, MutationAction, PipelineStage, x-ext:ExternalSurface, x-ext:PermissionGate | SideEffect | `effect` (open hint, not validator-checked) | `act:thread/SendMessage -> fx:haptic/Send {effect: haptic}` |
| `fires` | Trigger, x-ext:ExternalSurface, x-ext:PermissionGate | EffectAction, MutationAction, NavAction, QueryAction, Trigger, ViewStateAction | — | `trig:list/RowTap -> act:list/OpenThread` |
| `guardedBy` | BackendOp, DataSource, EffectAction, MutationAction, NavAction, PipelineStage, QueryAction, Trigger, ViewStateAction | Assertion, Policy | **polarity** (enum: `forbid`, `require`, or `else`) | `act:thread/SendMessage -> assert:thread/DraftSendable {polarity: require}` |
| `handles` | Component, Screen | Trigger | **event** (enum: `appear`, `change`, `doubleTap`, `dragDismiss`, `endReached`, `longPress`, `pullToRefresh`, `submit`, `swipe`, `tap`, `timer`, or `else`) | `screen:chat/ConversationList -> trig:list/RowTap {event: tap}` |
| `invokes` | BackendOp, MutationAction, PipelineStage | BackendOp, PipelineStage | **awaits** (bool) | `act:thread/SendMessage -> op:chat/SendMessage {awaits: true}` |
| `navigates` | MutationAction, NavAction, NavContainer | NavContainer, Screen, x-ext:ExternalSurface | **present** (enum: `deep-link`, `fullScreen`, `fullScreenCover`, `popover`, `push`, `replace-root`, `sheet`, `tab-switch`, or `else`) | `act:list/OpenThread -> screen:chat/Thread {present: push}` |
| `produces` | BackendOp, PipelineStage | DataSource, StateStore | **artifact** (string); `verification` (enum: `unverified`, `verified`, or `else`) | `stage:rt/Ingest -> store:ThreadStore {artifact: inboundMessages}` |
| `reads` | BackendOp, PipelineStage, QueryAction | DataSource, StateStore | `cachePolicy` (enum: `cached`, `fresh`, `stale-while-revalidate`, or `else`) | `stage:rt/Ingest -> ds:realtime/Socket {cachePolicy: fresh}` |
| `renders` | NavContainer, Screen | Component, NavContainer | — | `screen:chat/ConversationList -> comp:chat/ConversationRow` |
| `setsViewState` | ViewStateAction | ViewState | — | `act:thread/EditDraft -> vs:thread/Compose` |
| `writes` | BackendOp, MutationAction, PipelineStage | DataSource, StateStore | **mode** (enum: `append`, `delete`, `set`, `toggle`, or `else`) | `act:list/StartCompose -> store:ConversationStore {mode: append}` |

Every edge also accepts an optional top-level `when` (branch guard: an Assertion id for
`navigates`, an outcome token for `x-ext:returnsTo`, or the literal `"else"` — branch sets
are all-conditional and mutually exclusive, `E_BRANCH_*`).

## Extension edges

An app-specific edge verb is declared under the graph's `edgeRegistry` as `x-<ns>:verb`
(a declarative data overlay, never a code plugin — see the "Profiles are data, never
plugins" point in the [README](../README.md#why)). Shipped examples use a few: `x-behavior:reconciles`,
`x-ext:returnsTo`, `x-pipeline:consumes`, `x-pipeline:feeds`. Run `maapp schema` on any
graph to see its full registry, core plus registered extensions.
