# Implementation Plan: GPU-Real Render Backend

## Overview

Move Oblivion One from GPU-presented `wl_shm` snapshots to compositor-grade GPU
buffer ownership. The immediate target is not visual polish; it is a renderer
path where normal Wayland clients can render on the GPU, submit
`linux-dmabuf` buffers, and have Oblivion import and composite those buffers
without full-window CPU copies.

## Architecture Decisions

- Keep the current `wgpu` nested renderer as the development/fallback backend.
  It is useful and already fast enough for `wl_shm`, but it is not the final
  browser-GPU path.
- Introduce `render_backend` as the contract for graphics capabilities before
  migrating protocol code. This keeps the WM/DE code independent from a specific
  renderer implementation.
- Target EGL/GLES first for real `linux-dmabuf` import, modifier feedback, and
  explicit sync. Smithay can provide the implementation pieces, but the
  Oblivion-facing backend contract is `egl-gles`. Raw Vulkan can be a later
  backend after the compositor has the correct buffer lifecycle.
- Treat `brave` with its default flags as a regression smoke test. If the
  default path already works, the migration must preserve that before enabling
  stronger accelerated-buffer paths.

## Phase 1: Backend Contract

- [x] Add `render_backend` module with backend kind, GPU API, and capability
  profiles.
- [x] Mark the current `wgpu` nested backend as GPU-composition plus
  damage-tracked `wl_shm` fallback, not final dmabuf support.
- [x] Mark the EGL/GLES target as modifier-aware dmabuf, feedback,
  explicit-sync, direct-scanout, and multi-GPU capable.
- [x] Wire the nested renderer to the `wgpu-nested` profile in logs and tests.
- [x] Expose `--gpu-api=gles|vulkan` for strict comparison runs while keeping
  `wgpu-nested` auto mode on its stable Vulkan-first path until the native
  EGL/GLES backend lands.

## Phase 2: Buffer Model Split

- [x] Split committed client content into typed buffer records:
  `ShmBufferSnapshot` and `DmabufBufferHandle`.
- [x] Keep `wl_shm` snapshots as fallback without changing current app behavior.
- [x] Preserve `wl_buffer.release`, frame callback pacing, and damage tracking
  across both buffer kinds.

## Phase 3: EGL/GLES Import Path

- [x] Carry `create_immed` linux-dmabuf buffers as typed
  `DmabufBufferHandle` surfaces without forcing CPU pixels.
- [x] Add an EGL/GLES import contract that validates dmabuf format/modifier
  feedback before any unsafe renderer import code.
- [x] Keep `wgpu-nested` from creating texture resources for dmabuf surfaces it
  cannot import.
- [ ] Add EGL/GLES implementation dependencies behind a focused backend module.
- [ ] Query renderer-supported dmabuf formats/modifiers from the real renderer.
- [ ] Replace the linear-only dmabuf advertisement with renderer-derived
  feedback.
- [ ] Import dmabuf-backed buffers into GPU textures and notify clients only
  after the renderer accepts them.

## Phase 4: Real Browser GPU Smoke

- [ ] Run Brave with default flags against the owned Oblivion socket.
- [ ] Verify the browser creates xdg toplevels, commits buffers, and keeps its
  GPU process alive.
- [ ] Verify large-window animation/scrolling does not scale with full-window
  CPU copies.
- [ ] Keep `--disable-gpu`/`wl_shm` fallback available as a diagnostic mode, not
  the primary browser path.

## Phase 5: Later Compositor-Grade Work

- [ ] Add explicit sync protocol handling once dmabuf import is stable.
- [ ] Add direct scanout eligibility checks for fullscreen/unoccluded surfaces.
- [ ] Add multi-GPU import/copy policy for hybrid systems.
- [ ] Add color management/HDR only after the buffer import lifecycle is stable.

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Browser default path regresses while dmabuf is incomplete | High | Keep the existing `wl_shm` path and run Brave default as smoke before each backend milestone. |
| Advertising unsupported modifiers breaks clients | High | Derive feedback from renderer-supported formats; do not hand-write optimistic modifier lists. |
| EGL/GLES implementation bleeds into WM policy | Medium | Keep `render_backend` as the seam; WM continues to reason about surfaces/windows, not EGL internals. |
| Raw Vulkan backend distracts from buffer correctness | Medium | Land EGL/GLES first; revisit raw Vulkan after dmabuf lifecycle and sync are proven. |

## Current Acceptance Target

```bash
./target/release/oblivion-one compositor --renderer=gpu -- brave
```

This command must continue to open Brave in the owned compositor. The long-term
success condition is the same command using accelerated dmabuf buffers without
GPU-process restart loops or full-window CPU upload cost.
