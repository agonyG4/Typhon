# Astrea Lamp v1 Implementation Plan

## Goal

Implement the Astrea Lamp lifecycle animation for minimize and restore in Typhon, using retained live surfaces, a bounded static mesh, a dedicated vertex shader, and pageflip-confirmed lifecycle settlement. Keep logical window state, Dwindle layout, the Dock anchor protocol, and Animation Control Plane v1.1 authorities unchanged. Activate `minimize.lamp` only after the implementation and focused regressions pass.

## Constraints and invariants

- Work only in the Typhon source tree unless an Eclipse compatibility fixture genuinely requires an update.
- Preserve unrelated working-tree changes and stage only files belonging to this feature.
- Reuse the existing `target/` and native renderer build paths; never run `cargo clean` or create a new build directory.
- Keep geometry `TransitionId` and lifecycle `LifecycleTransitionId` distinct.
- Logical minimize and restore remain immediate. A minimized root never returns to canonical scene authority merely for animation.
- Lamp consumes `WindowState::minimized_surfaces`; restore suppresses the canonical root until an exact identity frame is physically acknowledged.
- Freeze the physically presented source basis and Dock anchor for each lifecycle transition.
- Use a 280 ms effective base duration at 1.0x, linear normalized progress, `K = 2.5`, and a late opacity fade.
- Do not add a `PresentationAnimationKind` variant, a generic animation framework, a timer or worker thread, a CPU pixel snapshot, Scale/Glide/Squash/open/close/workspace effects, or a Dock protocol change.

## 1. Build the lifecycle authority and pure reference model

Files: `src/window_lifecycle_animation.rs`, `src/lib.rs`.

Add `LifecycleTransitionId`, lifecycle direction/state, `WindowLifecycleAnimator`, and bounded frame-local sample types. Key transitions by exact `WindowId`, retain source/full-window/anchor rectangles, progress, duration, and the frozen geometry presentation basis. Support start, sample, reversal at the current progress, cancellation, endpoint snapping, and exact-ID acknowledgement. Keep the last pageflip-confirmed lifecycle ledger separate from logical active state.

Add pure finite CPU reference functions for the Astrea warp and opacity. Cover identity at `t=0`, anchor correspondence and zero opacity at `t=1`, direction-independent anchors, near-zero center distance, invalid dimensions, monotonic bounded pull, determinism, and the specified progress matrix. Add adaptive-grid topology calculation with hard row, column, and total-vertex budgets; topology is independent of ordinary progress updates.

## 2. Connect semantic minimize/restore operations

Files: `src/compositor/state/mod.rs`, a focused `src/compositor/state/lifecycle_animation.rs`, `src/compositor/state/windows.rs`, `src/compositor/state/active_scene.rs`, `src/compositor/server_control.rs`, `src/compositor/mod.rs`, `src/compositor/window_state.rs`, and relevant compositor tests.

Add compositor-owned lifecycle fields and helpers for:

- resolving the current minimize or restore slot independently;
- reading and validating the exact Dock anchor;
- selecting the last pageflip-confirmed geometry before current visual/canonical fallback;
- beginning, reversing, cancelling, and snapping lifecycle transitions;
- identifying render-suppressed roots and active lifecycle retained surfaces;
- preserving the physical lifecycle ledger during ordinary logical cancellation;
- destructively removing dead roots only during teardown.

Wrap the existing minimize and restore authorities without delaying their state mutations. Minimize captures source/anchor first, cancels only same-root geometry presentation as required for handoff, then performs the existing immediate logical minimize. Restore performs the existing immediate logical restore and suppresses the canonical root when the resolved restore slot is Lamp. If the effective slot is `None`, use immediate semantics and do not reverse a previous Lamp. Missing or invalid anchors and unavailable renderer support fall back cleanly with no stuck suppression.

Ensure tiled survivors reflow canonically underneath the overlay, maximized mode remains maximized, fullscreen ownership is re-established coherently on restore, and XDG and eligible managed XWayland roots share the same path. Add teardown cleanup and minimized-surface commit coverage.

## 3. Extend resolved native frame metadata and physical settlement

Files: `src/native_output/runtime/frame.rs`, `src/native_output/runtime/scene_history.rs`, `src/native_output/runtime/presentation_worker.rs`, `src/compositor/server.rs`, and native runtime tests.

Extend `ResolvedNativeFrameScene` with a lifecycle scene sample, lifecycle retained surfaces, lifecycle decoration instances, and a metadata-only `LifecycleFrameSnapshot`. Filter suppressed canonical roots from ordinary surfaces, decorations, and root-owned effects at frame resolution; keep lifecycle resources in a separate overlay representation consuming the same live resources. Preserve canonical snapshots and identities for unaffected windows.

Carry lifecycle metadata in `NativeFrameSceneSnapshot` and promote it through the existing ready/submitted/pageflip history. Publish the lifecycle ledger only from physical presentation publication. A final mathematical endpoint that was skipped cannot settle; a matching exact endpoint snapshot that pageflips can retire only its current `LifecycleTransitionId`. Stale frames from reversed or removed roots must be ignored safely.

## 4. Integrate scheduler, damage, and Direct Scanout authority

Files: `src/compositor/state/frames.rs`, `src/compositor/fullscreen.rs`, relevant server/runtime APIs, and focused tests.

Add lifecycle pending-visible work to `has_unowned_frame_work()` with visibility-aware behavior and final-endpoint settlement semantics. Add a stable lifecycle-specific Direct Scanout rejection reason. Reject scanout while an active or physically presented visible/non-identity Lamp can affect output, but allow the exact invisible minimize endpoint and exact canonical restore endpoint to settle without requiring an unrelated future frame or leaving scanout blocked.

Compute conservative lifecycle damage as the clipped output-space union of source and anchor rectangles, unioning concurrent windows through existing damage infrastructure. Damage on start, progress changes, and final endpoint while preserving ordinary scene cache reuse.

## 5. Add the dedicated static Lamp renderer path

Files: `src/egl_renderer.rs`, `src/egl_renderer/program.rs`, renderer geometry/effect executor modules, and renderer tests.

Compile a dedicated Lamp GL program through the normal renderer resource lifecycle. Use a bounded static subdivided mesh/VBO per active transition, explicit output/framebuffer/source/anchor/progress/opacity/pull uniforms, and a vertex shader implementing the same warp reference model. Rebuild/upload only when topology or source geometry materially changes; progress-only frames update uniforms. Count mesh builds/uploads in a narrow test seam and verify they remain far below frame count.

Render retained root/client/subsurface content and lifecycle decorations in one root visual-group coordinate system. Reuse existing texture/resource sampling and theme decoration resources. Lifecycle commands have empty opaque regions. Shader/program failure returns immediate semantic fallback and releases suppression/animator ownership.

Extend consumer reconciliation and realization to the union of canonical, lifecycle, and cursor consumers, while realizing only surfaces actually used by the lifecycle draw request. Preserve live minimized buffer generations. Release lifecycle mesh/resources after settlement or teardown.

## 6. Make pass ordering explicit

Files: `src/egl_renderer.rs`, effect executor code, and ordering tests.

Refactor the narrow native draw sequence so both LegacyScene and EffectGraph paths execute:

`base/effects -> Lamp -> external overlays/cursor`.

Keep Dock and cursor out of the Lamp mesh. Ensure canonical suppressed roots do not leave duplicate effects or decorations. Add command-order regressions for both renderer paths.

## 7. Add regression coverage before catalog activation

Files: focused lifecycle, native frame, renderer, scanout, and Animation Control Plane test modules.

Add state-level and integration-style tests for:

- reversal continuity and fresh lifecycle IDs;
- stale pageflip rejection;
- physical endpoint settlement only after publication;
- restore suppression and single representation;
- frozen anchors and missing-anchor fallback;
- physical-source selection during an unsettled maximize presentation;
- retained minimized surface generation updates;
- shared root/subsurface/SSD deformation metadata;
- concurrent windows and bounded meshes;
- floating, tiled, maximized, fullscreen, and managed XWayland lifecycle behavior;
- disable-mid-flight endpoint snapping and slot changes to `None`;
- damage bounds, scheduler ownership, scanout diagnostics, and renderer order.

Keep pre-existing unrelated failures visible and report them separately.

## 8. Activate the catalog last and qualify the result

Files: `src/animation_control/catalog.rs` and its tests; Eclipse only if a stale fixture explicitly encodes the old catalog state.

After the focused lifecycle and renderer regressions pass, change `MinimizeLamp::is_available()` to true and update catalog expectations. Verify Astrea resolves Lamp for minimize and restore while KDE and macOS remain `none`; no configuration migration or new Settings model is needed. If Eclipse source remains unchanged, do not rebuild it and state that the existing capability projection is sufficient. If a fixture requires an update, make only the compatibility expectation change and reuse Eclipse’s existing build directory.

## 9. Verification and handoff

Run the focused Lamp/lifecycle tests repeatedly during implementation, then run from the Typhon directory:

```text
rtk run -- cargo fmt --check
rtk run -- cargo check --locked --all-targets
rtk run -- cargo clippy --locked --all-targets -- -D warnings
rtk run -- cargo test --locked
rtk git diff --check
rtk run -- bash bin/check-source-layout
```

Do not run `cargo clean`. Inspect the existing 1920x1080/165 Hz environment if available for floating, reversal, tiled, maximized, fullscreen, XWayland, and effects behavior; report hardware qualification only when actually observed. Re-run status/diff checks, stage only Lamp files, commit the completed Typhon implementation, and explicitly report any unrelated failures or the fact that Eclipse was unchanged.
