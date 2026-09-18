# Dynamic Per-Surface DMA-BUF Scanout Feedback Implementation Plan

> **For agentic workers:** Execute this plan inline in the current session. Do not dispatch subagents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement version-4 Linux DMA-BUF feedback that keeps default renderer feedback separate from surface-specific Direct Scanout preferences, updates live resources only when their normalized snapshot changes, and activates the preference after a proven exact KMS format/modifier mismatch.

**Architecture:** Keep the renderer/import feedback and exact client Direct Scanout capability catalog as separate authorities. Build comparable normalized snapshots before creating immutable format-table payloads; register each feedback resource by scope using a weak Wayland handle and shared interior state; reconcile all live resources from current global state plus surface hints. The native presentation path activates the exact candidate source surface on the pre-import unsupported-pair branch and clears stale hints when the candidate source changes or disappears.

**Tech Stack:** Rust, `wayland-server` 0.31, `wayland-protocols` Linux DMA-BUF v1 version 4, EGL/GLES DMA-BUF feedback, Atomic KMS, Cargo tests and Clippy.

## Global Constraints

- Remain on `zwp_linux_dmabuf_v1` version 4; do not add v6 semantics.
- Preserve exact FOURCC + modifier pairs; do not rewrite modifiers, substitute formats, convert buffers, bypass KMS, or bypass Atomic TEST_ONLY.
- Client Direct Scanout capabilities are `primary plane exact pairs ∩ renderer/import-compatible pairs ∩ opaque XRGB8888/XBGR8888 policy`.
- GBM allocation probing remains only where Typhon allocates its own output/render buffers; it is not an authority for client-owned Direct Scanout feedback.
- Default feedback is renderer/fallback feedback and has no unconditional scanout tranche.
- Scanout preference tranches are surface-scoped, first in order, and followed by renderer fallback.
- Surface destruction makes associated feedback inert without forcibly destroying the feedback object.
- Preserve unrelated worktree changes and stage only files belonging to each commit.
- Run Cargo through `rtk cargo` and compile in `/home/agony/GitHub/Typhon` so build artifacts stay in the repository folder.

---

### Task 1: Separate exact capability discovery from GBM allocation and add snapshot primitives

**Files:**
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs:149-172, 482-501`
- Modify: `src/compositor/dmabuf.rs:61-220`
- Test: `src/native_output/scanout/atomic_egl_gbm.rs` test module
- Test: `src/compositor/dmabuf.rs` test module

**Interfaces:**
- Produce `discover_direct_scanout_capabilities(plane_formats, renderer_dmabuf_feedback) -> Vec<DirectScanoutFormatCapability>` with no GBM probe parameter.
- Produce a comparable `DmabufFeedbackSnapshot` and a materialization path that preserves normalized table formats and ordered tranche semantics.
- Preserve `DirectScanoutFeedbackCapabilities::new` sorting/deduplication and `supports(format, modifier)` exact-pair lookup.

- [ ] **Step 1: Write the failing capability and snapshot tests.**

Add tests with explicit XRGB8888, XBGR8888, ARGB8888, invalid-modifier, renderer-only, and plane-only pairs. Call the desired two-argument capability helper and assert a fake GBM failure cannot remove an otherwise valid XRGB/XBGR pair. Add snapshot tests proving two independently materialized payloads compare equal despite different file descriptors, and that changing a modifier, target device, tranche order, or scanout flag changes equality.

```rust
#[test]
fn direct_scanout_capability_filter_uses_plane_and_renderer_intersection_only() {
    let pairs = [xrgb_pair(7), xbgr_pair(9), argb_pair(11)];
    let renderer = EglGlesDmabufFeedback::from_formats([
        EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier(7)),
        EglGlesDmabufFormat::new(DrmFormat::Xbgr8888, DrmModifier(9)),
    ]);

    assert_eq!(
        discover_direct_scanout_capabilities(&pairs, &renderer),
        vec![capability(DrmFormat::Xrgb8888, 7), capability(DrmFormat::Xbgr8888, 9)]
    );
}
```

- [ ] **Step 2: Run the focused tests and confirm the intended red failure.**

Run:

```bash
rtk cargo test direct_scanout_capability_filter_uses_plane_and_renderer_intersection_only -- --exact
rtk cargo test dmabuf_feedback_snapshot -- --nocapture
```

Expected: compilation/test failure because the capability helper still requires the GBM probe and the comparable snapshot API is not implemented. Do not change production code before observing this failure.

- [ ] **Step 3: Implement the pure capability intersection.**

Remove `probe: &mut impl GbmAllocationProbe` from `discover_direct_scanout_capabilities`. Keep the existing structural filters and exact `renderer_dmabuf_feedback.supports(...)` check. Leave `DeviceAllocationProbe` and the probe passed to `select_output_format_modifier_for_presentation` intact so Typhon-owned GBM allocation still proves what it needs to prove.

```rust
fn discover_direct_scanout_capabilities(
    plane_formats: &[DrmFormatModifierPair],
    renderer_dmabuf_feedback: &EglGlesDmabufFeedback,
) -> Vec<DirectScanoutFormatCapability> {
    plane_formats.iter().copied().filter_map(|pair| {
        let format = DrmFormat::from_fourcc(pair.fourcc);
        (format.is_opaque_rgb8888()
            && pair.modifier != DrmModifier::INVALID.0
            && renderer_dmabuf_feedback.supports(format, DrmModifier(pair.modifier)))
            .then_some(DirectScanoutFormatCapability {
                format: pair.fourcc,
                modifier: pair.modifier,
            })
    }).collect()
}
```

- [ ] **Step 4: Implement normalized snapshots and immutable payload materialization.**

Represent snapshot tranches by exact `(format, modifier)` entries plus `scanout` and `target_device`; normalize table and tranche ordering deterministically before deriving format-table indices. Make the renderer-only builder use `feedback.formats()` and never unconditionally consume `feedback.scanout_formats()`. Make the surface-preference builder union only the current exact capability catalog with renderer formats, then place scanout before render. Keep file creation in a separate `materialize` function and never mutate a sent file.

- [ ] **Step 5: Run the focused red tests again until green.**

Run:

```bash
rtk cargo test direct_scanout_capability_filter -- --nocapture
rtk cargo test dmabuf_feedback_snapshot -- --nocapture
rtk cargo test scanout_tranche -- --nocapture
```

Expected: all focused capability, exact-pair, default-renderer, and snapshot equality tests pass.

- [ ] **Step 6: Commit the isolated data-model change.**

```bash
git add src/native_output/scanout/atomic_egl_gbm.rs src/compositor/dmabuf.rs
git commit -m "refactor: separate client scanout capabilities from gbm allocation"
```

### Task 2: Add scoped feedback resources and live-resource registry

**Files:**
- Modify: `src/compositor/dmabuf.rs`
- Modify: `src/compositor/protocols/buffers.rs:360-450`
- Modify: `src/compositor/mod.rs` imports and `CompositorState` fields
- Modify: `src/compositor/state/dmabuf_feedback.rs`
- Test: `src/compositor/tests/support/registry_state.rs`
- Test: `src/compositor/tests/support/clipboard_dmabuf.rs`
- Test: `src/compositor/tests/protocol_buffers.rs`

**Interfaces:**
- Add `DmabufFeedbackScope::{Default, Surface(u32), InertSurface}`.
- Add resource user data carrying shared mutable binding state and immutable latest materialized payload.
- Add `CompositorState` registry methods to register, reconcile, inert, and remove feedback resources.
- Make `GetDefaultFeedback` and `GetSurfaceFeedback` separate request branches; the latter resolves `surface.data::<SurfaceData>()` and stores its exact `surface_id`.

- [ ] **Step 1: Extend the client test state to identify complete feedback sequences.**

Record feedback resource object IDs, `done` counts, tranche scanout flags, tranche indices, target devices, and format-table contents for both default and surface feedback. Add helpers that create two `wl_surface` objects, request one surface feedback object per surface, and roundtrip after each update.

- [ ] **Step 2: Write failing protocol tests for scope separation and resource ownership.**

Add tests named `default_feedback_has_renderer_fallback_without_scanout`, `surface_feedback_initially_has_no_scanout_hint`, `surface_feedback_update_is_local_to_surface_a`, `surface_feedback_preserves_exact_pairs`, and `feedback_resource_destroy_removes_registry_entry`. The tests must assert A receives an ordered scanout-first sequence only after A is activated, B receives no update, and destroying the feedback object removes the registry entry.

```rust
assert_eq!(a.done_count, 2);
assert_eq!(a.tranche_scanout, [true, false]);
assert_eq!(b.done_count, 1);
assert_eq!(b.tranche_scanout, [false]);
assert_eq!(a.scanout_pairs(), [(XRGB8888, 7), (XBGR8888, 9)]);
```

- [ ] **Step 3: Run the protocol tests and verify red behavior.**

Run:

```bash
rtk cargo test default_feedback_has_renderer_fallback_without_scanout -- --exact
rtk cargo test surface_feedback_update_is_local_to_surface_a -- --exact
rtk cargo test feedback_resource_destroy_removes_registry_entry -- --exact
```

Expected: the tests fail because the existing OR-pattern gives both requests the same global policy and no resource registry exists.

- [ ] **Step 4: Implement the binding state and weak-resource registry.**

Use `wayland_server::Resource::downgrade()` for registry entries so the registry does not keep dead Wayland objects alive. Store the binding state in `Arc<Mutex<_>>` because `Dispatch::destroyed` receives only an immutable user-data reference. Keep the latest `DmabufFeedbackData`/format-table file alive in that binding state until the next snapshot replaces it.

```rust
struct DmabufFeedbackBinding {
    scope: DmabufFeedbackScope,
    last_snapshot: Option<DmabufFeedbackSnapshot>,
    payload: Option<DmabufFeedbackData>,
    inert: bool,
}

struct LiveDmabufFeedback {
    resource: wayland_server::Weak<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1>,
    binding: Arc<Mutex<DmabufFeedbackBinding>>,
}
```

Implement `destroyed()` for the feedback resource to remove its `ObjectId` from the registry. If a weak resource cannot be upgraded during reconciliation, remove the entry. Do not forcibly destroy resources from compositor state.

- [ ] **Step 5: Split the protocol request branches.**

For `GetDefaultFeedback`, build a `Default` binding and renderer-only snapshot. For `GetSurfaceFeedback { id, surface }`, resolve the exact `SurfaceData::surface_id`; use `InertSurface` if the surface metadata is unavailable, otherwise use `Surface(surface_id)`. Register the binding before sending its initial complete sequence. Leave the feedback resource request handler inert because updates are compositor-driven.

- [ ] **Step 6: Run the protocol tests and confirm green behavior.**

Run:

```bash
rtk cargo test default_feedback_has_renderer_fallback_without_scanout -- --nocapture
rtk cargo test surface_feedback_initially_has_no_scanout_hint -- --nocapture
rtk cargo test surface_feedback_preserves_exact_pairs -- --nocapture
rtk cargo test feedback_resource_destroy_removes_registry_entry -- --nocapture
```

Expected: default feedback contains renderer fallback only, initial surface feedback is renderer-only, exact pairs survive unchanged, and resource destruction leaves no registry entry.

- [ ] **Step 7: Commit scoped resource support.**

```bash
git add src/compositor/dmabuf.rs src/compositor/protocols/buffers.rs src/compositor/mod.rs src/compositor/state/dmabuf_feedback.rs src/compositor/tests/support/registry_state.rs src/compositor/tests/support/clipboard_dmabuf.rs src/compositor/tests/protocol_buffers.rs
git commit -m "feat: bind linux dmabuf feedback to surfaces"
```

### Task 3: Add surface hints, global reconciliation, and teardown semantics

**Files:**
- Modify: `src/compositor/state/dmabuf_feedback.rs`
- Modify: `src/compositor/mod.rs` state fields and initialization
- Modify: `src/compositor/server.rs` public state operations
- Modify: `src/compositor/state/surfaces.rs:1263-1382`
- Modify: `src/compositor/protocols/core.rs:346-357` to keep the existing surface resource destruction callback compatible with centralized teardown
- Test: `src/compositor/tests/protocol_buffers.rs`
- Test: `src/compositor/state/dmabuf_feedback.rs` unit tests

**Interfaces:**
- Add `OwnCompositorServer::activate_surface_scanout_hint(surface_id) -> bool`.
- Add `OwnCompositorServer::clear_surface_scanout_hint(surface_id) -> bool`.
- Add `OwnCompositorServer::reconcile_surface_scanout_candidate(Option<u32>) -> bool`.
- Add a compositor debug snapshot reporting active hint state, hinted surface, update count, duplicate suppressions, and advertised scanout pair count.

- [ ] **Step 1: Write failing tests for hint activation, deduplication, removal, and global replacement.**

Add `activating_same_surface_hint_sends_one_update`, `clearing_surface_hint_restores_renderer_feedback`, `surface_feedback_inert_after_surface_destroy`, and `global_capability_replacement_rebuilds_active_surface_once`. Use two surfaces for isolation and assert `done` counts rather than logs.

- [ ] **Step 2: Run the new tests and observe red failures.**

```bash
rtk cargo test activating_same_surface_hint_sends_one_update -- --exact
rtk cargo test clearing_surface_hint_restores_renderer_feedback -- --exact
rtk cargo test surface_feedback_inert_after_surface_destroy -- --exact
rtk cargo test global_capability_replacement_rebuilds_active_surface_once -- --exact
```

Expected: methods are absent and surface teardown does not currently detach feedback bindings.

- [ ] **Step 3: Add state-owned hint and diagnostics fields.**

Initialize an empty `HashSet<u32>`, an optional currently tracked candidate source, and bounded counters in `CompositorState::new`. Keep the authoritative `dmabuf_scanout_capabilities` as the only capability catalog; hints contain surface IDs only.

- [ ] **Step 4: Implement reconciliation and duplicate suppression.**

Have `activate_surface_scanout_hint` and `clear_surface_scanout_hint` change the set only when necessary, then reconcile affected surface bindings from the current renderer feedback, capability catalog, and target-device override. `set_dmabuf_feedback_with_scanout_capabilities_and_target` must update global fields first and reconcile all live bindings so recovery/output rebuilds update default and hinted surface resources. Compare snapshots before creating a new payload or sending events; increment the duplicate-suppressed counter for no-op reconciliation.

- [ ] **Step 5: Integrate centralized surface teardown.**

At the beginning or other single authoritative point in `unregister_surface_resource_with_reason`, clear the surface hint, mark all matching feedback binding state as `InertSurface`, remove those bindings from the live registry, and leave the actual Wayland feedback objects alive. The feedback `destroyed()` hook must remain safe after this removal.

- [ ] **Step 6: Integrate native candidate-source reconciliation.**

Expose the inspected candidate source ID from `DirectPresentationInspection`. After inspection, call `reconcile_surface_scanout_candidate(candidate_surface_id)`. Clear the previous active hint when the source changes or becomes `None`; do not activate a hint merely because a candidate exists. Preserve the hint when the same source receives a compatible replacement buffer.

- [ ] **Step 7: Run lifecycle and protocol tests until green.**

```bash
rtk cargo test activating_same_surface_hint_sends_one_update -- --nocapture
rtk cargo test clearing_surface_hint_restores_renderer_feedback -- --nocapture
rtk cargo test surface_feedback_inert_after_surface_destroy -- --nocapture
rtk cargo test global_capability_replacement_rebuilds_active_surface_once -- --nocapture
```

Expected: A changes exactly once per effective snapshot, B remains untouched, clearing returns A to renderer-only fallback, surface destruction makes A inert with no registry leak, and replacement capabilities rebuild the active preference exactly once.

- [ ] **Step 8: Commit state and lifecycle behavior.**

```bash
git add src/compositor/state/dmabuf_feedback.rs src/compositor/mod.rs src/compositor/server.rs src/compositor/state/surfaces.rs src/compositor/protocols/core.rs src/native_output/runtime/presentation_direct.rs src/native_output/runtime/presentation_cycle.rs src/compositor/tests/protocol_buffers.rs
git commit -m "feat: reconcile per-surface dmabuf scanout hints"
```

### Task 4: Trigger hints only for proven unsupported client allocations

**Files:**
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs:281-299`
- Modify: `src/native_output/scanout/direct_transition.rs` dispatch wrapper for the unchanged backend variants
- Test: `src/native_output/scanout/atomic_egl_gbm/direct.rs` tests or a focused native presentation test module
- Test: `src/native_output/runtime/presentation_direct.rs` tests for source identity

**Interfaces:**
- Keep the fallback reason string `primary_plane_format_modifier_unsupported`.
- Add a testable pre-import decision helper returning the original exact `(format, modifier)` and the source surface ID so the no-import/no-TEST_ONLY behavior is unit-testable.

- [ ] **Step 1: Write a failing pre-import trigger test.**

Construct a candidate with `surface_id != root_surface_id` and a modifier absent from `DirectScanoutFeedbackCapabilities`. Assert the trigger calls `activate_surface_scanout_hint(surface_id)`, returns the normal fallback reason, and records no import or TEST_ONLY attempt. Add a compatible-pair case that does not activate a new hint.

- [ ] **Step 2: Run the focused test and verify red.**

```bash
rtk cargo test primary_plane_format_modifier_unsupported_activates_source_surface_hint -- --exact
rtk cargo test compatible_client_pair_does_not_activate_scanout_hint -- --exact
```

Expected: the current code returns the fallback without changing compositor feedback state, so the activation assertion fails.

- [ ] **Step 3: Activate the hint at the existing exact-pair gate.**

Immediately before returning the existing unsupported-pair fallback, call `server.activate_surface_scanout_hint(candidate.surface_id)`. Do not rewrite `candidate_format` or `candidate_modifier`, do not import, and do not issue TEST_ONLY. Keep diagnostics bounded by relying on the state reconciliation deduplication path.

```rust
if !self.dmabuf_scanout_capabilities.supports(candidate_format, candidate_modifier) {
    server.activate_surface_scanout_hint(candidate.surface_id);
    return Ok(DirectScanoutAttempt::Fallback(
        "primary_plane_format_modifier_unsupported",
    ));
}
```

- [ ] **Step 4: Run the trigger/source tests until green.**

```bash
rtk cargo test primary_plane_format_modifier_unsupported_activates_source_surface_hint -- --nocapture
rtk cargo test compatible_client_pair_does_not_activate_scanout_hint -- --nocapture
rtk cargo test source_surface_hint_survives_compatible_reallocation -- --nocapture
```

Expected: only the exact mismatch activates feedback, the original buffer remains untouched, and the hint survives while the source remains the candidate.

- [ ] **Step 5: Commit the native trigger.**

```bash
git add src/native_output/scanout/atomic_egl_gbm/direct.rs src/native_output/scanout/direct_transition.rs src/native_output/runtime/presentation_direct.rs src/native_output/runtime/presentation_cycle.rs
git commit -m "feat: request surface scanout feedback after exact mismatch"
```

### Task 5: Diagnostics, NVIDIA policy wording, and semantic test updates

**Files:**
- Modify: `src/native_output/runtime/cycle_dispatch.rs` doctor detail formatting and data collection
- Modify: `src/native_output/scanout/feedback_policy.rs:124-139` policy diagnostic label
- Modify: `src/native_output/runtime/bootstrap.rs:1556-1584` compatibility input wording and tranche-count semantics
- Modify: `src/native_output/scanout/feedback.rs` to keep global rebuilds routed through the reconciliation operation
- Modify: existing protocol/native tests encoding unconditional default scanout policy

**Interfaces:**
- Report bounded doctor fields: `surface_feedback_hint_active`, `surface_feedback_hint_surface`, `surface_feedback_updates`, `surface_feedback_duplicate_suppressed`, and optionally `surface_feedback_scanout_pair_count`.
- Preserve same-device normalization and target-device override for dynamic scanout tranches.
- Use `surface_feedback_policy=dynamic-per-surface-scanout` in startup diagnostics.

- [ ] **Step 1: Write failing diagnostic and policy tests.**

Assert the startup diagnostic uses the dynamic policy label, the doctor string includes the new bounded fields, and the NVIDIA normalization tests still report the target device consistently for a dynamically built scanout tranche.

- [ ] **Step 2: Run the focused policy/diagnostic tests and confirm red.**

```bash
rtk cargo test surface_feedback_policy -- --nocapture
rtk cargo test dmabuf_feedback_compatibility -- --nocapture
rtk cargo test direct_scanout_doctor_detail -- --nocapture
```

Expected: the old `default-and-surface-same` string is still present and the doctor output lacks the new state fields.

- [ ] **Step 3: Implement bounded diagnostics and preserve normalization.**

Read the state debug snapshot at the existing doctor assembly point and append only scalar fields to the existing diagnostic string. Do not dump format tables or add per-frame logging. Ensure dynamic scanout snapshots use `scanout_target_device_override` exactly as renderer fallback does.

- [ ] **Step 4: Update semantic tests.**

Move scanout-first assertions from default-feedback tests to surface-feedback tests. Keep renderer/default tests asserting fallback availability, exact allowed renderer formats, main-device identity, and no scanout tranche without a surface hint. Preserve empty-capability omission and renderer-only-format exclusion coverage.

- [ ] **Step 5: Run focused policy and diagnostic tests until green.**

```bash
rtk cargo test surface_feedback_policy -- --nocapture
rtk cargo test dmabuf_feedback_compatibility -- --nocapture
rtk cargo test direct_scanout_doctor_detail -- --nocapture
rtk cargo test scanout_tranche -- --nocapture
```

- [ ] **Step 6: Commit diagnostics and semantic test updates.**

```bash
git add src/native_output/runtime/cycle_dispatch.rs src/native_output/scanout/feedback_policy.rs src/native_output/runtime/bootstrap.rs src/native_output/scanout/feedback.rs src/compositor/tests/protocol_buffers.rs src/native_output/scanout/atomic_egl_gbm.rs
git commit -m "test: cover dynamic surface feedback diagnostics"
```

### Task 6: Full verification and hardware handoff

**Files:**
- Modify only the implementation files listed in Tasks 1-5 when verification exposes a defect.
- No hardware-specific workaround files may be added.

- [ ] **Step 1: Run formatting and focused suites.**

```bash
rtk cargo fmt --check
rtk cargo test compositor::tests::protocol_buffers -- --nocapture
rtk cargo test direct_scanout_capability_filter -- --nocapture
rtk cargo test surface_feedback -- --nocapture
rtk cargo test feedback_policy -- --nocapture
```

- [ ] **Step 2: Run the complete required validation commands in the repository folder.**

```bash
rtk cargo check
rtk cargo test
rtk cargo clippy --all-targets --all-features -- -D warnings
git diff --check
```

Record each exit status and the exact failure scope. If unrelated concurrent edits block a global command, run the narrowest trustworthy command for this change and report the unrelated failure without modifying those edits.

- [ ] **Step 3: Inspect the final diff and worktree.**

```bash
git status --short
git diff --stat HEAD~5..HEAD
git diff --check HEAD~5..HEAD
git rev-parse HEAD
```

Confirm no unsupported modifier rewrite, KMS bypass, game-specific condition, ARGB opaque admission, or v6 protocol change entered the diff. Confirm only intended files are staged in each implementation commit.

- [ ] **Step 4: Commit any narrowly verified correction.**

If a verification command exposes an implementation defect, fix only that
defect, rerun the failing focused command and the affected global check, then
stage the exact Rust paths shown by `git status --short` and commit with
`fix: complete per-surface dmabuf feedback reconciliation`. Do not stage the
pre-existing unrelated worktree paths.

- [ ] **Step 5: Report hardware commands without claiming success.**

On the RTX 3060 Ti system, temporarily remove the same composition blockers used by the existing Cyberpunk investigation, then capture bounded logs and the doctor state while launching the native Wayland path:

```bash
drm_info -j
TYPHON_DIRECT_SCANOUT_DOCTOR=1 TYPHON_LOG=info typhon 2>&1 | tee /tmp/typhon-cyberpunk-feedback.log
WAYLAND_DEBUG=client wine-wayland Cyberpunk2077.exe 2>&1 | tee /tmp/wine-wayland-cyberpunk-feedback.log
```

Verify the first old `XBGR8888 + 0x300000000e08014` rejection is preserved, then look for a changed surface-specific feedback sequence, a client allocation using an exact plane-supported `...606...` pair or linear pair, and subsequent `import_attempts`, `test_only_attempts`, `submissions`, `presentations`, and `entries`. If Wine-Wayland keeps allocating `0x300000000e08014`, report that it ignored the feedback; do not rewrite the buffer in Typhon.
