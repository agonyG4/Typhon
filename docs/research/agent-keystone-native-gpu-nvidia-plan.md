# Agent Keystone Native GPU/NVIDIA Plan

Date: 2026-06-11

Scope: staged architecture plan for Oblivion One's native GPU path, focused on
NVIDIA, browser surfaces, dmabuf feedback/import, explicit sync, KMS pageflip
lifecycle, and safe fallback behavior.

This is a read-only architecture report. It does not change production code.

## Current Architecture

Oblivion already has most of the protocol and renderer pieces needed for a
native GPU path, but they are currently split across the nested EGL/GLES output
and the native CPU scanout path.

The current native execution flow is:

1. `src/main.rs` selects the native backend and intentionally binds the server
   through `OwnCompositorServer::bind_cpu_composition()`, printing
   `gpu buffer protocols: disabled for native CPU scanout`
   (`src/main.rs:403-408`).
2. `OwnCompositorServer::bind_cpu_composition()` registers the minimum Wayland
   globals with `gpu_buffers_enabled=false`, hiding `zwp_linux_dmabuf_v1`,
   `wp_linux_drm_syncobj_manager_v1`, and `wl_drm`
   (`src/compositor/server.rs:31-52`, `src/compositor/server.rs:253-280`).
3. Native app launch uses `spawn_cpu_compositor_app()` and applies software
   rendering guards such as `MOZ_WEBRENDER_SOFTWARE=1`,
   `LIBGL_ALWAYS_SOFTWARE=1`, `WEBKIT_DISABLE_DMABUF_RENDERER=1`, and Chromium
   `--disable-gpu*` switches (`src/native_output.rs:3057`,
   `src/launch_env.rs:318-323`, `src/launch_env.rs:390-412`).
4. `NativeGbmScanout` creates writable linear `XRGB8888` GBM scanout buffers
   with `SCANOUT | WRITE | LINEAR`, renders a CPU frame, copies it into a
   staging buffer, calls `bo.write()`, then schedules a legacy pageflip
   (`src/native_output.rs:3997-4053`, `src/native_output.rs:4070-4091`,
   `src/native_output.rs:4120-4145`).
5. Pageflip completion is sampled and counted, then the pending GBM buffer
   becomes current (`src/native_output.rs:4151-4159`,
   `src/native_output.rs:4256-4290`). The event metadata is not yet the
   compositor's presentation-completion boundary.

The reusable GPU pieces are:

- `EglGlesFrameRenderer` already imports shm and dmabuf-backed surfaces into GL
  resources, tracks failed surface generations, and avoids CPU pixels for dmabuf
  input (`src/egl_renderer.rs:435-512`, `src/egl_renderer.rs:880-985`).
- EGL dmabuf feedback is queried from the renderer's EGL display, with NVIDIA
  block-linear modifier detection and extra unindexed NVIDIA format-table
  entries (`src/egl_renderer/dmabuf.rs:38-105`,
  `src/egl_renderer/dmabuf.rs:141-175`).
- `EglGlesDmabufImportAttributes::from_handle()` builds modifier-aware
  `EGL_LINUX_DMA_BUF_EXT` attributes for every plane
  (`src/render_backend/egl_gles.rs:253-305`).
- The import plan rejects dmabuf buffers whose format/modifier pair is not
  advertised in renderer feedback (`src/render_backend/egl_gles.rs:62-84`).
- The compositor can publish renderer-derived dmabuf feedback with main device,
  format table, tranche target device, tranche formats, and `done`
  (`src/compositor/mod.rs:262-275`, `src/compositor/dmabuf.rs:85-180`).
- Explicit sync protocol state exists: clients can import timelines, set
  acquire/release points, unsignaled acquire points can defer commits, and
  release points are signaled after the compositor's current presentation path
  says the frame is done (`src/compositor/explicit_sync.rs:28-73`,
  `src/compositor/explicit_sync.rs:112-189`,
  `src/syncobj.rs:22-88`).

The key mismatch is therefore not protocol availability. The mismatch is that
native scanout cannot yet render/import browser dmabufs without falling back to
CPU composition and full `gbm_bo_write`.

## Problem Boundary

This plan targets:

- native GPU composition into KMS-presentable buffers;
- dmabuf feedback derived from the actual native EGL/GBM render device;
- NVIDIA modifier handling without lying to browsers;
- explicit sync acquire/release tied to real presentation lifecycle;
- pageflip metadata as the completion boundary;
- browser enablement only after the renderer can safely import and display
  browser buffers;
- safe fallback to the current CPU path.

This plan does not target:

- Vulkan renderer work;
- XWayland runtime support;
- shell redesign;
- portal/session cleanup unrelated to GPU import;
- direct scanout as the first browser milestone.

## Target Direction

The native GPU path should become an output-owned render target:

```text
Wayland client commit
  -> compositor validates shm/dmabuf/sync state
  -> native renderer imports or updates surface resources
  -> native output damage selects draw work
  -> EGL/GLES renders into GBM-backed KMS buffer
  -> KMS pageflip is submitted
  -> DRM pageflip event completes frame lifecycle
  -> buffers are released, syncobj release points signaled,
     wl_surface.frame callbacks completed, and presentation feedback sent
```

The important ownership split:

- `compositor`: protocol state, surfaces, buffer handles, damage metadata,
  frame callbacks, presentation feedback requests, explicit sync points.
- `render_backend` / native renderer: EGL display/context, GBM render targets,
  dmabuf import, SHM upload, GL resources, fences, draw commands, output damage.
- `native_output`: KMS/libseat/libinput orchestration, mode selection, pageflip
  event source, backend selection, fallback routing.
- `launch_env`: browser CPU/accelerated policy, switched only when native GPU
  readiness is proven.

Do not enable GPU buffer globals or remove browser CPU guards until native GPU
composition can import dmabuf surfaces and preserve frame lifecycle correctly.

## Phases

### Phase 0: Preserve the Guard and Baseline

Keep the native CPU path as the default.

Required behavior:

- native still uses `bind_cpu_composition()` while the active scanout producer
  is `NativeFrameRenderer`;
- native browser launches still use CPU policy;
- `zwp_linux_dmabuf_v1`, `wp_linux_drm_syncobj_manager_v1`, and `wl_drm` remain
  hidden in native CPU mode;
- perf logs continue to report `render_us`, `copy_us`, `write_us`, `bytes`,
  `cursor`, `surfaces`, and `render_generation`.

Risks:

- none operationally; this phase is protection against accidental optimistic
  browser enablement.

Acceptance criteria:

- `./bin/oblivion-one compositor --check` in native mode still prints the native
  CPU scanout guard.
- A native protocol listing does not expose dmabuf, wl_drm, or syncobj globals.
- Browser launches from native still contain CPU guards.
- Existing native CPU scanout remains usable.

### Phase 1: Native EGL/GBM Device Probe

Add a native GPU probe that does not render real clients yet.

Required behavior:

- open EGL on the same DRM/render device family used for native KMS scanout;
- query EGL vendor/renderer/version, dmabuf formats/modifiers, main device, and
  whether `EGL_EXT_image_dma_buf_import_modifiers`,
  `GL_OES_EGL_image`, native fences, and surfaceless/pbuffer context creation
  are available;
- reuse `query_egl_dmabuf_feedback()` instead of hand-writing a linear-only
  table;
- record whether NVIDIA block-linear modifiers were detected;
- do not advertise the result to clients yet.

Risks:

- choosing the wrong render node can produce feedback for a GPU that cannot
  import or present on the selected KMS device;
- NVIDIA may expose modifiers that EGL can import but KMS cannot scan out after
  composition.

Acceptance criteria:

- perf or diagnostic output includes native EGL vendor/renderer, main device,
  modifier count, tranche count, and NVIDIA modifier presence;
- the probe can fail without breaking CPU scanout;
- no client-visible protocol globals change in this phase;
- tests cover fallback when dmabuf feedback is empty or main device is missing.

### Phase 2: Render a Native GPU Test Frame

Create a native EGL/GLES render target that can draw a simple frame into a
GBM-backed buffer and pageflip it.

Required behavior:

- allocate GBM buffers suitable for rendering and scanout, preferring modifier
  aware allocation when available;
- create an EGL context and render target independent of the nested Wayland
  EGL window;
- draw wallpaper/test quads only;
- register the rendered BO as a KMS framebuffer and pageflip it;
- keep the existing CPU GBM write and dumb framebuffer fallbacks.

Risks:

- wrong BO usage/modifier choice can produce `addfb2` failure or black frames;
- NVIDIA may require explicit handling for renderable versus scanout-compatible
  modifiers;
- surfaceless/pbuffer context setup may vary by driver.

Acceptance criteria:

- `OBLIVION_ONE_SCANOUT_BACKEND=native-gpu-test` can show a stable test frame;
- failure automatically falls back to the existing CPU scanout backend unless a
  strict debug mode is requested;
- pageflip completion still restores the previous CRTC on exit;
- perf logs distinguish CPU write scanout from GPU render scanout.

### Phase 3: Share the Existing EGL Scene Renderer

Refactor toward one EGL/GLES scene implementation with two present targets:
nested Wayland and native GBM/KMS.

Required behavior:

- reuse shader/program setup, textured quad generation, SHM texture upload,
  dmabuf EGLImage import, shell overlay texture handling, cursor fallback
  texture handling, and surface resource lifetime from `src/egl_renderer.rs`;
- separate "draw scene" from "present target":
  - nested target: Wayland `EGLSurface` + `eglSwapBuffersWithDamage`;
  - native target: GBM/FBO or GBM EGL surface + KMS framebuffer/pageflip;
- preserve failed-surface-generation logging so a broken dmabuf import does not
  spam every frame;
- keep CPU renderer available as a separate fallback path, not as part of the
  native GPU hot path.

Risks:

- over-sharing target-specific code can make nested output less stable;
- under-sharing can fork dmabuf import behavior and recreate NVIDIA bugs in one
  path only.

Acceptance criteria:

- nested EGL behavior is unchanged by default;
- native GPU path can render SHM surfaces through GL without CPU output writes;
- a dmabuf import failure affects only that surface/generation and falls back
  safely instead of blanking the compositor;
- unit tests cover target-independent import planning and target-specific
  fallback selection.

### Phase 4: Enable Native Dmabuf Import for Test Clients

Enable native GPU buffer globals only for a native GPU backend that can import
and draw dmabuf surfaces.

Required behavior:

- bind `zwp_linux_dmabuf_v1` and `wl_drm` only after native EGL feedback is
  derived from the active native render device;
- bind `wp_linux_drm_syncobj_manager_v1` only when a DRM timeline syncobj device
  is available and the renderer lifecycle can honor acquire/release semantics;
- set compositor dmabuf feedback from the native renderer, including NVIDIA
  modifier handling;
- draw dmabuf-backed `RenderableSurface` instances through the existing
  EGLImage path;
- keep SHM fallback working.

Risks:

- advertising a modifier that native EGL can import but native KMS cannot
  present can lead to black browser content or GPU-process restart loops;
- advertising only linear modifiers may work for toy clients but is not a real
  NVIDIA browser path;
- wl_drm compatibility is linear-prime-only today, so it must remain secondary
  to linux-dmabuf feedback.

Acceptance criteria:

- a test client can create a dmabuf buffer, commit it, and see it on native GPU
  scanout;
- renderer feedback main device matches the native EGL device, not a guessed
  `/dev/dri/renderD128`;
- format/modifier rejection is explicit and logged;
- native CPU backend still hides GPU buffer globals;
- tests cover configured feedback, unsupported modifier rejection, and SHM to
  dmabuf replacement.

### Phase 5: Tie Explicit Sync to Real Frame Lifecycle

Move explicit sync release and presentation feedback from "loop decided to
present" to "pageflip completed".

Required behavior:

- split the current `present_frame()` shape into at least:
  - prepare protocol work before rendering;
  - finish presented frame after KMS completion;
- acquire points must gate dmabuf commit availability before import/draw;
- release points must be signaled only after the frame that last used the
  buffer has completed or after a safe discard path;
- presentation feedback must use DRM pageflip timestamp/sequence/flags where
  available;
- if pageflip submission fails, discard or defer feedback rather than sending
  false hardware-completion metadata.

Risks:

- early release can let browsers reuse a buffer while the compositor/GPU still
  samples it;
- late release can stall Chromium/Gecko GPU pipelines;
- incorrect feedback can destabilize client frame pacing at 165 Hz.

Acceptance criteria:

- tests can inject a fake pageflip completion and prove frame callbacks,
  presentation feedback, normal buffer release, and syncobj release happen
  afterward;
- no release point is signaled for a frame that never submitted or completed;
- no-damage visible clients can still receive frame progress without a full
  repaint;
- native perf logs include queued pageflip sequence, completion timestamp,
  render start/end, and missed-deadline markers.

### Phase 6: Turn On Accelerated Browser Policy Behind Readiness

Only after native dmabuf import and presentation lifecycle are working, relax
browser CPU guards.

Required behavior:

- native launch policy becomes conditional:
  - CPU compositor policy for CPU scanout/fallback;
  - accelerated policy for native GPU-ready backend;
- Gecko native GPU path removes `MOZ_WEBRENDER_SOFTWARE=1` and
  `LIBGL_ALWAYS_SOFTWARE=1`;
- Chromium native GPU path removes `--disable-gpu*` and `--disable-zero-copy`,
  while keeping conservative Wayland/Ozone and Vulkan-disabled defaults until
  Vulkan is intentionally supported;
- readiness must be runtime-derived from native renderer capabilities, not from
  a user-facing wish flag alone.

Risks:

- Chromium GPU process can restart-loop if modifiers, ANGLE, sync, or feedback
  disagree with driver reality;
- Gecko may fall back silently unless WebRender/dmabuf diagnostics are captured;
- browser profiles can retain bad state from earlier failed GPU attempts.

Acceptance criteria:

- Zen/Firefox launches without software-rendering guards and presents dmabuf or
  GPU-backed surfaces inside Oblivion native;
- Brave/Chromium launches without `--disable-gpu*` and does not emit repeated
  GPU-process initialization failures;
- perf logs show no full-frame CPU `bo.write()` path for browser steady-state
  frames;
- fallback to CPU policy is automatic if native GPU backend fails readiness.

### Phase 7: Damage-Aware Native GPU Rendering

Make native GPU composition avoid unnecessary texture uploads and draw work.

Required behavior:

- transform `RenderableSurfaceDamage` into output-space damage;
- update SHM textures by damage rects when the resource is reusable;
- keep dmabuf resource reuse keyed by surface generation and buffer identity;
- damage old/new bounds for move/resize;
- damage shell overlay regions and software cursor fallback rects;
- fall back to full output damage on unknown transforms, rect overflow, output
  mode change, scale change, stacking ambiguity, or failed import transition.

Risks:

- stale pixels are more damaging than extra work; full damage fallback must be
  cheap and obvious;
- browser resize can mix small damage, buffer replacement, subsurfaces, and
  presentation feedback in the same frame.

Acceptance criteria:

- damage debug/perf logs show reason, rect count, and fallback-to-full reason;
- SHM and dmabuf surfaces remain visually correct through move, resize, popup,
  and browser scrolling;
- no stale pixels during interactive resize;
- p95 steady browser frame cost drops below the selected refresh budget on the
  target NVIDIA session, or the remaining bottleneck is measured as GPU/driver
  time rather than CPU copy/write.

### Phase 8: Conservative Direct Scanout / Overlay

Direct scanout is not the first browser GPU milestone. Add it only after normal
GPU composition is stable.

Required behavior:

- attempt direct scanout before normal composition only for a narrow candidate:
  one fullscreen visible dmabuf surface, no shell overlay, no software cursor,
  no active effects, exact output transform/scale/source rect, compatible
  format/modifier, and safe color pipeline;
- test-only or opt-in at first;
- log rejection reasons for every candidate.

Risks:

- wrong acceptance can hide the shell, show stale browser content, or scan out a
  buffer whose color/transform does not match the composed result;
- NVIDIA modifier/KMS plane compatibility can differ from EGL import
  compatibility.

Acceptance criteria:

- direct scanout never activates when dock/topbar/Spotlight/overlay is visible;
- fallback to normal GPU composition is seamless when the browser exits
  fullscreen;
- presentation feedback and buffer release still follow pageflip completion;
- rejection logs make it clear why a candidate did or did not scan out.

## NVIDIA-Specific Requirements

Treat NVIDIA as a modifier and synchronization correctness problem, not as a
special "force linear" path.

Requirements:

- use EGL-reported modifier feedback from the active native EGL display;
- preserve existing NVIDIA block-linear modifier detection and unindexed format
  table handling from `src/egl_renderer/dmabuf.rs`;
- include `DRM_FORMAT_MOD_INVALID` in the tranche only when it matches the
  existing NVIDIA implicit-modifier logic;
- do not advertise broad linear support unless native import and presentation
  actually support it;
- keep Vulkan disabled for Chromium until a Vulkan renderer/swapchain path is
  deliberately designed;
- prefer explicit sync when available; otherwise make implicit-sync fallback a
  logged compatibility mode, not the primary NVIDIA story;
- distinguish EGL import success from KMS present success in diagnostics.

NVIDIA acceptance criteria:

- startup diagnostics identify NVIDIA renderer/vendor and modifiers;
- feedback includes usable non-linear modifier entries when the driver exposes
  them;
- a rejected modifier logs `format`, `modifier`, source surface id, and reason;
- browser acceleration tests do not require forcing software rendering;
- CPU fallback remains one environment variable or backend selection away.

## Compatibility and Rollback

Do not break these public/runtime contracts:

- `OBLIVION_ONE_SCANOUT_BACKEND`
- `OBLIVION_ONE_CURSOR`
- `OBLIVION_ONE_MODE`
- `OBLIVION_ONE_INPUT_BACKEND`
- `OBLIVION_ONE_PERF_LOG`
- current native CPU composition behavior
- nested EGL/GLES behavior
- `OwnCompositorServer::bind_cpu_composition()` as the safe native guard

Recommended fallback layers:

1. native GPU renderer disabled -> current GBM CPU write path;
2. GBM CPU write path unavailable -> dumb framebuffer path;
3. hardware cursor failure -> software cursor with damage/full repaint fallback;
4. dmabuf import failure -> keep surface unsupported for that generation or
   require SHM fallback, but do not display stale content as if import worked;
5. explicit sync unavailable -> hide syncobj global or use logged implicit sync
   compatibility only where safe;
6. browser GPU readiness false -> CPU launch policy.

Rollback must be possible at runtime. Avoid requiring source reverts to recover
a login session.

## Ownership Split

Suggested work scopes for parallel agents:

- Native renderer/device agent:
  owns EGL display/context, GBM render target, KMS framebuffer import, and
  renderer diagnostics. Write scope: `src/render_backend/`, optional
  `src/native_renderer/`, narrow calls from `src/native_output.rs`.
- Protocol lifecycle agent:
  owns prepare/finish split, pageflip metadata, frame callbacks, presentation
  feedback, normal buffer release, explicit sync release. Write scope:
  `src/compositor/server.rs`, `src/compositor/mod.rs`,
  `src/compositor/explicit_sync.rs`, tests.
- Browser policy agent:
  owns readiness-gated launch environment and browser flags. Write scope:
  `src/launch_env.rs`, `src/native_output.rs` call site, tests.
- Damage agent:
  owns output damage accumulator, transform rules, stale-pixel fallback, perf
  counters. Write scope: new damage module plus narrow renderer/native_output
  integration.

Do-not-touch boundaries:

- Do not vendor reference compositor code.
- Do not route browsers to host `DISPLAY`.
- Do not expose dmabuf/syncobj in native CPU mode.
- Do not remove CPU fallback while native GPU is experimental.
- Do not mix WM policy into renderer modules.

## Validation Gates

Use these gates in order:

1. Readiness diagnostics:
   - native EGL device opens;
   - dmabuf feedback has a main device and non-empty tranche;
   - NVIDIA modifier state is logged;
   - CPU scanout still works when diagnostics fail.
2. Renderer smoke:
   - test frame renders through native GPU target;
   - KMS pageflip completes;
   - CRTC restore works.
3. SHM correctness:
   - existing SHM clients render through native GPU;
   - no CPU output write in the GPU path;
   - software cursor fallback remains visible.
4. Dmabuf correctness:
   - dmabuf test client visible;
   - unsupported modifier rejected;
   - buffer replacement releases only after completion.
5. Explicit sync correctness:
   - acquire waits/deferred commits work;
   - release points signal after pageflip completion;
   - feedback uses pageflip metadata.
6. Browser readiness:
   - Gecko accelerated native launch without software guards;
   - Chromium accelerated native launch without GPU disable flags;
   - no GPU-process restart loop;
   - frame cost no longer includes full CPU write.
7. Damage and performance:
   - no stale pixels through resize/scroll/popup;
   - p95 frame time target tracked against selected refresh;
   - full-damage fallback reasons are visible in logs.

Recommended commands and checks:

```sh
cargo test native_gpu --bin oblivion-one
cargo test protocol_buffers --lib
cargo test compositor::tests --lib
./bin/oblivion-one compositor --check
./bin/oblivion-one compositor -- wayland-info
OBLIVION_ONE_DRY_RUN=1 ./bin/start-oblivion-one
OBLIVION_ONE_PERF_LOG=1 OBLIVION_ONE_SCANOUT_BACKEND=native-gpu start-oblivion-one-tty
grep '^perf ' ~/.local/state/oblivion-one/session.log
```

The exact test names will depend on implementation, but the gates should remain
the release criteria.

## Final Target

The target native NVIDIA path is:

- native renderer derives feedback from the actual EGL/GBM device;
- browsers receive accurate dmabuf/modifier/sync capabilities;
- Gecko and Chromium can submit GPU buffers without software guards;
- Oblivion imports those dmabufs as EGLImages and composites them on GPU;
- KMS pageflip completion drives buffer release, syncobj release, frame
  callbacks, and presentation feedback;
- CPU composition and full `gbm_bo_write` remain available only as fallback,
  diagnostics, or explicit compatibility mode.

When this is true, native browser surfaces stop being a CPU framebuffer problem
and become normal compositor GPU resources with a measured, reversible NVIDIA
path.
