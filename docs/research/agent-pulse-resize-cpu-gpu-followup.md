# Agent Pulse: resize CPU/GPU follow-up

Date: 2026-06-11

Scope: read-only investigation of resize-time CPU bottlenecks that should move to GPU or to less CPU-heavy paths. Source edits were intentionally avoided; this document is the only intended change.

## Biggest likely costs

1. Full-frame GBM writes are confirmed during resize.
   - Latest parsed session starting at `session.log:25946` spent 1530 frames inside `perf resize.begin`/`perf resize.end` windows.
   - Inside resize: `paint_us avg=10080.7 p95=16088.3 max=36333`, `render_us avg=5396.2 p95=10638.8`, `copy_us avg=1601.0 p95=2798.5`, `write_us avg=3082.0 p95=3182.0`.
   - `write_bytes` was fixed at `8294400` bytes for every sampled resize frame, matching a 1920x1080x4 full-frame write. This is confirmed CPU/driver upload pressure.

2. CPU scene render/rebuild dominates many resize frames.
   - During the same latest resize windows, `render_us` consumed about half the paint budget on average and exceeded the 165 Hz frame budget by itself at p95.
   - Prior recent session starting at `session.log:19849` was worse during resize: `paint_us avg=11619.5 p95=16875.5`, `render_us avg=7126.9 p95=12149.0`, `write_us avg=3092.6`.
   - The current log does not include `scene_rebuild`, so full vs partial rebuild is not confirmed from the log. The code path strongly suggests resize preview and committed-size changes often invalidate partial scene reuse.

3. `copy_scene_to_frame` is a confirmed full CPU copy in current code.
   - `DesktopFrameRenderer::compose_request` calls `rebuild_scene(...)` and then `copy_scene_to_frame(...)` for every composed frame.
   - `copy_scene_to_frame` performs `frame.copy_from_slice(&self.scene)` when sizes match, so even a partial scene rebuild still copies the retained CPU scene into the output frame.

4. Damage/copy handling is mixed: current source is better than the latest log proves.
   - Latest log resize frames show `copy_bytes avg=8720997.3 p95=14488188 max=14679016`, with session-wide max `17915232`, so the captured binary allowed damage copies to exceed one full frame.
   - Current `copy_argb_frame_to_xrgb_mapping_damage` now caps rectangle copy to a full-frame copy when summed rect bytes exceed full-frame bytes. This is not reflected in the latest log, so the log is probably from an older binary or older instrumentation.

5. Scale/damage math is suspected, not yet proven, as a meaningful CPU cost.
   - The code does per-rect conversion through output scale and resize-preview target math, but the log has no separate timer for scale math.
   - Compared with measured `render_us`, `copy_us`, and `write_us`, scale math should be treated as a secondary hypothesis until measured.

## Confirmed vs hypothesized costs

### Confirmed by recent logs

- Resize still misses 165 Hz budget heavily. Latest resize windows have `paint_us p95=16088.3us`, above the ~6060us target.
- `write_us` is a stable per-frame tax around 3.1ms during resize.
- `write_bytes` is always full-frame in the latest session, even when `damage_kind=surface_damage`.
- `copy_us` is non-trivial during resize and can spike above 5ms.
- `copy_bytes` in the latest log can exceed full-frame size, but that appears stale relative to current source.
- Resize periods are mostly logged as `render_cause=surface_commit` and `redraw_requested`, not `window_resize`.

### Confirmed by current code

- `src/native_output.rs:4184` renders into a CPU frame, copies ARGB words into a staging byte buffer, then calls `buffer.bo.write(&self.staging)` for the whole GBM buffer.
- `src/native_output.rs:4215` to `src/native_output.rs:4226` records `write_bytes=byte_len`, so the native GBM scanout path still uploads the full linear buffer every paint.
- `src/compositor/render.rs:159` to `src/compositor/render.rs:166` always rebuilds or validates the CPU scene and then copies the full scene to the frame.
- `src/compositor/render.rs:320` to `src/compositor/render.rs:326` performs the full `copy_from_slice`.
- `src/compositor/mod.rs:2344` advances render generation with `WindowResize`; `src/compositor/mod.rs:2354` to `src/compositor/mod.rs:2364` changes surface size/placement, sets resize preview, and marks surface damage as `Full`.
- `src/native_output.rs:3686` to `src/native_output.rs:3712` already has a native-output damage path for `WindowResize` using surface bounds changes.
- `src/native_output.rs:4611` to `src/native_output.rs:4670` now caps damage rectangle copy to full-frame bytes when rect copy would be larger.

### Hypothesized from code, needs fresh capture

- Many resize frames probably trigger `scene_rebuild=full` because resize changes target geometry and/or buffer dimensions. `partial_scene_damage_rects` can handle target changes as old+new bounds, but full rebuild still happens when scene readiness, snapshot identity/order, size, scale key, or damage constraints fail.
- `WindowResize` damage may already be improved in source but not deployed in the latest captured run. Fresh logs should show whether `render_cause=window_resize`, `damage_kind=surface_damage`, and `copy_bytes <= write_bytes` now hold.
- Scale math is probably not the primary cost, but resize can amplify it through repeated surface snapshot and damage rect conversions.

## Code path notes

### Resize generation and damage

- `RenderGenerationCause::WindowResize` exists and serializes as `window_resize`.
- `uses_surface_damage()` only returns true for `SurfaceCommit` and `SurfaceDamage`, so placement and resize changes need special handling outside normal surface damage.
- `preview_resize_root_window_to` currently sets `RenderableSurfaceDamage::Full` for resize preview. That is conservative and visually safe, but it gives the renderer fewer opportunities to keep rebuild/copy bounded.
- Native damage selection now has a bounds-change branch for `WindowMove`, `WindowResize`, and `SurfacePlacement`. That helps native output copy damage, but it does not remove the CPU scene rebuild or the full GBM write.

### CPU render and copy

- `compose_request` renders through CPU memory:
  - rebuild or validate retained scene,
  - copy retained scene to frame,
  - blend shell overlay,
  - draw software cursor when hardware cursor is not active.
- Hardware cursor is active in recent context, so cursor drawing should not be the resize bottleneck in the latest runs.
- Even when the scene rebuild is partial, `copy_scene_to_frame` still copies the whole scene to the frame.

### GBM scanout

- The native scanout backend is GBM/KMS pageflip, but the rendered content path is still CPU-to-linear-GBM:
  - CPU render into ARGB frame,
  - CPU copy ARGB/XRGB words into `self.staging`,
  - full `bo.write(&self.staging)`,
  - KMS page flip.
- This explains why `write_bytes` remains full-frame regardless of damage.

## Proposed technical sequence

1. Recapture metrics with the current binary before changing behavior.
   - Goal: confirm whether the newer `scene_rebuild` field, damage copy cap, and `WindowResize` damage path are present at runtime.
   - Expected current-source sanity checks:
     - `scene_rebuild` appears on `perf native.paint`.
     - `copy_bytes` does not exceed `write_bytes` for 1920x1080 frames.
     - resize preview frames include `render_cause=window_resize` when compositor-side preview drives the frame.

2. Keep the damage-copy cap and add resize-specific assertions/alerts in docs or tests.
   - Low risk because it preserves correctness and only avoids copying more rectangles than a full frame.
   - Before/after criterion: `copy_bytes p95 <= 8294400` at 1920x1080 during resize.

3. Reduce CPU frame copies before attempting a full GPU renderer.
   - Make `copy_scene_to_frame` damage-aware or retain the output frame and copy only damaged scene rects when `scene_rebuild=partial`.
   - Keep full copy for `scene_rebuild=full`, shell overlay, output size/scale changes, and any uncertain state.
   - Before/after criterion: resize `copy_us p95` drops below 1ms when damage is small; visual output remains correct under move/resize/commit storms.

4. Make resize preview less full-rebuild-heavy where safe.
   - Treat resize preview as old target + new target damage instead of forcing surface `Full` damage when the committed buffer content did not change.
   - Preserve conservative full rebuild on actual buffer-size commits, surface role changes, scale changes, and snapshot identity changes.
   - Before/after criterion: fresh logs show `scene_rebuild=partial` on compositor-driven resize-preview frames.

5. Move scanout rendering to EGL/GLES/GBM.
   - Create GBM BOs with renderable scanout usage, import/upload client buffers as textures, render scene quads directly with GLES, and page-flip the rendered BO.
   - Keep the CPU renderer as fallback for unsupported drivers/formats.
   - This is the first step that can remove the measured ~3ms full-frame `bo.write` tax.

6. Add direct DMA-BUF/SHM import strategy.
   - DMA-BUF clients should import as EGL images/textures where supported.
   - SHM clients can upload changed damage regions to textures; avoid full-window CPU composition even when SHM remains CPU-backed.
   - Before/after criterion: `write_bytes` disappears or becomes a GPU timing metric; resize `paint_us p95` approaches one refresh budget at 165 Hz.

7. Measure scale math only after the large copies are controlled.
   - Add optional timers around snapshot/damage conversion and scaled draw loops.
   - Optimize by caching scale keys/snapshots or precomputing output rects only if those timers show measurable cost.

## NVIDIA risks

- GBM/EGL modifier support may vary by driver version and PRIME/offload topology.
- Linear scanout buffers are currently simple but expensive; renderable scanout BOs may need explicit modifier negotiation.
- EGL image import for DMA-BUF can fail for formats/modifiers accepted by other drivers.
- Explicit synchronization/fences may be required to avoid tearing, stalls, or read-before-render page flips.
- Keep CPU GBM write and dumb framebuffer fallback paths until NVIDIA and non-NVIDIA paths are both validated.

## Validation recommended

### Commands used in this investigation

```bash
ls -lh /home/agony/.local/state/oblivion-one/session.log
rg -n "perf native\\.(paint|frame|resize)|render_cause|damage_kind|scene_rebuild|copy_bytes|write_bytes" /home/agony/.local/state/oblivion-one/session.log
nl -ba src/native_output.rs | sed -n '3288,3330p'
nl -ba src/native_output.rs | sed -n '3634,3712p'
nl -ba src/native_output.rs | sed -n '4184,4275p'
nl -ba src/native_output.rs | sed -n '4560,4695p'
nl -ba src/compositor/render.rs | sed -n '150,360p'
nl -ba src/compositor/render.rs | sed -n '480,565p'
nl -ba src/compositor/mod.rs | sed -n '120,170p'
nl -ba src/compositor/mod.rs | sed -n '2280,2370p'
```

Additional parsing was done with short local Python one-liners over `session.log` to group frames inside `perf resize.begin`/`perf resize.end` windows and summarize `paint_us`, `render_us`, `copy_us`, `write_us`, `copy_bytes`, `write_bytes`, `render_cause`, and `damage_kind`.

### Fresh measurement commands

After rebuilding/running the current binary, capture a resize-only session and parse:

```bash
rg -n "perf resize\\.|perf native\\.paint" /home/agony/.local/state/oblivion-one/session.log
rg -n "scene_rebuild|render_cause=window_resize|copy_bytes|write_bytes|damage_kind" /home/agony/.local/state/oblivion-one/session.log
```

Recommended acceptance checks:

- `scene_rebuild` is present in `perf native.paint`.
- During resize, count frames by `scene_rebuild=none|partial|full`.
- During resize, report `paint_us/render_us/copy_us/write_us` avg/p95/max.
- During resize, report `copy_bytes/write_bytes` avg/p95/max.
- Verify `copy_bytes <= write_bytes` after the current damage-copy cap is deployed.
- Verify whether resize frames are caused by `window_resize`, `surface_commit`, or `redraw_requested`.

## Before/after criteria

- Short term:
  - `copy_bytes p95 <= one full frame` at current output resolution.
  - `copy_us p95 < 1000us` on partial-damage resize frames.
  - `scene_rebuild=partial` appears for preview-only resize frames.

- Medium term:
  - `render_us p95 < 4000us` during resize without visual corruption.
  - `paint_us p95 < 6060us` for 165 Hz on common resize scenarios.

- GPU path:
  - Full-frame `write_bytes` is eliminated from steady-state native scanout metrics.
  - GPU path survives resize storms, SHM clients, DMA-BUF clients, shell overlay, hardware cursor, output scale, and NVIDIA GBM fallback testing.

## Final block

- Evidence: `/home/agony/.local/state/oblivion-one/session.log`; `src/native_output.rs`; `src/compositor/render.rs`; `src/compositor/mod.rs`; commands listed above.
- Changes: `docs/research/agent-pulse-resize-cpu-gpu-followup.md`.
- Validation: documentation-only change; no production tests run. Log/code inspection completed.
- Risks: latest log appears older than current source instrumentation because it lacks `scene_rebuild` and still shows `copy_bytes` above full-frame size; fresh capture from the current binary is required before treating the source-level improvements as deployed behavior.
