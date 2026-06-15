# Reference compositor performance architecture

Date: 2026-06-11

Scope: architecture comparison between Oblivion One's native compositor path and
the local reference compositor trees available under `WM para Referencia/`.

References present in this workspace:

- `Hyprland-main`
- `kwin-master`
- `ShojiWM-main`

`Mutter` and a standalone `wlroots` tree were not present in the reference
folder at the time of this pass, so this note does not claim source-level
analysis for them. ShojiWM is useful here as a Rust/Smithay-style comparator,
not as a mature production target on the same level as KWin or Hyprland.

This note intentionally does not repeat the Lagrange cursor study. Cursor is
covered only where it affects the remaining performance architecture gaps:
native scanout ownership, damage propagation, presentation pacing, resize
behavior, and input latency.

## Current Oblivion architecture

Oblivion has the right high-level module boundary for a mature compositor, but
the native execution path is still transitional.

The architecture contract is:

- `render_backend` declares the target renderer profile and capabilities.
  `RenderBackendProfile::egl_gles()` already lists GPU composition,
  damage-tracked shm upload, modifier-aware dmabuf import, dmabuf feedback,
  explicit sync, direct scanout, and multi-GPU import as the intended graphics
  contract (`src/render_backend/mod.rs:43-78`).
- `compositor` owns Wayland protocol state, committed surface snapshots,
  pending frame callbacks, presentation feedback, xdg resize state, and output
  description (`src/compositor/server.rs:222-230`,
  `src/compositor/output.rs:69-153`).
- `src/egl_renderer.rs` is the real GPU compositor for the nested Wayland
  output. It imports resources, tracks EGL output damage, and uses
  `EGL_KHR/EXT_swap_buffers_with_damage` when available
  (`src/egl_renderer.rs:245-283`, `src/egl_renderer.rs:672-690`).
- `src/native_output.rs` owns the native KMS/input loop. It selects KMS mode,
  opens native scanout, opens libseat/libinput where possible, handles cursor
  mode, drains input, paints the server frame, and calls pageflip.

The native hot path now has the target renderer shape in code, but still needs
real TTY/KMS validation:

1. The `native-egl-gbm` scanout path renders with GLES into a GBM-backed EGL
   surface, locks the GBM front buffer, caches a DRM framebuffer ID for that BO,
   and pageflips through KMS. The retained `gbm-cpu-write` fallback still
   renders a CPU frame, converts/copies it into a staging buffer, writes the
   whole buffer into a GBM BO, then pageflips.
2. Plain pointer motion can avoid a frame repaint when the native hardware
   cursor path is active. The input effect still marks a redraw internally, but
   `requires_frame_repaint()` suppresses it for cursor-only hardware motion
   (`src/native_output.rs:1268-1310`, `src/native_output.rs:2918-2935`).
3. The main loop remains sleep/timer driven. It samples Wayland, pageflip, and
   input work, then sleeps based on activity and refresh-derived pacing
   (`src/native_output.rs:726-903`, `src/native_output.rs:915-965`).
4. Pageflip completion clears native scanout state, but Wayland
   `present_frame()` is still called from pending frame work in the loop rather
   than directly as the pageflip/vblank completion boundary
   (`src/native_output.rs:758-763`, `src/native_output.rs:881-885`,
   `src/native_output.rs:3774-3783`).
5. Resize ACK handling has already moved toward the Hyprland pattern: Oblivion
   now promotes the latest sent resize serial not newer than the ACK
   (`src/compositor/mod.rs:2296-2305`,
   `src/compositor/tests/windows.rs:226-274`). That is no longer the main gap.

## Reference architecture patterns

### Hyprland

Hyprland's performance architecture is output-owned and damage-driven:

- Frame rendering starts only when the output needs a frame or a forced frame is
  active (`Hyprland-main/src/render/Renderer.cpp:2070-2071`).
- Direct scanout is attempted before normal composition, and a successful
  direct scanout returns without running the normal render pass
  (`Hyprland-main/src/render/Renderer.cpp:2073-2090`).
- Damage determines whether the workspace is rendered, and damage is transformed
  into output state before commit (`Hyprland-main/src/render/Renderer.cpp:2136-2164`,
  `Hyprland-main/src/render/Renderer.cpp:2231-2255`).
- Damage entry points are explicit: surface, window, monitor, box, and region
  paths all add scoped damage rather than invalidating the entire compositor by
  default (`Hyprland-main/src/render/Renderer.cpp:2698-2812`).
- Cursor movement is mostly backend cursor state. Hardware cursor movement calls
  output cursor movement; software cursor fallback damages only the cursor box
  (`Hyprland-main/src/managers/PointerManager.cpp:319-415`,
  `Hyprland-main/src/managers/PointerManager.cpp:1118-1129`).
- The monitor listens to backend `frame`, `needsFrame`, and `present` events.
  Presentation protocol state is updated from output present metadata, with a
  fallback monotonic timestamp when the backend timestamp is invalid
  (`Hyprland-main/src/output/Monitor.cpp:99-178`).
- The newer frame scheduler uses explicit sync and can render before the frame
  event if the last render missed the deadline, then commits at the earliest
  safe presentation point (`Hyprland-main/src/output/MonitorFrameScheduler.cpp:14-139`).

For resize, Hyprland separates window geometry from old client buffer size
during interactive resize. It avoids stretching a smaller surface to the new box
while resize is in progress (`Hyprland-main/src/render/pass/SurfacePassElement.cpp:26-50`).
Its ACK handling promotes the newest pending size serial `<= ack_serial`
(`Hyprland-main/src/desktop/view/Window.cpp:1414-1428`), which Oblivion now
matches in spirit.

### KWin

KWin's useful comparator is less about individual feature names and more about
ownership:

- `RenderLoop` owns presentation prediction. It estimates when to start
  compositing from refresh rate, previous presentation timestamp, recent render
  time, safety margin, pending frame count, VRR, and tearing mode
  (`kwin-master/src/core/renderloop.cpp:44-113`,
  `kwin-master/src/core/renderloop.cpp:256-274`).
- Frame completion updates render timing, vblank timestamp, pending frame count,
  and emits `framePresented` (`kwin-master/src/core/renderloop.cpp:130-161`).
- Refresh changes reschedule active timers (`kwin-master/src/core/renderloop.cpp:238-249`).
- The DRM EGL layer renders into GBM-backed render targets and tracks damage
  through damage journals / buffer age instead of full-frame CPU writes
  (`kwin-master/src/backends/drm/drm_egl_layer_surface.cpp:155-172`,
  `kwin-master/src/backends/drm/drm_egl_layer_surface.cpp:231-280`).
- Direct scanout is gated by an explicit import path with conditions for output
  layer type, modeset safety, shadow buffers, multi-GPU devices, color pipeline,
  source rect, and transform (`kwin-master/src/backends/drm/drm_egl_layer.cpp:75-122`).
- Surface damage is mapped from buffer space through surface transforms into
  view repaint regions (`kwin-master/src/scene/surfaceitem.cpp:118-156`).

The KWin takeaway for Oblivion is that the render loop, output layer, and DRM
framebuffer ownership should be a first-class native rendering subsystem. The
compositor server should not decide that "a frame is presented" before the
native output layer has actually completed the presentation boundary.

### ShojiWM

ShojiWM is valuable because it shows a Rust event-loop and Smithay-style
implementation with many of the same moving parts:

- The DRM event source calls `frame_finish()` directly on VBlank
  (`ShojiWM-main/src/shojiwm/src/backend/tty.rs:502-508`).
- `frame_finish()` marks the frame submitted, builds presentation feedback from
  DRM metadata, clears `frame_pending`, and schedules follow-up redraw only if
  the VBlank state says another redraw is needed
  (`ShojiWM-main/src/shojiwm/src/backend/tty.rs:527-682`).
- No-damage frames can still arm an estimated-vblank callback so visible clients
  keep receiving frame callbacks without forcing a full repaint
  (`ShojiWM-main/src/shojiwm/src/backend/tty.rs:4693-4720`).
- The state model tracks source damage, decoration damage, scene generations,
  runtime dirty sets, and output damage trackers separately
  (`ShojiWM-main/src/shojiwm/src/state.rs:260-305`).
- Source damage is extracted from Wayland surface damage and mapped through
  decoration/client geometry before rendering
  (`ShojiWM-main/src/shojiwm/src/state.rs:3225-3333`).
- `schedule_redraw()` is deliberately a state transition, not a self-wakeup; a
  previous unconditional wake/flush shape caused a Firefox CPU regression
  (`ShojiWM-main/src/shojiwm/src/state.rs:2909-2970`).

This is the most useful incremental reference for Oblivion's current native
loop because it suggests a path from sleep-driven loop to event-driven output
state without needing to copy KWin's full architecture in one jump.

## Gap table

| Gap | Reference | Suggested Oblivion implementation | Risk | Recommended order |
| --- | --- | --- | --- | --- |
| Native renderer ownership is still partly inside `native_output.rs`, though scene rendering is now shared. | KWin DRM EGL layer owns GBM/EGL render targets and imports framebuffers; Hyprland's renderer commits output state after damage/direct-scanout decisions. | Move the new `NativeEglGbmScanout`, framebuffer cache, and pageflip buffer ownership into a smaller native-render target module after hardware validation. Keep `native_output.rs` as session/event orchestration. | Medium-high: mostly file/module ownership now, but regressions can still blank native session. | 4 |
| Native CPU GBM fallback renders full CPU frames and writes whole buffers. | KWin `EglGbmLayerSurface` renders to GBM swapchain slots and imports buffers; Hyprland renders GPU passes and adds output damage. | Keep `native-egl-gbm` as the normal path and retain dumb framebuffer plus CPU GBM write as explicit rollback modes until TTY evidence is stable. | High: fallback must remain available while GPU path is proven. | 5 |
| Damage tracking exists for committed surfaces and nested EGL cursor/overlay, but native scanout does not consume region damage. | Hyprland `damageSurface`/`damageWindow`/`damageBox`; KWin `SurfaceItem::addDamage`; Shoji source damage vectors and output trackers. | Add a native `OutputDamageAccumulator`: surface commit damage, old/new window bounds, resize frame bounds, shell overlay rects, software cursor rects, and full-output invalidation reasons. Feed it to native renderer and perf logs. | Medium: incorrect damage causes stale pixels. Start with full-damage fallback on overflow/unknown. | 2 |
| Cursor plane is now present, but cursor ownership still lives inside the native loop and software fallback damage is not integrated with native output damage. | Hyprland pointer manager owns hardware/software fallback and damages cursor box only; KWin has separate cursor layer concepts in backend layers. | Keep current `NativeHardwareCursor`, but move cursor-plane policy into the native renderer/output layer. When hardware cursor fails, add old/new cursor rects to the native damage accumulator instead of treating cursor as part of a full frame. | Medium: cursor loss/flicker under modeset or failed move. | 1 |
| Direct scanout / overlay is declared as a target capability but not implemented in native output. | Hyprland attempts direct scanout before normal render; KWin gates scanout import on output layer, modeset, GPU, color, source rect, and transform. | Implement a conservative `DirectScanoutCandidate` contract: one fullscreen opaque dmabuf surface, matching output transform/scale/format/modifier, no shell overlays, no software cursor, no active effects, no pending color transforms. Add "why rejected" perf logging before enabling by default. | High: wrong gating can produce black frames, stale shell, bad colors, or security leaks. | 6 |
| Presentation scheduling is loop-sleep based; pageflip completion is sampled and not the protocol completion boundary. | Hyprland monitor `present` event updates presentation protocol; KWin `RenderLoop::notifyFrameCompleted`; Shoji `frame_finish()` from DRM VBlank. | Split `server.present_frame()` into prepare/finish phases. Send pending configures before rendering, then complete frame callbacks/presentation feedback from pageflip/VBlank completion. Add an estimated-vblank no-damage callback path for visible surfaces. | High: client frame pacing and resize ordering are protocol-sensitive. | 3 |
| Input latency still depends on the native loop waking, draining input, possibly rendering CPU frames, then sleeping. | KWin and Shoji route backend fds through event-loop sources; Hyprland schedules frames from backend output/input events instead of periodic polling. | Introduce a small fd-driven native poll/calloop phase for Wayland socket, libseat/libinput fd, DRM fd, and timerfd. Preserve the current wakeup policy as fallback, but let input/DRM readiness wake the loop directly. | Medium/high: lifecycle and signal handling can regress. | 7 |
| Resize configure is now serial-aware, but native prepare/finish ordering still sends resize protocol work through `present_frame()` after repaint decisions. | Hyprland sends resize sizes and damages geometry as the target changes; KWin frame loop separates scheduling and presentation completion. | After splitting frame phases, flush pending resize configures before choosing paint work. Keep old-buffer-not-stretched behavior explicit: render committed buffer at committed size and draw compositor-owned resize frame/background around it when visual geometry leads client commit. | Medium: toolkits differ in ACK/commit timing. | 3 |
| Output mode/refresh is advertised, but render timing still uses approximate sleep intervals. | KWin reschedules on refresh change and limits pending frames; Hyprland frame scheduler adapts to explicit sync deadlines. | Keep `OutputRefreshRate` as the client-facing source of truth, but make native frame targets derive from DRM events/timerfd. Add perf fields for target vblank, actual pageflip timestamp, render start/end, and missed deadline. | Medium: measurement first; behavior can stay conservative. | 3 |
| Immediate client flush on every high-level input send can help latency but may add syscall pressure under high-rate input. | Shoji's redraw scheduling comment calls out unconditional flush/wakeup as a Firefox CPU regression; Hyprland separates pointer motion from frame scheduling. | Measure before changing. Add perf counters for input events, pointer frames, flush count, and flush elapsed. If needed, batch pointer motion flush once per coalesced input drain while keeping button/key flush immediate. | Low/medium: batching can hurt perceived latency if overdone. | 2 |

## Recommended phases

### Phase 1: Finish cursor-plane ownership

The current hardware cursor work is a good first win. The remaining architecture
step is to make cursor plane policy part of native output/renderer ownership,
not a special case in the main loop.

Expected changes:

- keep `OBLIVION_ONE_CURSOR=auto|hardware|software`;
- centralize cursor backend status, fallback reason, and old/new cursor damage;
- software cursor fallback damages only cursor rects when the rest of the scene
  is unchanged;
- perf logs report cursor move failures and whether motion caused repaint.

Rollback:

- `OBLIVION_ONE_CURSOR=software`
- current full-frame CPU render remains available.

Validation:

- native unit tests like
  `native_input_pointer_motion_can_skip_frame_repaint_with_hardware_cursor`;
- perf run showing pointer-only motion does not produce full `native.frame`
  repaint when hardware cursor is active.

### Phase 2: Native damage accumulator

Build a damage model before replacing the renderer. This keeps the next GPU
phase testable.

The accumulator should accept:

- committed surface damage from `RenderableSurfaceDamage`;
- old/new window bounds for move/resize;
- shell overlay packed rects;
- software cursor old/new rects;
- output mode/scale/full refresh invalidation;
- a "damage overflow" or "unknown reason" full-output fallback.

Do not remove full repaint. Full repaint should be the safe fallback whenever
the accumulator cannot prove a bounded region.

Validation:

- unit tests for old/new cursor rects, moved window old/new bounds, surface
  damage clipping, and full-output fallback;
- perf/debug logs that show damage reason and rect count per frame.

### Phase 3: Presentation phase split

Separate protocol preparation from presentation completion.

Suggested contract:

- `prepare_frame_protocol()`:
  - commit ready explicit-sync buffers;
  - flush pending color/output info;
  - flush pending resize configure;
  - flush clients.
- `finish_presented_frame(presentation_metadata)`:
  - release buffers whose content reached the output boundary;
  - complete frame callbacks;
  - complete presentation feedbacks with pageflip/VBlank metadata;
  - flush clients.

The existing `present_frame()` can remain as a compatibility wrapper for nested
or tests while native migrates.

Validation:

- resize tests proving configure is flushed before paint/present decisions;
- presentation feedback tests with injected timestamp/sequence metadata;
- no-damage visible-client callback test.

### Phase 4: Native EGL/GBM render target

Status: implemented in code, pending real TTY/KMS hardware validation.

The main native render path moved from CPU frame -> staging -> `bo.write()` to
GPU render target -> framebuffer import/cache -> pageflip.

Backend choices:

- `OBLIVION_ONE_SCANOUT_BACKEND=auto` tries `native-egl-gbm`,
  `gbm-cpu-write`, then `dumb`;
- `OBLIVION_ONE_SCANOUT_BACKEND=gpu` or `native-egl-gbm` requires native
  EGL/GBM;
- `OBLIVION_ONE_SCANOUT_BACKEND=gbm-cpu-write` or `cpu` forces the CPU GBM
  write fallback, with `gbm` and `gbm-egl` retained as legacy aliases;
- dumb framebuffer fallback remains diagnostic.

Important boundary:

- WM policy and Wayland protocol state should not know whether the output is
  CPU, EGL, direct scanout, or dumb framebuffer. They should supply surfaces,
  damage, shell overlay state, and presentation needs.

Validation:

- startup smoke with no clients;
- `wayland-info`;
- a shm client;
- a dmabuf client;
- hardware and software cursor modes;
- perf log comparing render/copy/write/present time before and after.

### Phase 5: Direct scanout / overlay

Only after native EGL/GBM render targets and damage are stable, add direct
scanout.

Start with a narrow candidate:

- one fullscreen toplevel;
- dmabuf-backed buffer;
- opaque or otherwise known safe;
- output scale/transform/source rect exact;
- no shell overlay/dock/topbar/Spotlight;
- no software cursor;
- no animation/effect;
- matching GPU/import device and modifier;
- presentation feedback still delivered through the same native output boundary.

Log rejection reasons before enabling success cases by default. Direct scanout
bugs are hard to see in unit tests, so diagnostics matter as much as code.

### Phase 6: Event-driven native loop

Once presentation completion is tied to DRM events, replace sleep-as-primary
with fd readiness:

- Wayland socket/client dispatch;
- libseat/libinput dispatch;
- DRM pageflip events;
- timerfd for scheduled repaint deadlines and fallback no-damage callbacks;
- signal/exit path.

The current refresh-derived sleep can remain a fallback while the fd loop is
introduced.

## Compatibility and rollback

Keep these contracts stable during the migration:

- `OBLIVION_ONE_MODE`
- `OBLIVION_ONE_CURSOR`
- `OBLIVION_ONE_INPUT_BACKEND`
- `OBLIVION_ONE_SCANOUT_BACKEND`
- `OBLIVION_ONE_PERF_LOG`
- compositor registry/protocol names
- `OwnCompositorServer::present_frame()` compatibility wrapper until nested and
  tests move to split phases

Rollback must remain a runtime choice, not a source revert:

- software cursor fallback;
- CPU GBM write fallback;
- dumb framebuffer fallback;
- full damage fallback;
- timer/sleep fallback if fd-driven scheduling misbehaves.

## Do-not-touch boundaries

- Do not route apps to the host `DISPLAY` as a performance shortcut. XWayland
  compatibility must remain compositor-owned.
- Do not move WM policy into the renderer. Renderer/output owns pixels, damage,
  planes, fences, and presentation. WM owns placement, focus, move, resize,
  maximize, minimize, and fullscreen policy.
- Do not collapse native and nested behavior into one output path too early.
  Nested EGL damage and native KMS pageflip have different presentation
  boundaries.
- Do not remove CPU/dumb fallback paths until native EGL/GBM has real SDDM/TTY
  evidence.

## Validation gates

Use these gates when implementing the phases:

1. Unit tests for pure state changes:
   - damage accumulator;
   - cursor repaint gating;
   - resize ACK/prepare ordering;
   - direct scanout candidate rejection reasons.
2. Protocol tests:
   - resize configure before render decision;
   - frame callbacks after injected presentation completion;
   - presentation feedback timestamp/sequence propagation.
3. Native dry/smoke:
   - `./bin/oblivion-one compositor --check`
   - `./bin/oblivion-one compositor -- wayland-info`
   - `OBLIVION_ONE_DRY_RUN=1 ./bin/start-oblivion-one`
4. Native perf run:
   - `OBLIVION_ONE_PERF_LOG=1 start-oblivion-one-tty`
   - compare `native.frame` render/copy/write/present fields;
   - verify pointer-only motion under hardware cursor does not repaint frames;
   - verify resize emits configure promptly and frame completion follows
     pageflip/VBlank.
5. Manual native session checks only after deterministic tests pass:
   - no-client desktop;
   - terminal;
   - browser/Gecko resize;
   - fullscreen video candidate before direct scanout is enabled.

## Summary

The remaining performance gap is not mainly "missing cursor optimization"
anymore. The bigger architectural gap is that Oblivion's native backend still
has the output loop, renderer, scanout buffer, damage model, and presentation
completion collapsed into `src/native_output.rs`.

Hyprland and KWin both put output rendering behind a damage-aware output state
and treat presentation completion as a backend event. ShojiWM shows an
incremental Rust path: event-loop sources drive redraw state, VBlank finishes
frames, no-damage callbacks can progress without repaint, and damage is tracked
as state rather than as a reason to rebuild the whole screen.

The recommended order is therefore:

1. finish cursor-plane ownership and software cursor damage;
2. add native damage accumulation and measurement;
3. split protocol prepare from presentation finish;
4. move native composition to EGL/GBM render targets;
5. add conservative direct scanout / overlay candidates;
6. make the native loop fd/event driven.
