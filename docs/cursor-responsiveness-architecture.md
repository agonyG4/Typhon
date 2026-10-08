# Cursor responsiveness: architecture findings and implementation

## Status and evidence

The current checkout is authoritative. The supplied archive `Typhon-source(20261008-141911).zip` and RFC `TYPHON_CURSOR_RESPONSIVENESS_ARCHITECTURE_RFC_2026-10-08.md` were not present under `/home/agony`; no source comparison against those files was possible. The CodebaseMCP project was `home-agony-GitHub-Typhon`, generation `2026-10-08T15:34:35Z`, full index mode. Its coverage report marked `cycle_dispatch.rs:2010` and `presentation_cycle.rs:299` as parse-partial; those ranges were read directly. `frame.rs` had changed metadata after edits and was read directly as well.

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
| Presentation decision | `NativeCursorOutputArbitration` selects `PiggybackPrimary`, `DeferForPrimary`, `SubmitPlaneDelta`, software overlay, or no operation. Primary work retains priority; an eligible, due idle hardware cursor can use a plane delta. |
| KMS submission and completion | One `AtomicCommitArbiter` and the existing worker/output transaction path own Atomic admission, submission, and completion identity. Cursor scheduling is a class within that authority, not a second submitter. |

The input batch budget and checkpoints bound work between reactor opportunities. They do not guarantee a real-time bound: kernel scheduling, the compositor thread's non-interruptible sections, GPU/EGL waits, Atomic backpressure, and driver behavior remain relevant. The source alone cannot determine which of those dominated the hardware observation.

## Scene-on-change implementation

`EffectSceneSummary` is already a canonical summary maintained with effect and scene mutations. Its `continuous_instance_count` answers whether a live effect-demand calculation can produce continuous frame work. `EffectFrameDemandSnapshot` is default when there are no continuous instances; static `OnDamage` effects need redraw on damage, not a new frame just because the cursor moved.

`CompositorState::effect_frame_demand_snapshot()` now returns the default snapshot when the published continuous count is zero. When that count is nonzero, it retains the existing `resolved_effect_scene().frame_demand_snapshot()` path so dynamic effects continue to advance and current dirty regions are preserved. No new generation counter, parallel scene cache, retained surface reference, KMS owner, or cross-thread state was added.

The summary's validity depends on existing mutation publication: surface effect binding/removal, trusted effect/material changes, blur assignment and scene publication, and background-effect changes refresh or replace it. The optimization depends on those mutation paths continuing to publish the summary. Tests exercise 20 static per-surface effects without scene resolution and a continuous effect with live dirty-region resolution. Existing rendering, cursor policy, KMS ordering, direct scanout, buffering, and VRR policy are unchanged.

## Effect-anchor diagnostics

Presentation effect influences are used to attach effect influence regions to window-group animation transforms. A surface anchor without a mapped `SceneNode` is therefore missing an animation owner; this lookup is not the source of physical effect damage. The frame builder freezes physical effect damage separately. The missing-owner branch skips only the animation-owner influence and still retains that physical damage.

The current bounded diagnostic uses two process-wide atomic occurrence counters, disabled unless `TYPHON_EFFECT_OWNER_DIAGNOSTICS=1`; teardown emits one aggregate line per nonzero reason. It retains constant memory and performs no per-occurrence formatting or synchronous I/O. A missing presentation owner can describe a legitimate non-window surface as well as an unmapped/stale reference. The current lookup does not carry enough identity/classification data to distinguish those cases, and the old IDs 3, 4, and 7 cannot be classified retrospectively. Do not interpret the aggregate count as proof of stale ownership. The repeated-frame test uses a fixed effect scene for 10,000 resolutions, checks bounded counter accumulation, and verifies physical damage each time.

## Cursor arbitration decision

No arbitration policy change was made. The existing disposition order is explicit: no pending desired epoch means `Noop`; pending primary work chooses `PiggybackPrimary`; a cursor before its response deadline is deferred while a primary response window is open; an eligible due hardware cursor with no software overlay need chooses `SubmitPlaneDelta`; remaining work is software overlay. Desired cursor epoch and image state are selected for a plane plan, then carried by the existing output transaction identity and KMS arbiter/worker. Worker admission and KMS submission are distinct from KMS acceptance and later pageflip feedback; feedback is matched by output generation and transaction/token identity. Pageflip feedback still does not measure optical cursor visibility.

Submitted-epoch settlement clears only the matching current desired epoch. If a newer desired epoch arrived while the older submission was in flight, the newer epoch remains pending with a refreshed response window. This latest-wins display-state handling does not coalesce away ordered client input events.

This policy can permit repeated piggybacking while primary work remains ready. No bounded-fairness change is justified without measured cursor age, primary deadlines, worker reservations, in-flight state, VRR policy, and A/B comparison. Cursor movement remains latest-wins for display state while protocol input events retain semantic order. A high polling-rate mouse does not imply one KMS commit per event.

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

No hardware A/B trial was run for this change. The host reports an NVIDIA GeForce RTX 3060 Ti with driver `615.71.09` and a connected `card0-DP-1`, but an Xorg/Hyprland session currently owns `seat0` and no Typhon process is running. The active refresh mode and Typhon's 165 Hz path were not verified; starting a second native compositor on that seat would disturb the user's running session.

The full requested scenario matrix, numeric p50/p95/p99 before/after comparison, GPU cost, and NVIDIA/VRR/Direct Scanout qualification remain open until the hardware trials are run. Advanced spatial indexing, arbitration fairness changes, input priorities, and cross-thread scene work remain gated on those measurements. The missing-owner aggregate does not distinguish legitimate non-window anchors from stale mappings; improving that classification requires the owner registry to expose a safe bounded reason code.
