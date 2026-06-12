# Implementation Plan: Real Apps and Responsive Nested Output

## Overview

Move Oblivion One from a static nested desktop into a minimal real-app host. The
first target is not a full DE yet; it is a compositor loop that can keep the
mouse responsive, expose enough Wayland globals for normal clients, and render
committed shared-memory surfaces without relying on Hyprland as the app host.

## Architecture Decisions

- Keep the owned compositor path first. Hyprland and Gamescope remain legacy
  comparison backends only.
- Keep a CPU fallback renderer, but make the nested output prefer a GPU-backed
  presentation path. A slow render loop makes every later protocol feature feel
  broken.
- Add protocol support in small compatibility slices: frame callbacks, output,
  seat, then subcompositor. Avoid advertising capabilities that are not
  minimally handled.
- Use tests for every protocol contract that can be checked with
  `wayland-client`; use short local smokes for real apps.

## Task List

### Phase 1: Render Loop and Cursor Performance

- [x] Cache the procedural wallpaper per output size instead of recomputing it
  for every redraw.
- [x] Track compositor render generations so the nested window redraws when
  client surfaces change.
- [x] Replace the unbounded redraw loop with a paced loop that still ticks the
  Wayland server frequently.
- [x] Cache the composed client scene separately from the cursor so mouse motion
  does not re-blit every real-app surface.
- [x] Move buffer `wl_surface.frame.done` completion to the nested presentation
  path so clients are paced by presented frames.

### Phase 2: Minimum Real-App Protocol Surface

- [x] Send `wl_surface.frame.done` callbacks after commits so clients can keep
  drawing.
- [x] Advertise and initialize one `wl_output` with stable geometry/mode data.
- [x] Advertise one `wl_seat` with pointer capability and create pointer
  resources safely.
- [x] Advertise keyboard capability and send an initial XKB keymap/repeat info
  when a client requests `wl_keyboard`.
- [x] Advertise `wl_subcompositor` and accept basic `wl_subsurface` lifecycle
  requests used by real toolkits.
- [x] Advertise `wl_data_device_manager` and accept basic data source/device
  lifecycle requests used by GTK apps.
- [x] Forward basic nested-output keyboard and pointer events into the focused
  xdg surface.

### Phase 3: Validation

- [x] `cargo fmt -- --check`
- [x] `cargo test`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `./bin/oblivion-one compositor --check`
- [x] `wayland-info` shows `wl_output` and `wl_seat` from Oblivion.
- [x] `kitty` connects to Oblivion, creates a toplevel, commits buffers, and
  increments renderable surface count.
- [x] `nautilus` connects to Oblivion, creates a toplevel, commits buffers, and
  increments renderable surface count.
- [x] `brave --ozone-platform=wayland` connects to Oblivion, creates a toplevel,
  commits buffers, and increments renderable surface count.
- [x] A real external xdg/winit app process connects to Oblivion, creates a
  toplevel, commits buffers, and increments renderable surface count.

### Phase 4: GPU Output Bridge

- [x] Add a `wgpu` nested-output renderer that opens a Vulkan/GLES surface and
  presents frames through the GPU.
- [x] Keep the previous `softbuffer` renderer as `--renderer=cpu` and as the
  fallback for `--renderer=auto`.
- [x] Add `--renderer auto|gpu|cpu` for explicit GPU and fallback testing.
- [x] Verify `--renderer=gpu` opens on the NVIDIA adapter and appears in
  `nvidia-smi`.
- [x] Replace full-frame CPU uploads with persistent GPU textures per client
  surface and same-size damage-rect uploads for `wl_shm` commits.
- [x] Prefer Vulkan before GL/GLES, split scene/cursor vertex buffers, and pack
  narrow damaged rectangles before GPU texture upload.
- [ ] Advertise and implement `linux-dmabuf` imports for accelerated client
  buffers.

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Real apps request optional protocols we do not support yet | Medium | Do not advertise unsupported optional protocols; add only the core globals first. |
| Frame callbacks are sent too early | Medium | Treat this as a development milestone; later move callback completion to presentation timing. |
| Debug builds remain slower than release | Low | Remove the major recomputation path now; recommend `cargo run --release` for heavier app testing. |
| Accelerated clients still need dmabuf import | Medium | Keep the damage-rect `wl_shm` path fast enough for fallback, then move to modifier-aware dmabuf import. |

## Current Root Cause Notes

- The slow mouse path is caused by recomputing the wallpaper for every frame and
  requesting redraw continuously from `about_to_wait`.
- The second slow path was cursor-only redraws re-blitting all client surfaces.
  The renderer now caches the composed scene by render generation.
- Buffer frame callbacks were being completed immediately on commit, which let
  clients request more frames before the nested output presented. They are now
  completed from the presentation path.
- The nested output now prefers a `wgpu` renderer and keeps `softbuffer` as a CPU
  fallback. This makes presentation GPU-backed. Client buffers are still copied
  through the current `wl_shm` snapshot path until dmabuf lands, but same-size
  commits now copy/upload only damaged rectangles. Cursor-only frames now update
  a small cursor vertex buffer instead of rewriting the cached scene vertices.
- `kitty` reaches the owned compositor, creates one xdg toplevel, and increments
  renderable surface count to 9/10 in a short smoke run. The earlier blocker was
  compositor compatibility and render-loop behavior, not app spawning.
