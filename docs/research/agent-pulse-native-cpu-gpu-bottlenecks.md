# Agent Pulse: Native CPU/GPU Bottlenecks

Date: 2026-06-11

Scope: read-only performance investigation of the latest
`/home/agony/.local/state/oblivion-one/session.log` plus current
`src/native_output.rs` and `src/compositor/render.rs`.

## Summary

The latest native session starts at log line `25946` and runs
`1920x1080@165Hz` on the NVIDIA GBM backend with the hardware cursor active.
The capture confirms that pointer-only motion is mostly out of the repaint path:
all measured native frames have `cursor=hardware`, and the log also contains
`1131` `native.frame_skip reason=input_forwarded_no_visual` lines.

The remaining cost is CPU-rendered native scanout. Across the latest `2883`
`perf native.frame` samples:

- `paint_us`: avg `9919.8`, p95 `15771.8`, p99 `19051.2`, max `68913`.
- `render_us`: avg `5274.3`, p95 `10247.8`, max `62749`.
- `copy_us`: avg `1559.7`, p95 `2733.2`, max `6349`.
- `write_us`: avg `3084.3`, p95 `3182.0`, max `6582`.
- `copy_bytes`: avg `8686733.1`, p95 `14126684.0`, max `17915232`.
- `write_bytes`: always `8294400`.
- Frames above the `6060 us` 165 Hz budget: `2503 / 2883` (`86.8%`).

That says the compositor has partial damage-aware CPU copy now, but still pays a
full scanout write on every repaint and still composes the scene on CPU.

## Biggest Likely Costs

1. **CPU scene rendering (`render_us`)**

   Confirmed. This is the largest measured bucket in the latest run. Repaint
   frames average `5254.4 us` in `render_us`, and surface commits average
   `5614.5 us`. Top outliers are render-heavy: the worst repaint frame has
   `paint_us=36333`, `render_us=26892`, `copy_us=5660`, `write_us=3779`.

2. **Full GBM BO write (`write_us` / `write_bytes`)**

   Confirmed. `write_bytes` is fixed at `8294400` for every frame, regardless
   of `damage_kind`, `damage_rects`, or `copy_bytes`. `write_us` is remarkably
   stable around `3.1 ms`, which alone consumes about half of the `6060 us`
   frame budget at 165 Hz.

3. **Damage-to-staging copy (`copy_us` / `copy_bytes`)**

   Confirmed but partially improved. `copy_bytes` varies with damage and can be
   below a full frame, but overlapping/large damage often exceeds `write_bytes`.
   Frames with `copy_bytes > 1.5x write_bytes` average `14002.4 us` paint time.

4. **Partial scene rebuild still runs on CPU**

   Confirmed by source, not directly measured in the latest log because the
   current `scene_rebuild` perf field is missing from that capture. Current
   source reports `scene_rebuild` from `DesktopSceneRenderer::last_rebuild_kind()`,
   but every frame in the latest log parsed as `scene_rebuild=<missing>`.

5. **Surface damage can over-count output copy**

   Suspected from the metrics. `damaged_pixels` averages `2171683.3`, above the
   physical output pixel count of `2073600`, and maxes at `4478808`. Because
   damage rects are accumulated and copied independently, overlapping rects can
   inflate CPU staging work.

## Confirmed Costs

### Native Repaint Path Is CPU Render + CPU Copy + Full GBM Write

Current source path:

- `src/native_output.rs:843-870` decides to repaint on accepted clients,
  render-generation changes, pending frame work, or redraw requests, then calls
  `scanout.paint_server_frame(...)`.
- `src/native_output.rs:4057-4104` renders a CPU frame, resizes staging,
  copies ARGB frame bytes into the XRGB staging buffer according to damage, then
  calls `buffer.bo.write(&self.staging)`.
- `src/native_output.rs:4088-4098` records `write_us` and sets `write_bytes` to
  the full scanout buffer byte length.

Log evidence:

- Latest backend: `native scanout: GBM write/pageflip buffers ready:
  1920x1080, 3 buffer(s), backend nvidia`.
- `write_bytes` is `8294400` on every one of `2883` frames.
- `write_us` avg `3084.3`, p95 `3182.0`; this remains even when
  `damage_kind=surface_damage`.

### Damage Reduces Staging Copy But Not Scanout Write

Current source path:

- `src/native_output.rs:3293-3323` exposes `copy_bytes`, `write_bytes`,
  `scene_rebuild`, `paint_us`, `render_us`, `copy_us`, and `write_us`.
- `src/native_output.rs:3399-3421` chooses full copy when
  `scene_rebuild == Full`; otherwise it uses output damage rects.
- `src/native_output.rs:4484-4568` copies only the requested rect rows into
  staging and returns copied bytes.

Log evidence:

- `damage_kind=surface_damage` appears on `2494` frames.
- `surface_damage` frames average `copy_bytes=8747927.1` and
  `write_bytes=8294400.0`.
- `full` frames average `copy_bytes=8294400.0` and `write_bytes=8294400.0`.
- Damage helps in some cases: frames with `copy_bytes < 0.5x write_bytes`
  average `5947.9 us` paint time.
- Damage hurts or becomes too broad in others: frames with
  `copy_bytes > 1.5x write_bytes` average `14002.4 us`.

### CPU Scene Renderer Has Partial Rebuilds, But Still Copies The Scene

Current source path:

- `src/compositor/render.rs:198-242` decides between no rebuild, partial
  rebuild from damage, and full rebuild.
- `src/compositor/render.rs:244-292` rebuilds damaged scene rects by copying
  wallpaper rects and redrawing clipped client surfaces on CPU.
- `src/compositor/render.rs:295-327` rebuilds the full CPU scene.
- `src/compositor/render.rs:329-335` still copies the CPU scene into the frame
  buffer after rebuild/no-rebuild.
- `src/compositor/render.rs:417-446` draws client surfaces through CPU blits,
  optionally clipped to a damage rect.

Log evidence:

- `render_us` dominates the worst frames and averages `5614.5 us` for
  `surface_commit`.
- `render_cause=surface_commit` appears on `2494` frames and has p95
  `paint_us=15981.8`.
- `render_cause=window_move` appears on `92` frames and averages
  `render_us=7796.2`, with `damage_kind=full`/full-frame behavior implied by
  the current repaint-damage policy for non-surface-damage causes.

### Hardware Cursor Is Not The Current Paint Bottleneck

Log evidence:

- Latest run logs `native cursor backend active: hardware (64x64)`.
- All `2883` native frame samples have `cursor=hardware`.
- The run includes `1131` skipped input repaint records, covering `2076` raw
  input events coalesced to `1140` forwarded events.

This confirms the expensive frames are mostly client/surface repaint and window
movement work, not plain pointer motion.

## Suspected Costs / Gaps To Measure

- `scene_rebuild` cannot be correlated in this capture because the latest log
  does not contain the field even though current source would emit it. Rebuild
  the binary or recapture after confirming the deployed binary matches
  `src/native_output.rs`.
- Output damage rects are likely overlapping or too broad. The evidence is
  `damaged_pixels` and `copy_bytes` exceeding a full frame on many
  `surface_damage` frames.
- External shell UI now contributes through normal layer-shell surface commits,
  so scene/content generation is driven by the same render generation as other
  client surfaces.
- `copy_scene_to_frame()` is still an unconditional CPU copy after scene
  rebuild/no-rebuild. The log cannot isolate it from other `render_us` work, but
  current source confirms it exists.

## Prioritized Low-Risk Optimizations

1. **Validate and ship the `scene_rebuild` metric in the running binary**

   Low risk, high diagnostic value. The current source already emits it, but the
   latest log misses it. Recapturing with `scene_rebuild=none|partial|full`
   would separate "GPU needed for everything" from "partial CPU path is working
   but still bottlenecked by final write".

2. **Coalesce or de-overlap native output damage rects before staging copy**

   Low-to-medium risk. The current copy loop copies each damage rect
   independently. A conservative merge/clamp pass could reduce
   `copy_bytes > write_bytes` cases without changing rendering semantics. Keep
   full-output fallback if rect count or union gets awkward.

3. **Avoid full output damage for window move when possible**

   Medium risk but contained. `window_move` averages `12050.8 us` paint and
   `7796.2 us` render. Damage old and new window bounds instead of full output
   for move/resize paths where geometry is known.

4. **Move final scanout write to GPU-rendered GBM BOs**

   Highest impact, larger implementation. `write_us` is a confirmed steady
   `~3.1 ms` tax and `write_bytes` never shrinks. Rendering directly into
   GBM/EGL scanout targets would remove `bo.write(&self.staging)` from normal
   frames.

5. **Move CPU surface composition to shared EGL/GLES scene rendering**

   Highest impact, larger implementation. `render_us` dominates the worst
   frames. Existing nested EGL/GLES logic should become the model for native
   scanout so SHM uploads and DMABUF imports become GL textures instead of CPU
   blits.

6. **Keep CPU GBM-write as an explicit fallback**

   Low risk. Keep the current path as `cpu-gbm-write`/diagnostic fallback while
   native EGL/GBM comes online. This keeps rollback simple and lets perf logs
   distinguish fallback from the real GPU path.

## Recommended Validation

Run a fresh native capture after confirming the deployed binary includes the
current perf fields:

```sh
OBLIVION_ONE_MODE=1920x1080@165 \
OBLIVION_ONE_SCANOUT_BACKEND=gbm \
OBLIVION_ONE_CURSOR=auto \
OBLIVION_ONE_PERF_LOG=1 \
./target/release/oblivion-one
```

Then extract:

```sh
rg 'native scanout target|native scanout backend|native cursor backend|perf native.cursor' /home/agony/.local/state/oblivion-one/session.log
rg 'perf native.frame ' /home/agony/.local/state/oblivion-one/session.log
rg 'perf native.frame_skip' /home/agony/.local/state/oblivion-one/session.log
```

Before/after criteria:

- `scene_rebuild` appears on `perf native.frame` lines.
- `write_bytes` drops to `0` or disappears on the GPU scanout path.
- `write_us` is no longer a steady `~3 ms` tax.
- `copy_bytes` does not exceed full-frame bytes for normal partial damage.
- `render_us` p95 falls below the 165 Hz budget for static/small-damage frames.
- `cursor=hardware` remains active and pointer-only motion continues to produce
  `native.frame_skip` rather than repaint-rate frames.

## Commands Used

```sh
ls -l /home/agony/.local/state/oblivion-one/session.log
tail -n 240 /home/agony/.local/state/oblivion-one/session.log
rg -n "perf native\\.frame" /home/agony/.local/state/oblivion-one/session.log
rg -n "scene_rebuild|damage_kind|copy_bytes|write_bytes|render_us|copy_us|write_us|perf native\\.cursor|native cursor backend active|native scanout" /home/agony/.local/state/oblivion-one/session.log
python3 - <<'PY'
# Parsed latest native session, native.frame fields, percentiles, groups by
# damage_kind/render_cause, frame_skip counts, and scene_rebuild presence.
PY
rg -n "copy_bytes|write_bytes|damage_kind|scene_rebuild|paint_server_frame|render_server_frame|copy_argb|bo\\.write|compose_request|copy_scene_to_frame|blit_surface_to_rect" src/native_output.rs src/compositor/render.rs
nl -ba src/native_output.rs | sed -n '840,890p'
nl -ba src/native_output.rs | sed -n '1020,1145p'
nl -ba src/native_output.rs | sed -n '3288,3430p'
nl -ba src/native_output.rs | sed -n '3520,3592p'
nl -ba src/native_output.rs | sed -n '4050,4110p'
nl -ba src/native_output.rs | sed -n '4460,4585p'
nl -ba src/compositor/render.rs | sed -n '45,535p'
nl -ba src/compositor/render.rs | sed -n '620,735p'
```

## Evidence

- Latest log session: `/home/agony/.local/state/oblivion-one/session.log`,
  starting at line `25946`.
- Native repaint/write path: `src/native_output.rs:843-870`,
  `src/native_output.rs:4057-4104`, `src/native_output.rs:4484-4568`.
- Native damage/perf fields: `src/native_output.rs:3293-3421`,
  `src/native_output.rs:3520-3592`.
- CPU scene renderer: `src/compositor/render.rs:156-184`,
  `src/compositor/render.rs:198-335`, `src/compositor/render.rs:417-446`.

## Changes

- `docs/research/agent-pulse-native-cpu-gpu-bottlenecks.md`

## Validation

Documentation-only investigation. No source code was edited and no runtime
tests were run.

## Risks

- The latest log is missing `scene_rebuild`, so source-level conclusions about
  full/partial scene rebuilds are confirmed by code but not yet correlated with
  the current capture.
- Native EGL/GBM scanout will have NVIDIA modifier/sync risks; keep the current
  CPU GBM-write path as a fallback until the GPU path proves correctness and
  timing.

## Paths Altered

- `docs/research/agent-pulse-native-cpu-gpu-bottlenecks.md`
