# Wayland Selection Qualification Follow-up Design

**Base commit:** `e8350fd`

**Goal:** Complete the activated Clipboard, PRIMARY, ext-data-control, and Idle Inhibit implementations without rewriting the approved architecture or disturbing DnD.

## Canonical selection runtime

`SelectionState` is the only mutable authority for Clipboard and PRIMARY. It owns two independent channels, each with its active source key/backend, generation, offer bindings, and mutation watermark. Protocol maps retain generated Wayland resources and resource-specific lifecycle state, but they do not decide which source is current. DnD remains in the existing `wl_data_source` state machine.

Selection source records carry explicit backend identity sufficient for transfer dispatch. A common runtime operation validates that the key is active for the requested channel and then dispatches exactly one transfer to a normal Wayland source, a PRIMARY source, an ext-data-control source, or the Clipboard HostBridge. Offer handlers perform wire/resource validation before calling that operation; they do not search protocol maps as a fallback chain.

The mutable `active_clipboard` and duplicate Clipboard generation ownership are removed. HostBridge integration is represented as a canonical source backend plus a derived adapter needed to request host data. Any remaining protocol cache is invalidation-only and cannot mutate selection truth.

## Causal ordering and source policy

Eligible compositor input serials are recorded with a monotonic `SelectionMutationEpoch(u64)`. Normal Clipboard and PRIMARY requests resolve a serial to its epoch only after client ownership, eligible input origin, and current focus generation validation. The request is accepted only when its epoch is not older than the affected channel watermark. Data Control set/clear operations allocate epochs directly and advance only their affected channel. Raw Wayland `u32` serial values are never ordered numerically.

Data Control sources remain strict single-use and reuse posts the generated `UsedSource` error. PRIMARY sources do not inherit that rule: a live, client-owned, MIME-valid source may be selected again even if its generic source record has been used. Destroyed resources, stale generations, and invalid clients remain rejected. Normal `wl_data_source` semantics are unchanged.

Clipboard and PRIMARY publication, generations, and offer invalidation are channel-local. Publication order is offer creation, all MIME events, then the selection event. Data Control devices receive current Clipboard and PRIMARY immediately and remain focus-independent; normal Clipboard and PRIMARY devices use the existing focused-client policy.

## Lifecycle and module boundaries

Selection source registration, MIME handling, channel mutation, publication, offer construction, receive/transfer, and HostBridge polling/install/clear move from `state/surfaces.rs` into `state/selection_runtime.rs` and narrowly scoped selection modules where required. `surfaces.rs` retains surface lifecycle logic. Protocol modules retain request/event validation and resource creation. DnD code remains in its existing owner.

Idle Inhibit retains exact target surfaces and central reconciliation. Effective inhibition is derived from live inhibitor/resource/client state and an eligible mapped visible root tree, excluding minimized or unmapped trees. Subsurface targets follow their visible root, ordinary occlusion does not matter, and map/unmap, minimize/restore, teardown, and repeated reconciliation are covered through compositor lifecycle operations.

## Qualification and documentation

The existing client harness gains Dispatch implementations for all PRIMARY and ext-data-control manager/device/source/offer resources. Real pipe-transfer tests cover normal Wayland Clipboard → Data Control, PRIMARY → Data Control, HostBridge Clipboard → Data Control, Data Control Clipboard → normal `wl_data_device`, and Data Control PRIMARY → PRIMARY. Tests cover stale offers, unsupported MIME, generation independence, mutation races, source reuse, client cleanup, publication ordering, and exact protocol errors.

Registry tests are corrected to match activated capability profiles, with explicit negative baseline and positive native Idle Inhibit coverage and duplicate-global checks. Compliance documentation claims only behavior covered by deterministic tests; XWayland selection bridging remains out of scope. The final verification runs the complete locked Linux command set, while manual Wayland smoke checks are reported separately when unavailable in the environment.
