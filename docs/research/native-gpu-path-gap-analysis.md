# Native GPU Path Gap Analysis

Date: 2026-06-11

Scope: native KMS/GBM scanout after the hardware cursor backend became active
in a real session. This is an investigation/design note only.

## Summary

The newest native-session evidence shows the cursor plane is no longer the main
problem: the real-session log reports `native cursor backend active: hardware
(64x64)`, `perf native.cursor backend=hardware`, and `cursor=hardware` on all
frames. Even with pointer-only repaint removed from the hot path, the latest
capture still reports about `2965` frames with average `paint_us` near `12288`,
p95 near `15679`, and `96.7%` of frames above the `165 Hz` budget of about
`6060 us`.

The remaining bottleneck is the native scanout path itself. `NativeGbmScanout`
uses KMS pageflips, but its frame producer is still CPU composition plus a
full-frame CPU upload into writable linear GBM BOs. The project already has an
EGL/GLES renderer, dmabuf metadata types, dmabuf feedback, modifier-aware
EGLImage import, explicit sync plumbing, and output damage logic for the nested
Wayland renderer. Those pieces are not yet connected to the native KMS scanout
target.

## Biggest Likely Costs

1. Full-frame GBM write on every repaint.
   `NativeGbmScanout::create()` allocates `Xrgb8888` BOs with
   `SCANOUT | WRITE | LINEAR`, and `paint_server_frame()` writes the whole BO
   with `buffer.bo.write(&self.staging)`.

2. CPU scene generation before scanout.
   `paint_server_frame()` calls `NativeFrameRenderer::render_server_frame()`,
   which reaches `DesktopSceneRenderer::compose_request()`. That path rebuilds
   or copies a CPU scene, blends shell overlay pixels, and optionally draws the
   software cursor.

3. ARGB-to-XRGB staging copy.
   The native renderer keeps a `Vec<u8>` staging buffer sized to full scanout
   pitch times height, converts/copies the CPU `u32` frame into it, then writes
   that full staging buffer to GBM.

4. DMABUF import exists, but not in native scanout.
   `RenderableSurface` can carry `DmabufBufferHandle` without CPU pixels, and
   the nested EGL/GLES renderer can import it with `EGL_LINUX_DMA_BUF_EXT`.
   The CPU compositor path in `src/compositor/render.rs` only draws
   `surface.cpu_pixels()`, so DMABUF content needs the EGL renderer to be useful
   on native scanout.

5. Frame completion is still not strictly pageflip-bound.
   `OwnCompositorServer::present_frame()` releases buffers, completes frame
   callbacks, and sends presentation feedback in one method. The native loop
   drains pageflip events and schedules pageflips, but compositor frame
   completion is still coupled to loop-level frame work rather than a completed
   KMS flip boundary.

## Confirmed Versus Hypothesized Costs

Confirmed:

- Hardware cursor is active in the latest user-provided capture, so cursor
  drawing is no longer the likely explanation for the current `paint_us`.
- Native GBM scanout uses full-frame CPU write: `src/native_output.rs` stores
  `staging: Vec<u8>` in `NativeGbmScanout`, creates writable linear XRGB scanout
  BOs, renders a CPU frame, fills staging, and calls `bo.write()`.
- Native paint stats explicitly measure `paint_us`, `render_us`, `copy_us`, and
  `write_us`, which matches the previous log analysis where render/copy/write
  were all visible costs.
- CPU compositor composition is full-scene oriented: `compose_request()`
  rebuilds/copies the scene, blends shell overlay, and then draws cursor when
  requested.
- `blit_surface_to_rect()` only consumes `surface.cpu_pixels()`. A DMABUF-backed
  surface has no CPU pixels by design and is skipped by this CPU path.
- `wl_shm` commit handling already has damage-aware CPU reads for same-size SHM
  updates, but that only reduces client-buffer ingestion. It does not stop the
  native output from writing the whole monitor BO.
- The nested EGL/GLES renderer already has real GL composition, damage-aware
  `eglSwapBuffersWithDamage`, SHM texture upload, and DMABUF EGLImage import.

Hypothesized until measured in the current capture:

- `gbm::BufferObject::write()` is probably the largest steady-state single cost
  on NVIDIA because it uploads about 8.3 MiB per 1920x1080 frame into a linear
  scanout BO, but the latest summary only included aggregate `paint_us`.
- CPU render and staging copy likely split the remaining frame time with GBM
  write. Use existing `render_us`, `copy_us`, and `write_us` perf fields to
  confirm the exact shares in the 2965-frame capture.
- NVIDIA modifier/implicit-sync behavior may constrain the final GBM/EGL path.
  The nested renderer already contains NVIDIA-specific feedback handling, but
  native scanout must separately prove KMS accepts the rendered BOs and
  modifiers.

## Evidence Map

### Native GBM Is Pageflip, But Not GPU Render

- `src/native_output.rs:3625-3636` defines `NativeGbmScanout` with a `staging`
  CPU byte buffer.
- `src/native_output.rs:3644-3662` creates three GBM BOs as
  `gbm::Format::Xrgb8888` with `SCANOUT | WRITE | LINEAR`, then registers KMS
  FBs with `add_gbm_framebuffer()`.
- `src/native_output.rs:3685-3727` renders through `NativeFrameRenderer`, copies
  ARGB CPU pixels into full-size XRGB staging, calls `buffer.bo.write()`, and
  records render/copy/write timings.
- `src/native_output.rs:3742-3771` pageflips the ready FB ID with
  `DRM_MODE_PAGE_FLIP_EVENT`.
- `src/native_output.rs:3774-3783` drains DRM pageflip events and marks the
  pending buffer as current.

### Hardware Cursor Is Separated

- `src/native_output.rs:3271-3349` creates a GBM cursor BO, writes the cursor
  texture once, and uses legacy DRM `set_cursor`/`move_cursor`.
- The latest real-session log confirms hardware mode. That removes the
  software cursor overlay from the native composed frame during normal cursor
  motion, but does not change the CPU full-frame scanout path for real repaints.

### CPU Composition Path Still Assumes CPU Pixels

- `src/compositor/render.rs:108-135` runs `compose_request()`: rebuild scene,
  copy scene to frame, blend shell overlay, draw cursor if present.
- `src/compositor/render.rs:170-185` copies wallpaper to the CPU scene and draws
  all client surfaces each time the scene cache is invalidated.
- `src/compositor/render.rs:188-194` copies the scene into the output frame.
- `src/compositor/render.rs:643-723` blits and scales surfaces from
  `surface.cpu_pixels()`. DMABUF-backed surfaces return `None` here.

### SHM And DMABUF Protocol Pieces Exist

- `src/compositor/shm.rs:55-95` reads full SHM buffers into CPU pixels and
  normalizes formats to ARGB.
- `src/compositor/shm.rs:97-156` can read only damaged SHM rects for same-size
  updates.
- `src/compositor/dmabuf.rs:55-78` advertises dmabuf formats/modifiers.
- `src/compositor/dmabuf.rs:158-179` sends v4-style feedback with format table,
  main device, tranche target device, and tranche formats.
- `src/compositor/dmabuf.rs:275-313` validates params and builds
  `DmabufBufferHandle`.
- `src/compositor/state_data.rs:507-517` converts pending SHM to
  `ShmSnapshot` and pending DMABUF to `DmabufHandle`.
- `src/render_backend/buffer.rs:181-233` keeps committed SHM and DMABUF buffers
  typed separately; DMABUF intentionally has no CPU pixels.

### EGL/GLES Renderer Exists, But Targets Nested Wayland

- `src/egl_renderer.rs:95-223` creates an EGL/GLES context from a Wayland window
  and stores GL textures/resources.
- `src/egl_renderer.rs:237-284` draws an EGL scene with output damage tracking.
- `src/egl_renderer.rs:435-512` syncs surface resources and recreates/imports
  changed surfaces.
- `src/egl_renderer.rs:652-700` draws textured layers and swaps with
  `eglSwapBuffersWithDamage` when available.
- `src/egl_renderer.rs:880-985` creates either uploaded SHM textures or DMABUF
  EGLImage-backed textures.
- `src/egl_renderer.rs:987-1135` still packs SHM ARGB pixels to RGBA for GL
  uploads, but only for SHM input; DMABUF import avoids CPU pixels.
- `src/egl_renderer/dmabuf.rs:38-106` queries EGL dmabuf formats/modifiers and
  includes NVIDIA modifier handling.
- `src/nested_renderer.rs:81-178` wires this EGL renderer only into the nested
  output renderer, with CPU fallback.

### Target Capabilities Are Declared

- `Cargo.toml` already depends on `glow`, `gbm`, `khronos-egl`,
  `wayland-egl`, DRM crates, and Wayland protocols.
- `src/render_backend/mod.rs:43-78` declares the intended EGL/GLES capabilities:
  GPU composition, SHM import fallback, damage-tracked SHM upload,
  modifier-aware DMABUF import, DMABUF feedback, explicit sync, direct scanout,
  and multi-GPU import.
- `src/render_backend/egl_gles.rs:253-305` builds
  `EGL_LINUX_DMA_BUF_EXT` attributes from a `DmabufBufferHandle`.

## Technical Proposal

### P0: Instrument The Current Native Cost Split

Before changing behavior, preserve a baseline from the latest hardware-cursor
session:

- Extract `paint_us`, `render_us`, `copy_us`, `write_us`, `bytes`,
  `surfaces`, and `cursor`.
- Group by idle, browser animation, resize, and shell interaction.
- Confirm no pointer-only hardware-cursor motion emits repaint-rate full frames.

This turns the 2965-frame summary into a before/after table for each
implementation step.

### P1: Introduce A Native EGL/GBM Renderer Skeleton

Create a native renderer separate from the Wayland-window renderer:

- Open EGL on the DRM/GBM device instead of a Wayland display/window.
- Create a GLES context with a pbuffer or surfaceless context where supported.
- Allocate GBM BOs with `RENDERING | SCANOUT` and driver-preferred modifiers
  when the crate/API allows it; keep a linear fallback only for diagnostics.
- Register each rendered BO with KMS via `add_fb2`.
- Keep the existing `NativeGbmScanout` pageflip state, but replace
  `staging + bo.write()` with "render into BO, then pageflip its FB ID".

The first milestone can render only wallpaper + solid frame quads into a GBM
target. That proves EGL display/context/GBM target/KMS FB/pageflip integration
without involving client imports.

### P2: Reuse The Existing EGL Scene Model For Native Scanout

Refactor shared renderer logic out of `src/egl_renderer.rs` so both nested
Wayland and native GBM can use:

- shader/program setup;
- textured quad command generation;
- SHM texture upload and damaged rect upload;
- DMABUF EGLImage import;
- shell overlay/cursor texture handling;
- output damage calculation.

Keep output-target operations separate:

- nested target: Wayland `EGLSurface` + `eglSwapBuffersWithDamage`;
- native target: GBM BO/FBO or GBM EGL window surface + KMS pageflip.

The goal is one compositor scene renderer with two present targets, not two
divergent implementations of surface import and draw ordering.

### P3: Make Native Scanout Damage-Aware

Once rendering is on GPU, damage still matters:

- Transform `RenderableSurfaceDamage` into output coordinates for SHM texture
  updates and output damage.
- Damage old/new window bounds on move/resize.
- Damage shell overlay regions directly.
- Keep full-output damage for size changes, unknown modifier/import failures,
  fallback transitions, and mode changes.

For KMS, use damage primarily to avoid unnecessary texture uploads and draw
work. Pageflip still presents a whole FB, but GPU composition can avoid the CPU
full-frame copy/write that currently dominates.

### P4: Connect DMABUF Import To Native Rendering

After native GL draws to scanout BOs, imported client DMABUFs can be drawn as
textures like the nested renderer already does:

- Use the existing `DmabufBufferHandle` and
  `EglGlesDmabufImportAttributes::from_handle()`.
- Import through `eglCreateImage(..., EGL_LINUX_DMA_BUF_EXT, ...)`.
- Bind with `glEGLImageTargetTexture2DOES`.
- Track EGLImage/texture lifetime per committed surface generation.
- Delay client release until the rendered frame that used the old buffer has
  actually presented.

This is where browser GPU acceleration becomes meaningful in native mode:
clients can submit GPU buffers, the compositor imports them, and KMS scans out a
GPU-composited BO without CPU readback.

### P5: Split Protocol Prep From Presentation Finish

Refactor `OwnCompositorServer::present_frame()` into two phases:

- `prepare_frame_protocol()`: commit ready explicit-sync buffers, flush pending
  resize configure/color state, and flush clients before render.
- `finish_presented_frame()`: release buffers, signal explicit-sync release
  points, complete frame callbacks, send presentation feedback, and flush after
  a pageflip completion.

For GBM/KMS, call finish only from the pageflip event path. For dumb fallback,
keep immediate finish and label it as approximate.

### P6: Keep CPU/Dumb Paths As Debug Fallbacks

Do not delete the CPU renderer while bringing up native GPU:

- Keep `OBLIVION_ONE_SCANOUT_BACKEND=dumb` and any CPU renderer switch as
  diagnostics.
- Log a clear backend label such as `native.gpu=egl-gbm` versus
  `native.gpu=cpu-gbm-write`.
- Preserve the current path as a fallback for EGL/GBM bring-up failures, but
  make perf logs make the fallback obvious.

## NVIDIA Risks

- GBM modifier selection may differ between renderable BOs and scanout-accepted
  BOs. The current native code forces linear writable BOs; direct rendering
  should prefer driver-supported render+scanout modifiers, but must verify KMS
  `add_fb2`/pageflip acceptance.
- `EGL_EXT_image_dma_buf_import_modifiers` can expose NVIDIA block-linear
  modifiers that are not always represented cleanly by older protocol paths.
  The nested renderer already has NVIDIA-specific feedback handling; native
  scanout needs equivalent logging for chosen BO modifiers and FB metadata.
- Implicit versus explicit sync is easy to get wrong. Existing explicit-sync
  protocol handling should not release client DMABUFs before the KMS-presented
  frame is complete.
- Legacy cursor APIs are currently working in the latest log, but atomic KMS
  migration could change cursor-plane constraints. Keep automatic software
  fallback and log cursor failures separately from GPU render failures.
- Multi-GPU/import paths are declared as a target capability, but this native
  session should first prove same-device KMS/render-node operation. Cross-device
  import should remain a later milestone.

## Validation Recommended

Measurement commands:

```sh
OBLIVION_ONE_MODE=1920x1080@165 \
OBLIVION_ONE_SCANOUT_BACKEND=gbm \
OBLIVION_ONE_CURSOR=auto \
OBLIVION_ONE_PERF_LOG=1 \
./target/release/oblivion-one
```

For launcher/session validation without entering the login-critical path:

```sh
OBLIVION_ONE_DRY_RUN=1 ./bin/start-oblivion-one
```

Suggested perf extraction:

```sh
rg 'perf native.frame' ~/.local/state/oblivion-one/session.log
rg 'native cursor backend active|perf native.cursor|native scanout' ~/.local/state/oblivion-one/session.log
```

Before/after criteria:

- Hardware cursor remains active: log contains `native cursor backend active:
  hardware` and frame logs keep `cursor=hardware`.
- Pointer-only idle motion does not produce repaint-rate `native.frame` lines.
- Native GPU path logs a distinct backend label, for example `egl-gbm`, and
  does not use `bo.write()` for normal GBM scanout frames.
- `write_us` should disappear or become near-zero on the GPU path.
- `copy_us` should disappear for scanout; remaining CPU packing should only be
  SHM texture upload, preferably damage-limited.
- Average and p95 `paint_us` should fall below the `6060 us` 165 Hz budget on
  static/small-damage cases. Browser-heavy full-scene cases should improve
  substantially and be dominated by GPU work/pageflip waits rather than CPU
  copy/write.
- DMABUF browser surfaces should remain visible in native mode, proving the
  native path is not falling back to `surface.cpu_pixels()`.
- Frame callbacks and presentation feedback should complete after pageflip
  completion on GBM/KMS.

## Low-Risk Fixes Before Full Native EGL

- Add clearer perf/backend labels around the current path:
  `cpu-gbm-write`, bytes written, and whether the frame used SHM-only CPU
  composition.
- Add parser/documentation for extracting current `render_us/copy_us/write_us`
  percentiles from `OBLIVION_ONE_PERF_LOG`.
- Add unit coverage around "DMABUF surface cannot be drawn by CPU renderer" so
  future work does not accidentally reintroduce CPU readback assumptions.
- Add native logging of GBM BO format, modifier, pitch, and flags for scanout
  buffers. This will be needed for NVIDIA comparisons once renderable BOs are
  introduced.

## Practical Implementation Sequence

1. Baseline current hardware-cursor session and save the split metrics.
2. Add native EGL context on the GBM/DRM device with no client surfaces.
3. Render wallpaper/solid test quads into a GBM render+scanout BO and pageflip.
4. Move reusable GL scene/resource code behind a target abstraction.
5. Enable SHM texture upload in native GPU mode.
6. Enable DMABUF EGLImage import in native GPU mode.
7. Move buffer release/frame callbacks/presentation feedback to pageflip finish.
8. Add output damage and resize/move damage mapping.
9. Keep CPU GBM-write and dumb framebuffer paths as explicit diagnostics.

## Evidence

Files and docs used:

- `src/native_output.rs`
- `src/render_backend/mod.rs`
- `src/render_backend/buffer.rs`
- `src/render_backend/egl_gles.rs`
- `src/egl_renderer.rs`
- `src/egl_renderer/dmabuf.rs`
- `src/egl_renderer/damage.rs`
- `src/nested_renderer.rs`
- `src/compositor/render.rs`
- `src/compositor/server.rs`
- `src/compositor/mod.rs`
- `src/compositor/state_data.rs`
- `src/compositor/shm.rs`
- `src/compositor/dmabuf.rs`
- `src/compositor/surface.rs`
- `Cargo.toml`
- `docs/ARCHITECTURE.md`
- `docs/KNOWN_ISSUES.md`
- `docs/research/native-session-log-analysis-2026-06-11.md`
- `docs/research/native-performance-mouse.md`
- `docs/research/hyprland-hardware-cursor-and-gecko.md`

Changes: `docs/research/native-gpu-path-gap-analysis.md`.

Validation: documentation-only investigation; no production code was edited and
no runtime tests were run.

Risks: the exact cost split for the newest 2965-frame capture still needs
extraction from the latest log. NVIDIA render+scanout modifier compatibility and
explicit-sync release timing remain the highest-risk implementation areas.

## Paths Altered

- `docs/research/native-gpu-path-gap-analysis.md`
