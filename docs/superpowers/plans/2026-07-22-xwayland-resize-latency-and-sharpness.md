# Typhon XWayland Resize Latency and Sharpness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make XWayland interactive resize geometry immediate and epoch-safe while rendering stale client content at 1:1 with black uncovered backing and correct XSync ordering.

**Architecture:** Keep compositor visual geometry, pointer ownership, and X11 client-content synchronization as separate state. Add an epoch to resize/content completion so late acknowledgements can publish pixels but cannot restore older placement. Render a compositor-owned backing rectangle below each managed XWayland tree, remove resize catch-up scaling, and classify integer-aligned crops as nearest/exact sampling.

**Tech Stack:** Rust, Cargo, x11rb, XSync/XWayland, compositor scene snapshots, EGL/GLES and CPU render paths.

## Global Constraints

- Preserve the prior XWayland publication reapplication fix for `render_placement` and `visual_clip`.
- Preserve `_NET_WM_SYNC_REQUEST`, `_XWAYLAND_ALLOW_COMMITS`, timeout fallback, ACK/commit floors, frame callbacks, buffer release, and explicit-sync ownership.
- Do not add sleeps, pointer debounce, configure throttling, forced full repaint, commit suppression, or unrelated KMS/pageflip changes.
- Keep stale XWayland content pixel-sharp at 1:1; black is the only fill for uncovered visual-box pixels.
- Keep generic Wayland resize and ordinary client-driven `render_target_size` behavior unchanged unless a focused regression proves otherwise.
- Work in the existing checkout because it contains user changes that must be preserved; do not reset or overwrite unrelated files.

---

### Task 1: Establish red protocol-order and transaction regressions

**Files:**
- Modify: `src/xwayland/xwm/commands.rs`
- Modify: `src/xwayland/xwm/resize_sync.rs`
- Modify: `src/xwayland/xwm/properties.rs`
- Modify: `src/xwayland/xwm/events_regression_tests.rs`
- Test: existing XWM command/resize-sync test modules

**Interfaces:**
- `begin_resize_sync()` must emit allow-off, sync client message, configure, then flush.
- The XWM must expose enough recording/test state to assert counter initialization and same-geometry suppression without changing runtime scheduling.

- [ ] **Step 1: Add a failing command-order test.** Record requests around a real `begin_resize_sync()` invocation and assert the sequence `ChangeProperty(_XWAYLAND_ALLOW_COMMITS=0)`, `SendEvent(_NET_WM_SYNC_REQUEST)`, `ConfigureWindow`, `Flush`.
- [ ] **Step 2: Run the order test and confirm it fails because configure precedes the sync message.**
- [ ] **Step 3: Add a failing counter-initialization test.** Manage a window with an arbitrary nonzero existing counter, assert baseline initialization occurs once, first request is above baseline, unrelated property refresh does not reinitialize, and counter replacement reinitializes.
- [ ] **Step 4: Add failing same-geometry final-release tests.** Cover both an idle/presented geometry and a final target equal to the in-flight transaction; assert no alarm, serial, allow gate, or duplicate ConfigureWindow is created.
- [ ] **Step 5: Implement only request ordering, counter initialization ownership, and duplicate suppression.** Keep counter values monotonic and reset ownership on counter replacement, destruction, and generation teardown.
- [ ] **Step 6: Run the focused XWM tests and existing `resize_sync` tests green.**

### Task 2: Separate size-content serialization from position updates

**Files:**
- Modify: `src/xwayland/xwm/commands.rs`
- Modify: `src/xwayland/xwm/resize_sync.rs`
- Modify: `src/compositor/server_backend.rs`
- Modify: `src/xwayland/xwm/resize_sync.rs` tests

**Interfaces:**
- Size-changing geometry remains in `ResizeSyncTracker`.
- Position-only configure requests must apply immediately or merge into current geometry authority without becoming stale `desired` content transactions.

- [ ] **Step 1: Add a failing position-only test** with a pending size transaction; assert x/y is emitted immediately while pending width/height remain unchanged.
- [ ] **Step 2: Add a failing queued-final test** where final geometry equals the presenting transaction; assert lifecycle promotion/finalization without another transaction.
- [ ] **Step 3: Implement geometry-field classification at the XWM command boundary.** Route only width/height changes through `queue_resize_desired`; apply x/y-only requests directly and update expected configure state.
- [ ] **Step 4: Run focused XWM and server-backend tests.** Confirm timeout and stale ACK behavior remains unchanged.

### Task 3: Add geometry epochs and seal compositor geometry on release

**Files:**
- Modify: `src/compositor/state/window_resize.rs`
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/desktop_windows.rs`
- Modify: `src/compositor/state/subsurfaces.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/compositor/server_backend.rs`
- Test: `src/compositor/tests/xwayland_resize_visual.rs` and window interaction tests

**Interfaces:**
- Introduce a monotonic compositor geometry epoch associated with the current root visual/authoritative geometry.
- Resize content transactions carry the epoch they were created for.
- `finalize_x11_resize()` is idempotent and accepts/clears only matching or non-authoritative content state.

- [ ] **Step 1: Add a failing release-then-move regression** using real interaction release, delayed ACK, delayed XWayland commit, and `ResizeSyncPresented`; assert the newer move position survives every event.
- [ ] **Step 2: Add a failing rapid-pointer regression** proving visual geometry advances on every pointer sample even while content transactions remain coalesced.
- [ ] **Step 3: Implement release sealing.** Apply the final pointer sample, persist the final visual geometry as the new authoritative starting point, end pointer ownership, and retain only content-pending state.
- [ ] **Step 4: Thread the epoch through pending resize completion and reject stale placement publication.** Late completion may clear matching content state but may not copy old placement into `surface_placements` or `toplevel_visual_geometries`.
- [ ] **Step 5: Make new move/resize start from `current_visual_root_window_geometry()` or sealed geometry and advance the epoch.**
- [ ] **Step 6: Run the release/move, interaction, timeout, stale ACK, and previous XWayland visual-assignment tests.**

### Task 4: Render a compositor-owned XWayland backing rectangle

**Files:**
- Modify: `src/compositor/render.rs`
- Modify: `src/compositor/surface.rs`
- Modify: `src/egl_renderer.rs`
- Modify: `src/egl_renderer/geometry.rs`
- Modify: `src/native_output/output/damage.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/compositor/state/window_resize.rs`
- Modify: `src/compositor/state/xwayland_windows.rs`
- Test: compositor render and XWayland scene snapshot tests

**Interfaces:**
- Add a root-only compositor-derived backing element with a solid black color and current visual box.
- The element must be below the complete XWayland root/subsurface tree, participate in scene snapshots and damage, and be present while content size is stale or resize content is pending.

- [ ] **Step 1: Add a failing grow-preview scene test** for an 800×600 committed XWayland buffer and 1100×760 visual geometry; assert a black 1100×760 backing, unchanged 800×600 client target, and no scaled target.
- [ ] **Step 2: Add failing shrink/crop scene tests** for all four edges; assert content remains 1:1 and uncovered/removed pixels are handled by the backing/clip rather than texture scaling.
- [ ] **Step 3: Implement backing element creation from the root visual geometry.** Keep the backing below all root-tree client elements and above wallpaper/desktop background.
- [ ] **Step 4: Add old/new backing bounds to scene comparison and damage calculation.** Do not force full-output repaint.
- [ ] **Step 5: Remove the backing only after matching/newer content has been accepted and no visual-content mismatch remains.**
- [ ] **Step 6: Run EGL/GLES and CPU/fallback scene tests with partial repaint both disabled and enabled.**

### Task 5: Remove stale XWayland resize scaling and fix sampling classification

**Files:**
- Modify: `src/compositor/render.rs`
- Modify: `src/compositor/surface.rs`
- Modify: `src/compositor/state/desktop_windows.rs`
- Modify: `src/compositor/state/window_resize.rs`
- Modify: `src/compositor/state/xwayland_windows.rs`
- Modify: `src/egl_renderer/geometry.rs`
- Modify: `src/egl_renderer.rs`
- Test: renderer sampling and XWayland resize scene tests

**Interfaces:**
- Ordinary interactive XWayland resize must not set `render_target_size` solely because frame geometry differs from committed content.
- `surface_sampling_for_plan()` selects `ExactNearest` for integer-aligned 1:1 UV crops and `ScaledLinear` only for actual scale/fractional sampling.

- [ ] **Step 1: Add failing sampling tests** for full identity, integer 620×480 crop, 801×600 scale, fractional UV crop, and output clipping with 1:1 mapping.
- [ ] **Step 2: Add a failing XWayland resize test** proving stale content is not assigned an 1100×760 render target during grow preview.
- [ ] **Step 3: Implement source-span calculation from UVs and buffer dimensions with a small tolerance.** Require integer-aligned boundaries and equal sampled/target dimensions for nearest classification.
- [ ] **Step 4: Make XWayland resize content use committed dimensions and visual clipping/backing rather than `render_target_size`.** Preserve explicit non-resize target sizing if an existing feature requires it.
- [ ] **Step 5: Run renderer, compositor, and XWayland sharpness tests green.**

### Task 6: Add diagnostics and preserve publication ownership

**Files:**
- Modify: `src/xwayland/xwm/commands.rs`
- Modify: `src/xwayland/xwm/resize_sync.rs`
- Modify: `src/xwayland/xwm/resize_runtime.rs`
- Modify: `src/compositor/state/xwayland_windows.rs`
- Modify: `src/compositor/server.rs`
- Test: XWayland trace-driven regressions

- [ ] **Step 1: Extend existing trace events** with geometry epoch, transaction/counter, requested/current geometry, command-order marker, ACK/allow/commit ages, pointer/content phases, current/queued desired geometry, source/crop/backing sizes, sampling mode, and stale-completion acceptance reason.
- [ ] **Step 2: Keep diagnostics behind existing trace/log environment flags.**
- [ ] **Step 3: Add/retain a real publication regression** asserting a fresh XWayland renderable still rederives `render_placement` and `visual_clip` after insertion.
- [ ] **Step 4: Run trace-enabled focused tests and verify ordering/epoch evidence without changing scheduling.**

### Task 7: Full regression and live qualification

**Files:**
- Modify: focused test modules only as needed
- Test: all affected XWM/compositor/render/native-output suites

- [ ] **Step 1: Run the named focused tests for ordering, counter initialization, duplicate final, grow backing, nearest crop, late presentation, position-only move, rapid pointer updates, and previous ghosting.**
- [ ] **Step 2: Run `cargo fmt --check`, `cargo check --locked --all-targets`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, `cargo build --locked --release`, `./bin/check-source-layout`, and `git diff --check`.**
- [ ] **Step 3: Run XWayland compositor/reactor tests with `OBLIVION_ONE_ENABLE_PARTIAL_REPAINT=0` and `=1`.**
- [ ] **Step 4: Inspect the live session for Steam and a sync-capable X11 client.** If available, run rapid grow/shrink, release-then-move, reverse-direction, stationary-pointer, and 60-second runs with structured tracing. If the live client/session is unavailable, report that limitation without inferring runtime behavior.
- [ ] **Step 5: Report exact publication paths, red assertions, post-fix geometry/content/sampling states, complete gate results, and any remaining artifact with trace evidence.**
