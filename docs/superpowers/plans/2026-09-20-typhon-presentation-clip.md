# Typhon Presentation Engine v2 — Clip Implementation Plan

> **For agentic workers:** Use the inline, task-by-task execution flow with review checkpoints. Subagent execution is excluded by the repository instructions. Steps use checkbox syntax for tracking.

**Goal:** Implement Clip as a third transactional WindowGroup Presentation Engine property with canonical state, immutable physical evidence, rendering/effects masks, physical damage, and Direct Scanout qualification.

**Architecture:** Keep the property explicitly typed and parallel to Geometry and Opacity: `PresentationClipRect`, semantic `Unbounded`, `clip_tracks`, and exact frame/ACK evidence. Resolve and freeze output-space Clip from the same timestamp's Geometry sample; apply it at the WindowGroup rendering boundary while preserving surface aperture state.

**Tech Stack:** Rust, Cargo, Wayland compositor state, CPU renderer, GLES/EGL renderer, native output frame history.

## Global Constraints

- Canonical Clip belongs to `DesktopWindow`; interpolation belongs to `PresentationEngine`; identity/topology stays in `CanonicalSceneRegistry`.
- `PresentationClip::Unbounded` is explicit identity; zero-area Rect is valid.
- `PresentationClipRect` is WindowGroup-local, finite, and may extend outside the canonical client.
- An identity endpoint uses only a finite transition-local visual envelope and samples back to semantic `Unbounded` exactly.
- `RenderableSurface.visual_clip` and `SurfaceVisualAperture` remain independent technical aperture state.
- Geometry, Opacity, and Clip validate and reserve IDs atomically before track mutation; exact property revisions settle independently on physical ACK.
- Popup VisualGroups and SSD inherit Clip through the stable WindowGroup presentation owner exactly once.
- Effects use Clip as an outer final contribution mask; same-owner source capture bypasses only that owner's mask.
- Physical damage compares immutable previous/current frames; `NativeSceneHistory` remains sole physical authority.
- Direct Scanout distinguishes `presentation_clip` from `visual_clip_present` and qualifies only the covering candidate's WindowGroup.
- Clip does not change input, layout, canonical Geometry, protocol state, or configure events; do not activate visible animations or migrate Lamp.
- Add zero mandatory framebuffer passes, per-window textures, animation threads, scheduler timers, or per-frame layout solves.
- Do not raise source-layout limits or grow `src/compositor/effects.rs` significantly.

---

### Task 1: Typed Clip and canonical window state

**Files:**
- Create: `src/presentation_animation/clip.rs`
- Modify: `src/presentation_animation/mod.rs`
- Modify: `src/compositor/desktop_window.rs`
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/state/mod.rs` to register the focused state tests
- Test: `src/presentation_animation/clip.rs`
- Test: `src/compositor/state/presentation_clip_tests.rs`

- [ ] Test finite coordinates, nonnegative zero-permitting dimensions, explicit `Unbounded`, and XDG/X11 identity defaults.
- [ ] Run `rtk cargo test --manifest-path /mnt/Aether/Desktop/GitHub/Typhon/Cargo.toml --target-dir /mnt/Aether/Desktop/GitHub/debug --locked presentation_animation::clip` from `/mnt/Aether/Desktop/GitHub` and confirm the new tests fail before implementation.
- [ ] Add the semantic Clip types and canonical `DesktopWindow` field; keep Clip out of `CanonicalSceneRegistry`.
- [ ] Add a compositor-internal immediate/animated mutation seam that freezes a bounded visual envelope only when an effective transition has an `Unbounded` endpoint.
- [ ] Re-run the focused Clip model and state tests.

### Task 2: Transactional Clip tracks

**Files:**
- Modify: `src/presentation_animation/ids.rs`
- Modify: `src/presentation_animation/transaction.rs`
- Modify: `src/presentation_animation/engine.rs`
- Test: `src/presentation_animation/transactions_tests.rs`

- [ ] Add Clip mutations, transition-local envelope data, prepared Clip mutations, and `PresentationPropertyKind::Clip`.
- [ ] Test Clip-only, Geometry+Clip, Opacity+Clip, and Geometry+Opacity+Clip transactions for one transaction ID and distinct per-member revisions.
- [ ] Test validation atomicity, no-op elimination, moving retarget velocity, settled-unACKed same-target preservation, exact stale/wrong-output ACK rejection, and independent three-property settlement.
- [ ] Add spring tests proving visible width/height never go negative and clamped dimension velocity becomes zero.
- [ ] Run `rtk cargo test --manifest-path /mnt/Aether/Desktop/GitHub/Typhon/Cargo.toml --target-dir /mnt/Aether/Desktop/GitHub/debug --locked presentation_animation::transactions_tests` from `/mnt/Aether/Desktop/GitHub` after the tests fail against the unmodified engine.
- [ ] Add sparse `clip_tracks`, clip cancellation/query/accounting, and exact Clip ACK retirement without changing Geometry/Opacity semantics.
- [ ] Re-run both `presentation_animation::clip` and `presentation_animation::transactions_tests`.

### Task 3: Immutable frame evidence and Geometry projection

**Files:**
- Modify: `src/presentation_animation/frame.rs`
- Modify: `src/presentation_animation/engine.rs`
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/server.rs`
- Test: `src/presentation_animation/transactions_tests.rs`
- Test: `src/compositor/state/presentation_clip_tests.rs`

- [ ] Add canonical Clip to `PresentationWindowTarget`; sample Geometry first, then semantic Clip at the same timestamp.
- [ ] Map local Clip through the sampled Geometry transform and freeze both local semantic and output-space values in scene/frame snapshots.
- [ ] Keep static canonical Rect evidence sparse; emit active settled `Unbounded` evidence with transition/revision for physical ACK.
- [ ] Include only actual nonidentity rendered Clip values in the visual signature; keep transaction/revision evidence separate.
- [ ] Test Geometry+Clip coherence, zero-area projection, identity-signature stability, and old frame evidence after canonical state changes.
- [ ] Run `rtk cargo test --manifest-path /mnt/Aether/Desktop/GitHub/Typhon/Cargo.toml --target-dir /mnt/Aether/Desktop/GitHub/debug --locked presentation_animation::transactions_tests` from `/mnt/Aether/Desktop/GitHub`.

### Task 4: CPU rendering and surface aperture intersection

**Files:**
- Modify: `src/compositor/render.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Create: `src/compositor/render/presentation_clip_tests.rs`

- [ ] Thread owner Clips through the CPU renderer's WindowVisualGroup projection.
- [ ] Intersect output damage/rebuild clipping with owner PresentationClip for client surfaces, subsurfaces, popups, and decorations; leave `SurfaceRenderPlan.clip` intact and independent.
- [ ] Test pixel output for client/aperture intersection, popup owner inheritance, SSD clipping, and zero-area Clip.
- [ ] Run `rtk cargo test --manifest-path /mnt/Aether/Desktop/GitHub/Typhon/Cargo.toml --target-dir /mnt/Aether/Desktop/GitHub/debug --locked compositor::render::presentation_clip` from `/mnt/Aether/Desktop/GitHub`.

### Task 5: EGL visibility, scissor, and effects ordering

**Files:**
- Modify: `src/egl_renderer/geometry.rs`
- Modify: `src/egl_renderer.rs`
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `src/egl_renderer/effects/capture.rs`
- Create: `src/egl_renderer/presentation_clip_tests.rs`

- [ ] Associate commands with their presentation owner without replacing `VisualGroupId` or editing technical aperture data.
- [ ] Plan visibility from `command bounds ∩ owner Clip`, intersect opaque regions, and skip zero-area commands so clipped-away pixels do not occlude.
- [ ] Track the effective GL scissor; intersect frame damage and owner Clip and update GL state only when the effective scissor changes.
- [ ] Add a typed same-owner capture bypass. Other owners retain their actual Clip during background capture; internal effect passes stay unclipped; final owned effect composites use Clip and `OutputPostProcess` does not.
- [ ] Test source capture beyond the final Clip, final effect masking, and real GLES/pbuffer pixels where the current harness supports them.
- [ ] Run `rtk cargo test --manifest-path /mnt/Aether/Desktop/GitHub/Typhon/Cargo.toml --target-dir /mnt/Aether/Desktop/GitHub/debug --locked egl_renderer::presentation_clip` from `/mnt/Aether/Desktop/GitHub`.

### Task 6: Physical Clip damage and history

**Files:**
- Modify: `src/native_output/output/presentation_damage.rs`
- Modify: `src/native_output/output/damage.rs`
- Modify: `src/native_output/runtime/scene_history.rs`
- Test: `src/native_output/output/presentation_damage.rs`
- Test: `src/native_output/runtime/scene_history.rs`

- [ ] Compare Clip values by stable WindowGroup SceneNode across immutable previous/current presentation snapshots.
- [ ] Damage previous and current clipped owner surface bounds, popup surfaces, SSD, and owner-scoped final effect influence; do not damage unrelated owners.
- [ ] Test Unbounded→Rect hide, Rect→Unbounded reveal, Rect A→Rect B union, zero-area, unrelated owner, and submitted old-frame evidence after backing replacement.
- [ ] Run `rtk cargo test --manifest-path /mnt/Aether/Desktop/GitHub/Typhon/Cargo.toml --target-dir /mnt/Aether/Desktop/GitHub/debug --locked presentation_damage` from `/mnt/Aether/Desktop/GitHub`.

### Task 7: Direct Scanout and XWayland continuity

**Files:**
- Modify: `src/compositor/direct_scanout.rs`
- Modify: `src/compositor/state/direct_scanout.rs`
- Modify: `src/compositor/state/xwayland_windows.rs` only if current code fails the stable-DesktopWindow invariant
- Test: `src/compositor/state/presentation_clip_tests.rs`

- [ ] Add distinct `PresentationClip` rejection with diagnostic `presentation_clip`.
- [ ] Block the covering candidate for its active Clip track, canonical nonidentity Clip, or physically presented nonidentity Clip; ignore unrelated owners.
- [ ] Prove a physical Unbounded frame clears the blocker and a stale pre-replacement frame does not change or ACK a newer revision.
- [ ] Prove XWayland root A→B preserves WindowId, SceneNode, canonical Clip, and active Clip revision without touching unrelated XWM transfer work.
- [ ] Run `rtk cargo test --manifest-path /mnt/Aether/Desktop/GitHub/Typhon/Cargo.toml --target-dir /mnt/Aether/Desktop/GitHub/debug --locked direct_scanout` from `/mnt/Aether/Desktop/GitHub`.

### Task 8: Fresh verification and completion record

**Files:**
- Modify: `docs/presentation-qualification.md` or the current Presentation Engine documentation after verification

- [ ] Run from `/mnt/Aether/Desktop/GitHub/Typhon` with the worktree manifest: `rtk cargo fmt --manifest-path /mnt/Aether/Desktop/GitHub/Typhon/Cargo.toml --check`, `rtk cargo check --manifest-path /mnt/Aether/Desktop/GitHub/Typhon/Cargo.toml --target-dir /mnt/Aether/Desktop/GitHub/debug --locked --all-targets`, `rtk cargo clippy --manifest-path /mnt/Aether/Desktop/GitHub/Typhon/Cargo.toml --target-dir /mnt/Aether/Desktop/GitHub/debug --locked --all-targets -- -D warnings`, and `rtk cargo test --manifest-path /mnt/Aether/Desktop/GitHub/Typhon/Cargo.toml --target-dir /mnt/Aether/Desktop/GitHub/debug --locked`.
- [ ] Run the existing source-layout checker in the checkout and compare the result with the starting baseline; do not raise limits.
- [ ] Update documentation to mark Clip implemented only after every architectural invariant and fresh verification result is checked.
- [ ] Commit only Clip-owned files, preserving all unrelated dirty changes.
