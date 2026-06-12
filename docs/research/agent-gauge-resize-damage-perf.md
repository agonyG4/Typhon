# Agent Gauge: Native Resize Damage Performance

Date: 2026-06-11

Scope: read-only performance investigation for native SDDM/KMS resize behavior.
No production source was edited.

## Biggest Likely Costs

1. Partial damage is being measured, but GBM scanout still writes the full
   frame. In the latest SDDM run, `damage_kind=surface_damage` appears on most
   repaint frames, but `write_bytes` remains `8294400` for every GBM repaint.
   Code confirms this: `NativeGbmScanout::paint_server_frame()` copies damage
   rects into staging, then always calls `buffer.bo.write(&self.staging)` and
   reports `write_bytes: byte_len`.

2. Damage rects are not unioned before CPU copy. The latest SDDM run has
   `1244` frames where `copy_bytes > 8294400`, with a maximum of `17915232`.
   That is more than two full 1920x1080 frames copied into staging. The native
   accumulator pushes surface rects into a vector and sums/copies them as-is;
   overlapping surface/window damage is copied repeatedly.

3. Resize sessions are still dominated by client surface commits and pending
   frame callbacks. In the SDDM run, `2494 / 2882` repaint frames had
   `render_cause=surface_commit`, and `1887` frames had
   `pending_frame_work=true`. That is expected during browser resize, but it
   means the compositor is often doing protocol completion and repaint work in
   the same loop.

4. Pageflip scheduling is still indirect. The loop drains pageflip events,
   calls `scanout.present()` before input/repaint, paints if needed, calls
   `scanout.present()` again, then calls `server.present_frame()` when pending
   frame work remains. `repaint_present_us` has p50 `0 us`, which means many
   repaint-side present calls return immediately, likely because a previous
   pageflip is still pending or there is no eligible ready buffer.

5. Visual correctness risk is concentrated in damage under/over-coverage, not
   in resize ACK basics. There are tests for resize configure, thresholded
   motion, left-edge anchor, and scaled-buffer logical size; however, native
   output damage currently only uses surface commit damage for
   `SurfaceCommit`/`SurfaceDamage`. Window move/resize, stacking, unmap,
   overlays, and full scene rebuilds still need conservative full damage or
   explicit old/new rect coverage.

## Confirmed Versus Hypothesized Costs

Confirmed from `~/.local/state/oblivion-one/session.log`, latest SDDM run:

- Run start: `2026-06-11T18:27:43-03:00`
- Mode: `1920x1080@165`, budget about `6060 us`
- Scanout: `GBM/KMS pageflip`, cursor `hardware`
- Repaint samples: `2882`
- Frame skips: `1131` `native.frame_skip reason=input_forwarded_no_visual`
- `native.present_frame` samples: `1887`

Key metrics:

| Metric | Average | p50 | p95 | p99 | Max |
| --- | ---: | ---: | ---: | ---: | ---: |
| `paint_us` | `9899.3` | `9746` | `15746` | `19028` | `36333` |
| `render_us` | `5254.4` | `5164` | `10236` | `13090` | `26892` |
| `copy_us` | `1559.3` | `1450.5` | `2726` | `3793` | `6349` |
| `write_us` | `3084.2` | `3115` | `3182` | `3267` | `6582` |
| `copy_bytes` | `8686869.3` | `8294400` | `14126684` | `15446304` | `17915232` |
| `damaged_pixels` | `2171717.3` | `2073600` | `3531671` | `3861576` | `4478808` |

Confirmed distributions:

- `damage_kind`: `surface_damage=2494`, `full=388`
- `render_cause`: `surface_commit=2494`, `redraw_requested=285`,
  `window_move=92`, `accepted_client=6`, `surface_unmap=5`
- `pending_frame_work=true`: `1887` frames
- `copy_bytes > full frame`: `1244` frames, all `render_cause=surface_commit`
- `write_bytes`: always full frame on GBM repaint
- Pageflip timings: `pageflip_drain_us` p95 `5 us`, max `64 us`;
  `present_us` p95 `163 us`, max `2923 us`; `repaint_present_us` p50 `0 us`,
  p95 `112 us`, max `281 us`

Hypotheses needing validation:

- The `copy_bytes > full` spikes are almost certainly overlapping damage rects
  and not a pitch bug. `damaged_pixels * 4` matches `copy_bytes`, and the
  accumulator does not union rects before copying.
- Some resize jank is likely pageflip/back-pressure: pending frame work is
  common, while repaint-side present often returns immediately.
- The latest source has `scene_rebuild` perf plumbing, but the latest SDDM log
  does not include `scene_rebuild=` fields. A fresh run is needed before tying
  resize stutter to `full` versus `partial` scene rebuilds.

## Concrete Bottlenecks

### Full GBM Write Despite Partial Damage

The native GBM path computes damage-aware `copy_bytes`, but it does not perform
damage-aware BO writes:

- `src/native_output.rs:4079-4086` copies only
  `damage.frame_copy_damage_for_scene(...)` into staging.
- `src/native_output.rs:4088-4099` writes the whole staging buffer with
  `buffer.bo.write(&self.staging)` and records `write_bytes: byte_len`.

Impact: even when `damage_kind=surface_damage`, `write_us` stays roughly
constant around `3.1 ms`. That is half of a 165 Hz frame budget before render
and copy are counted.

### Damage Rect Overlap / No Union

The native accumulator maps each damaged surface rect into output coordinates
and pushes it directly:

- `src/native_output.rs:3522-3532` iterates surfaces.
- `src/native_output.rs:3535-3547` pushes every clipped surface damage rect.
- `src/native_output.rs:3377-3388` stores rects and sums pixels without union.
- `src/native_output.rs:4530-4568` copies each rect row-by-row, so overlapping
  rects are copied repeatedly.

Impact: SDDM max `copy_bytes=17915232`, larger than two full frames. For large
browser surfaces or overlapping child/subsurface damage, partial damage can be
more expensive than full copy.

Low-risk guard: if summed damaged pixels exceed output pixels, or rect count is
high, collapse to full copy. Better fix: union/coalesce rects before copy.

### Scene Rebuild Still Needs Fresh Measurement

Current source has partial scene rebuild:

- `src/compositor/render.rs:219-231` attempts
  `rebuild_scene_from_damage`.
- `src/compositor/render.rs:253-292` redraws only output damage rects when
  previous and current surface snapshots match.
- `src/compositor/render.rs:488-530` returns partial damage only when surface
  layout is stable and changed surfaces carry `RenderableSurfaceDamage::Partial`.

But the latest SDDM log has no `scene_rebuild=` field, so the current capture
cannot prove whether resize frames are mostly `partial` or falling back to
`full`. Given `render_us` p95 `10236 us`, this field should be verified in the
next run.

### Resize Protocol Work Is Still Coupled To Present

`server.present_frame()` currently flushes resize configures, releases buffers,
completes callbacks, and presentation feedback together:

- `src/compositor/server.rs:226-232`
- `src/compositor/mod.rs:2687-2705` for pending frame callbacks

The native loop calls it after repaint when `server.has_pending_frame_work()`
is still true. During resize, this can make configure/callback progression
compete with paint and pageflip timing.

Impact in latest SDDM run: `1887` present-frame calls, matching
`pending_frame_work=true` repaint count. Pending frames also have higher average
paint/copy cost than non-pending frames.

### Resize Visual Corruption Risks

Already guarded:

- Configure-only resize does not advance render generation before client commit
  (`src/compositor/tests/windows.rs` has this coverage in the resize suite).
- Tiny/same-size resize motion avoids repeated visual update.
- Left-edge shrink keeps old buffer origin until client commit.
- Scaled-buffer resize keeps logical size separate from physical buffer size.
- Top-left client resize uses the requested edge and expected origin.

Remaining risks:

- Under-damaged old/new window bounds during resize can leave stale pixels
  around frame/titlebar/background if surface damage only covers client content.
- Full scene rebuild with partial copy damage would be corrupt, but source
  guards this by forcing full copy when `scene_rebuild == Full`. This must be
  validated in fresh logs with `scene_rebuild=`.
- Overlapping damage is visually safe but wasteful; rect union is needed before
  using `copy_bytes` as a win metric.
- Hardware cursor hides pointer damage, but software fallback still needs
  old/new cursor rect damage to avoid trails.

## Low-Risk Fixes To Consider

No fix was applied in this task. Low-risk follow-ups:

- Emit and verify `scene_rebuild=none|partial|full` in the native SDDM log.
- Add `damage_union_pixels` beside current summed `damaged_pixels`.
- Collapse damage to full when summed rect pixels exceed output pixels.
- Add rect coalescing/union before `copy_argb_frame_to_xrgb_mapping_damage`.
- Add `pageflip_pending=true/false` and present result fields so `present_us=0`
  can be separated into "already pending" versus "no ready buffer".
- Add resize-specific perf events: begin/update/end, edge, target size,
  configure serial, ACK serial, commit size, pending_frame_work.

## Measurement Commands

Summarize latest native sessions:

```sh
python - <<'PY'
from pathlib import Path
import re, statistics
from collections import Counter
lines = Path('/home/agony/.local/state/oblivion-one/session.log').read_text(errors='replace').splitlines()
starts = [(i, l) for i, l in enumerate(lines) if 'start-oblivion-one output=native' in l]
for start, label in starts[-3:]:
    rows = []
    skips = 0
    for no, line in enumerate(lines[start:], start + 1):
        if no > start + 20000:
            break
        if 'start-oblivion-one output=native' in line and no != start + 1:
            break
        if 'perf native.frame_skip' in line:
            skips += 1
        if 'perf native.frame ' in line and 'phase=repaint' in line:
            data = dict(re.findall(r'(\w+)=((?:"[^"]+")|\S+)', line))
            rows.append({k: v.strip('"') for k, v in data.items()})
    if not rows:
        continue
    full = max(int(r.get('bytes', 0)) for r in rows)
    print(label)
    print('rows', len(rows), 'skips', skips)
    print('damage', Counter(r.get('damage_kind', '') for r in rows))
    print('cause', Counter(r.get('render_cause', '') for r in rows))
    print('copy_gt_full', sum(int(r.get('copy_bytes', 0)) > full for r in rows))
    for key in ['paint_us', 'render_us', 'copy_us', 'write_us', 'copy_bytes']:
        vals = [int(r[key]) for r in rows if key in r]
        if vals:
            print(key, 'avg', round(statistics.mean(vals), 1), 'p95', sorted(vals)[int(.95 * (len(vals)-1))], 'max', max(vals))
PY
```

Find oversize damage-copy frames:

```sh
python - <<'PY'
from pathlib import Path
import re
lines = Path('/home/agony/.local/state/oblivion-one/session.log').read_text(errors='replace').splitlines()
for no, line in enumerate(lines, 1):
    if 'perf native.frame ' not in line or 'copy_bytes=' not in line:
        continue
    data = dict(re.findall(r'(\w+)=((?:"[^"]+")|\S+)', line))
    data = {k: v.strip('"') for k, v in data.items()}
    if int(data.get('copy_bytes', 0)) > int(data.get('bytes', 0)):
        print(no, data.get('index'), data.get('copy_bytes'), data.get('bytes'), data.get('damage_rects'), data.get('damaged_pixels'), data.get('render_cause'))
PY
```

Fresh validation run targets:

```sh
OBLIVION_ONE_PERF_LOG=1 OBLIVION_ONE_CURSOR=hardware ./bin/start-oblivion-one
rg -n 'scene_rebuild|copy_bytes|write_bytes|damage_kind|pending_frame_work|present_us|repaint_present_us' ~/.local/state/oblivion-one/session.log
cargo test compositor::tests::windows compositor::tests::surface_frames
```

## Before/After Criteria

Before:

- `write_bytes=8294400` on every GBM repaint.
- `copy_bytes > 8294400` on `1244` SDDM frames.
- `paint_us` p95 around `15.7 ms`, well over a 165 Hz budget.
- `pending_frame_work=true` on `1887 / 2882` repaint frames.

After a good damage/resize pass:

- `copy_bytes` never exceeds full-frame bytes unless explicitly labeled as
  overlapping diagnostic cost.
- `write_bytes` becomes partial on a backend that supports partial mapped
  writes, or the metric is clearly labeled full-write fallback.
- `scene_rebuild=partial` dominates surface-damage resize commits when layout
  is stable.
- Window move/resize damages old/new bounds exactly once, with no stale edges.
- Pageflip pending/result fields explain every `present_us=0` and every delayed
  `present_frame`.

## Evidence

- Log studied: `/home/agony/.local/state/oblivion-one/session.log`
- Prior local reports read:
  `docs/research/native-repaint-damage-bottlenecks.md`,
  `docs/research/native-session-log-analysis-2026-06-11.md`,
  `docs/research/hyprland-resize-refresh-mouse.md`
- Code read:
  `src/native_output.rs`,
  `src/compositor/render.rs`,
  `src/compositor/mod.rs`,
  `src/compositor/server.rs`,
  `src/compositor/tests/windows.rs`,
  `src/compositor/tests/surface_frames.rs`

Changes: `docs/research/agent-gauge-resize-damage-perf.md` only.

Validation: ran log parsing commands against the local session log and read the
current source paths. Did not run cargo tests because this was a read-only
research task and no production code changed.

Risks: the latest source includes `scene_rebuild` logging, but the latest SDDM
log does not show that field. Scene rebuild conclusions must be refreshed after
one more native SDDM/KMS run.
