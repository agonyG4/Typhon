# Agent Keystone Resize/Scale Architecture

Date: 2026-06-11

Scope: read-only architecture study for resize/scale escalation in Oblivion One.
The goal is to separate visual target, committed buffer target, frame/decorations,
damage, and the future native GPU path without breaking the current CPU path or
the NVIDIA/browser safety posture.

This report does not change production code.

## Current Architecture

Oblivion has already moved past the most dangerous xdg configure serial bug:
`ack_xdg_surface_configure()` promotes the newest resize configure whose serial
is not newer than the client ACK (`src/compositor/mod.rs:2497-2511`). The
remaining performance/correctness problem is not serial ownership. It is that
the render model still lets several different concepts share one pair of
dimensions.

Current resize flow:

1. Interactive resize computes a compositor-side target and queues a configure
   (`queue_resize_root_window_to()`, `src/compositor/mod.rs:2284-2311`).
2. The same path immediately previews the resize by mutating the root
   `RenderableSurface.width`, `height`, and `placement`
   (`preview_resize_root_window_to()`, `src/compositor/mod.rs:2314-2365`).
3. The preview stores `ResizePreview { committed_width, committed_height,
   anchor_right, anchor_bottom }`, which gives the CPU renderer enough data to
   avoid upscaling undersized committed buffers in some cases
   (`src/compositor/surface.rs:41-47`,
   `src/compositor/render.rs:1137-1169`).
4. A later compatible ACKed commit clears `resize_preview` and applies the real
   committed logical size through `update_renderable_surface_buffer()`
   (`src/compositor/mod.rs:2470-2494`, `src/compositor/mod.rs:2948-2964`).

Current surface model:

- `RenderableSurface.width` / `height` are the committed logical size during a
  normal commit, but become the interactive visual target during a resize
  preview (`src/compositor/surface.rs:5-17`,
  `src/compositor/mod.rs:2354-2364`).
- The real buffer size remains available through `RenderableSurface::buffer_size`
  (`src/compositor/surface.rs:32-34`).
- Viewporter destination and `wl_surface.set_buffer_scale` are committed into a
  logical surface size (`src/compositor/state_data.rs:403-431`).
- Fractional scale and output scale are advertised/provided separately, but the
  scene renderer still builds output-space target rects from
  `surface.width/height` (`src/compositor/render.rs:461-476`).

Current CPU render flow:

- `DesktopSceneRenderer` snapshots each surface target and uses target changes
  for partial scene damage (`src/compositor/render.rs:189-233`,
  `src/compositor/render.rs:440-563`).
- Full scene composition draws server frame rects first, then blits each surface
  into a target rect (`src/compositor/render.rs:389-430`).
- The blit path calls `resize_preview_content_target()` before sampling pixels,
  so a resize preview can shrink the content target back toward the committed
  dimensions (`src/compositor/render.rs:1044-1169`).
- `server_frame_rects_for_surface()` currently returns an empty list
  (`src/compositor/render.rs:703-712`), so frame/decorations are not yet a
  first-class geometry participant.

Current EGL/GLES render flow:

- `EglGlesFrameRenderer` builds draw commands from `surface.width/height` and
  `server_frame_rects_for_surface()` (`src/egl_renderer.rs:526-585`).
- The EGL path imports shm and dmabuf surfaces and can use swap-buffers damage,
  but it does not yet have the CPU renderer's `ResizePreview` content-target
  logic in its draw-command geometry.
- This means future native GPU rendering must not blindly inherit
  `surface.width/height` as "sample the client texture at this size" during an
  interactive resize.

Current native output/damage flow:

- Native CPU output tracks surface damage and surface bounds changes through
  `NativeDamageAccumulator` (`src/native_output.rs:3444-3595`).
- Surface damage is scaled from buffer coordinates to `surface.width/height`
  (`src/native_output.rs:3462-3494`), so conflating resize preview dimensions
  with committed content dimensions can also distort damage.
- Prior local profiling found that native CPU still writes full frame-sized GBM
  buffers on repaint, overlapping damage can make `copy_bytes` exceed one frame,
  and resize is dominated by client commits and pending-frame work
  (`docs/research/agent-gauge-resize-damage-perf.md:168-190`).

## Reference Comparison

### Hyprland

Hyprland updates compositor-owned window geometry immediately during resize, but
keeps committed client surface size separate from that visual target.

- `WindowTarget.cpp` damages before and after mutating window geometry, writes
  logical and visual boxes, updates decorations, and sends the requested window
  size (`WM para Referencia/Hyprland-main/src/layout/target/WindowTarget.cpp:34-58`,
  `WM para Referencia/Hyprland-main/src/layout/target/WindowTarget.cpp:107-127`,
  `WM para Referencia/Hyprland-main/src/layout/target/WindowTarget.cpp:216-228`).
- `CWindow::onAck()` applies the newest pending size ACK whose serial is not
  newer than the ACK serial, matching the current Oblivion ACK direction
  (`WM para Referencia/Hyprland-main/src/desktop/view/Window.cpp:1414-1428`).
- `SurfacePassElement.cpp` is the key resize/scale reference: when a small
  committed surface is being rendered during interactive resize, the interactive
  branch uses the viewporter-corrected committed size instead of stretching the
  stale buffer to the live window box
  (`WM para Referencia/Hyprland-main/src/render/pass/SurfacePassElement.cpp:26-50`).
- On commit, Hyprland damages the committed surface at the current real window
  position and handles tearing/busy scheduling separately
  (`WM para Referencia/Hyprland-main/src/desktop/view/Window.cpp:2575-2633`).
- During render, Hyprland turns accumulated damage into output damage and commits
  with explicit sync at the presentation boundary
  (`WM para Referencia/Hyprland-main/src/render/Renderer.cpp:2231-2263`).

Takeaway for Oblivion: visual resize target should move immediately, but client
texture sampling must use committed buffer/logical geometry unless a committed
viewport/buffer-scale path explicitly authorizes scaling.

### KWin

KWin's useful pattern is the split between pending configure geometry,
move-resize geometry, committed client geometry, output scale, and layer
fallbacks.

- Configure events are scheduled through an idle timer to coalesce redundant
  requests (`WM para Referencia/kwin-master/src/xdgshellwindow.cpp:85-90`).
- ACK handling consumes configure events up to the acknowledged serial before
  role commit processing (`WM para Referencia/kwin-master/src/xdgshellwindow.cpp:135-168`).
- When geometry-changing configures are pending, KWin avoids syncing
  move-resize geometry from old committed geometry, preventing visual rollback
  during resize (`WM para Referencia/kwin-master/src/xdgshellwindow.cpp:181-198`).
- KWin snaps requested client/frame sizes through the target scale and either
  updates geometry immediately or schedules a configure
  (`WM para Referencia/kwin-master/src/xdgshellwindow.cpp:262-280`).
- Toplevel configure records `moveResizeGeometry()` as bounds, keeping the
  target geometry as the configure contract
  (`WM para Referencia/kwin-master/src/xdgshellwindow.cpp:820-831`).
- The compositor maps logical geometry to output device coordinates using the
  output scale and transform, then separately tests direct scanout/rendering
  layers and downgrades through overlay/cursor/primary fallbacks
  (`WM para Referencia/kwin-master/src/compositor.cpp:345-420`,
  `WM para Referencia/kwin-master/src/compositor.cpp:636-790`,
  `WM para Referencia/kwin-master/src/compositor.cpp:940-964`).

Takeaway for Oblivion: the scale boundary belongs in the render plan, not in
ad-hoc resize math. Fallback testing must happen before enabling GPU/browser
surfaces, especially on NVIDIA modifiers.

## Problem Boundary

This plan targets:

- separating compositor-owned visual target from committed client buffer target;
- preserving committed logical size from viewport destination and buffer scale;
- adding a frame/decorations target that can follow pointer resize independently;
- making damage cover old/new visual bounds, committed buffer damage, and future
  frame/decorations damage;
- providing one render plan that CPU, nested EGL, and future native GPU can
  consume;
- keeping the current CPU path and native browser CPU guards intact until native
  GPU dmabuf import/presentation is proven.

This plan does not target:

- changing xdg configure ACK semantics, which are already aligned with the local
  Hyprland/KWin references;
- enabling native GPU/browser acceleration by default;
- direct scanout/overlay as a first resize milestone;
- repeating cursor-plane research beyond acknowledging that software cursor
  damage still needs old/new rect coverage.

## Target Direction

Introduce a render-facing model with explicit ownership:

```text
WindowVisualTarget
  compositor-owned box used by move/resize, frame/decorations, hit testing,
  configure bounds, and old/new visual damage

CommittedBufferTarget
  client-owned committed buffer handle, buffer size, committed logical size,
  viewport destination, buffer scale, damage in buffer coordinates, sync state

ContentSampleTarget
  output/logical rect where the committed client content may be sampled this
  frame; during interactive resize it may be smaller than WindowVisualTarget

FrameDecorationTarget
  compositor-owned rects around/behind content, allowed to follow
  WindowVisualTarget immediately and damage independently

OutputDamagePlan
  union/coalesced old visual bounds, new visual bounds, committed content damage,
  frame/decor damage, cursor/software fallback damage, and full-output fallback
```

Core invariant:

> A stale client buffer must not be scaled just because interactive resize moved
> the visual target. Scaling is allowed only for committed viewporter,
> buffer-scale/output-scale mapping, or an explicit future policy that is tested
> independently from resize.

The target flow should become:

```text
pointer resize
  -> update WindowVisualTarget
  -> damage old/new visual/frame bounds
  -> queue/coalesce xdg configure
  -> render frame/decorations at visual target
  -> render committed content at ContentSampleTarget

client ACK + compatible commit
  -> update CommittedBufferTarget
  -> clear resize preview for that serial/size
  -> recompute ContentSampleTarget from committed logical size
  -> damage committed content + any visual delta

CPU/EGL/native GPU render
  -> consume the same RenderSurfaceView list
  -> CPU samples pixels, EGL/native GPU samples textures/dmabufs
  -> pageflip/presentation completion releases buffers and frame callbacks
```

## Phases

### Phase 0: Freeze Existing Contracts

Document and test the current intended behavior before changing geometry
ownership.

Implementation direction:

- Add tests around the existing CPU no-upscale preview behavior, left/top anchored
  resize, shrink, and final compatible commit.
- Record native CPU resize perf fields from the existing logs: scene rebuild
  kind, surface damage count, damage pixels, copy bytes, write bytes, pending
  pageflip/frame work.
- Keep browser/NVIDIA guardrails from
  `docs/research/agent-keystone-native-gpu-nvidia-plan.md`: native CPU scanout
  must keep dmabuf/sync globals hidden and browser GPU disabled while CPU
  composition is active.

Acceptance:

- Existing CPU render tests still pass.
- Native CPU mode still launches browsers with CPU policy.
- No new public Wayland globals are exposed in native CPU mode.

### Phase 1: Add Render-Plan Vocabulary Without Behavior Change

Create adapter-level structures that can be built from current
`RenderableSurface` without changing output pixels.

Implementation direction:

- Build a `RenderSurfaceView`/equivalent in the compositor or render module with
  named fields for visual target, content sample target, committed logical size,
  buffer size, frame/decor rects, and damage source.
- Populate it from current fields:
  `visual_target = surface.width/height`, `content_sample_target =
  resize_preview_content_target(...)`, `committed_logical_size =
  resize_preview.committed_* or surface.width/height`.
- Make CPU scene snapshots and native damage talk about the view type, even if
  the first patch still delegates to the old fields internally.

Acceptance:

- Byte-for-byte CPU output is unchanged for non-resize tests.
- Resize preview tests show the same no-upscale behavior as before.
- The EGL command builder can be pointed at the same view data in a later phase.

### Phase 2: Make No-Stretch a Shared Renderer Contract

Move resize preview content targeting out of the CPU-only blit path and into the
shared render plan.

Implementation direction:

- CPU `blit_surface_to_rect_clipped()` should consume a precomputed
  `ContentSampleTarget`, not recompute resize preview policy privately.
- EGL/GLES draw commands should use the same `ContentSampleTarget` for client
  texture quads, while frame/decor/background quads use `WindowVisualTarget` or
  `FrameDecorationTarget`.
- Keep legitimate viewport/buffer-scale scaling by marking the reason for any
  content target that differs from buffer size: `viewport`, `buffer_scale`,
  `output_scale`, `resize_preview_no_stretch`, or `explicit_policy`.

Acceptance:

- CPU and EGL render-plan tests agree on the target rects for normal, viewport,
  buffer-scale, fractional/output-scale, and interactive resize cases.
- A stale 800x600 browser/Gecko buffer previewed into a 1000x700 visual target
  is not upscaled; the exposed region is compositor-filled/decorated.
- A committed viewport destination still scales as specified by the protocol.

### Phase 3: Promote Frame/Decorations to First-Class Geometry

Make decorations independent from client buffer geometry before adding richer
resize visuals.

Implementation direction:

- Replace the current empty `server_frame_rects_for_surface()` placeholder with a
  frame/decor model that derives from `WindowVisualTarget`.
- Keep client content origin/clip separate from frame origin/clip.
- Damage frame/decor old and new bounds independently from client buffer damage.

Acceptance:

- Resize can move/resize frame/decorations immediately even if the client has not
  committed a new buffer.
- No stale pixels are left in the old frame/titlebar/background region.
- Frame/decor damage can collapse to full output when it becomes cheaper or safer.

### Phase 4: Damage Accumulator Union and Fallback Policy

Make damage describe output work, not just surface damage.

Implementation direction:

- Represent damage sources as old visual bounds, new visual bounds, content
  buffer damage mapped through `ContentSampleTarget`, frame/decor damage, shell
  overlay damage, software cursor damage, and full-scene fallback.
- Union/coalesce overlapping rects before using `copy_bytes` or GPU scissor
  damage as a performance signal.
- Collapse to full-output damage when summed or unioned damaged area crosses a
  threshold, when scene rebuild is full, or when output transform/scale math is
  ambiguous.

Acceptance:

- Native perf logs include unioned damaged pixels and rect count.
- Full scene rebuild never presents with partial stale output.
- Resize damages old and new visual bounds even when client surface damage is
  empty or delayed.

### Phase 5: Scale Contract Hardening

Separate all scale domains explicitly.

Implementation direction:

- Track buffer pixels, committed logical surface size, visual logical size, and
  output device pixels separately.
- Use viewporter destination and buffer scale only at commit time to derive
  committed logical content size.
- Use output scale only when mapping logical render views into output device
  coordinates.
- Never use visual resize size as a substitute for committed logical size.

Acceptance:

- Tests cover `wp_viewporter`, `wl_surface.set_buffer_scale(2)`, fractional scale
  announcement, output scale changes, and interactive resize at non-1.0 output
  scale.
- Damage mapping uses the same scale domain as the render target.
- KWin-style snapped logical-to-device mapping is deterministic for odd sizes.

### Phase 6: Presentation Lifecycle Split

Prepare resize/configure state before paint, but finish buffer release/frame
callbacks after real presentation.

Implementation direction:

- Keep configure flush/coalescing before render decisions.
- Tie frame callbacks, explicit-sync release points, presentation feedback, and
  buffer release to the output present completion boundary.
- For native CPU, that boundary is still DRM pageflip completion; for future
  native GPU, it is the same pageflip event after EGL render into a KMS buffer.

Acceptance:

- No frame callback is completed just because a resize configure was sent.
- Pageflip pending/no-ready-buffer cases are visible in perf logs.
- The native GPU/NVIDIA plan can consume this lifecycle without inventing a
  second completion model.

### Phase 7: Feed Native GPU From the Same Render Plan

Only after CPU/EGL plan semantics are shared, make native GPU consume them.

Implementation direction:

- Native GPU uses the same `RenderSurfaceView` list as CPU and nested EGL.
- SHM upload, dmabuf import, NVIDIA modifier validation, explicit sync acquire,
  and release signaling follow the existing native GPU/NVIDIA plan.
- Browser GPU policy remains disabled until dmabuf import, no-stretch resize,
  damage, and pageflip lifecycle acceptance all pass on the native path.

Acceptance:

- Browser surfaces are never CPU-composited/written when native GPU browser mode
  is enabled.
- Unsupported NVIDIA modifier/import combinations fall back before globals or
  browser GPU policy lie to clients.
- CPU fallback can be selected at startup without changing client-visible resize
  behavior.

## Gap Table

| Gap | Reference | Suggested implementation | Risk | Recommended order |
| --- | --- | --- | --- | --- |
| `RenderableSurface.width/height` mean both committed size and preview visual target | Hyprland `WindowTarget` visual/logical boxes; KWin `moveResizeGeometry()` | Add render-plan fields for `WindowVisualTarget`, `CommittedBufferTarget`, and `ContentSampleTarget`; initially adapt from current fields | Medium: broad naming churn if done as a deep refactor first | 1 |
| CPU no-upscale resize policy lives inside CPU blit | Hyprland `SurfacePassElement.cpp:26-50` | Move no-stretch target selection into shared render view consumed by CPU and EGL | High for future GPU/browser: stale browser buffers could stretch | 2 |
| EGL draw commands use `surface.width/height` directly | Hyprland keeps stale surface at committed size during interactive resize | Build EGL texture quads from `ContentSampleTarget`; draw frame/decor quads from visual target | High: native GPU would inherit wrong browser resize behavior | 2 |
| Frame/decorations are not first-class (`server_frame_rects_for_surface()` empty) | Hyprland updates decorations with visual target; KWin separates frame/client size | Add frame/decor target and damage independent from client buffer target | Medium: visible stale pixels if damage is incomplete | 3 |
| Damage scales buffer damage through preview `surface.width/height` | KWin maps logical to device at output boundary; Hyprland damages old/new target | Map content damage through content target; separately damage old/new visual/frame bounds | High: under-damage causes artifacts; over-damage hurts perf | 4 |
| Overlapping damage metrics can exceed frame bytes | Local `agent-gauge` resize/damage profiling | Union/coalesce rects and report unioned damaged pixels before copy/write metrics | Low visually, medium for perf decisions | 4 |
| Scale domains are implicit | KWin output scale/view scale; Hyprland viewport/fractional UV handling | Track buffer pixels, committed logical size, visual logical size, and output pixels separately | High: fractional/NVIDIA/browser bugs are hard to debug later | 5 |
| Presentation lifecycle is mixed with render preparation | Hyprland commits explicit sync after damage/render; KWin `OutputFrame` lifecycle | Separate configure flush, render prepare, pageflip completion, frame callbacks, sync release | High for explicit sync and browser smoothness | 6 |
| Native GPU could bypass CPU safety assumptions | KWin layer test/downgrade; existing Keystone GPU/NVIDIA plan | Feed native GPU from same render plan and keep browser GPU disabled until import/present/no-stretch pass | Very high on NVIDIA/browser | 7 |

## Ownership Split

- `compositor`: owns protocol state, ACK/configure tracking, committed buffer
  metadata, viewport/buffer-scale commit state, explicit sync acquire/release
  objects, and the logical render-view description.
- `compositor::render`: owns CPU scene composition, render-plan snapshots,
  output-scale mapping helpers, and CPU damage tests.
- `egl_renderer`: owns GL texture resources, dmabuf/shm import/upload, draw
  command generation from render views, EGL damage submission, and texture target
  correctness.
- `native_output`: owns KMS/libinput/session orchestration, native output damage
  conversion, pageflip lifecycle, frame pacing, and CPU/GPU backend selection.
- `launch_env`: owns browser CPU/GPU policy and must remain conservative until
  native GPU acceptance gates pass.

Do-not-touch boundaries for the incremental work:

- Do not change public xdg configure ACK semantics unless a separate protocol
  migration is opened.
- Do not expose native dmabuf/sync globals in CPU scanout mode.
- Do not remove browser CPU launch guards while the active native backend writes
  CPU-composited frames.
- Do not make frame/decorations depend on client commit cadence.

## Compatibility and Rollback

Compatibility path:

- CPU remains the default native path.
- Existing `ResizePreview` behavior is preserved until the shared render view is
  tested.
- Full-output damage remains the safety fallback for uncertain scale/transform,
  full-scene rebuild, or excessive rect complexity.
- Browser acceleration remains gated by the native GPU/NVIDIA plan.

Rollback path:

- Each phase can fall back to the current CPU render path by deriving render
  views from `RenderableSurface` and forcing `content_sample_target =
  surface_target`.
- Damage work can be rolled back to full-output damage without changing protocol
  behavior.
- Native GPU can be disabled at backend selection while keeping the shared
  CPU/EGL resize semantics.

## Validation Gates

Required tests/checks before implementation is considered safe:

- CPU resize preview:
  stale smaller committed buffer is not upscaled during grow; left/top anchored
  resize positions stale content correctly; shrink clips safely; final ACKed
  compatible commit clears preview.
- Scale:
  viewport destination, buffer scale, fractional-scale announcement, output scale,
  and odd-size snapping map to expected output pixels.
- Damage:
  old visual bounds, new visual bounds, content buffer damage, frame/decor damage,
  and full-scene rebuild all produce safe output damage; rect union metrics are
  reported.
- EGL parity:
  CPU and EGL render plans produce the same target rects for resize/scale cases
  before native GPU consumes them.
- Native lifecycle:
  pageflip pending/completed states are logged; frame callbacks and explicit-sync
  release points are completed only after presentation completion.
- Browser/NVIDIA:
  native CPU mode still hides dmabuf/sync globals and keeps browser GPU disabled;
  native GPU mode only advertises modifier/import combinations proven by the EGL
  native renderer.

Suggested command groups when production work starts:

- `cargo test surface_frames resize`
- `cargo test compositor::render`
- `cargo test input_output`
- native perf smoke with resize begin/update/end, scene rebuild, unioned damage,
  copy/write bytes, pageflip pending/completed

## Final Recommendation

Do not start the native GPU/browser enablement by teaching the renderer to scale
the current `RenderableSurface` harder. Start by making resize/scale geometry
explicit and shared. The first production milestone should be a render-view
adapter that preserves current CPU pixels while naming the separate targets. The
second should make no-stretch resize a renderer contract shared by CPU and EGL.
Only after damage and scale domains are explicit should native GPU/NVIDIA browser
surfaces consume the path.

This keeps the current CPU path safe, avoids lying to NVIDIA/browser clients, and
turns Hyprland/KWin's useful lesson into an Oblivion-native invariant:
interactive resize owns the visual target, client commits own the buffer target,
and presentation owns release/completion.

## Evidence

- `src/compositor/surface.rs`
- `src/compositor/mod.rs`
- `src/compositor/render.rs`
- `src/compositor/state_data.rs`
- `src/egl_renderer.rs`
- `src/native_output.rs`
- `docs/research/agent-gauge-resize-damage-perf.md`
- `docs/research/agent-raman-hyprland-resize-gecko.md`
- `docs/research/agent-keystone-native-gpu-nvidia-plan.md`
- `WM para Referencia/Hyprland-main/src/layout/target/WindowTarget.cpp`
- `WM para Referencia/Hyprland-main/src/desktop/view/Window.cpp`
- `WM para Referencia/Hyprland-main/src/render/pass/SurfacePassElement.cpp`
- `WM para Referencia/Hyprland-main/src/render/Renderer.cpp`
- `WM para Referencia/kwin-master/src/xdgshellwindow.cpp`
- `WM para Referencia/kwin-master/src/compositor.cpp`

Changes: this report file only.

Validation: source/reference inspection only; no production tests were run
because this was a read-only architecture/documentation task.

Risks: the largest remaining risk is hidden divergence between CPU and EGL
geometry during the future shared-render-view migration. Treat CPU/EGL parity
tests as mandatory before enabling native GPU browser acceleration.
