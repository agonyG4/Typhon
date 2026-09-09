# Typhon Presentation-Animation Closure Report

Date: 2026-09-08

Repository: `/home/agony/GitHub/Typhon`

## Status

The presentation-animation closure is implemented and passes the locked Rust
build, lint, test, formatting, and whitespace gates. The source-layout checker
still reports the repository's existing oversized-module debt; no source-layout
refactor was included in this closure. Native DRM/165 Hz hardware qualification
was not run in this environment.

## Implemented architecture

- Transitions now carry an overflow-safe `TransitionId`. Mathematical
  settlement is separate from physical settlement: a settled transition keeps
  emitting its exact target until the frame containing that exact ID and target
  is physically presented. A stale acknowledgement cannot retire a retargeted
  transition.
- `PresentationGroupTransform` is the shared frame-local transform for a root
  and its complete visual group: root surface, subsurfaces, SSD, policy-owned
  popups, effects, damage mapping, and input inverse mapping. Canonical layout
  and `RenderableSurface` state remain unchanged.
- The physically presented projection is stored in
  `NativeFrameSceneSnapshot.presentation` and carried through ready, submitted,
  and presented ownership. Only successful immediate/pageflip promotion updates
  compositor input authority and performs exact transition acknowledgement.
- Scheduler demand is based on visible stored transitions pending physical
  acknowledgement. Hidden roots do not keep the frame loop alive. Teardown
  cancels root transitions.
- Direct Scanout remains blocked by visible pending transitions or a nonidentity
  physically presented transform, and only physical identity permits scanout.
- Wayland and XWayland visual mutations share one layout-batch animation epoch.
- Presentation geometry has a bounded deterministic signature in EGL scene-cache
  keys, independent of content generations. Transition counters are exposed by
  `OwnCompositorServer::presentation_animation_metrics()`.

## Inherited foundation work

The current working tree also contains the previously approved resource and
effects work: copy-on-write SHM snapshots, consumable effect-resource eviction,
incremental presentation trace export, cached shader/effect resources, and the
associated bounded metrics and tests. Those changes were preserved while this
closure was integrated.

## Fractional geometry boundary

The presentation model and group-transform math retain finite `f64` geometry,
including fractional translation and scale. The existing integer-compatible
`RenderableSurface`, SSD, decoration, and effect rectangle consumers quantize
when the frame-local projection is materialized for those consumers. A future
end-to-end fractional GPU primitive path remains deferred; this report does not
claim fractional values survive those legacy integer structures.

## Verification

All commands were run in the existing checkout and reused `target/`:

- `rtk run -- cargo fmt --check` — passed.
- `rtk run -- cargo check --locked --all-targets` — passed.
- `rtk run -- cargo clippy --locked --all-targets -- -D warnings` — passed.
- `rtk run -- cargo test --locked` — passed: 2,216 library tests, 1,288 main
  binary tests, and all integration suites completed with zero failures. The
  only reported skips are tests explicitly requiring a live native session or
  installed Xwayland.
- `rtk git diff --check` — passed.

Focused presentation, effect-transform, decoration-transform, cache-signature,
and scene-history coverage also passed during implementation. The relevant
presentation-animation unit group reports 12 passing tests.

`rtk run -- bash bin/check-source-layout` remains non-zero because 42 existing
files exceed repository limits, including pre-existing large compositor and
native-runtime modules. The checker also reports touched modules such as
`src/compositor/mod.rs`, `src/compositor/server.rs`, `src/compositor/state/frames.rs`,
`src/compositor/state/surfaces.rs`, and native presentation-runtime files; this
closure did not attempt unrelated module extraction.

## Qualification limits and deferred scope

- No live DRM/KMS, 165 Hz, GPU/CPU parity, pageflip-latency, or hardware
  present-distribution qualification was available; automated model and unit
  coverage must not be read as hardware qualification.
- Open/close retention, workspace-retention animation, scrolling, Infinite
  Canvas, camera transforms, and a public animation DSL remain out of scope.
- The integer-consumer fractional boundary described above remains the next
  geometry-quality follow-up.
- The source-layout debt remains separately tracked from presentation authority
  correctness.

## Working-tree note

No commit, reset, checkout, or destructive cleanup was performed. Existing
working-tree changes were preserved.

## Post-review corrective closure (2026-09-08)

The post-review corrective pass closed the remaining source-level authority
gaps while preserving the earlier deterministic implementation and its test
history:

- Native output now resolves at the scheduled `CLOCK_MONOTONIC`
  presentation timestamp, computes damage from that resolved scene, freezes
  one owned `ResolvedNativeFrameScene`, and carries that same scene through
  the atomic signature, presentation snapshot, compatibility paint, or
  atomic EGL/GLES draw. Freezing clones only render metadata; SHM pixel
  storage remains shared through its existing `Arc` backing.
- Popup input keeps the visual stack root for ordering, but resolves the
  physical presentation transform through the popup ancestry's explicit
  `owner_root_id`. The inverse affine is intentionally unbounded; existing
  surface, decoration, and input-region testers retain hit rejection.
- Interaction takeover derives its initial move/resize geometry from the
  physically presented snapshot, cancels the pending transition at begin,
  keeps the old nonidentity projection as a Direct Scanout blocker until the
  replacement frame is physically published, and bypasses that old projection
  for active interaction input. This preserves the first-motion baseline and
  avoids a terminal catch-up animation.
- `sampled_windows` is now cumulative truthful sampling telemetry. The
  `OBLIVION_ONE_ANIMATIONS=on|off` policy defaults to normal/on and falls back
  to normal policy for invalid values; disabled policy has no visible pending
  demand and no transition state.

Corrective verification, all run in the existing checkout and reusing
`target/`:

- `rtk run -- cargo fmt --check` — passed.
- `rtk run -- cargo check --locked --all-targets` — passed.
- `rtk run -- cargo clippy --locked --all-targets -- -D warnings` — passed.
- `rtk run -- cargo test --locked` — passed: 2,231 library tests, 1,291 main
  binary tests, and all integration suites completed with zero failures.
- Focused presentation-animation tests — 15 passed.
- Frozen-scene SHM backing test — passed.
- `rtk git diff --check` — passed.
- `rtk run -- bash bin/check-source-layout` — remains non-zero for 42
  oversized files. This includes touched modules such as
  `src/compositor/state/window_interaction.rs`,
  `src/compositor/state/pointer_constraints.rs`,
  `src/egl_renderer/effects/executor.rs`,
  `src/native_output/runtime/presentation_cycle.rs`, and
  `src/native_output/scanout/atomic_egl_gbm.rs`; no unrelated extraction was
  attempted.

The corrective pass still does not claim end-to-end fractional GPU primitive
preservation or live DRM/165 Hz hardware qualification. The model retains
finite `f64` presentation geometry and quantizes only at legacy integer
consumer boundaries, as stated above.

## Final physical-root and tiled-resize authority closure (2026-09-08)

This section records the preceding closure state; the 2026-09-09 follow-up
below supersedes its root-surface terminology and physical projection details.

The final review found one remaining ambiguity: a pageflip-confirmed identity
frame carried no transition transform, so transition-only metadata could not
prove where an unchanged root was physically displayed. The resolved native
frame now records metadata-only `PresentedRootGeometry` for every visible
toplevel root, including identity frames. Each record is derived from the
materialized `RenderableSurface` origin and size used by the renderer, carries
only root identity and rectangle metadata, and is included in the existing
ready/submitted/presented `NativeSceneHistory` lifecycle. Only immediate or
pageflip physical promotion advances the presented projection.

Input now maps the current canonical root rectangle into that last physically
promoted root rectangle before running the existing surface, decoration, and
input-region hit tests. This covers pre-first-animation-frame input,
retarget-before-pageflip input, render-ahead/O1 frames, and identity frames
without sampling a future transition. Popup stacking remains rooted in the
visual stack while its physical projection follows the explicit presentation
owner. SSD/titlebar geometry therefore follows the same promoted root
projection. Interaction takeover still captures the physical baseline before
cancelling the transition, preserving first-motion continuity.

Tiled resize preparation now retains its canonical layout solution and rebases
only the interaction start boundaries by the presented-client-edge minus
canonical-client-edge delta. Parent/split/topology and the existing client
offset remain unchanged, including constrained tiled windows. The pure resize
API and constrained-edge tests prove that a zero first displacement cannot
jump to the future canonical divider.

The final self-review answers are therefore: identity roots remain known after
pageflip; canonical mutation and future render-ahead cannot advance input;
floating and tiled takeover use the visible baseline; popup stack and
presentation ownership remain separate; SSD input follows the root projection;
history promotes only physical frames; and no client image payloads are copied.

Final verification, all run in the existing checkout while reusing `target/`:

- `rtk run -- cargo fmt --check` — passed.
- `rtk run -- cargo check --locked --all-targets` — passed.
- `rtk run -- cargo clippy --locked --all-targets -- -D warnings` — passed.
- `rtk run -- cargo test --locked` — passed: 2,250 library tests, 1,297 main
  binary tests, and all auxiliary integration targets completed with zero
  failures; 2 library tests were ignored as declared by the suite.
- `rtk git diff --check` — passed.
- `rtk run -- bash bin/check-source-layout` — remains non-zero for 42 existing
  oversized files; no unrelated extraction or limit change was made.

The real GLES shader-contract and trusted-wrapper tests also pass after the
final shader declaration-order correction. No live DRM/KMS or 1920x1080@165 Hz
hardware qualification was available, and the previously documented
fractional-rendering limitation remains unchanged. No additional presentation
features were added; the next compositor task remains Dwindle v1.2.

## Final window-space authority follow-up (2026-09-09)

The remaining presentation defect was a root/window-space hybrid in the
animation target path. A CSD toplevel with canonical window geometry
`(100,100,944,526)` and a client root at `(100,124,944,502)` could derive its
active target from the raw root surface, shifting the physical target by the
client frame margin. The target path now uses the canonical window geometry and
the shared window-space `PresentationRect` helper. The regression test proves
that the canonical rect differs from the raw root rect, identity remains an
identity transform, and an active transition stores the window-space rect.

`PresentedRootGeometry` is now `PresentedWindowGeometry`. It is explicitly
metadata for the physically presented toplevel/window rect, never raw
`wl_surface` geometry. Composed frames derive this metadata from canonical
window geometry plus the sampled presentation transform; direct-scanout
candidates capture the validated window-space rect and carry it through the
lease, submitted, presented, and completion records. Direct presentation
publishes that rect only after a successful pageflip, so an accepted A frame
cannot be replaced by canonical B before A physically presents. Direct-to-
composited transitions continue to publish through the composed pageflip path;
no synthetic composed scene is created.

The physical projection is centralized in one saturating materialization helper:
rounded physical-minus-canonical deltas adjust local placement, physical
width/height determine the presented window size, and the canonical root mode
is retained. Input maps canonical window coordinates into the last physically
presented window, popup visual-stack roots remain distinct from presentation
owners, and tiled resize rebasing remains presented client edge minus canonical
client edge. The history/lifecycle model is unchanged apart from this window
space meaning.

Final verification, all run in the existing checkout while reusing `target/`:

- `rtk run -- cargo fmt --check` — passed.
- `rtk run -- cargo check --locked --all-targets` — passed.
- `rtk run -- cargo clippy --locked --all-targets -- -D warnings` — passed.
- `rtk run -- cargo test --locked` — passed on the final run: 2,255 library
  tests, 1,305 main binary tests, and all auxiliary integration targets
  completed with zero failures; 2 library tests and the declared optional
  environment-dependent tests were ignored.
- Focused CSD, presentation-race, identity, tiled-rebase, and direct-lease
  ownership tests — passed.
- `rtk git diff --check` — passed.
- `rtk run -- bash bin/check-source-layout` — reports the existing 42-file
  oversized-module debt; no unrelated extraction or source-layout limit change
  was made.

The first combined test invocation exposed a timing-sensitive empty helper-PID
file in an existing process cleanup test; the exact test passed in isolation,
and the final complete locked run passed. No live DRM/KMS or 1920x1080@165 Hz
hardware qualification is claimed. The next compositor task remains Dwindle
v1.2.

## Final native-frame membership authority closure (2026-09-09)

The final review found that the native frame builder sampled presentation
transitions from the active scene before fullscreen composition had selected the
surfaces that belonged to the frame. A rear window hidden by a solitary
fullscreen owner could therefore be sampled, included in physical presentation
metadata, acknowledged by the owner's pageflip, counted as pending visible work,
or block Direct Scanout even though it was absent from the rendered frame.

Native frame resolution now computes the fullscreen/composition membership once,
derives one immutable frame-local target set from those canonical surfaces, and
passes that same set through animation sampling, surface transform application,
and `PresentedWindowGeometry` creation. Presentation-owner roots are deduplicated
in deterministic frame-surface order, so subsurfaces and popup trees retain their
existing owner semantics without re-scanning the active scene. Hidden transitions
remain stored and continue to use the existing absolute-time analytical clock, but
they are unsampled, absent from the physical snapshot, not acknowledged, and do
not create visible animation demand until their owner is frame-visible again.

Physical input now treats an absent tracked toplevel in the latest promoted
window projection as not hittable. This preserves popup/SSD ownership and
non-window fallback behavior while closing the fullscreen-exit gap where
canonical state could become interactive before its restore frame pageflipped.
The existing replacement semantics of the physical projection and render-ahead
scene history remain unchanged; a future frame cannot change input membership or
acknowledge a transition before physical promotion. Direct Scanout behavior is
unchanged for the fullscreen owner's own transitions, while a transition owned
only by a culled rear window no longer produces the animation blocker.

Added regressions cover the real native `ResolvedNativeFrameScene` path, hidden
transition retention and final-frame acknowledgement after fullscreen reveal,
physical input membership, and the Direct Scanout hidden-versus-owner animation
blocker distinction. The native frame test also asserts that rendered surface
membership, presentation-owner target membership, and physical window metadata
are identical for the fullscreen frame.

Verification for this follow-up reuses the existing `target/` directory. No live
DRM/KMS or 1920x1080@165 Hz hardware qualification was performed, so no hardware
qualification claim is made. Fractional/subpixel presentation remains deferred
at the existing integer-consumer boundaries. The known source-layout check still
reports the repository's pre-existing 42 oversized files; this closure does not
perform unrelated module extraction. The next compositor task remains Dwindle
v1.2.

## Final animated-fullscreen coverage closure (2026-09-09)

The native-frame membership closure exposed one remaining coverage edge during
fullscreen entry. Canonical fullscreen geometry can cover the output before the
fullscreen owner's floating-to-fullscreen presentation transition has physically
settled. Treating canonical coverage as proof of current coverage would cull the
rear desktop while the owner still occupied only part of the physical frame.

Solitary fullscreen composition now additionally requires that the fullscreen
owner have no pending presentation transition. This uses the existing
owner-specific animator query, not global frame-visible pending state, so it does
not create recursive membership resolution and a hidden rear transition cannot
disable steady-state fullscreen culling. Mathematical settlement is not enough:
the transition remains pending until its final frame is physically promoted and
the exact transition ID is acknowledged. The next naturally resolved frame may
then cull the rear scene without a synthetic cleanup frame, timer, or render
generation bump.

During entry, normal active-scene membership remains underneath the partially
presented owner, while the existing window-space presentation transform remains
authoritative for the owner, subsurfaces, SSD, popups, and effects. XDG and
XWayland use the same `FullscreenPresentationState` and render-plan policy; no
backend-specific culling rule was added. Direct Scanout keeps its existing global
visible-animation blocker: the owner's transition blocks it during entry, while
hidden rear transitions remain irrelevant after owner settlement.

The regression covers canonical output-sized ownership with a pending owner
transition, mathematically settled-but-unacknowledged state, exact physical ACK,
post-ACK solitary culling, and a hidden rear transition that does not disable
solitude or create visible animation demand. No live DRM/KMS or 1920x1080@165 Hz
hardware qualification was performed. Existing fractional/subpixel deferral and
the known 42-file source-layout debt remain unchanged; the next compositor task
remains Dwindle v1.2.
