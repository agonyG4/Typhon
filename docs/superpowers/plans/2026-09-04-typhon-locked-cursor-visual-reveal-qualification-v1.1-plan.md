# Typhon Locked Cursor Visual Reveal Qualification v1.1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the two Gate 1 observability blockers in the locked-cursor visual reveal trace: cover synchronous cursor KMS submissions with the same exact payload semantics as worker submissions, and freeze reveal ownership against the exact physical pageflip identity before asynchronous delivery.

**Architecture:** Refactor `src/native/kms/atomic.rs` so KMS cursor geometry is described once and consumed both by the real atomic property writer and the canonical trace formatter. Add a bounded trace-only reveal ledger under `src/native_output/presentation/` keyed by `(output_generation, crtc_id, PageFlipToken)`, with a second bounded per-reveal lifecycle collection. Carry frozen snapshots through existing worker cursor owner/sidecar metadata; bind synchronous snapshots at their real token-bearing submission sites; consume the ledger from pageflip promotion. Keep the existing global compositor authority only for lifecycle/source logging, never for late pageflip ownership.

**Tech Stack:** Rust, DRM atomic cursor state, existing NativeRuntime/KMS worker/sidecar/pageflip pipeline, Rust unit/integration tests, `rtk` command wrapper.

## Global Constraints

- Work in `/home/agony/GitHub/Typhon`; compile and test in this same checkout.
- Use `/home/agony/.local/bin/rtk` for shell commands.
- Do not use subagents.
- Preserve unrelated user edits already present in the working tree; stage v1.1 paths explicitly.
- Use `apply_patch` for source and documentation edits.
- Do not change cursor scheduling or policy, worker replacement, visibility ordering, pointer position/client semantics, KMS ordering, input cadence/backlog, `NativeInputEpoch`, anchor, surface transactions, or regions.
- Do not implement a semantic cursor teleport correction or enter Gate 2.
- Trace-disabled execution must allocate no new ledger/snapshot state and perform no trace-only formatting or clocks.
- Commit each coherent task and record exact starting/ending HEADs in the final qualification report.

---

## Task 1: Add the shared exact cursor-plane assignment description

**Files:**
- Modify: `src/native/kms/atomic.rs`
- Modify: `src/native/kms/tests.rs` (or the existing atomic request test module)
- Test first: tests in the same module that exercise hidden, visible, hotspot-offset, and no-plane cases

- [ ] Add a traceable `AtomicCursorPlaneAssignment` representation for unavailable/no-plane, disabled, and enabled assignments. Include exact framebuffer, CRTC, source rectangle, hotspot-adjusted destination rectangle, plane, and image-generation fields where the real cursor state provides them.
- [ ] Add a helper that derives that representation from `AtomicPipelineProperties` and `Option<&AtomicCursorVisualState>` using the current `append_cursor_plane_state()` semantics.
- [ ] Refactor `append_cursor_plane_state()` to write the properties from the helper without changing error behavior or property values.
- [ ] Add tests proving visible geometry and disable assignments match the existing atomic request builder, and that unchanged/unavailable information is not fabricated.
- [ ] Run focused atomic KMS tests and `rtk cargo fmt --check`.
- [ ] Commit: `refactor: share exact cursor plane assignment semantics`.

## Task 2: Build the bounded frozen-reveal ledger and full visual comparison

**Files:**
- Add: `src/native_output/presentation/cursor_trace.rs`
- Modify: `src/native_output/presentation/mod.rs`
- Modify: `src/native_output/presentation/plane.rs` only if existing `PresentedCursorState` needs to carry already-known image-generation/size evidence
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/bootstrap.rs`

- [ ] Add `CursorRevealTraceLedger` behind an `Option` in `NativeRuntime`, constructed only when `cursor_presentation_trace_enabled()` is true.
- [ ] Define exact physical key `(output_generation, crtc_id, PageFlipToken)` and frozen reveal snapshot fields: constraint identity, final position, visibility request, expected epoch/revision, delivery, position/hotspot, framebuffer, image generation, source, and ownership.
- [ ] Use bounded `VecDeque` collections with a small fixed capacity (32). On overflow, emit an explicit trace event and retire only the oldest safe diagnostic entry.
- [ ] Implement bind/take/retire operations, preserving an older reveal’s pending physical entries when a newer reveal supersedes it.
- [ ] Implement `true`/`false`/`unknown` match values for position, revision, delivery, hotspot, framebuffer, image generation, source, visual, and overall. Require all known required invariants for `overall=true`; report `unknown` when important evidence is unavailable.
- [ ] Add pure unit tests before integration for: A/TA and B/TB overlap, A pageflip after B starts, independent first-visible slots, stale same-position state, exact matching state, no-visible terminal, bounded overflow, and disabled state neutrality.
- [ ] Run the new focused ledger tests. Preserve RED evidence if the old late-bound implementation fails the overlap/identity assertions; then make them GREEN with the ledger implementation.
- [ ] Commit: `feat: add bounded frozen cursor reveal ledger`.

## Task 3: Canonicalize worker and synchronous KMS submission evidence

**Files:**
- Modify: `src/native_output/presentation/cursor_trace.rs`
- Modify: `src/native/kms/backend.rs` or its existing pipeline accessor only if needed for trace-only pipeline access
- Modify: `src/native_output/kms_worker/presentation_executor.rs`
- Modify: `src/native_output/kms_worker/cursor_sidecar.rs`
- Modify: `src/native_output/kms_worker/bundle.rs`
- Modify: `src/native_output/kms_worker/thread.rs`
- Modify: `src/native_output/runtime/plane_cycle.rs`
- Modify: `src/native_output/runtime/presentation_transactions.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm_transactions.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`

- [ ] Define one `CursorKmsSubmitTrace` formatter with stable fields for generation, transaction, token, CRTC, plane, cursor epoch/revision, submission kind, transport, assignment, exact geometry, framebuffer/image identity, and position/hotspot.
- [ ] Route worker cursor-only, worker primary-plus-cursor, and worker sidecar submissions through the formatter with `transport=worker` and the actual job token.
- [ ] Route synchronous cursor-only `submit_plane_delta()` and synchronous primary-plus-cursor `submit_ready_frame()` through the same formatter with `transport=synchronous` and the actual token.
- [ ] Reuse the shared assignment helper; do not recompute geometry in trace code and do not emit zero for unavailable metadata.
- [ ] Carry the frozen reveal snapshot in existing worker cursor owner/sidecar metadata rather than mutating worker scheduling behavior.
- [ ] Add focused tests for exact synchronous cursor-only fields, synchronous primary-bundled fields, worker regression, and sidecar regression.
- [ ] Run focused KMS worker, sidecar, plane-delta, and cursor trace tests.
- [ ] Commit: `feat: trace worker and synchronous cursor submissions`.

## Task 4: Bind frozen identity at submission and consume it at pageflip

**Files:**
- Modify: `src/native_output/runtime/kms_worker.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/cycle/pageflip.rs`
- Modify: `src/native_output/runtime/presentation_cursor.rs`
- Modify: `src/compositor/input.rs`
- Modify: `src/compositor/state/pointer_constraints.rs`

- [ ] Capture the current reveal snapshot only when the real cursor or primary-plus-cursor state receives its physical `PageFlipToken`; cover sync, worker, primary bundle, sidecar, cursor-only, hardware, and software/embedded-primary ownership.
- [ ] Bind worker ownership to the ledger from the main runtime’s submitted-event path, where output generation, CRTC, and the actual job token are known.
- [ ] Bind synchronous ownership immediately after the real token-bearing backend call succeeds.
- [ ] Change `trace_presented_cursor` to retrieve the snapshot by exact pageflip identity and never consult mutable `last_cursor_reveal_authority` for ownership.
- [ ] Replace the global first-visible boolean with per-reveal state. Emit the full comparison fields on first visibility and ensure a no-visible reveal emits `cursor_reveal_terminal reason=no_visible_cursor_requested` without waiting or claiming unrelated reveal B.
- [ ] Emit deterministic `superseded_by_new_reveal` when a new reveal starts, while retaining pending physical entries for correct late pageflip attribution.
- [ ] Add integration tests for TA→TB interleavings and late A pageflip after B begins, plus no-visible/unrelated-visible behavior.
- [ ] Run focused cursor trace, pointer unlock, pageflip, and cursor policy tests.
- [ ] Commit: `feat: freeze cursor reveal ownership at physical submission`.

## Task 5: Update the qualification report and run complete verification

**Files:**
- Add: `docs/superpowers/specs/2026-09-04-typhon-locked-cursor-visual-reveal-qualification-v1.1-report.md`

- [ ] Record starting HEAD `cafd18e901b564b3d3cee650c1506a9f91d49dab` and the exact implementation/report ending HEAD.
- [ ] Document both blockers, worker and synchronous coverage, the canonical payload, frozen identity key, bounded ledger, disabled neutrality, no-visible terminal, supersession, full comparison, and RED/GREEN tests.
- [ ] Explicitly state that no Gate 2 semantic cursor correction was implemented; ownership is frozen before the async boundary; pageflip never infers ownership from mutable current reveal; and no-visibility reveals cannot claim unrelated visible cursors.
- [ ] State the manual native A/B capture remains the next step with `OBLIVION_ONE_CURSOR=hardware` versus software and `TYPHON_CURSOR_PRESENTATION_TRACE=1`; do not claim teleport fixed.
- [ ] Run focused tests for cursor trace, pointer unlock, cursor policy, `NativeAtomicCursor`, pageflip, synchronous plane delta, KMS worker, and sidecar.
- [ ] Run the exact full verification:
  - `rtk cargo fmt --check`
  - `rtk cargo check --locked --all-targets`
  - `rtk cargo clippy --locked --all-targets -- -D warnings`
  - `rtk cargo test --locked`
  - `rtk git diff --check`
- [ ] Commit: `docs: report frozen cursor reveal trace qualification`.

## Completion criteria

The work is complete when the existing v1 trace chain remains intact, every worker and synchronous cursor submission has canonical exact KMS evidence, reveal attribution is keyed to the actual physical pageflip identity, overlapping reveals cannot cross-claim, first-visible comparison is per-reveal and full-state, no-visible reveals terminate explicitly, trace-disabled state is neutral, all focused and full verification passes, and the report records the Gate 1 boundary without asserting a semantic cursor fix.
