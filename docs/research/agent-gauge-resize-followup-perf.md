# Agent Gauge resize follow-up perf

Date: 2026-06-11

Scope: read-only investigation of native SDDM/KMS resize performance after the
latest resize preview, no-upscale, old/new move damage, and copy-cap changes.
Production source was not edited.

## Executive summary

The current source already contains the intended low-risk resize primitives:

- Resize drag updates now call `preview_resize_root_window_to()` and advance
  `RenderGenerationCause::WindowResize`.
- Resize preview stores the committed client size and anchors content for
  left/top-edge resize.
- Preview blit uses a no-upscale target so old committed buffers are not
  stretched while the client has not committed the new size.
- Native output damage covers old and new bounds for `WindowMove`,
  `WindowResize`, and `SurfacePlacement`.
- The XRGB copy path caps overlapping damage copies by falling back to one full
  frame copy when summed rect bytes would be at least a full frame.

The latest available SDDM log at
`~/.local/state/oblivion-one/session.log` does not validate those newest
results yet. It contains `perf resize.*` events, but repaint rows have no
`scene_rebuild` field, no `render_cause=window_resize`, and still show
`copy_bytes > full_frame_bytes`. Treat it as a stale or mixed-generation
baseline until a fresh native run is captured from the current binary.

## Latest log metrics

Parsed latest native SDDM block:

- Start line: `25915`
- Start marker:
  `[2026-06-11T18:27:43-03:00] start-oblivion-one output=native renderer=gpu profile=release socket=oblivion-one-sddm`
- Repaint frames: `2882`
- Frame skips: `1131`
- `native.present_frame`: `1887`
- Resize perf events: `1383`
  - `resize.begin`: `8`
  - `resize.update`: `1367`
  - `resize.end`: `8`

Field presence in that block:

- `scene_rebuild`: `0 / 2882` repaint rows
- `render_cause=window_resize`: `0 / 2882` repaint rows
- `copy_bytes > 1920x1080x4`: `1244 / 2882`

Distributions:

| Metric | p50 | p95 | max |
| --- | ---: | ---: | ---: |
| `paint_us` | `9746` | `15770` | `36333` |
| `render_us` | `5160` | `10246` | `26892` |
| `copy_bytes` | `8294400` | `14126684` | `17915232` |
| `write_bytes` | `8294400` | `8294400` | `8294400` |
| `pageflip_drain_us` | `3` | `5` | `64` |
| `repaint_present_us` | `0` | `112` | `281` |

Repaint cause/damage distribution:

- `damage_kind=surface_damage`: `2494`
- `damage_kind=full`: `388`
- `render_cause=surface_commit`: `2494`
- `render_cause=redraw_requested`: `285`
- `render_cause=window_move`: `92`
- `render_cause=accepted_client`: `6`
- `render_cause=surface_unmap`: `5`

Pending work by cause:

- `pending_frame_work=true`, `surface_commit`, `surface_damage`: `1582`
- `pending_frame_work=false`, `surface_commit`, `surface_damage`: `912`
- `pending_frame_work=true`, `redraw_requested`, `full`: `263`
- `pending_frame_work=false`, `window_move`, `full`: `51`
- `pending_frame_work=true`, `window_move`, `full`: `41`

Interpretation: the captured log is still useful as a before/baseline, but it
cannot prove whether the current copy cap and `WindowResize` damage path are
working in the running SDDM binary. A fresh log should have `scene_rebuild=...`
on every repaint and should show `render_cause=window_resize` during resize
preview.

## Current code hotspots

### Native repaint loop

File: `src/native_output.rs`

- Repaint condition is still broad: accepted clients, render generation change,
  pending frame work, or explicit redraw all repaint.
- For each repaint, native output computes damage from current and previous
  renderable surfaces, paints, presents, then stores the latest surfaces.
- Logged fields now include `damage_kind`, `damage_rects`, `damaged_pixels`,
  `copy_bytes`, `write_bytes`, `scene_rebuild`, `render_cause`,
  `pending_frame_work`, pageflip timing, and CPU deltas.

Relevant source ranges:

- `native_output_damage_for_repaint()` selects partial surface damage, old/new
  bounds damage, or full output: `src/native_output.rs:3640`
- `WindowResize` is included with `WindowMove` and `SurfacePlacement` in the
  old/new bounds path: `src/native_output.rs:3650`
- `NativeOutputDamage::frame_copy_damage_for_scene()` forces full copy after a
  full scene rebuild: `src/native_output.rs:3415`
- Repaint logging happens after paint/present: `src/native_output.rs:885`

Biggest likely cost: even with correct output damage, a `scene_rebuild=full`
forces full framebuffer copy. Also, every GBM repaint still writes the full
scanout buffer through `bo.write()`.

### DesktopSceneRenderer

File: `src/compositor/render.rs`

Existing caches:

- Cached wallpaper buffer by output size.
- Cached scene buffer by frame size, output scale key, and content generation.
- Per-surface scene snapshots containing surface id, generation, target rect,
  and buffer size.

Partial rebuild already exists:

- If the scene is ready and snapshots are compatible, `rebuild_scene_from_damage`
  computes damage from previous/current snapshots.
- Target rect movement uses old and new target rects.
- Buffer-size changes redraw the current target rect.
- Surface full damage redraws the surface target.
- Surface partial damage maps client damage through output scale.

Remaining pressure:

- `compose_request()` always calls `copy_scene_to_frame()`, and
  `copy_scene_to_frame()` copies the complete cached scene to the frame when
  scene/frame lengths match.
- Shell overlay is blended after the scene copy. If shell overlay generation
  changes often, it can push content generation and scene/copy work even when
  client damage is small.
- Full scene rebuild disables partial scanout copy through
  `frame_copy_damage_for_scene(Full)`.

Relevant source ranges:

- Scene cache and rebuild kind: `src/compositor/render.rs:93`
- `compose_request()`: `src/compositor/render.rs:147`
- partial scene rebuild: `src/compositor/render.rs:235`
- full scene rebuild: `src/compositor/render.rs:286`
- full scene-to-frame copy: `src/compositor/render.rs:320`
- snapshot damage rules: `src/compositor/render.rs:479`

### Resize preview and no-upscale

File: `src/compositor/mod.rs`, `src/compositor/render.rs`

Confirmed current behavior:

- `queue_resize_root_window_to()` records a pending configure and immediately
  calls preview resize.
- `preview_resize_root_window_to()` updates renderable surface width/height and
  placement, sets `resize_preview`, marks surface damage full, and advances
  generation as `WindowResize`.
- Real buffer commit clears `resize_preview`.
- During preview blit, `resize_preview_content_target()` uses committed size
  and current preview size to avoid upscaling the old buffer during grow.
- Right/bottom anchoring keeps old content visually attached during left/top
  edge drags.

Relevant source ranges:

- `RenderGenerationCause::WindowResize`: `src/compositor/mod.rs:130`
- queue resize and preview: `src/compositor/mod.rs:2284`
- preview surface mutation: `src/compositor/mod.rs:2314`
- commit clears preview: `src/compositor/mod.rs:2938`
- preview target in blit path: `src/compositor/render.rs:1044`
- no-upscale extent: `src/compositor/render.rs:1137`

Likely visual corruption risks:

- Newly exposed preview area around the no-upscaled committed content must be
  covered by scene/background/server-frame damage. The old/new bounds damage
  path covers this at output copy level, but a full scene rebuild or incorrect
  partial scene rect can hide the bug until scanout copy becomes more partial.
- Top/left anchored resize is more sensitive because content x/y shifts while
  the window frame also changes.
- If a client commits a different size than the latest preview, the placement
  correction in the pending resize commit path must still match the committed
  dimensions before clearing preview.

### Copy cap and scanout write

File: `src/native_output.rs`

Confirmed current behavior:

- `copy_argb_frame_to_xrgb_mapping_damage()` computes `full_copy_bytes`.
- If damage is rect-based and `damage_rect_copy_bytes(...) >= full_copy_bytes`,
  it replaces rects with a single full-output rect.
- `damage_rect_copy_bytes()` sums clipped rect byte areas; it does not union
  overlapping rects.
- GBM still writes the full mapped buffer size every repaint.

Relevant source ranges:

- copy cap branch: `src/native_output.rs:4565`
- damage byte summation: `src/native_output.rs:4668`
- GBM staging copy and full BO write: `src/native_output.rs:4184`
- test forcing full copy after full scene rebuild: `src/native_output.rs:5030`
- test coverage for old/new resize damage exists next to move coverage:
  `src/native_output.rs:4913`

Expected result in a fresh current run:

- `max(copy_bytes) <= 8294400` for 1920x1080.
- `copy_bytes > full_frame_bytes == 0`.
- `copy_bytes == full_frame_bytes` may still be common when the scene rebuild
  is full or when damage rect sum reaches the cap.
- `write_bytes` remains `8294400` for every GBM repaint until partial BO write
  or a different scanout upload path exists.

## Biggest likely costs

1. Full scene-to-frame copy on every repaint.
   Confirmed by source: `copy_scene_to_frame()` copies the whole cached scene.
   The log cannot separate this from `render_us` yet, but p95 `render_us` is
   `10246 us` in the stale baseline.

2. Full GBM buffer write on every repaint.
   Confirmed by source and log: `write_bytes=8294400` for all `2882` repaints.
   This is independent of damage rect size.

3. Full scanout copy when scene rebuild is full.
   Confirmed by source and tests. Needs fresh log to quantify
   `scene_rebuild=full` during resize preview.

4. Resize preview generation churn.
   Confirmed by source: every changed preview advances `WindowResize`.
   Baseline log had `1367` resize updates but no `render_cause=window_resize`,
   so quantify only after a fresh run.

5. Pending frame work during client commits.
   Confirmed by stale log: `1582` surface-commit frames still had
   `pending_frame_work=true`, plus `1887` `native.present_frame` calls.
   This may be correct protocol pacing, but it is a resize-session pressure
   multiplier.

## Confirmed vs hypothesized

Confirmed by current source:

- Resize preview exists and uses `WindowResize`.
- No-upscale preview logic exists.
- Old/new bounds damage includes `WindowResize`.
- Copy cap exists in current source.
- Full scene rebuild forces full copy.
- GBM path writes full buffer every repaint.
- Scene partial rebuild machinery exists and uses snapshots/damage.

Confirmed by latest available log:

- The latest SDDM session recorded `1383` resize perf events.
- The same session still has `copy_bytes > full` in `1244` repaint rows.
- Repaints are dominated by `surface_commit` and `surface_damage`.
- Pageflip/present timings are low; paint/render/copy/write dominate.
- The log lacks `scene_rebuild` and `window_resize` repaint causes.

Hypotheses requiring fresh validation:

- The SDDM binary used for the latest log predates the current copy cap and
  `WindowResize` damage changes.
- Current resize preview will produce `render_cause=window_resize` and
  `damage_kind=surface_damage` with old/new bounds during active drag.
- `scene_rebuild=partial` should be possible for window resize because snapshot
  target changes can produce old/new target damage, but current code may still
  fall back to full if snapshots are incompatible or shell generation changes.
- Shell overlay generation can force extra content-generation churn during
  resize if it changes while client content does not.

## Low-risk next wins

1. Capture a fresh SDDM native resize log from the current binary.
   This is the required first win. Success means `scene_rebuild` appears on
   repaint rows and `copy_bytes > full_frame_bytes` is zero.

2. Add a parser gate for copy cap.
   Keep it outside production first: fail the analysis if any repaint has
   `copy_bytes > width * height * 4`. This catches stale binaries and future
   regressions quickly.

3. Quantify resize preview by cause.
   Parse `resize.update changed=true`, then correlate nearby repaint rows with
   `render_cause=window_resize`, `damage_kind`, `damage_rects`,
   `scene_rebuild`, and `copy_bytes`.

4. Prefer partial scene rebuild during `WindowResize` where current snapshots
   allow it.
   The source has the mechanism already. The low-risk step is measurement and
   a focused test, not a behavioral rewrite: prove whether resize preview is
   hitting `scene_rebuild=partial` or falling back to `full`.

5. Split render timing in a future instrumentation pass.
   `render_us` currently includes scene rebuild, full scene-to-frame copy, layer
   surface composition, and cursor handling. Add debug-only fields later such as
   `scene_rebuild_us`, `scene_copy_us`, `layer_surface_us`, and
   `cursor_blend_us`.

6. Keep GBM write optimization as a later, higher-risk item.
   Partial BO writes or direct damaged upload could reduce `write_bytes`, but
   it touches KMS/GBM correctness. Do it only after copy cap and resize damage
   are validated.

## Expected metrics after the next fresh run

For a 1920x1080 output:

- `full_frame_bytes = 8294400`
- `copy_gt_full = 0`
- `max(copy_bytes) <= 8294400`
- `scene_rebuild` field present on every repaint.
- `render_cause=window_resize` appears during active resize preview.
- `damage_kind=surface_damage` appears for `window_resize` frames where old/new
  bounds are non-empty.
- `write_bytes` remains `8294400` on GBM.
- `pageflip_drain_us` and `repaint_present_us` should remain low; any resize
  jank is more likely in render/copy/write than in pageflip.

Before/after criteria:

- Copy cap: pass if `copy_gt_full` falls from `1244` to `0`.
- Resize damage: pass if `window_resize` frames are mostly
  `damage_kind=surface_damage` and `copy_bytes < full_frame_bytes` for windows
  smaller than the output.
- Scene cache: pass if resize preview mostly reports `scene_rebuild=partial`
  after the first/initial frame. If it reports `full`, investigate snapshot
  incompatibility and shell generation.
- Visual correctness: pass if left/top and right/bottom resize drags show no
  stale exposed pixels, no stretched committed buffer, and no missing frame
  border/titlebar redraw.

## Parsing commands

Find latest native sessions:

```bash
rg -n "start-oblivion-one output=native" ~/.local/state/oblivion-one/session.log
```

Check whether a log is fresh enough for current source:

```bash
rg -n "scene_rebuild=|render_cause=window_resize|perf resize\\.|copy_bytes=" ~/.local/state/oblivion-one/session.log
```

Parse latest native block:

```bash
python - <<'PY'
from pathlib import Path
import re
from collections import Counter

path = Path("/home/agony/.local/state/oblivion-one/session.log")
lines = path.read_text(errors="replace").splitlines()
starts = [(i, line) for i, line in enumerate(lines) if "start-oblivion-one output=native" in line]
start, label = starts[-1]
end = len(lines)
for i in range(start + 1, len(lines)):
    if "start-oblivion-one output=native" in lines[i]:
        end = i
        break

rows = []
skips = []
resize = []
for no, line in enumerate(lines[start:end], start + 1):
    pairs = dict(re.findall(r'(\\w+)=((?:"[^"]*")|\\S+)', line))
    pairs = {key: value.strip('"') for key, value in pairs.items()}
    pairs["_line"] = no
    if "perf native.frame " in line and "phase=repaint" in line:
        rows.append(pairs)
    elif "perf native.frame_skip" in line:
        skips.append(pairs)
    elif "perf resize." in line:
        pairs["_raw"] = line
        resize.append(pairs)

def ints(key):
    values = []
    for row in rows:
        if key in row:
            values.append(int(row[key]))
    return sorted(values)

def pct(values, p):
    if not values:
        return None
    return values[min(len(values) - 1, round((len(values) - 1) * p / 100))]

full = 1920 * 1080 * 4
print("latest_start", start + 1, label)
print("frames", len(rows), "skips", len(skips), "resize_events", len(resize))
print("scene_rebuild_rows", sum("scene_rebuild" in row for row in rows))
print("window_resize_rows", sum(row.get("render_cause") == "window_resize" for row in rows))
print("damage_kind", Counter(row.get("damage_kind", "<missing>") for row in rows))
print("render_cause", Counter(row.get("render_cause", "<missing>") for row in rows))
print("scene_rebuild", Counter(row.get("scene_rebuild", "<missing>") for row in rows))
for key in ["paint_us", "render_us", "copy_us", "write_us", "copy_bytes", "write_bytes"]:
    values = ints(key)
    if values:
        print(key, "p50", pct(values, 50), "p95", pct(values, 95), "max", values[-1])
copy = ints("copy_bytes")
print("copy_gt_full", sum(value > full for value in copy), "full_frame_bytes", full)
print("resize_begin", sum("resize.begin" in row.get("_raw", "") for row in resize))
print("resize_update", sum("resize.update" in row.get("_raw", "") for row in resize))
print("resize_end", sum("resize.end" in row.get("_raw", "") for row in resize))
PY
```

Correlate resize updates with repaint causes:

```bash
python - <<'PY'
from pathlib import Path
import re

lines = Path("/home/agony/.local/state/oblivion-one/session.log").read_text(errors="replace").splitlines()
events = []
for no, line in enumerate(lines, 1):
    if "perf resize.update" in line or ("perf native.frame " in line and "phase=repaint" in line):
        fields = dict(re.findall(r'(\\w+)=((?:"[^"]*")|\\S+)', line))
        fields = {key: value.strip('"') for key, value in fields.items()}
        events.append((no, line, fields))

for index, (no, line, fields) in enumerate(events):
    if "perf resize.update" not in line or fields.get("changed") != "true":
        continue
    next_frames = [
        item for item in events[index + 1:index + 8]
        if "perf native.frame " in item[1] and "phase=repaint" in item[1]
    ]
    if not next_frames:
        continue
    frame_no, _, frame = next_frames[0]
    print(
        no,
        "->",
        frame_no,
        "cause",
        frame.get("render_cause"),
        "damage",
        frame.get("damage_kind"),
        "scene",
        frame.get("scene_rebuild"),
        "copy",
        frame.get("copy_bytes"),
    )
PY
```

Check copy-cap invariant across the whole log:

```bash
python - <<'PY'
from pathlib import Path
import re

full = 1920 * 1080 * 4
bad = []
for no, line in enumerate(Path("/home/agony/.local/state/oblivion-one/session.log").read_text(errors="replace").splitlines(), 1):
    if "perf native.frame " not in line or "phase=repaint" not in line:
        continue
    fields = dict(re.findall(r'(\\w+)=((?:"[^"]*")|\\S+)', line))
    if "copy_bytes" in fields and int(fields["copy_bytes"]) > full:
        bad.append((no, int(fields["copy_bytes"]), line[:240]))
print("copy_gt_full", len(bad), "full_frame_bytes", full)
for item in bad[:20]:
    print(item)
PY
```

Focused test commands after source changes:

```bash
cargo test native_output_damage_for_window_resize_covers_old_and_new_surface_bounds
cargo test native_xrgb_copy_damage_caps_overlapping_rects_at_full_frame_copy
cargo test resize_preview_does_not_upscale_undersized_committed_buffer
```

## Risks and blockers

- The available log is not sufficient to validate the current source. It should
  be treated as stale until a current binary emits `scene_rebuild`.
- Output damage and scene rebuild are separate layers. `damage_kind` can be
  partial while `scene_rebuild=full` still forces a full copy.
- Copy cap limits worst-case copy bytes but does not reduce full GBM writes.
- More aggressive partial scanout writes risk stale pixels if shell overlay,
  cursor mode, server-frame borders, and no-upscale exposed regions are not
  included in damage.
- Resize preview intentionally repaints during input interaction even with a
  hardware cursor. Do not optimize this like pointer-only movement.

## Final block

- Evidence: `src/native_output.rs`, `src/compositor/render.rs`,
  `src/compositor/mod.rs`, and
  `~/.local/state/oblivion-one/session.log`.
- Changes: this report only:
  `docs/research/agent-gauge-resize-followup-perf.md`.
- Validation: ran read-only code inspection and Python log parsing. No cargo
  tests were run because this was a read-only investigation/reporting cycle.
- Risks: newest source needs a fresh native SDDM resize run before declaring
  copy cap, `WindowResize` damage, or scene partial rebuild wins confirmed in
  runtime.
