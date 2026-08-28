# Typhon Synchronous SHM Buffer Lifetime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make published `wl_shm` buffer uses releasable immediately after successful pixel materialization, with no readable released backing retained in current surface state, while leaving O1 and DMA-BUF/KMS ownership unchanged.

**Architecture:** Replace `current_surface_buffers: HashMap<u32, PendingSurfaceBuffer>` with a current-content state that stores either an explicitly unmaterialized `PendingSurfaceBuffer` lease or a materialized compositor-owned `CommittedSurfaceBuffer` plus metadata. A consuming `PendingSurfaceBuffer::materialize_for_publication` operation creates the owned content and release proof together. Ordinary materialized SHM release is emitted at that boundary; only unmaterialized leases and external DMA-BUF release authorities remain eligible for deferred cleanup.

**Tech Stack:** Rust, Wayland server resources, Smithay-style compositor state, `rtk cargo` checks/tests, deterministic protocol integration tests.

## Global Constraints

* Keep O1/triple render-ahead enabled; do not change scheduler defaults or force 60 Hz.
* Do not special-case Electron, ChatGPT, Chromium, or client application identity.
* Never reread a released SHM backing; current published pixels must be compositor-owned.
* Do not apply SHM early-release semantics to DMA-BUF, explicit sync, or direct scanout/KMS ownership.
* Same `wl_buffer` object reattachment is a new use and must receive one release for that use.
* Synchronized subsurface content must not publish before its parent transaction applies.
* Preserve partial-damage locality and the existing cursor commit-sequence identity behavior.
* Use `apply_patch` for file edits and `rtk` for repository commands, reads, checks, and tests.
* Do not modify or stage unrelated existing worktree changes.

---

### Task 1: Establish RED protocol tests and test observability

**Files:**
- Modify: `src/compositor/tests/surface_frames.rs`
- Modify: `src/compositor/tests/support/frame_buffer_client.rs`
- Modify: `src/compositor/tests/support/server_runtime.rs`
- Modify: `src/compositor/state/frame_tests.rs`
- Create: `src/compositor/tests/support/shm_lifetime_oracle.rs` if the deterministic oracle does not fit existing support helpers

**Interfaces:**
- Consume existing `ServerCommand`, `RegistryTestState`, `OwnCompositorServer`, and test SHM buffer helpers.
- Produce tests that observe release count/order, rendered owned pixels, release metrics, and deterministic A/B reuse.

- [ ] **Step 1: Add Test A for release before presentation.**

Change the current SHM release integration helper so it commits a visible SHM buffer, performs the protocol roundtrip, observes one release before sending `ServerCommand::PresentFrame`, then holds presentation and asserts no second release. Name the test `wayland_client_shm_release_happens_after_materialization_before_present`.

- [ ] **Step 2: Add Test B and reverse the invalid old expectation.**

Keep the first commit as P1, wait for its release, mutate the SHM file to P2, issue a damage-only commit without attaching a buffer, and assert the rendered pixels remain P1. Rename the old test to `wayland_surface_damage_only_commit_keeps_owned_shm_snapshot`.

- [ ] **Step 3: Add Test C for same-object reuse.**

Use one `TestShmBuffer`, commit P1 and observe its release, mutate the same backing to P2, attach the same `wl_buffer` again, commit, and assert P2 is rendered and the release counter increases by exactly one.

- [ ] **Step 4: Add the deterministic Test D oracle.**

Implement a virtual-clock model with `const REFRESH_NS: u64 = 6_060_606`, two client buffers A/B, and two output slots (`pending`, `ready`). Drive commit/materialize/release events without `thread::sleep`; assert `render_ahead_successes > 0`, both buffers rotate, each release occurs at materialization, no release waits on READY/PENDING presentation, and one-refresh production remains possible.

- [ ] **Step 5: Add red tests E–J at the narrowest existing seams.**

Add tests for synchronized application, superseded cached attachment release, unassigned adoption, copy failure, SHM/DMA-BUF/remove/destroy/shutdown transitions, cursor damage-only behavior, and XWayland retirement. Each test must assert behavior rather than only internal collection sizes.

- [ ] **Step 6: Run the focused red suite.**

Run:

```bash
rtk cargo test --lib compositor::tests::surface_frames::wayland_client_shm_release_happens_after_materialization_before_present
rtk cargo test --lib compositor::tests::surface_frames::wayland_surface_damage_only_commit_keeps_owned_shm_snapshot
rtk cargo test --lib compositor::tests::surface_frames::wayland_same_buffer_object_reuse_materializes_new_content_once
rtk cargo test --lib compositor::tests::surface_frames::two_buffer_shm_o1_oracle_keeps_rotating_with_render_ahead
```

Expected: the new ownership assertions fail against the current frame-batch-bound SHM release and damage-only reread implementation; failures must be behavior failures, not compilation or test setup errors.

### Task 2: Introduce materialized current-content types

**Files:**
- Modify: `src/compositor/state_data.rs`
- Modify: `src/render_backend/buffer.rs` only if an owned-content constructor/accessor is needed
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/surface.rs` only if current-content-to-renderable conversion needs a focused helper

**Interfaces:**
- Produce `CurrentSurfaceBuffer`, `MaterializedSurfaceBuffer`, or equivalent names with metadata accessors for buffer ID, size, transform, viewport, and placement.
- Produce `PendingSurfaceBuffer::materialize_for_publication(previous: Option<&CommittedSurfaceBuffer>, damage: &RenderableSurfaceDamage) -> io::Result<(MaterializedSurfaceBuffer, SurfaceBufferRelease)>` or an equivalent consuming API.
- The returned release proof must only be constructible after `to_committed_buffer_for_size` and any required damage copy succeeds.

- [ ] **Step 1: Write unit tests for the materialization contract.**

Cover full copy, same-ID partial copy into an owned snapshot, DMA-BUF handle preservation without CPU reads, and copy failure returning `Err` without a release proof.

- [ ] **Step 2: Add the current-content representation.**

Store no `wl_buffer::WlBuffer` or `ShmBufferData` in the materialized variant. Keep protocol identity as scalar metadata (`BufferId` and protocol ID) and keep owned pixels in `CommittedSurfaceBuffer::ShmSnapshot`.

- [ ] **Step 3: Implement consuming SHM materialization.**

For a new/size-changed SHM use, copy the full buffer. For a same-ID/same-size re-use with an existing SHM snapshot, clone the owned snapshot and call `read_pixels_into_with_damage` only for the normalized damage. For DMA-BUF, retain the existing handle path and return its existing release authority separately.

- [ ] **Step 4: Run unit tests and format.**

Run `rtk cargo test --lib render_backend::buffer` and the new state-data tests, then `rtk cargo fmt --check`.

### Task 3: Publish materialized SHM and fix damage-only semantics

**Files:**
- Modify: `src/compositor/state/surface_commits.rs`
- Modify: `src/compositor/state/helpers.rs`
- Modify: `src/compositor/state/surface_commit_cursor.rs`
- Modify: `src/compositor/state/xwayland_windows.rs`

**Interfaces:**
- Consume the materialization API from normal, minimized, cursor, XWayland, and role-adoption paths.
- Replace `update_renderable_surface_buffer(..., &PendingSurfaceBuffer, ...)` with a helper that updates from materialized owned content and never reads client backing.

- [ ] **Step 1: Refactor the normal publication path.**

Materialize before mutating `RenderableSurface`; on failure release the unmaterialized use through cleanup and return. Update the renderable surface from owned content, install the materialized current state, then immediately send the SHM release proof. Preserve DMA-BUF insertion/replacement in `active_dmabuf_buffers`.

- [ ] **Step 2: Refactor minimized and XWayland publication.**

Use the same boundary for minimized content and `commit_xwayland_surface_buffer`. Update `adopt_current_xwayland_surface_content` to consume current owned content, and make `retire_xwayland_attachment` release only unmaterialized leases while relying on the existing DMA-BUF authority for external buffers.

- [ ] **Step 3: Refactor cursor publication and damage-only update.**

Materialize cursor SHM before installing `client_cursor_surfaces`; make cursor damage-only update its existing owned pixels and metadata without accessing `current_surface_buffers` as a pending backing. Preserve commit sequence and cursor generation updates.

- [ ] **Step 4: Remove the damage-only SHM reread.**

Change `commit_surface_damage_only` to derive dimensions/metadata from current committed state and use the already-owned `RenderableSurface.buffer` pixels. A bufferless commit cannot create a new SHM copy or release lease.

- [ ] **Step 5: Run Tests A–C and focused cursor/XWayland tests.**

Run the four new tests plus:

```bash
rtk cargo test --lib compositor::tests::surface_frames::wayland_same_size_buffer_rotation_preserves_partial_damage
rtk cargo test --lib compositor::tests::xwayland
rtk cargo test --lib compositor::tests::xwayland_pointer_batch
```

### Task 4: Handle unassigned and synchronized ownership explicitly

**Files:**
- Modify: `src/compositor/state/surface_commits.rs`
- Modify: `src/compositor/state/surface_transactions.rs`
- Modify: `src/compositor/subsurface.rs`
- Modify: `src/compositor/state/subsurfaces.rs`
- Modify: `src/compositor/state/shutdown.rs`

**Interfaces:**
- Unassigned current state retains `CurrentSurfaceBuffer::Unmaterialized(PendingSurfaceBuffer)` until adoption.
- Cached synchronized commits retain unmaterialized attachments until transaction application; supersede and teardown paths consume leases without reading.

- [ ] **Step 1: Add unassigned lease tests and implementation.**

Store unassigned SHM as an explicit lease, release an older superseded lease immediately without reading, materialize on role adoption, replace it with owned current content, and release exactly once. A materialized unassigned state must never be reread.

- [ ] **Step 2: Materialize at synchronized transaction application.**

Keep `CachedSubsurfaceCommit` atomic. `apply_cached_subsurface_commit` must materialize a buffer before `commit_surface_buffer_by_role` publishes it; no child is published early. `CachedSubsurfaceCommit::merge` may release replaced leases immediately because they will never be read.

- [ ] **Step 3: Make shutdown and destruction release leases exactly once.**

Drain cached and pending unmaterialized attachments through a helper that releases without reading. Materialized SHM requires no deferred lease; DMA-BUF keeps its existing release authority.

- [ ] **Step 4: Run Tests E–I and existing subsurface suites.**

Run the new synchronized/unassigned/copy-failure/transition tests and:

```bash
rtk cargo test --lib compositor::subsurface
rtk cargo test --lib compositor::tests::surface_frames
```

### Task 5: Remove ordinary SHM from presentation frame batches

**Files:**
- Modify: `src/compositor/frame_batch.rs`
- Modify: `src/compositor/state/frames.rs`
- Modify: `src/compositor/state/frame_callbacks.rs`
- Modify: `src/compositor/state/surface_pacing.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/tests/support/server_runtime.rs`
- Modify: `src/compositor/state/frame_tests.rs`

**Interfaces:**
- `CompositorFrameBatch` retains callbacks, feedback, timing/FIFO claims, damage lineage, and `dmabuf_releases_to_complete_on_present`; it has no `shm_buffer_releases` field.
- `pending_buffer_releases` is removed or reduced to unmaterialized cleanup only; no ordinary published SHM release enters a frame batch.

- [ ] **Step 1: Update frame-batch constructors and tests to compile against the reduced shape.**

Delete SHM capture/restore/retirement/scrub loops and adjust all test fixture literals in `surface_pacing.rs` and frame tests. Keep DMA-BUF capture and completion unchanged.

- [ ] **Step 2: Add frame-batch invariant assertions.**

Assert in tests that an output frame with SHM content has zero presentation-bound SHM releases while a replaced DMA-BUF remains presentation-owned.

- [ ] **Step 3: Run frame-focused tests.**

Run `rtk cargo test --lib compositor::state::frame_tests`, `rtk cargo test --lib compositor::tests::surface_frames`, and `rtk cargo test --lib compositor::native_output`.

### Task 6: Add bounded SHM lifetime metrics and prove no released-backing reads

**Files:**
- Modify: `src/compositor/state_data.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/frames.rs`
- Modify: `src/compositor/state/surface_commits.rs`
- Modify: `src/compositor/tests/support/server_runtime.rs`
- Modify: `src/compositor/tests/surface_frames.rs`

**Interfaces:**
- Add a bounded `ShmBufferLifetimeMetrics` value and getter with counters named `shm_materializations_total`, `shm_materialization_failures_total`, `shm_releases_after_materialization_total`, `shm_releases_deferred_unmaterialized_total`, `shm_releases_superseded_without_read_total`, `presentation_bound_shm_release_total`, and `released_shm_backing_read_attempts_total`.

- [ ] **Step 1: Add metric unit tests.**

Assert counters increment once per materialization/release/failure/supersede and that normal published SHM leaves `presentation_bound_shm_release_total == 0`.

- [ ] **Step 2: Instrument the ownership transitions.**

Increment counters at the consuming materialization, failed copy, immediate release, unmaterialized defer/supersede, and impossible released-backing access guard. Use saturating arithmetic and no per-frame unbounded storage.

- [ ] **Step 3: Expose metrics through the existing test server query.**

Extend the existing server-runtime metrics response without changing production scheduler behavior.

### Task 7: Adversarial review and verification

**Files:**
- Review all changed ownership paths and tests; modify only the scoped files when a review question lacks evidence.

- [ ] **Step 1: Search for forbidden live-backing paths.**

Run:

```bash
rtk rg -n 'current_surface_buffers|read_pixels_into_with_damage|shm_buffer_releases|pending_buffer_releases|release_target' src/compositor src/render_backend
```

Confirm every SHM read is reachable only from an unmaterialized use before its release proof, and every current published state points to owned content.

- [ ] **Step 2: Run all required checks.**

```bash
rtk cargo fmt --check
rtk cargo check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk cargo test
rtk git diff --check
rtk git status --short
```

- [ ] **Step 3: Review each ownership question against source/tests.**

Answer released-backing reread, bufferless reacquisition, same-object reuse, synchronized atomicity, unassigned adoption, superseded cleanup, destroy/disconnect/shutdown, failed materialization, DMA-BUF explicit sync, O1 READY/PENDING ownership, partial damage, cursor identity, and XWayland duplication with exact test names and paths.

- [ ] **Step 4: Commit the implementation in reviewable slices.**

Use separate commits for red tests, materialization/current-content implementation, frame-batch/metrics cleanup, and any optional trace-rate-limit change. Stage only files belonging to each commit.

### Task 8: Native O1 qualification and report

**Files:**
- Create: `REPORT-2026-08-28-typhon-shm-buffer-lifetime-closure.md`

- [ ] **Step 1: Run the supplied native configuration with O1 enabled.**

Use the exact environment from the request, including `OBLIVION_ONE_TRIPLE_BUFFERING=auto`, `OBLIVION_ONE_MODE=1920x1080@165`, native EGL GBM, direct scanout off, and `TYPHON_FRAME_PACING_TRACE=1`.

- [ ] **Step 2: Reproduce the active Electron scenarios.**

Capture idle, hover/tooltips, UI open/close, scroll, resize, and post-resize interaction. Segment active windows and report render-ahead attempts/successes, release timing, frame callback cadence, root commit cadence, and KMS stability.

- [ ] **Step 3: Report unavailable evidence honestly.**

If the DRM/KMS machine or `session(9).zip`/`session(10).zip` is unavailable, record the exact blocker and do not claim native acceptance. Include the supplied A/B facts as baseline context, not as post-fix results.
