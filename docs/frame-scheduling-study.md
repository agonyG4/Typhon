# Frame scheduling study

Status: study in progress (2026-10-23). Goal: redesign Typhon's frame
scheduling (prediction, Double/Triple credit, KMS lead) so it beats KWin,
Mutter and Hyprland on measured latency and missed-vblank rate on the target
hardware (RTX 3060 Ti, NVIDIA 615.78, DP-1 1920x1080@165).

## 1. How Typhon schedules today

- `src/native/adaptive_buffering.rs` — `AdaptiveRenderJournal` keeps 120-sample
  windows for two exact branches measured from composite start:
  - **R** (render-ready): until the render sync_file signals;
  - **Q** (queue-ready): until the frame is queued on the KMS worker.
- Branch risk (`BranchRiskHistory::selection`):
  `max(p95, min(ewma + 2·positive_dev, window_max))`. There is **no cap tied to
  the refresh interval**: one outlier in the window becomes the plan.
- Warm total = wake guard + KMS apply guard + `max(R, Q + kms_budget)`.
  This total is how far before the target vblank composition starts.
- `AdaptiveBufferingController` + `O1CreditDemandController` turn predicted
  overlap into future-primary credit 1 (Double) or 2 (Triple), with a
  clean-release horizon of 10 opportunities.
- Cold wake guard: after 100 idle refreshes, total ≥ refresh − 0.1 ms
  (same idea as KWin's `pageflipsSince > 100`).
- `presentation_deadline.rs` owns targets. `preRenderAbandoned` counts
  scheduled targets dropped **before** rendering (bookkeeping, not wasted GPU
  work).
- The EGL context already runs at `EGL_IMG_context_priority` HIGH.

## 2. What the measurements show

Live session, 60 s of `astreactl --json performance` every 250 ms with
`nvidia-smi` clocks every 100 ms (light desktop use):

| GPU graphics clock | R median | R p90 |
|---|---|---|
| ≤ 300 MHz (P8) | 6.03 ms | 6.26 ms |
| 300–1000 MHz | 2.56 ms | 4.47 ms |
| ≥ 1400 MHz | 1.50 ms | 2.08 ms |

- At P8 a composite takes about one whole refresh (6.06 ms).
- The large R spikes (15, 18, 20, 31, 93 ms) line up with the clock jumping
  210 → 1740 MHz. Because the selector reaches for the window max, the
  planned total then sits at 18–94 ms (3–15 refreshes ahead) until the spike
  leaves the 120-sample window.

Presentation trace (`~/Bench/typhon-trace/20261023-145905`, ~11k primary
frames, partial run):

| GPU clock | KMS submit → pageflip p50 | p95 |
|---|---|---|
| ≤ 300 MHz | 8.02 ms | 8.59 ms |
| 300–1000 MHz | 3.24 ms | 7.53 ms |
| ≥ 1000 MHz | 1.23 ms | 3.77 ms |

At low clocks the Atomic commit waits on the render fence past the intended
vblank, so the flip lands one refresh late.

Same run, `astreactl` stats every 0.5 s while watching a film (~30k frames):

| GPU clock | R latest p50 | p90 | p99 |
|---|---|---|---|
| ≤ 300 MHz | 6.19 ms | 6.63 ms | 9.75 ms |
| 300–1000 MHz | 3.13 ms | 5.67 ms | 7.63 ms |
| ≥ 1000 MHz | 1.05 ms | 2.37 ms | 4.46 ms |

- Planned total (composite start → target vblank): p50 11.0 ms, p90 17.2 ms,
  p99 33.7 ms, max 122.7 ms. **37.7 % of samples plan more than 2 refreshes
  ahead**, which KWin and Mutter would never do.
- The GPU spends most of the time at ≤ 300 MHz (1055 of 1542 samples).

Instrumentation: the trace had no render timing (`RenderStarted` /
`RenderCompleted` are never emitted). Commit 2fa59109 adds a `frame_timing`
event per composited pageflip with composite start, fence signal, target and
actual sequence, render-start deadline, credit and estimator total.
`~/Bench/typhon-trace/tools/trace_an.py` reads it.

### Baseline scenarios (2026-10-23, binary 2fa59109)

`~/Bench/typhon-trace/20261023-152758`, `frame_timing` events, misses
classified by real pageflip time against the target vblank:

| Scenario | fps | R p50 | R p99 | lead p50 | lead p99 | late /1000 | early /1000 | credit 2 | GPU clock p50 |
|---|---|---|---|---|---|---|---|---|---|
| desktop (before marks) | 108 | 2.69 | 6.61 | 7.32 | 11.76 | 8.6 | 60.7 | 70 % | 615 MHz |
| window drag | 113 | 2.04 | 9.93 | 9.07 | 11.79 | 7.4 | 14.8 | 100 % | 765 MHz |
| open/close/minimize | 137 | 3.04 | 6.23 | 7.74 | 11.77 | 12.2 | 1.6 | 96 % | 870 MHz |
| browser scroll | 132 | 1.86 | 4.50 | 7.28 | 11.67 | 4.8 | 12.3 | 77 % | 1335 MHz |
| idle desktop | 59 | 2.68 | 8.65 | 8.19 | 12.51 | 21.6 | 50.5 | 95 % | 870 MHz |

(times in ms; lead = composite start → target vblank)

- Real misses are 0.5–2 %. Composition starts 7–9 ms before a vblank that
  needs 2–3 ms of render at p50, and credit 2 is held 70–100 % of the time.
- "Early" frames reached the screen one refresh before their target: the
  target was planned further out than the hardware needed.

**Bug found (fixed in 0661c2b9):** `PresentationDeadlinePlanner::prepare_presented`
forced at least one refresh between recorded presentations. NVIDIA completes
cursor-only Atomic commits immediately with the current vblank's timestamp,
so each one added a phantom refresh. 749 of 14 102 composited frames were
presented on their target vblank but counted one sequence late, classified as
`KmsApplyGuardMiss` (`applyGuardMisses` 152 → 820 over the run), and grew
the adaptive apply guard from 250 to 414 µs.

### After the fix (binary 0661c2b9, `~/Bench/typhon-trace/20261023-155242`)

| Scenario | lead p50 before → after | credit 2 before → after | late /1000 before → after | early /1000 before → after |
|---|---|---|---|---|
| window drag | 9.07 → 5.82 | 100 % → 30 % | 7.4 → 6.2 | 14.8 → 2.6 |
| open/close/minimize | 7.74 → 7.15 | 96 % → 94 % | 12.2 → 18.9 | 1.6 → 0.0 |
| browser scroll | 7.28 → 6.54 | 77 % → 58 % | 4.8 → 12.6 | 12.3 → 3.3 |

- False sequence misses: 757 → 0. `applyGuardMisses`: 668 in the baseline
  window → 5. The adaptive apply guard decayed to 0 µs instead of growing to
  414 µs. Frames presented a refresh early: 526 → 17.
- Real late frames overall: 0.99 % → 1.30 % (140/14 102 vs 82/6 311). About
  half are submits that land too close to the vblank (0.52 → 0.67 %), the
  rest render fences after the target (0.40 → 0.60 %). The runs are short and
  had different GPU clocks, so this is within noise, but the submit share is
  worth watching now that the guard sits at 0.
- Credit 2 is still held 94 % of the time during animations.

### Queue-risk fix (binary d41f7d1f, `~/Bench/typhon-trace/20261023-162404`)

| Scenario | credit 2: 0661c2b9 → d41f7d1f | lead p50 | late /1000 | R p99 | GPU clock p50 |
|---|---|---|---|---|---|
| window drag | 30 % → 48 % | 5.82 → 5.83 | 6.2 → 11.5 | 9.00 → 6.26 | 930 → 750 MHz |
| open/close/minimize | 94 % → 87 % | 7.15 → 6.69 | 18.9 → 28.2 | 6.29 → 11.16 | 1020 → 1125 MHz |
| browser scroll | 58 % → 33 % | 6.54 → 5.78 | 12.6 → 8.2 | 5.20 → 6.58 | 1500 → 1740 MHz |

- Scroll improved clearly. Drag and animations moved with the workload: the
  animation segment's own render p99 was 11 ms (well over a 6.06 ms refresh),
  so credit 2 there is mostly justified, not a feedback artifact.
- Both branch risks still come from `evidence_ceiling` (the 120-sample window
  max) in every snapshot: `ewma + 2·dev` sits above the max, so one spike in
  the last ~1 s sets the plan. KWin/Mutter decay their upper estimate over
  seconds and cap it at 2–3 refreshes instead.
- Late frames 1.30 % → 1.54 %, now mostly GPU fences after the target (57 of
  92). The remaining lever is GPU cost at mid/low clocks, not queue
  accounting.

### Where the render time goes (`~/Bench/typhon-trace/20261023-164622`, `TYPHON_EFFECT_GPU_TIMING=1`)

Effect-graph GPU time joined to `frame_timing` by frame id:

| Scenario | R p50 | blur graph p50 | blur / R p50 | biggest blur part |
|---|---|---|---|---|
| window drag | 2.83 ms | 1.79 ms | 0.67 | capture 61 % |
| open/close/minimize | 2.74 ms | 1.73 ms | 0.65 | capture 56 % |
| browser scroll | 2.14 ms | 0.84 ms | 0.43 | composite 44 % |

- The blur graph is two thirds of the composite while windows move, and
  capturing the backdrop (scene replay into a texture) is most of it:
  ~0.9 ms replay capture per frame during drag.
- During drag/animations the capture caches miss: ~2–3 full refreshes per
  frame, a few kilopixels saved, `checkpoint_causal_scene_prefix_changed` on
  every instance. The scene under the blurred surfaces genuinely changes when
  a window moves. During scroll the caches work (3.1 Mpx saved per frame).
- `capture_downsample_fusion_ineligible` ≈ 1 per graph in drag/animations:
  the capture is copied at full resolution and then downsampled, instead of
  being written straight into the first Kawase level.

## 3. How the others do it

| | Estimator | Ceiling | Idle / power state | Miss handling |
|---|---|---|---|---|
| KWin (`core/renderjournal.cpp`, `core/renderloop.cpp`) | EWMA (500 ms) + 2× upward-only variance (6 s decay); min 2 ms per sample | `min(result + margin, 2 × vblank)` | after >100 idle vblanks, start ≥ one vblank early | Double↔Triple with 10-frame hysteresis; stays Triple if ≥ 0.95 vblank |
| Mutter (`clutter-frame-clock.c`) | separate **max** and **min** update-duration estimates; max jumps up instantly, decays over ~4 s; dynamic margin ≤ refresh/4 | 2× refresh (3× when triple buffering) | none explicit | if the max estimate is already late, retry with the **min** estimate before skipping a vblank |
| Hyprland (`output/MonitorFrameScheduler.cpp`) | none: render on the frame event | — | — | reactive: if the sync fd fires later than one refresh, render a third frame immediately and commit it on present |
| Typhon | per-branch EWMA + 2·dev, p95 floor, window max | **none** | 100-refresh cold guard | O1 credit 1↔2, clean-release horizon 10 |

None of them models the GPU's power state. KWin and Mutter only limit the
damage with a ceiling. That is the opening.

## 4. Directions for the redesign (to be confirmed by the trace)

1. **Ceiling.** Cap the planned total at 2× refresh (3× while Triple). A rare
   miss costs one frame. A 20–94 ms lead costs latency on every frame.
2. **DVFS-aware evidence.** Treat the GPU power state as a model input instead
   of noise: tag samples by regime (cold after idle, transition, warm) and
   keep separate histories, so a clock-transition spike does not poison the
   warm estimate. Sources to evaluate: idle gap before the frame (free),
   R itself, NVML clock query (cost to measure).
3. **Late-start fallback (Mutter min/max).** When the conservative start is
   already late, try the optimistic estimate for the same vblank before
   giving it up.
4. **Low-clock cost.** At P8 a composite costs one full refresh. Look at what
   keeps the GPU at 210 MHz during interaction and what the composite does
   there (blur passes, full repaints: `damage_area_threshold` caused 20k of
   26k full repaints in the first snapshot).
5. **Instrumentation first.** Emit `RenderStarted`/`RenderCompleted` in the
   trace, and record target sequence vs presented sequence so misses are
   exact instead of inferred from flip gaps.

## 5. Metrics for "best"

Measured on fixed scenarios (window drag, animations, scrolling, video,
VRR game), compared with KWin and Hyprland on the same machine:

- missed target vblanks per 1000 frames;
- planned lead (composite start → target vblank), p50/p99;
- input → pageflip latency, p50/p99;
- time spent at credit 2 vs 1;
- frame-time jitter (pageflip interval stddev during continuous motion).

## 6. Overlapping window blur (2026-10-23 evening)

Shared window blur fixes: capture stored bottom-left (4764ccb3), windows
over other windows fall back to exact blur like Hyprland's
`new_optimizations` (84026941), shared blur default on (be5a73ce), and the
shared backdrop is reused across frames while the scene below is unchanged
(06b0367a).

Stacked-window runs (`sobrepostos`, 165 Hz, up to 16 blurred windows):

| run | flag | fps | R p50 | R p99 | late/1k |
|---|---|---|---|---|---|
| 165456 (buggy shared blur) | on | 82 | 3.34 | 14.39 | 31.9 |
| 172848 | on | 144 | 2.99 | 6.21 | 14.0 |
| 181021 | off | 87 | 3.96 | 11.24 | 25.6 |

In 172848 and 181021 the shared blur was never used (never two eligible
windows), so both ran the exact path; the difference is the workload (181021
spent more frames with 10–16 stacked windows). The user reported both as
smooth. With 16 stacked blurs the checkpoint cache hits 14–16 captures and
the effect graph costs ~2.4 ms per frame. The 7–11 ms blur spikes do the same
pixel work as typical frames (27 M downsample pixels), so they are most
likely GPU clock (DVFS), not extra work. Remaining cost: every frame still
re-runs the Kawase blur and composite of every window even when its backdrop
is unchanged; caching blurred outputs per instance is the next cut.
Analysis tool: `~/Bench/typhon-trace/tools/rtail_an.py RUN LABEL`.

### Estimator check after the fixes

`tools/risk_sim.py` replays render-risk selectors over the exact R samples of
the day's traces (under% = frames whose R exceeded the predicted risk):

| run | current (max of 120) under% | p95 only | p95 + max(last 8) | p95 + max(last 24) |
|---|---|---|---|---|
| 155242 | 1.24 | 6.71 | 3.74 | 3.00 |
| 162404 | 1.36 | 6.72 | 3.92 | 3.25 |
| 172848 | 1.71 | 7.20 | 4.23 | 3.55 |
| 181021 | 1.11 | 6.68 | 3.17 | 2.49 |

Every faster-decaying selector under-predicts 2–6× more often. Credit 2 is
also mostly earned: R alone exceeds 0.6 refresh (the double-buffered budget
after wake and KMS guards) in 14–64% of frames. At 165 Hz the way back to
credit 1 is a smaller R (blur is ~2/3 of it), not a looser estimator. The
estimator stays as is.
