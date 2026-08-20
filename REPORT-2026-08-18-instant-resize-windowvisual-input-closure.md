# Typhon Instant Resize and WindowVisual Pointer Ownership Closure

Date: 2026-08-18

## 1. Baseline HEAD and status

The supplied dirty implementation baseline was recorded before editing:

```text
HEAD: 2fc5fd1528f614eb5bae8a6491d0aee80f2975de
```

The repository was already substantially dirty: 64 tracked files were modified and prior closure files were untracked. The required design and plan were committed separately as:

```text
0ef9f7b docs: design instant resize and pointer ownership closure
```

The implementation remains in the dirty working tree. Task-owned changes are mixed with earlier closure work in several shared files, so no broad staging or artificial commit was made.

## 2. Resize latency root causes

### CONFIRMED — deferred visual authority

Before this closure, pointer motion computed a target, stored `PendingInteractiveResizeUpdate`, and waited for `prepare_frame()` to apply it. The pointer-to-visual path therefore crossed a frame-preparation boundary.

The pending visual record has been removed. Accepted resize motion now clamps the target and calls the existing `queue_resize_root_window_to()` / `preview_resize_root_window_to()` path during `update_window_interaction_by_id()`. The existing `ToplevelVisualGeometry` remains the visual authority for rendering, SSD layout, X11 frame geometry, hit testing, clipping, damage generation, and pointer coordinates.

### CONFIRMED — local one-configure serialization

`ResizeConfigureFlow` previously refused every new send while any configure, ACKed-but-uncaptured state, or captured commit existed. Captured commits now remain owned in the capture ledger but do not consume protocol-pressure capacity.

### STRONG HYPOTHESIS — perceived lag mechanism

The old visual frame delay and configure starvation jointly explained the observed trailing frame/titlebar. Client content can still lag when a client is slow to ACK or commit; stale client content remains un-stretched by default.

## 3. Titlebar focus root cause

### CONFIRMED

The old pointer path searched only client renderable surfaces. A point over A's server-side titlebar could therefore resolve B's client surface underneath. `PointerEnter` focus then changed `focused_window_id` from A to B, which correctly made A's SSD inactive and caused the associated focus/input work.

## 4. Hyprland comparison

Hyprland's floating resize path mutates the target box synchronously from pointer motion and warps the position/size so ordinary animation does not trail direct manipulation. Typhon adopts that user-visible invariant while retaining Typhon's existing visual geometry, damage, XDG lifecycle, and render-ahead ownership model. No Hyprland architecture was ported.

Reference: [Hyprland DragController](https://github.com/hyprwm/Hyprland/blob/main/src/layout/supplementary/DragController.cpp), [WindowTarget](https://github.com/hyprwm/Hyprland/blob/main/src/layout/target/WindowTarget.cpp), and [ViewHitTester](https://github.com/hyprwm/Hyprland/blob/main/src/desktop/state/ViewHitTester.cpp).

## 5. KWin comparison

KWin computes the next interactive geometry directly from the pointer and applies normal Wayland interactive resize synchronously. Its special synchronization path is primarily relevant to synchronization-sensitive X11 behavior. Typhon keeps X11 backend synchronization separate from the immediate compositor visual preview.

KWin also resolves the top-level decorated window before deciding whether the pointer is in client or decoration space. Typhon now expresses the same ownership invariant through `PointerSceneHit` without fabricating a Wayland decoration surface.

Reference: [KWin window.cpp](https://github.com/KDE/kwin/blob/master/src/window.cpp), [xdgshellwindow.cpp](https://github.com/KDE/kwin/blob/master/src/xdgshellwindow.cpp), [input.cpp](https://github.com/KDE/kwin/blob/master/src/input.cpp), and [pointer_input.cpp](https://github.com/KDE/kwin/blob/master/src/pointer_input.cpp).

## 6. Previous and new resize data flow

Previous:

```text
pointer motion
  -> compute target
  -> PendingInteractiveResizeUpdate
  -> prepare_frame()
  -> visual geometry
  -> render
```

New:

```text
pointer motion
  -> clamp target
  -> ToplevelVisualGeometry immediately
       -> WindowVisual/SSD/X11 frame geometry
       -> hit testing and local coordinates
       -> render generation and old/new scene damage
  -> bounded latest-wins configure queue
  -> same-dispatch client flush
  -> ACK/commit capture and final reconciliation
```

`prepare_frame()` no longer applies a resize preview. A deterministic test, `interactive_resize_updates_visual_geometry_before_frame_prepare`, calls the state interaction update and observes the new geometry without preparing a frame.

## 7. Configure-window design and chosen bound

The selected wire-level bound is:

```text
MAX_IN_FLIGHT_RESIZE_CONFIGURES = 3
```

The ledger counts only:

```text
sent but not ACKed
ACKed but not captured
```

Captured commits remain in an exact ownership queue but do not block a newer desired target. At capacity, only one unsent latest target is retained. `take_sendable()` prioritizes the final `resizing=false` target over an intermediate target.

The flow records requested/sent/coalesced targets, capacity blocking, retained peak, protocol-pressure peak, preview age, and completion/release counts. The native performance record now also exposes `resize_configure_capacity_blocked`.

## 8. ACK, supersede, and capture semantics

The ledger behavior is:

```text
ACK newer serial C
  -> retire older outstanding A/B
  -> make C the only uncaptured ACK owner

ACK A, ACK B, one commit
  -> replace the uncaptured owner with B
  -> capture B

capture A, send B/C
  -> retain exact capture A ownership
  -> allow B/C to consume the protocol window
  -> applying A cannot replace the active visual target
```

Older serials are retained in the retired-serial journal and later ACKs are stale. A matching final commit removes the active preview only after the final visual target has already been installed; preview retirement therefore does not pull geometry backward.

## 9. Pointer scene-hit architecture

Normal pointer routing now resolves one ordered scene authority:

```rust
enum PointerSceneHit {
    Client { target: PointerTarget },
    Decoration { window_id: WindowId, root_surface_id: u32, hit: DecorationHit },
    None,
}
```

The walk follows the existing renderable stacking order, checks relevant child surfaces before the owning root decoration, then checks the server-side frame/resize input region before the root client surface. CSD windows have no decoration hit. Layer and popup surfaces remain in the renderable order and are not given a separate decoration z-order.

Decoration hits focus their owning `WindowId` under the existing `PointerEnter` policy, clear Wayland client pointer focus while over compositor-owned decoration, and never route to a lower client. Existing locked-pointer, confined-pointer, implicit-grab, drag, and popup precedence is checked before ordinary decoration button handling.

## 10. Overlap regression results

Added deterministic scene coverage proves that a front SSD titlebar and invisible resize margin resolve as `PointerSceneHit::Decoration` for the front window rather than as the rear client.

Added real-client regression:

```text
overlapping_server_decoration_does_not_focus_window_underneath
```

The test asserts A remains focused, pointer focus becomes `None` over compositor-owned titlebar space, and B receives no pointer enter. It cannot execute in this environment because `OwnCompositorServer::bind()` fails with `EPERM` before the test body.

## 11. Popup, layer, CSD, and XWayland results

- CSD/fullscreen decoration construction remains covered and passes; no SSD occluder is created for CSD.
- Popup and layer-shell ordering paths remain in the existing integration suites and the scene walk preserves renderable ordering. Their real-client qualification is blocked by the same socket restriction.
- XWayland visual resize and ownership tests compile with the immediate preview path. Real XWayland compositor-server tests are likewise socket-blocked.
- Decoration handling does not create a fake `wl_surface`.

These are structural and test-harness results, not a claim of native end-to-end qualification.

## 12. Pointer-to-visual latency metrics

Resize debug events now include:

```text
monotonic timestamp_ns
input_hardware_timestamp_usec
interaction/configure serial and sequence
visual geometry
outstanding/ACKed/captured/queued/final ledger state
```

Events cover input update, configure queue/send/ACK, preview application, commit capture, and final commit. The native cycle performance record already correlates frame preparation, submission, and presentation/pageflip timing and now includes the configure-capacity metric.

No p50/p95 numeric latency distribution was captured because the native session stopped before a frame could pass KMS TEST_ONLY. The structural pointer-to-visual metric is proven by the same-dispatch regression: no `prepare_frame()` boundary remains.

## 13. Configure throughput metrics

The bounded flow tests cover 1,000 updates with a non-ACKing client. They prove protocol pressure remains at or below 3 and at most one latest unsent target is retained. The fast-path ledger tests prove three fresh targets can be in flight and that a captured old commit does not block newer sends.

Focused results include:

```text
ResizeConfigureFlow interaction tests: 23 passed
Task 05.8 configure ownership tests: 18 passed
```

## 14. 165 Hz native qualification

The requested launcher was attempted with the supplied Astrea shell command. The native bootstrap detected:

```text
connected output: card1-DP-1
preferred mode: 1920x1080
native scanout target: 1920x1080@165Hz
native frame scheduler: 6060 us absolute interval
```

It then stopped at the pre-render atomic TEST_ONLY commit with `Permission denied (os error 13)`. No resize gesture reached a presented frame, so no native p50/p95 or visual-follow qualification is claimed.

## 15. Before/after subjective resize result

The code path now applies the compositor visual target synchronously and independently of client commit. However, the native run could not present a frame, so this report does not claim the subjective result is “Hyprland-equivalent.” A real-session comparison remains required after KMS permissions are available.

## 16. Ghosting and render-ahead regression results

The resize path continues to use `ToplevelVisualGeometry`, render generation, `ResolvedNativeFrameScene`, `NativeFrameSceneSnapshot`, `NativeSceneHistory`, presentation-transition damage, and the existing buffer-age planner. It does not disable triple buffering, buffer age, explicit sync, or the KMS worker.

Passing focused coverage:

```text
frame ownership tests: 29 passed
fullscreen frame-scene tests: 3 passed
output retry tests: 3 passed
buffer-age-focused tests: 7 passed
window-decoration tests: 14 passed
window-interaction tests: 53 passed
```

Aggressive native resize ghosting qualification remains blocked by KMS permissions.

## 17. Full validation

```text
cargo fmt --check                         PASS
cargo check --locked --all-targets        PASS
cargo test --locked                       1153 passed, 540 failed, 2 ignored
cargo clippy --locked --all-targets ...   BLOCKED by pre-existing XwmEvent large-enum error
bash bin/check-source-layout              BLOCKED by existing oversized files
git diff --check                          PASS
```

The full-suite failures are dominated by the environment refusing compositor socket setup with `EPERM`; the native KMS qualification additionally fails at TEST_ONLY permission. Clippy reports the existing `src/xwayland/xwm/event_types.rs:21` `XwmEvent` large-enum diagnostic. Source layout reports the known oversized `windows.rs`, `server.rs`, and `mod.rs` files.

## 18. Commits

```text
0ef9f7b docs: design instant resize and pointer ownership closure
```

Implementation and tests are intentionally uncommitted because the working tree contains substantial prior closure work mixed through the same files. No prior work was reset, restored, stashed, cleaned, or rewritten.

## 19. Remaining blockers

1. Run the real-client overlap suite after Unix compositor socket creation is permitted.
2. Run the requested launcher after native KMS TEST_ONLY permission is available.
3. Capture pointer-to-pageflip p50/p95 and compare subjective resize behavior on Kitty, Firefox/Zen, GTK, Qt, and XWayland clients.
4. Run the popup/layer/CSD/XWayland native overlap qualification in that session.

## 20. Final `git status --short`

The final status remains intentionally dirty with the prior closure changes preserved. The task-owned implementation is present in the existing modified compositor/native files, the required design/plan commit is `0ef9f7b`, and the report itself is newly added.

```text
 M Cargo.lock
 M Cargo.toml
 M docs/superpowers/specs/2026-08-11-wayland-selection-idle-inhibit-design.md
 M src/compositor/desktop_window.rs
 M src/compositor/interaction.rs
 M src/compositor/mod.rs
 M src/compositor/protocols/core.rs
 M src/compositor/protocols/data_control.rs
 M src/compositor/protocols/primary_selection.rs
 M src/compositor/server.rs
 M src/compositor/state/desktop_window_tests.rs
 M src/compositor/state/frame_callbacks.rs
 M src/compositor/state/frames.rs
 M src/compositor/state/fullscreen.rs
 M src/compositor/state/hit_testing.rs
 M src/compositor/state/input_dispatch.rs
 M src/compositor/state/input_resources.rs
 M src/compositor/state/resize.rs
 M src/compositor/state/selection_runtime.rs
 M src/compositor/state/support_types.rs
 M src/compositor/state/surface_commits.rs
 M src/compositor/state/surface_focus.rs
 M src/compositor/state/surfaces.rs
 M src/compositor/state/task_05_8_tests.rs
 M src/compositor/state/window_decoration.rs
 M src/compositor/state/window_decoration_tests.rs
 M src/compositor/state/window_interaction.rs
 M src/compositor/state/window_interaction_tests.rs
 M src/compositor/state/window_resize.rs
 M src/compositor/state/windows.rs
 M src/compositor/state/xwayland_mode.rs
 M src/compositor/state/xwayland_windows.rs
 M src/compositor/tests/data_control.rs
 M src/compositor/tests/input_output/pointer_cursor.rs
 M src/compositor/tests/input_output/pointer_cursor_lifecycle.rs
 M src/compositor/tests/input_output/relative_and_constraints.rs
 M src/compositor/tests/input_output/window_interaction.rs
 M src/compositor/tests/primary_selection.rs
 M src/compositor/tests/protocol_buffers.rs
 M src/compositor/tests/protocol_error.rs
 M src/compositor/tests/support/clipboard_dmabuf.rs
 M src/compositor/tests/support/locked_relative.rs
 M src/compositor/tests/support/registry_state.rs
 M src/compositor/tests/support/server_runtime.rs
 M src/compositor/tests/support/subsurface_client.rs
 M src/compositor/tests/support/window_ops.rs
 M src/compositor/tests/windows.rs
 M src/compositor/tests/xdg.rs
 M src/compositor/tests/xwayland_resize_visual.rs
 M src/egl_renderer.rs
 M src/egl_renderer/damage.rs
 M src/egl_renderer/damage_tests.rs
 M src/egl_renderer/geometry.rs
 M src/launch_env.rs
 M src/native_output/launch.rs
 M src/native_output/mod.rs
 M src/native_output/output/damage.rs
 M src/native_output/presentation/trace.rs
 M src/native_output/runtime/bootstrap.rs
 M src/native_output/runtime/cycle.rs
 M src/native_output/runtime/cycle/pageflip.rs
 M src/native_output/runtime/cycle_direct.rs
 M src/native_output/runtime/cycle_dispatch.rs
 M src/native_output/runtime/frame.rs
 M src/native_output/runtime/kms_worker/rejection.rs
 M src/native_output/runtime/mod.rs
 M src/native_output/runtime/presentation.rs
 M src/native_output/runtime/presentation_ready.rs
 M src/native_output/runtime/presentation_worker.rs
 M src/native_output/runtime/session_io.rs
 M src/native_output/scanout/atomic_egl_gbm.rs
 M src/native_output/scanout/dumb.rs
 M src/native_output/scanout/egl_gbm.rs
 M src/native_output/scanout/gbm_cpu.rs
 M src/native_output/scanout/mod.rs
 M src/native_output/tests/fullscreen_frame_scene.rs
 M src/native_output/tests/mod.rs
 M src/native_output/tests/output.rs
?? .codex/
?? REPORT-2026-08-17-fullscreen-frame-scene-authority-closure.md
?? REPORT-2026-08-18-instant-resize-windowvisual-input-closure.md
?? REPORT-2026-08-18-render-ahead-buffer-age-ghosting-closure.md
?? docs/superpowers/plans/2026-08-14-keyboard-focus-selection.md
?? docs/superpowers/plans/2026-08-15-kms-worker-timing-throughput-closure.md
?? docs/superpowers/plans/2026-08-16-ssd-damage-mactahoe-closure.md
?? docs/superpowers/plans/2026-08-17-oversized-resize-presentation-ghosting.md
?? docs/superpowers/plans/2026-08-17-presented-scene-retry-buffer-age-closure.md
?? docs/superpowers/plans/2026-08-17-windowvisual-post-closure-qualification.md
?? docs/superpowers/plans/2026-08-18-render-ahead-presentation-damage-domain-closure.md
?? docs/superpowers/specs/2026-08-16-ssd-damage-mactahoe-closure-design.md
?? docs/superpowers/specs/2026-08-17-oversized-resize-presentation-ghosting-design.md
?? docs/superpowers/specs/2026-08-17-presented-scene-retry-buffer-age-closure.md
?? docs/superpowers/specs/REPORT-2026-08-17-oversized-resize-presentation-ghosting.md
?? docs/superpowers/specs/REPORT-2026-08-17-presented-scene-retry-buffer-age-closure.md
?? error.txt.save
?? src/native_output/runtime/scene_history.rs
?? src/native_output/tests/output_retry.rs
```
