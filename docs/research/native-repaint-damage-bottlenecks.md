# Native Repaint and Damage Bottlenecks

Date: 2026-06-11

Scope: investigation only. No production compositor code was changed.

## Executive Summary

The latest native TTY capture confirms that hardware cursor is active, but it
does not remove the dominant repaint cost. The remaining hotspot is scene
damage: many frames repaint with `cursor=hardware`, no raw input, no pending
frame work, no explicit redraw request, and a changed `render_generation`.

That pattern points at client surface commits, not mouse motion. The compositor
currently treats any render generation change as a reason to compose and write a
full 1920x1080 scanout buffer. Surface-level damage already exists, and EGL
nested rendering already has a small output damage tracker, but the native
KMS/GBM scanout path still performs full-frame CPU render, full ARGB/XRGB copy,
and full `gbm_bo_write` on every repaint.

## Recent Log Metrics

Source: `~/.local/state/oblivion-one/session.log`, latest native TTY run
starting at `2026-06-11T17:38:00-03:00`.

Environment observed in the log:

- Output: `1920x1080@165`
- Cursor: `hardware`
- Scanout: `GBM/KMS pageflip`
- Per-frame bytes: `8294400` (`1920 * 1080 * 4`)
- 165 Hz budget: about `6060 us`

From `2964` `phase=repaint` samples with `cursor=hardware`:

| Metric | Average | p95 | p99 | Max |
| --- | ---: | ---: | ---: | ---: |
| `paint_us` | `12267.9` | `15653` | `17618` | `24983` |
| `render_us` | `7718.3` | `10727` | `12388` | `19518` |
| `copy_us` | `1465.7` | `2126` | `2521` | `3311` |
| `write_us` | `3082.3` | `3188` | `3258` | `6393` |

Budget pressure:

- `2866 / 2964` frames exceeded `6060 us` (`96.7%`).
- `1828 / 2964` frames exceeded `12000 us` (`61.7%`).
- `2806` consecutive hardware-cursor repaint samples changed
  `render_generation`; only `157` kept the same generation.
- `1330` frames had `raw_input_events=0`,
  `pending_frame_work=false`, `redraw_requested=false`,
  `accepted_clients=0`, and a changed generation from the previous repaint.

Representative line shape from the latest capture:

```text
perf native.frame scanout="GBM/KMS pageflip" width=1920 height=1080 bytes=8294400 paint_us=14182 render_us=9738 copy_us=1280 write_us=3162 index=2896 phase=repaint mode=1920x1080@165 cursor=hardware refresh_hz=165 surfaces=2 render_generation=3385 ... raw_input_events=0 ... pending_frame_work=false redraw_requested=false accepted_clients=0
```

Interpretation: hardware cursor is doing its job for pointer pixels, but active
client commits still advance `render_generation`, and each generation change
still triggers a full monitor repaint/write.

## Confirmed Versus Hypothesized Costs

Confirmed costs:

- Full-frame native scanout is confirmed by `bytes=8294400` on every repaint
  sample and by the GBM scanout path rendering, copying, and writing the whole
  buffer (`src/native_output.rs:3685-3727`).
- Repaint scheduling is tied to `accepted > 0`, changed
  `render_generation`, `pending_frame_work`, or `redraw_requested`
  (`src/native_output.rs:825-831`).
- Hardware cursor does not prevent client-driven repaint. The loop includes
  `cursor=hardware`, but `render_generation` changes continue to trigger
  `paint_server_frame()` (`src/native_output.rs:836-841`).
- Surface damage is tracked at commit time and stored on `RenderableSurface`
  (`src/compositor/protocols/core.rs:46-72`,
  `src/compositor/mod.rs:812-902`, `src/compositor/surface.rs:40-103`).
- SHM damage-only updates already update only dirty pixels inside a cached
  surface buffer (`src/compositor/mod.rs:907-962`,
  `src/compositor/tests/surface_frames.rs:265-296`).
- The EGL nested renderer already has output damage for cursor and shell
  overlay changes and uses `eglSwapBuffersWithDamage` where available
  (`src/egl_renderer/damage.rs:172-223`, `src/egl_renderer.rs:672-699`).

Hypothesized costs that need direct instrumentation:

- The `raw_input_events=0` generation churn is most likely browser/client
  surface commits, because the commit paths increment `render_generation`.
  Current logs do not label generation causes, so this should be confirmed by
  cause-tagged generation logging.
- Some `render_generation` increments may come from stacking, popup, focus, or
  placement changes rather than buffer damage. These are rarer in normal steady
  state but should be cause-counted before optimizing.
- Partial scanout writes may help the dumb/mapped path immediately. On NVIDIA
  GBM with `gbm_bo_write`, partial writes may require a mapped BO path or a
  different native renderer before they reduce `write_us`.

## Biggest Likely Costs

1. Full CPU composition on every changed generation.
   `DesktopSceneRenderer::compose_request()` always rebuilds the scene if the
   content generation changed, then copies the whole cached scene into the
   output frame and blends shell/cursor (`src/compositor/render.rs:108-135`).

2. Full scanout conversion and write.
   The GBM native path always converts the full ARGB frame into XRGB staging and
   writes the full BO (`src/native_output.rs:3695-3716`). This alone averages
   about `4.55 ms` in the latest hardware-cursor run
   (`copy_us + write_us`).

3. Generation is too coarse for repaint damage.
   `render_generation` is a binary "something changed" signal. It cannot tell
   the native renderer whether the change was a 2x2 SHM damage rect, a window
   move, a shell overlay update, or a full output invalidation.

4. Native CPU renderer has no output damage parameter.
   `NativeFrameRenderer::render_frame()` passes only a synthetic
   `content_generation` into `DesktopComposeRequest`; there is no damage region
   in the request or in `paint_server_frame()` (`src/native_output.rs:1040-1075`).

5. Pageflip/frame lifecycle still creates wakeups.
   `present_frame()` flushes resize configures, releases buffers, completes
   callbacks, and presentation feedback as one operation
   (`src/compositor/server.rs:222-229`). This is not the main full-frame
   bandwidth cost, but it complicates measuring "no damage" frames and real
   pageflip completion.

## Why Generation Changes Without Input

The loop logs `raw_input_events=0`, `pending_frame_work=false`, and
`redraw_requested=false` after input drain and before repaint. In that state the
condition that still explains repaint is:

```rust
render_generation != last_render_generation
```

The code path that can change it with no raw input is `server.tick()`, which
dispatches Wayland clients and processes surface commits. Surface commits bump
generation even when the damaged area is small:

- New buffer commit: increments generation, updates/creates a
  `RenderableSurface`, stores its `damage`, then assigns
  `self.render_generation = generation`
  (`src/compositor/mod.rs:812-902`).
- Damage-only commit without a new buffer: reuses the current buffer, applies
  dirty SHM pixels if possible, stores `existing.damage`, and increments
  generation (`src/compositor/mod.rs:907-962`).
- Unmap/destroy/minimize/restore/raise/placement changes also bump generation
  (`src/compositor/mod.rs:440-477`, `src/compositor/mod.rs:1176-1190`,
  `src/compositor/mod.rs:1388-1393`,
  `src/compositor/mod.rs:1928-1945`,
  `src/compositor/mod.rs:2094-2097`).

The resize-specific test already confirms that configure-only resize does not
advance generation before a client commit
(`src/compositor/tests/windows.rs:363-379`). So during browser activity or
resize, the steady generation churn is expected to be real client commits.

## Caches Already Present

- `DesktopSceneRenderer` caches wallpaper and scene buffers by output size,
  scale key, and content generation (`src/compositor/render.rs:56-68`,
  `src/compositor/render.rs:146-186`). This avoids rebuilding scene pixels when
  only the cursor moves, but `copy_scene_to_frame()` still copies the full frame
  every compose (`src/compositor/render.rs:188-194`).
- External shell UI now renders through normal layer-shell surfaces, so it uses
  the same surface texture/cache path as other Wayland clients.
- `OwnCompositorState` caches surface origins for hit/render calculations and
  invalidates the cache by render generation
  (`src/compositor/mod.rs:1196-1207`).
- The EGL renderer caches GL resources for wallpaper, cursor, server-frame
  colors, and client surfaces (`src/egl_renderer.rs:78-88`,
  `src/egl_renderer.rs:307-432`, `src/egl_renderer.rs:435-512`).
- The EGL renderer can upload only SHM surface damage when the surface resource
  is reusable (`src/egl_renderer.rs:455-468`).
- `EglOutputDamageTracker` tracks old/current cursor rects and shell overlay
  rects, returning partial damage when scene content is unchanged
  (`src/egl_renderer/damage.rs:112-223`).

Important gap: these caches reduce some rebuild/upload work, but the native KMS
CPU scanout still writes a full output buffer whenever it repaints.

## Repaints That Could Become Partial Damage

Low-risk first targets:

- Hardware cursor movement: should stay out of scene damage entirely. The latest
  run shows this mostly works, but input events that also forward pointer motion
  to clients can coincide with client commits and should be measured separately.
- Software cursor fallback: old cursor rect plus new cursor rect. This is
  already modeled in `EglOutputDamageTracker`; native scanout needs the same
  dirty rects plus retained front/back contents.
- Shell overlay changes: damage old and new topbar/dock/Spotlight overlay
  rects. EGL already tracks shell overlay regions; native CPU scanout can reuse
  equivalent bounds.
- Window move: damage old window frame rect plus new window frame rect,
  including server decoration bounds.
- Interactive resize target changes: damage old and new frame/window bounds.
  During the configure/commit gap, avoid stretching old client buffers into the
  future size; fill newly exposed area separately.
- Surface commit with explicit damage: transform `RenderableSurfaceDamage` from
  surface/buffer space through surface origin, placement, scale, and frame
  decoration into output coordinates.
- Damage-only SHM commits: the source-side pixels are already updated partially;
  the output write should not be full-screen if the surface position and size
  are unchanged.

Conservative fallback:

- Unknown placement/stacking/subsurface changes, output resize, failed damage
  transform, dmabuf buffer-size changes, and effects with uncertain coverage
  should mark full-output damage until tested.

## Incremental Damage Design

### Phase 1: Cause-Tagged Generation Metrics

Add debug-only or perf-only generation cause counters before changing behavior:

- `surface_buffer_commit`
- `surface_damage_only_commit`
- `surface_unmap_destroy`
- `surface_placement_change`
- `stacking_raise`
- `window_minimize_restore`
- `layer_surface_geometry_change`
- `resize_configure_only`

Goal: prove the `raw_input_events=0` churn is dominated by surface commits and
identify the first damage path worth optimizing.

### Phase 2: Output Damage Accumulator

Introduce an output-coordinate accumulator with:

- `Full`
- `Rects(Vec<OutputDamageRect>)`
- capped rect count with fallback to `Full`
- helpers for union, clipping, output-scale conversion, and old/new rect damage

Keep it separate from `render_generation`. Generation remains cache
invalidation; damage becomes "what pixels need scanout update".

### Phase 3: Cursor and Shell Overlay Damage

Port the proven EGL idea to the native path:

- Track previous/current software cursor rect.
- Track previous/current shell overlay generation and output regions.
- If only cursor/overlay changed, compose/copy/write only those rects.

For hardware cursor, cursor motion should not add scene damage. Only cursor
surface content/hotspot changes need cursor-plane update, not full scanout.

### Phase 4: Surface Commit Damage

On commit:

- Convert `RenderableSurfaceDamage::Partial` to output rects using
  `surface_origins()`, placement, buffer scale, viewport destination, and output
  scale.
- If surface size, placement, or buffer source changes, damage old and new
  visible surface bounds.
- For new/unmapped surfaces, damage the affected full surface/window bounds.
- For dmabuf commits, start conservative: use full surface bounds unless import
  metadata and partial-damage semantics are known to be correct.

### Phase 5: Native Partial Compose/Copy/Write

The native renderer needs a retained scanout model. Two possible directions:

- CPU retained frame: keep a previous full composed frame per scanout buffer,
  update dirty rects in `NativeFrameRenderer::frame`, copy only dirty rects to
  staging/mapped memory, and preserve untouched pixels.
- Native EGL/GLES-to-GBM renderer: render to real GBM/EGL buffers and let GPU
  scissor/damage drive partial presentation. This is the larger but more
  Hyprland/KWin-like path.

For the current GBM `gbm_bo_write` path, partial write viability must be
measured. If `gbm_bo_write` only accepts whole-buffer writes in practice, partial
damage still helps CPU composition/copy only after switching to mapped BOs or a
GPU render target.

## Low-Risk Fixes

No production fix was applied in this investigation. Low-risk follow-up changes
to consider:

- Add perf-only generation cause logging. It is reversible and does not change
  presentation behavior.
- Add a pure Rust damage accumulator module with unit tests, unused by the live
  path until validated.
- Add tests for old/new cursor rects, shell overlay region diffs, window
  old/new bounds, and surface damage-to-output conversion.
- Add native perf fields for `damage_kind`, `damage_rects`, and
  `damaged_pixels` once the accumulator exists.

## Measurement Commands

Current log summary:

```sh
python - <<'PY'
from pathlib import Path
import re, statistics
lines = Path('/home/agony/.local/state/oblivion-one/session.log').read_text(errors='replace').splitlines()
start = [i for i,l in enumerate(lines) if 'start-oblivion-one output=native' in l][-1]
rows = []
for i, line in enumerate(lines[start:], start + 1):
    if 'perf native.frame' not in line or 'phase=repaint' not in line:
        continue
    data = dict(re.findall(r'(\\w+)=((?:"[^"]+")|\\S+)', line))
    data = {k: v.strip('"') for k, v in data.items()}
    data['_line'] = i
    rows.append(data)
hw = [r for r in rows if r.get('cursor') == 'hardware']
print('hardware samples', len(hw))
for key in ['paint_us', 'render_us', 'copy_us', 'write_us']:
    vals = [int(r[key]) for r in hw]
    print(key, statistics.mean(vals), sorted(vals)[int(.95 * (len(vals) - 1))], max(vals))
print('raw0 clean', sum(r.get('raw_input_events') == '0' and r.get('pending_frame_work') == 'false' and r.get('redraw_requested') == 'false' for r in hw))
PY
```

Targeted log grep:

```sh
rg -n 'cursor=hardware|raw_input_events=0|render_generation|phase=repaint' ~/.local/state/oblivion-one/session.log
```

Suggested future live runs:

```sh
OBLIVION_ONE_PERF_LOG=1 OBLIVION_ONE_CURSOR=hardware ./bin/start-oblivion-one
OBLIVION_ONE_PERF_LOG=1 OBLIVION_ONE_CURSOR=software ./bin/start-oblivion-one
OBLIVION_ONE_PERF_LOG=1 OBLIVION_ONE_SCANOUT_BACKEND=dumb ./bin/start-oblivion-one
```

Suggested tests after a damage accumulator lands:

```sh
cargo test compositor::tests::surface_frames
cargo test compositor::tests::windows
cargo test native_output
```

## Before/After Criteria

Before:

- Hardware-cursor repaint samples still write `8294400` bytes per repaint.
- `paint_us` p95 is `15653 us`, above the 165 Hz budget.
- `copy_us + write_us` averages about `4548 us`.
- `1330` clean no-input hardware frames still repaint because generation
  changed.

After a successful incremental damage pass:

- Pointer-only hardware cursor motion emits no full `native.frame phase=repaint`
  lines.
- Software cursor fallback reports two small cursor damage rects instead of a
  full-output repaint.
- Surface damage-only commits expose `damage_kind=rects` and damaged pixels
  close to client damage, not `1920x1080`.
- Window move/resize damages old and new bounds, not the full output.
- `bytes`, `copy_us`, and eventually `write_us` scale with damaged area where
  the backend supports partial writes.
- No stale trails, missing redraws, or delayed frame callbacks in SHM, dmabuf,
  popup, browser, and resize cases.

## Risks

- Under-damage is visually worse than over-damage. Start with conservative
  full-output fallback on unknown paths.
- `RenderableSurfaceDamage` is surface-local; output damage must account for
  output scale, viewport, subsurface placement, server frames, and clipping.
- Partial CPU scanout needs retained buffer contents. Updating a dirty rect in a
  freshly selected GBM BO without valid previous pixels would leave stale areas.
- `gbm_bo_write` may not provide useful partial-write semantics. A mapped dumb
  path or native EGL/GLES path may be required for real `write_us` reduction.
- Pageflip completion and `present_frame()` timing are coupled today. Changing
  this affects frame callbacks, buffer release, presentation feedback, and
  explicit sync behavior.
- Browser dmabuf paths may not expose useful CPU pixels. Keep dmabuf damage
  conservative until GPU import and damage are integrated.

## Evidence

Files and commands used:

- `~/.local/state/oblivion-one/session.log`
- `docs/research/native-session-log-analysis-2026-06-11.md`
- `docs/research/hyprland-hardware-cursor-and-gecko.md`
- `src/native_output.rs`
- `src/compositor/render.rs`
- `src/egl_renderer/damage.rs`
- `src/egl_renderer.rs`
- `src/compositor/mod.rs`
- `src/compositor/protocols/core.rs`
- `src/compositor/state_data.rs`
- `src/compositor/surface.rs`
- `src/compositor/server.rs`
- `src/compositor/tests/windows.rs`
- `src/compositor/tests/surface_frames.rs`
- Metrics command: Python parser over
  `/home/agony/.local/state/oblivion-one/session.log`

Changes: this documentation file only.

Validation: code was not built or tested because this task was investigation and
documentation only. The log parser ran successfully against the latest session
log and produced the metrics above.

Risks: no runtime behavior changed. The main remaining assumption is that the
clean no-input generation churn is dominated by browser/client commits; the
recommended cause-tagged logging should verify that before behavior changes.
