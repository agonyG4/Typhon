# Cursor responsiveness: architecture findings and implementation

## Status and evidence

The current live checkout is authoritative. CodebaseMCP was refreshed as `home-agony-GitHub-Typhon`, generation `2026-10-09T14:39:31Z`. Its coverage report marked `cycle_dispatch.rs:2019` and `presentation_cycle.rs:301` as parse-partial; those ranges and the required runtime source were read directly. The supplied task's source findings were confirmed, including primary-first disposition, policy modes not being distinguished, refresh-boundary deadlines, EBUSY deadline sliding, scene prediction leaking into PlaneDelta target planning, worker wake pacing, and acquire/prepare preceding cursor presentation.

| Evidence class | Finding |
|---|---|
| User observation | Hardware cursor motion feels less fluid in populated workspaces; NVIDIA, 165 Hz, and retained hardware-cursor use are reported. This is not an optical timing measurement. |
| User diagnostic record | A libinput late-dispatch warning of about 26 ms, roughly 26,850 repeated effect-anchor warnings, cursor-primary piggyback activity, slow cycles, and a capped verbose trace were observed. Log volume may have biased the measurements. |
| Source-confirmed cost | A presentation cycle asks for effect frame demand before cursor arbitration. The old accessor resolved the complete effect scene on every call. Resolution clones effect instances, may enumerate active scene surfaces for background blur, resolves assignments, and orders instances. A successful hardware cursor plane-delta submission returns `WaitForPageFlip` and bypasses the later primary render/scene-resolution branch, so this early demand query was unnecessary scene-dependent CPU work on that cursor-only path. |
| Source-confirmed change | Static effect demand now reads the existing published `EffectSceneSummary`; continuous effects still resolve the live scene to retain the current dirty region. Tests assert that the static path does not call `resolved_effect_scene` and that the continuous path still does. |
| Not established | There is no NVIDIA hardware A/B result in this change. The scene-resolution cost is a proven avoidable workload; the available evidence does not prove it was the only cause of the reported pointer sensation or the late libinput dispatch. |

## Production path and state ownership

```text
libinput fd readiness
  -> NativeEventLoop / wait_for_events_and_pageflips
  -> run_native_cycle / run_cycle
  -> dispatch_wayland_and_input
       -> bounded NativeInputEpoch chunk (budget 256)
       -> input event decode and semantic routing
       -> pointer constraints, focus/hit testing, seat events
       -> canonical pointer and cursor desired state
  -> input-routing checkpoints before later cycle tail work
  -> matured Auto cursor-service checkpoint before acquire/prepare
  -> render_present_and_update_metrics
       -> effect frame-demand query
       -> cursor image/visibility and plane eligibility
       -> NativeCursorOutputArbitration
       -> cursor plane-delta submit -> WaitForPageFlip (no primary scene resolve)
       -> or primary/software/fallback path -> scene resolve and damage
  -> AtomicCommitArbiter / KMS worker admission and submission
  -> exact output transaction/pageflip feedback
```

The actual code dispatches bounded input work; it does not equate each physical motion report with one full scene render. Consecutive pointer-motion samples may be coalesced within the input batch, while ordered key, button, axis, and other semantic events retain their event ordering. `NativeInputTransitionLatencyGuard` adds input-service checkpoints after a routing transition and before later pacing, cursor/control, XWayland, buffer-acquire, and presentation work. Those checkpoints service input microturns; they do not make KMS submission concurrent with input dispatch. A successful cursor plane-delta path returns `WaitForPageFlip` before the `Render` / `RenderAhead` branch containing `resolve_scene_and_damage`; cursor-plane rejection or software fallback may enter that conservative branch because it can require primary composition.

| Responsibility | Authoritative state / code |
|---|---|
| Reactor readiness and event ordering | `NativeEventLoop`, `NativeCycleState`, and `NativeInputEpoch` in `src/native_output/runtime/` and `src/native_output/input/`. Input fd readiness is registered at runtime bootstrap. |
| Semantic input, seat, focus, constraints | `NativeInputState` and `CompositorState`; pointer scene hit testing is in `src/compositor/state/hit_testing.rs`. It already has generation-qualified cache and owner-locality fast paths, with a front-to-back scan fallback. |
| Cursor image and desired position | Existing cursor manager / atomic cursor state, including cursor image revision, hotspot, visibility, and desired epoch. The output cursor arbitration does not own a second pointer-position truth. |
| Scene and effects | `CompositorState` owns effect bindings and `EffectSceneSummary`; `OwnCompositorServer` exposes the frame-demand query. `ResolvedNativeFrameScene` captures presentation-specific effect damage and animation-owner influences. |
| Presentation decision | `NativeCursorOutputArbitration` tracks cursor age, a KMS service opportunity, retry timing, and primary/KMS ownership. Auto distinguishes an attachable ready carrier from dirty scene work and an owned KMS lane. |
| KMS submission and completion | One `AtomicCommitArbiter` and the existing worker/output transaction path own Atomic admission, submission, and completion identity. Cursor scheduling is a class within that authority, not a second submitter. |

## Pointer hit validity boundary

Render, pixel, and presentation activity does not invalidate pointer hit state by default. Only changes that can alter the hit target, input acceptance, or local-coordinate mapping advance `pointer_hit_generation`. Buffer contents, damage, opacity, effects, frame IDs, and repeated presentation samples at the same physical geometry do not serve as input-validity signals.

Visual-stack grouping has a separate `interaction_topology_generation`. It tracks the active surface order and IDs, canonical and effective render parent relationships, and popup membership used to build `visual_stack_groups`. A topology change advances both topology and pointer-hit generations. Geometry or input-region changes can advance pointer-hit generation without rebuilding visual-stack topology. The cache is keyed only by topology generation.

Physical presentation advances pointer-hit validity only when its input projection changes: authority becomes active/inactive, a root enters/leaves the presented-window set, or a root's physical rectangle changes. Pointer mapping uses those rectangles to reject roots without presented geometry and to inverse-map coordinates. Presentation node IDs, opacity, clip, sample timestamps, frame IDs, and pixel contents are not part of that projection. Lifecycle physical publication advances pointer-hit validity only when a successful exact Restore acknowledgement removes logical suppression and makes the root interactable; empty and Minimize-only publications do not.

The legacy surface-origin invalidation path conservatively advances topology as well as pointer-hit validity because its callers include both topology and geometry mutations. That leaves occasional safe false-positive topology invalidations until those callers can be classified individually.

Pointer hit lookup now has four stages. The exact-point cache serves the same output coordinates. A `PointerHitCertificate` serves different coordinates inside one proven safe rectangle for the same client surface and pointer-hit generation. If that proof is unavailable or the pointer leaves the rectangle, owner locality still traverses cached groups and surfaces to prove the old owner remains frontmost. The canonical full scan remains the exact result authority.

V1 certificates cover conservative ordinary client hits with a live pointer-capable surface role, valid render assignment and logical extent, no visual clip, the default committed input region, and no active generic window interaction. Higher-priority surface targets and server-decoration resize-input envelopes clip the safe rectangle; an envelope containing the current pointer rejects the certificate. Custom input regions, visual apertures, server-side decoration complexity, and ambiguous overlaps intentionally fall back to the existing exact/locality paths.

When certificate eligibility holds, ordinary non-fullscreen steady client motion can now be O(1) with respect to scene size after the initial hit and proof. `fullscreen_composition_plan()` is still built before cache reuse, so this is not a claim that pointer processing is globally O(1) in every fullscreen state. The generation and bounded-counter changes establish source-level behavior; no hardware latency improvement is claimed without measurement.

The input batch budget bounds dispatch. The cursor checkpoint runs after pageflip drain, bounded input, session/shutdown checks, and output-transition checks, immediately before acquire/prepare. It calls the same cursor presentation helper, `AtomicCommitArbiter`, output transaction ledger, and KMS worker as the later presentation path. It does not resolve effects or render scene work. It is enabled only for a matured Auto hardware cursor without an attachable primary carrier; it skips independent admission when a ready frame, worker/KMS ownership, or unavailable worker lane blocks it. Primary acquire/prepare continues after cursor admission or deferral. Kernel scheduling, GPU/EGL waits, Atomic backpressure, and driver behavior still limit any real-world bound.

## Scene-on-change implementation

`EffectSceneSummary` is already a canonical summary maintained with effect and scene mutations. Its `continuous_instance_count` answers whether a live effect-demand calculation can produce continuous frame work. `EffectFrameDemandSnapshot` is default when there are no continuous instances; static `OnDamage` effects need redraw on damage, not a new frame just because the cursor moved.

`CompositorState::effect_frame_demand_snapshot()` now returns the default snapshot when the published continuous count is zero. When that count is nonzero, it retains the existing `resolved_effect_scene().frame_demand_snapshot()` path so dynamic effects continue to advance and current dirty regions are preserved. No new generation counter, parallel scene cache, retained surface reference, KMS owner, or cross-thread state was added.

The summary's validity depends on existing mutation publication: surface effect binding/removal, trusted effect/material changes, blur assignment and scene publication, and background-effect changes refresh or replace it. The optimization depends on those mutation paths continuing to publish the summary. Tests exercise 20 static per-surface effects without scene resolution and a continuous effect with live dirty-region resolution. The effect optimization leaves rendering, KMS ordering, direct scanout, buffering, and VRR policy unchanged. Cursor scheduling is described separately below.

## Effect-anchor diagnostics

Presentation effect influences are used to attach effect influence regions to window-group animation transforms. A surface anchor without a mapped `SceneNode` is therefore missing an animation owner; this lookup is not the source of physical effect damage. The frame builder freezes physical effect damage separately. The missing-owner branch skips only the animation-owner influence and still retains that physical damage.

The current bounded diagnostic uses two process-wide atomic occurrence counters, disabled unless `TYPHON_EFFECT_OWNER_DIAGNOSTICS=1`; teardown emits one aggregate line per nonzero reason. It retains constant memory and performs no per-occurrence formatting or synchronous I/O. A missing presentation owner can describe a legitimate non-window surface as well as an unmapped/stale reference. The current lookup does not carry enough identity/classification data to distinguish those cases, and the old IDs 3, 4, and 7 cannot be classified retrospectively. Do not interpret the aggregate count as proof of stale ownership. The repeated-frame test uses a fixed effect scene for 10,000 resolutions, checks bounded counter accumulation, and verifies physical damage each time.

## Cursor scheduling policies and service opportunities

The existing `OBLIVION_ONE_CURSOR_SCHEDULING` modes now have distinct meanings:

- `auto` uses a KMS-timed service opportunity. It waits for an attachable ready primary carrier, then permits an independent PlaneDelta at that target's decision deadline when no binding primary target or occupied KMS lane owns the opportunity. Dirty or unready primary scene work alone does not defer a matured cursor indefinitely.
- `piggyback` preserves legacy primary-first behavior and is the same-binary A/B rollback baseline.
- `software` keeps cursor presentation on the existing software path.

Auto derives a physical `PresentationTarget` and `KmsSubmitWindow` from the existing presentation deadline and KMS timing model. The target decision deadline is the worker wake time minus a 2 ms transaction-preparation guard. PlaneDelta planning uses this cursor guard plus KMS apply and dispatch costs; full primary scene prediction is excluded. Advisory scheduled visual targets do not become cursor carriers. Reserved primary targets remain ordering lower bounds, including while their scene is unrendered, so an independent cursor target follows the reservation.

Cursor motion replaces the desired latest epoch while the same physical service opportunity remains pending. `pending_since_ns` preserves original cursor age. EBUSY sets `retry_not_before_ns` without resetting that age or moving the absolute freshness deadline. Trace evidence includes retry timing and overdue age. If a physical opportunity is missed, a later target can be planned while the absolute age remains. Matured deadlines are omitted from future wake requests to avoid a past-deadline spin.

The production cursor transport remains vblank-locked, late-latched `PlaneDelta` service. A future capability-gated transport may select among four tiers: Tier 0, a proven safe true asynchronous hardware-cursor update; Tier 1, vblank-locked late-latched `PlaneDelta` (the required baseline); Tier 2, primary presentation piggyback; and Tier 3, software cursor. Tier 0 must prove driver support, exact ownership, same-plane in-flight safety, cadence quality, and NVIDIA/AMD/Intel qualification before it can become the default.

The reactor checkpoint runs before acquire/prepare because source tracing showed that primary preparation could otherwise run before the later cursor decision. It does not cancel or mutate queued/frozen transactions and does not independently admit work when a primary is ready, attachable, or owns the lane. Cursor submission continues through the exact output transaction, cursor revision, generation, framebuffer pin, Atomic arbiter, worker, and pageflip token paths. Sidecars remain latest-wins only before primary freeze. Existing async/tearing and direct-scanout validation remain authoritative. Pageflip feedback is a presentation observation, not optical visibility.

Motion-only late-latching remains out of scope. It would need a separately justified frozen image, visibility, hotspot, capability, framebuffer, and output-generation contract plus evidence for the submitted motion revision.

## Upstream comparison

These are branch-head revisions observed on 2026-10-08, not release versions. Source links are pinned to those revisions where the host supports commit-addressed links.

| Project and inspected revision | Actual mechanism, state owner, trigger, composition/KMS and backpressure | Reusable lesson for Typhon / risk of copying |
|---|---|---|
| KWin `3e56d45e2314483af46e42c94c165f3492cdd47d` | DRM pipeline owns output/plane state. `presentAsync` constructs an Atomic update for plane state and goes through KWin's commit thread. Async presentation has explicit mode and TEST_ONLY checks; cursor planes are disabled for async tearing paths where the kernel requires primary-only changes. The operation is not a second KMS authority. | Reuse typed transaction validation and serialized commit-thread admission. Do not copy async/VRR/plane policy without Typhon's backend, driver, and presentation-mode qualification. [DRM pipeline](https://github.com/KDE/kwin/blob/3e56d45e2314483af46e42c94c165f3492cdd47d/src/backends/drm/drm_pipeline.cpp), [DRM output](https://github.com/KDE/kwin/blob/3e56d45e2314483af46e42c94c165f3492cdd47d/src/backends/drm/drm_output.cpp) |
| Hyprland `5e04ae1f9fc44705799ed9c0f97b99a305c6db25` and Aquamarine `52ea11f06dd492f34b3cc0b39a9c805beecafcff` | Hyprland's pointer manager routes movement and selects hardware/software cursor behavior. Aquamarine owns output cursor position and DRM state. Its async path can sample a latest-position mailbox immediately before submit, but commits remain subject to pending-frame/async-commit checks and a serialized queue/completion path. A late cursor position may be folded into the queued output transaction; this is not a free-running independent KMS submitter. | Reuse bounded latest-wins desired-position sampling only within the one transaction owner. Copying the mailbox without its generation, queue, and completion rules could apply stale coordinates or invalidate worker reservations. [Hyprland pointer manager](https://github.com/hyprwm/Hyprland/blob/5e04ae1f9fc44705799ed9c0f97b99a305c6db25/src/pointer/PointerManager.cpp), [Aquamarine DRM output](https://github.com/hyprwm/aquamarine/blob/52ea11f06dd492f34b3cc0b39a9c805beecafcff/src/backend/drm/DRM.cpp), [Aquamarine async commit](https://github.com/hyprwm/aquamarine/blob/52ea11f06dd492f34b3cc0b39a9c805beecafcff/src/backend/drm/AsyncCommit.cpp) |
| Smithay `cf43a02acd6b5863909665c67f289f6f3478823a` | `DrmCompositor` receives an element stack, plans planes, TEST_ONLY-checks combinations, and falls back to primary rendering when plane assignment fails. It tracks pending/queued/next frame ownership and exposes cursor-only skip behavior in eligible presentation modes. Cursor-only deferral has known VRR trade-offs: a cursor can remain visually stale until another update if skipped under on-demand VRR. | Reuse explicit plane-plan fallback and pending-frame ownership concepts. Do not assume cursor-only updates should always be skipped or always be submitted; VRR behavior is workload and driver dependent. [DRM compositor](https://github.com/Smithay/smithay/blob/cf43a02acd6b5863909665c67f289f6f3478823a/src/backend/drm/compositor/mod.rs), [cursor-only VRR issue](https://github.com/Smithay/smithay/issues/1610) |
| wlroots `c5c57cd316581c0779a51309f1ae90b7e1abd8c4`; Sway `1652c54b73f67df17b7b4ab0b0f7048204aa8104` | Sway routes pointer motion through its seat/cursor APIs. wlroots owns cursor hardware/software selection and calls backend `set_cursor` / `move_cursor`; hardware cursor movement updates output frame demand, while software fallback contributes scene damage. Backend/DRM output code remains responsible for Atomic state. | Reuse clear separation between protocol input routing and backend cursor-plane eligibility. Avoid inferring that a backend cursor move necessarily bypasses render or output scheduling. [wlroots cursor API](https://gitlab.freedesktop.org/wlroots/wlroots/-/blob/c5c57cd316581c0779a51309f1ae90b7e1abd8c4/types/output/cursor.c), [wlroots Atomic backend](https://gitlab.freedesktop.org/wlroots/wlroots/-/blob/c5c57cd316581c0779a51309f1ae90b7e1abd8c4/backend/drm/atomic.c), [Sway pointer routing](https://github.com/swaywm/sway/blob/1652c54b73f67df17b7b4ab0b0f7048204aa8104/sway/input/cursor.c) |
| Mutter `d82671c3035bfdb10fdc1ffd2c0e31859bb00ff7` | Input-side cursor position changes are handed to a KMS cursor manager on the KMS implementation thread. That manager owns per-CRTC cursor buffers/state, marks position state invalid, schedules KMS processing, and injects cursor-plane changes into the central KMS update path. Pageflip/result listeners settle state and buffer lifetime. Cursor position work need not rebuild application scene content, while KMS serialization remains centralized. | Reuse thread-owned cursor update messages and central KMS update admission as a possible future design only if Typhon measures compositor-thread blocking. A new thread or priority path without measured contention would add lifetime, session recovery, synchronization, and ordering complexity. [Mutter native KMS cursor manager](https://gitlab.gnome.org/GNOME/mutter/-/blob/d82671c3035bfdb10fdc1ffd2c0e31859bb00ff7/src/backends/native/meta-kms-cursor-manager.c), [Mutter KMS device](https://gitlab.gnome.org/GNOME/mutter/-/blob/d82671c3035bfdb10fdc1ffd2c0e31859bb00ff7/src/backends/native/meta-kms-impl-device.c) |

Smithay, wlroots, and Mutter implement useful components, not a reason to replace Typhon's current rendering and DRM ownership. Typhon already has a native cursor state machine, Atomic transaction arbiter, worker reservations, output generations, explicit synchronization, and presentation identities. The measured source-level waste is upstream-agnostic: avoid rebuilding scene/effect information for an unchanged static scene.

## Qualification and remaining work

`docs/cursor-responsiveness-diagnostics.md` specifies bounded per-motion / transaction tracing and repeatable empty-versus-populated workspace trials. Hardware runs must use the actual target NVIDIA system and report input-service age, hit testing, effect resolution, cursor disposition, KMS queue/submit/feedback intervals, per-thread scheduling, primary pacing, and trace overhead separately. Pageflip feedback is not optical cursor latency.

No hardware A/B trial was run for this change. The active desktop belongs to another compositor; this work did not restart or replace it. Existing host information is a dated observation and does not qualify Typhon cursor latency.

The scenario matrix, numeric p50/p95/p99 before/after comparison, GPU cost, and NVIDIA/VRR/Direct Scanout qualification remain open until hardware trials run. Deterministic tests establish policy behavior and ordering invariants; they do not prove cursor-age improvement or primary-throughput parity on hardware. The missing-owner aggregate does not distinguish legitimate non-window anchors from stale mappings; improving that classification requires the owner registry to expose a safe bounded reason code.
