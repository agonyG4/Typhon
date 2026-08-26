# Typhon Pointer Protocol Quiescence v1 Implementation Plan

> **For agentic workers:** Execute this plan inline in the current checkout. The user explicitly forbids subagents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Separate native input semantics from full Wayland read-side maintenance so stable pointer motion avoids broad server ticks, clean Astrea reconciliation, redundant flushes, and generic interaction hover hit-tests while preserving pointer protocol latency and focus correctness.

**Architecture:** Keep the existing compositor server and native reactor, but expose independent operations for readable Wayland dispatch, native input handling, Astrea publication service, and outgoing protocol flush. Treat Astrea publication as a dirty-driven protocol-only domain serviced by its existing immediate deadline; treat an exclusive move/resize grab as an interaction-local pointer-state path with terminal hover restoration.

**Tech Stack:** Rust, Cargo, Wayland server/display, existing native epoll reactor, Astrea toplevel publisher, existing compositor/native runtime test harness, and `rtk` wrappers for Cargo/Git commands where supported.

## Global Constraints

- Treat the current dirty working tree as authoritative; do not reset, clean, stash, restore unrelated work, or replace it with `HEAD`.
- Do not redesign rendering, O1 buffering, KMS scheduling, Direct Scanout, Dwindle layout authority, or scheduler-admitted interaction geometry.
- A pure input wake must not call `OwnCompositorServer::tick()` merely because input is ready.
- Pointer protocol events must still flush promptly and must not wait for presentation.
- Publication cleanup must remain correct through explicit destroy, disconnect, and admission-limit paths.
- Diagnostics must remain disabled-cheap; use aggregate counters rather than hot-path logs.
- Do not claim real-host CPU/GPU improvement without running the qualification matrix on real KMS hardware.

---

### Task 1: Establish operation-domain and publication metrics

**Files:**
- Modify: `src/native_output/runtime/work_domains.rs`
- Modify: `src/native_output/runtime/resource_efficiency.rs`
- Modify: `src/control_snapshots.rs` only if existing public telemetry requires new serialized fields
- Test: `src/native_output/runtime/work_domains.rs`

**Interfaces:**
- Consume the existing `WakeReasons`, `NativeWakeup`, and `NativeRuntimeState`.
- Produce explicit `wayland_protocol`, `input`, and `astrea_publication` service decisions plus counters for input-only cycles, Wayland read dispatch, full server progression, publication gate/reconcile/prune, flushes, and interaction-local hover avoidance.

- [ ] **Step 1: Write failing domain-operation tests** for input-only, Wayland-only, combined input+Wayland, publication-timer-only, and input-with-an-older-pending-transaction cases. Assert the individual service flags, not only `NativeWorkClass`.
- [ ] **Step 2: Run the focused domain tests** with `rtk cargo test --locked work_domains -- --test-threads=1` and confirm the new operation assertions fail against the current coupled model.
- [ ] **Step 3: Add the smallest allocation-free state/domain fields** needed to represent publication due independently from input and Wayland read readiness.
- [ ] **Step 4: Add aggregate counters** using saturating increments and extend the existing telemetry snapshot only when the current public schema requires it.
- [ ] **Step 5: Run the focused domain tests again** and confirm the operation matrix passes.

### Task 2: Make Astrea publication dirty-driven and independently serviceable

**Files:**
- Modify: `src/compositor/server_toplevel.rs`
- Modify: `src/compositor/toplevel_publication.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/work_domains.rs`
- Test: `src/compositor/toplevel_publication_tests.rs`
- Test: `src/native_output/runtime/work_domains.rs`

**Interfaces:**
- Add or tighten a server operation equivalent to `publish_astrea_toplevel_updates_if_pending()` that gates before collection, pruning, metrics refresh, or reconcile.
- Add native service ownership for pending Astrea publication on its timer/deadline wake, without classifying it as primary-scene work.

- [ ] **Step 1: Add failing publisher tests** for a clean gate, stable-owner repeated motion, active transaction liveness without pointer input, input while an older transaction is pending, and lifecycle cleanup without later motion.
- [ ] **Step 2: Run the publisher and domain tests** and verify failures identify missing gating/service ownership rather than test setup errors.
- [ ] **Step 3: Implement the pre-reconcile gate** so clean publication returns before `prune_dead_resources()`, metrics scans, collection construction, or manager-ID allocation; retain explicit lifecycle pruning at its existing ownership boundaries.
- [ ] **Step 4: Route pending publication deadline wakeups** to the publication service and flush publication-generated events once, while keeping pointer samples from advancing an older transaction.
- [ ] **Step 5: Preserve focus-transition publication** by allowing dirty snapshots/structure changes to schedule publication and by retaining normal Wayland-read lifecycle behavior.
- [ ] **Step 6: Run focused publisher, lifecycle, and domain tests** and review the diff for accidental primary-scene or input-frequency coupling.

### Task 3: Split native input dispatch from Wayland read-side dispatch

**Files:**
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/native_output/runtime/session_io.rs` only if suspended-source handling needs the same domain distinction
- Modify: `src/compositor/server.rs`
- Test: native runtime/domain tests and existing compositor input/output tests

**Interfaces:**
- Keep `OwnCompositorServer::tick()` available for synthetic/headless callers.
- Add narrow native-runtime use of readable-Wayland dispatch and input-only handling so `server.tick()` executes only when listener/client readiness is present.

- [ ] **Step 1: Add a deterministic native-cycle seam/test** that invokes 1000 independent input-only wake decisions with no Wayland readiness and records read-dispatch/full-progression counters.
- [ ] **Step 2: Run the seam test** and confirm the current implementation records the undesired full dispatch/tick work.
- [ ] **Step 3: Split `dispatch_wayland_and_input()`** into conditional read-side server progression and always-available input draining, preserving one input service for combined readiness.
- [ ] **Step 4: Keep key/button pointer-constraint follow-up behavior** only where the existing correctness path proves it is required; pointer motion must not pay an unconditional second tick.
- [ ] **Step 5: Preserve client acceptance, disconnect teardown, launch tracking, XWayland, control, pacing, explicit-sync, and session domain ordering.**
- [ ] **Step 6: Run the 1000-wake proof and combined-readiness tests** and confirm no duplicate input or Wayland dispatch occurs.

### Task 4: Establish one input flush boundary and close the interaction hover fast path

**Files:**
- Modify: `src/compositor/server_toplevel.rs`
- Modify: `src/compositor/server_interaction.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/state/window_decoration.rs` or its owning input-state module
- Modify: `src/native_output/input/routing.rs`
- Test: compositor input/output tests, pointer-constraint tests, and interaction tests

**Interfaces:**
- State-only interaction mutation must not flush.
- Native input handling must retain a single explicit write-side flush after generated pointer/input/publication events.
- Add an interaction-only pointer-position update that updates coordinates/cursor generation without generic decoration hover scene traversal.

- [ ] **Step 1: Add failing flush-count and hover-count tests** for stable pointer motion, active move/resize motion, pointer delivery, and terminal release/cancel restoration.
- [ ] **Step 2: Run those focused tests** and confirm the existing duplicate flush and generic hover traversal are observed.
- [ ] **Step 3: Remove the state-only pre-pointer flush** from the native interaction mutation path while preserving direct/synthetic callers that intentionally rely on immediate server wrappers.
- [ ] **Step 4: Implement the exclusive-interaction pointer-state method** and route only proven compositor-owned move/resize grabs through it; leave ordinary, popup, implicit-grab, locked, confined, and DnD paths unchanged.
- [ ] **Step 5: Keep grabbed-surface pointer delivery, relative-pointer ordering/timestamps, and pointer-frame semantics unchanged.**
- [ ] **Step 6: Use the existing terminal pointer refresh as the sole hover/focus restoration boundary and run the interaction/pointer-constraint tests.**

### Task 5: Adversarial verification, report, and scoped commit

**Files:**
- Create: `REPORT-2026-08-26-typhon-pointer-protocol-quiescence-v1.md`
- Modify only task files identified by the staged diff; preserve all unrelated dirty files.

**Interfaces:**
- Report the exact before/after counters, test commands, known failures, real-host qualification status, and residual risks.

- [ ] **Step 1: Run the mandatory Review Pass 1** against source and tests: read-side dispatch separation, prompt pointer flush, focus publication, transaction liveness, lifecycle cleanup, combined readiness, locked/confined semantics, terminal restoration, scheduler geometry, and synthetic `tick()` callers.
- [ ] **Step 2: Run the mandatory Review Pass 2** looking for renamed `tick()` paths, post-prune gates, duplicate flushes, indirect hover hit-tests, hidden clipboard/pacing work, stalled transactions, frame-coupled pointer output, dropped relative events, and unrelated dirty-tree edits.
- [ ] **Step 3: Run final verification** with `rtk cargo fmt --all -- --check`, `rtk cargo check --locked`, focused tests, the full available suite, and `git diff --check`.
- [ ] **Step 4: Write the English report** with deterministic evidence and explicitly mark real-host hardware results as not verified unless executed.
- [ ] **Step 5: Stage only the scoped task hunks interactively**, audit `rtk git diff --cached --stat`, `rtk git diff --cached --check`, and `rtk git diff --cached --name-only`.
- [ ] **Step 6: Commit with** `rtk git commit -m "perf: quiesce pointer protocol hot paths"` and confirm unrelated work remains unstaged.
