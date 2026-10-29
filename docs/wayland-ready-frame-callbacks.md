# Frame callbacks at bounded READY admission

## Problem and evidence

Chromium's `WaylandFrameManager::MaybeProcessPendingFrame` waits for the previous
`wl_surface.frame` callback before sending another buffer. Typhon previously
marked callbacks rendered after `finish_render_owned`, but did not send them
until the rendered frame could enter the KMS submission lane. A successfully
rendered frame can therefore occupy a bounded READY slot while its client is
unnecessarily blocked by the predecessor's presentation or deferred target
binding.

The measured latency across all scroll-update events, including coalesced
updates, was 20.6 ms median on native Wayland versus 14.2 ms through
Xwayland/satellite at 165 Hz. This is evidence to
investigate the extra wait, not a measurement of the improvement from this fix.
The original live compositor predates the checkout, and different client
backends/presentation timestamp mechanisms limit that comparison.

References inspected:

- [Wayland frame callback contract](https://wayland.freedesktop.org/docs/html/apa.html#protocol-spec-wl_surface-request-frame):
  readiness to prepare another frame, independently of presentation feedback.
- [Chromium Wayland frame manager](https://github.com/chromium/chromium/blob/main/ui/ozone/platform/wayland/host/wayland_frame_manager.cc):
  `MaybeProcessPendingFrame`, `HandleFrameCallback`, and the separate
  `HandlePresentationFeedback` path.
- [Niri render loop](https://github.com/niri-wm/niri/blob/main/src/niri.rs):
  sends callbacks after rendering, with per-output refresh-cycle throttling.
- [Niri DRM backend](https://github.com/niri-wm/niri/blob/main/src/backend/tty.rs):
  presentation feedback is completed separately on the DRM vblank event.
- [KWin render loop](https://github.com/KDE/kwin/blob/master/src/core/renderloop.cpp):
  bounded pending frames and hysteresis avoid repeatedly changing the buffering
  depth. This fix preserves Typhon's output capacity and scheduling policy.

## Scope and design

Admit callbacks when the atomic EGL/GBM output successfully accepts the rendered
frame in its bounded READY slot. Both synchronous and worker rendering settle
through this same point. Do not admit callbacks during snapshot capture, GPU
dispatch, failed rendering, or direct-scanout candidate preparation.

Keep the existing per-batch terminal accounting. Later KMS admission and
pageflip must not send the callback a second time. The frame batch remains alive
and owns its presentation feedback, FIFO/commit-timing claims, damage, and buffer
release obligations until their existing completion events.

Alternatives considered were a client-specific Chromium flag and reducing global
buffering capacity. Neither corrects the callback coupling itself, and changing
global capacity can trade latency for dropped frames. No application rule or
unbounded callback pump is added.

## Implementation and verification

1. Name the rendered READY admission boundary without changing its old behavior.
2. Add live Wayland-client regressions demonstrating that READY callbacks must
   complete without KMS admission and must not complete twice afterward.
3. Verify the regressions fail with the old mark-only behavior.
4. Complete only callbacks at that boundary; keep presentation obligations held.
5. Run the new regressions, surrounding callback/release tests, and compile both
   the library and native runtime. Use only the stable Aether Cargo target tree.

A new native compositor session is required for a live before/after scroll
measurement. Unit tests establish the callback and ownership behavior; they do
not establish a reduction in physical mouse-to-display latency.

### Results

- Both new regressions failed with the old mark-only behavior and passed after
  READY admission completed callbacks. They also passed with test temporary and
  runtime directories on Aether.
- Callback suite: 86 passed, 2 failed. The same two failures occurred before the
  behavior change (84 passed, 2 failed):
  `compatibility_no_visual_change_with_callback_owns_one_terminal_batch` and
  `wayland_content_commits_skip_global_stack_reorder`.
- Native presentation/runtime tests: 94 passed. Atomic EGL/GBM tests: 33 passed.
- Strict all-target Clippy is blocked by existing diagnostics outside this
  change, including argument counts and boolean simplification warnings; it is
  not a passing check.
- The change does not restart the user's compositor or modify the Vesktop
  profile. The release artifact belongs in
  `/mnt/Aether/Desktop/GitHub/Typhon-target/release/oblivion-one`; the session
  launcher supports selecting it with `OBLIVION_ONE_BINARY`.
- The offline release build and the resulting binary's `--help` smoke check
  completed successfully. No live before/after improvement is claimed.

### Follow-up after session restart

The user's subsequent session ran a newer checkout build containing this fix.
The executable contained `admit_rendered_frame_callbacks`, and disassembly of
`AtomicEglGbmScanout::settle_render_completion` confirmed its call through the
corresponding relocation. The running executable's SHA-256 was
`1608c8ccebc0bf584591b10cc0e1b96f79bcc3333c9f385dbfcc2354ca0bbef1`;
the checkout head at validation was `e58ca77d`.

The same conversation and six synthetic scroll gestures were sampled again,
with AlwaysAnimate enabled and the window focused at the start and end:

| All scroll-update EventLatency, including coalesced updates | Original sample | Follow-up sample |
| --- | ---: | ---: |
| Median | 20.60 ms | 17.92 ms |
| p95 | 30.49 ms | 30.27 ms |
| p99 | 42.35 ms | 30.43 ms |

The follow-up recorded 1,826 requestAnimationFrame intervals over approximately
12 seconds, versus 1,718 originally, with no JavaScript long tasks in either
steady-scroll sample. These are partial improvements in observed measurements:
the builds and background load differ, p95 barely changed, and this is not an
isolated causal estimate or proof that the perceived problem is fully resolved.
The follow-up evidence is saved in
`/mnt/Aether/Desktop/GitHub/Typhon-target/diagnostics/post-fix-validation.json`.

### Remaining latency: presented events and pipeline stages

The aggregate metric above includes updates that Chromium coalesces without
presenting a distinct frame. Filtering for `GESTURE_SCROLL_UPDATE` events with a
`display_trace_id` gives a more specific measure of updates actually presented:

| Presented scroll updates | Original sample | Follow-up | Follow-up without FPS probe |
| --- | ---: | ---: | ---: |
| Samples | 849 | 850 | 851 |
| Median | 30.228 ms | 24.222 ms | 24.265 ms |
| p95 | 36.307 ms | 30.337 ms | 30.395 ms |
| p99 | 42.380 ms | 36.257 ms | 36.257 ms |

The later capture omitted the injected requestAnimationFrame measurement loop
to reduce probe interference, while retaining the same six synthetic gestures,
conversation, profiler, and trace. No JavaScript long tasks were recorded in
that capture. The approximately one-refresh reduction is consistent with
removing the callback hold; build/load differences still prevent attributing
the entire improvement exclusively to the patch. Synthetic gestures are
synchronized differently from physical mouse events, so these are not measured
physical input-to-panel latencies.

For the quiet capture's median presented event, the non-overlapping stages were:

| Stage | Duration |
| --- | ---: |
| Input transport and initial processing | 0.130 ms |
| Chromium compositor queue and wait for main-frame work | 7.068 ms |
| Main-frame work | 6.698 ms |
| Commit, activation, and submission preparation | 0.725 ms |
| Submission through presentation | 9.644 ms |

These are components of one representative event, not a sum of independent
percentiles. Layerize alone took 5.515 ms median and 6.521 ms p95 across the
725 layer-preparation events. This approaches/exceeds the 6.06 ms refresh
budget before the rest of the client pipeline and native composition.

The native Wayland swap acknowledgement took approximately 4.1 ms median.
Chromium's `MaybeProcessSubmittedFrames` waits for the preceding frame's buffers
to be released before acknowledging the next submission. Typhon's composed
release path transfers obligations to a GPU-fenced lease. This is a separate
source of backpressure from the callback hold fixed above; the trace alone
does not prove the GPU/buffer wait can safely be shortened.

A temporary `WaylandExternalBeginFrameSource` experiment did not complete the
CDP synthetic gestures, both with and without the FPS probe. At inspection the
parsed client log had no outstanding frame callback, so the experiment is
inconclusive and is not evidence of a compositor callback leak. The feature is
disabled by default upstream and was not enabled persistently. Vesktop was
restored to its original launch and settings after the experiment.

Evidence and the category-scoped asynchronous-event parser are in the stable
Aether diagnostics directory: `decompose-scroll.py`,
`pacing-default-quiet-trace.json`, `pacing-default-quiet-scroll-decomposition.json`,
and `vesktop-layers.json`. The layer snapshot contained 75 compositor layers;
that count alone does not establish a defect or justify removing visual effects.

### Follow-up: buffer release and swap acknowledgement

Matching frame IDs in the existing quiet trace confirms the Chromium ordering:
submission N is acknowledged after buffer N-1 is released, rather than after
buffer N itself is released. Across 1,404 matched frames:

| Client-observed interval | Median | p95 | p99 |
| --- | ---: | ---: | ---: |
| Previous buffer release to new submission acknowledgement | 0.065 ms | 0.769 ms | 2.149 ms |
| New frame callback delivery to previous buffer release | 2.053 ms | 3.390 ms | 4.051 ms |
| New frame callback request to previous buffer release | 4.547 ms | 8.732 ms | 11.538 ms |

These are measurements from client-side trace events. The last two rows include
GPU execution, compositor scheduling and client delivery; they do not isolate
any one of these costs. The first row shows prompt median acknowledgement once
the release arrives, with a longer tail. Swap waits overlap the existing input
pipeline and must not be added again to its total latency. Evidence:
`/mnt/Aether/Desktop/GitHub/Typhon-target/diagnostics/release-to-swap-ack.json`.

The GPU-fenced release registry now exposes three entries in the existing
`performance` JSON `timingScopes` map, without changing buffer ownership:

- `dmabuf_gpu_fence_wait`: registration to the kernel's actual completion-fence
  signal. A fence already signalled at registration contributes zero. This is
  remaining GPU wait after registration, not the entire render duration.
- `dmabuf_gpu_release_service`: wall time servicing a ready release, including
  the fence timestamp query and completion callback. This separates service
  work from the delay before that work starts. It can be sampled even when
  the GPU timestamp is unavailable.
- `dmabuf_gpu_signal_to_release`: kernel fence signal to return from completing
  the release obligations. This includes reactor scheduling, timestamp query,
  ownership revalidation and successful release signalling/event enqueueing.
  It excludes subsequent Wayland socket flushing and Chromium event handling.

All expose microseconds and aggregate the output, rather than identifying an
individual application. Use controlled scroll captures and correlate with the
client trace before attributing the samples to Vesktop. Counts, means and maxima cover the session; exact
p50/p95/p99 use the latest 256 valid samples. A delivery sample represents a
GPU-fenced lease with at least one successfully completed obligation. Deferred
current buffers, failed release signals, unavailable timestamps, zero signal
timestamps and completion clocks earlier than the GPU signal do not create a
delivery sample. Release paths outside this registry are not sampled. Always
check `count` before interpreting zero timings.

The release logic is unchanged, including direct-KMS ownership guards and
reactivation checks. A regression first failed with an empty timing snapshot,
then passed after recording completion. It verifies post-completion timing,
exact quantiles, exclusion of deferred obligations and readiness replay.
Another test verifies missing/inverted clocks do not block release or produce
false zero-delay samples. All 42 registry tests passed. The targeted Clippy
performance check remains blocked by diagnostics outside these changed files,
including large enum/error variants and an always-false protocol comparison;
it is not a passing check. Its output is in the Aether diagnostics directory
as `release-timing-clippy.log`.

The offline release build and its `--help` smoke check passed, and the binary
contains all three diagnostic scope names. A new compositor session using that
Aether executable is required to collect the new metrics. This instrumentation is not a demonstrated latency
fix, and no GPU-versus-reactor attribution is claimed before that measurement.
Query the running session with:

```sh
/mnt/Aether/Desktop/GitHub/Typhon-target/release/astreactl --json performance
```

Select the built compositor when launching the next session:

```sh
OBLIVION_ONE_BINARY=/mnt/Aether/Desktop/GitHub/Typhon-target/release/oblivion-one bin/start-oblivion-one-tty
```

### Measured GPU release path after restart

The new timing scopes were present in the running compositor after restart.
During the controlled `release-followup` scroll capture, the output-wide delta
covered 1,459 completed GPU-fenced leases:

| Interval | Mean over the measured counter delta |
| --- | ---: |
| Registration to completion-fence signal | 1.845 ms |
| Ready-release service routine | 0.013 ms |
| Completion-fence signal to successful release completion | 0.023 ms |

The representative in-scroll rolling medians were 1.834 ms, 0.012 ms and
0.016 ms respectively. The small post-signal delay contradicts the hypothesis
of a multi-millisecond reactor release delay in this capture. These aggregate
output measurements do not identify every lease with a specific application.
Presented scroll latency remained 24.333 ms median / 30.541 ms p95 / 36.425 ms
p99. Chromium Layerize took 5.904 ms median wall time, with a 5.855 ms median
thread CPU duration. The page had about 9,806 DOM nodes and 60 message nodes;
those counts alone do not establish a page bug. Native `perf record` sampling
was unavailable under the current process/kernel permissions, so no
per-function native C++ profile is claimed.

All 1,459 output frames in that sample were full repaints: 1,328 exceeded the
repair-area threshold and 131 lacked sufficient target history. A later
same-session blur comparison retained full repainting both enabled (4,294
frames) and disabled (3,799 frames). Median presented-event latency was
30.277/30.269 ms. The higher latency relative to the earlier session and
changing tails preclude treating this single sequential comparison as a
controlled estimate of blur's performance cost. It does show that disabling
blur did not remove the persistent full-repaint behavior. The original blur
configuration was restored byte-for-byte, and Vesktop was relaunched without
diagnostic arguments afterward.

Evidence in the stable Aether diagnostics directory:
`release-followup-trace.json`, `release-followup-samples.json`,
`release-followup-scroll-decomposition.json`, `release-followup-render-stages.json`,
`blur-on-render-stages.json`, `blur-off-render-stages.json`, and the matching
blur trace/decomposition files. Captures contain local page data and are not
included in the repository.

### Per-commit damage journal defect

Wayland damage belongs to an individual commit; the compositor may separately
accumulate all unpresented commits for rendering. See the
[Wayland damage contract](https://wayland.freedesktop.org/docs/html/apa.html#protocol-spec-wl_surface-request-damage).

The normal buffer-publication path accumulated new damage into
`RenderableSurface.damage`, then copied that already accumulated region into
`SurfaceDamageJournal`. Retained-buffer damage/mapping commits and retained
cursor commits had the same pattern. If A is rendered and B arrives before A's
pageflip, B's journal entry contains A union B. Presenting A then queries
`damage_since(A)` and incorrectly retains A through B. Continuous commits can
keep a Full region alive indefinitely, even when subsequent client requests
contain only small rectangles.

The correction records the normalized current commit's damage before merging
it into pending renderable damage. Rendering still retains all unpresented
regions, and presentation still consumes only its frozen commit token. New or
changed mappings still promote damage to Full. Xwayland's conservative Full
policy, SHM materialization, callback admission, buffer releases and output
buffer-age repair are unchanged.

The regression fixture uses a real Wayland client and a frame batch explicitly
bound to frozen surface-damage evidence. Four tests cover Full-to-partial
updates with rotated and retained buffers, disjoint partial regions (both
publication paths), and an empty rotated-buffer commit. All four fail against
the original implementation because presented regions remain pending. They
also assert that both regions remain pending before the older presentation,
so reducing the accumulated renderable damage itself would be incorrect.

Live performance improvement from this correction requires a new compositor
session with the rebuilt executable; the code regression is independent of
that subsequent measurement.

#### Damage-journal verification

| Check | Corrected implementation | Original implementation comparison |
| --- | --- | --- |
| New pipelined Wayland journal regressions | 4 passed | 4 failed with inherited Full/older regions |
| Surface/frame suite | 90 passed, 2 failed | 86 passed, same 2 failed (new regressions excluded) |
| Surface state tests | 31 passed | Not repeated; unchanged public contracts |
| Cursor-filtered library tests | 214 passed, 3 failed | 214 passed, same 3 failed |
| Renderer damage/repair tests | 91 passed, 1 failed | The failing threshold test also fails without the correction |
| Physical effect damage tests | 22 passed | Not repeated; unchanged effect propagation |

The six broader failures were verified against the original production code:

- `compatibility_no_visual_change_with_callback_owns_one_terminal_batch`
- `wayland_content_commits_skip_global_stack_reorder`
- `custom_client_cursor_is_restored_after_resize`
- `hidden_client_cursor_is_overridden_during_resize`
- `hidden_client_cursor_state_is_restored_after_resize`
- `effect_execution_repair_promotes_structured_plan_at_area_threshold`

The failing/passing regression cycle uses the same test fixture and frozen
surface-damage binding on both versions. It asserts pending-region preservation
before pageflip, exact newer damage after the older pageflip, and Empty only
after all sampled damage is presented. Test outputs are saved on Aether as
`damage-journal-*-tests.log`; all test runtime/temporary directories used Aether.
Rustfmt checks for the changed Rust files and `git diff --check` passed.

The final offline release build completed successfully using
`CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target`, and the resulting
`oblivion-one --help` smoke check passed. The executable is
`/mnt/Aether/Desktop/GitHub/Typhon-target/release/oblivion-one`. Its checksum and
build output are saved in `damage-journal-release.sha256` and
`damage-journal-release-build.log` under the stable diagnostics directory.
The current compositor process is not restarted by this change.

### Live check of d38f1a00 and active-scene damage cache

After the next restart, `/proc/<compositor>/exe` matched the built executable's
SHA-256 (`b0a25876235400098bc0fa28666dd001cfe2f859b879073c1fab7d10092699a6`).
The focused Vesktop conversation was captured with the same six synthetic
gestures, AlwaysAnimate enabled, a 1920x929 viewport at device scale 1, and no
injected requestAnimationFrame probe. No JavaScript long tasks were recorded.

| Presented scroll events | Before journal correction | After restart |
| --- | ---: | ---: |
| Samples | 852 | 851 |
| Median | 24.333 ms | 24.322 ms |
| p95 | 30.541 ms | 30.421 ms |
| p99 | 36.425 ms | 36.257 ms |

This is essentially unchanged latency, not evidence of a speedup. All 2,920
output frames during the later sampling interval remained full repaints
(2,723 area-threshold, 197 insufficient-history). The interval includes trace
export time; counts should not be compared as equal-duration throughput.
Across those leases, remaining GPU wait averaged 2.116 ms, ready-release
service 0.012 ms, and signal-to-release 0.019 ms. Chromium Layerize median was
5.536 ms. Session load and GPU clock differences limit causal interpretation
of the small changes. Evidence is saved as `damage-fixed-trace.json`,
`damage-fixed-scroll-decomposition.json`, `damage-fixed-samples.json`, and
`damage-fixed-render-stages.json` in the stable Aether diagnostics directory.
Vesktop was subsequently restored to its default launch without debugging.

The real native snapshot consumes `ActiveSceneView`, which holds cloned
renderable surfaces. `settle_surface_damage` updated pending damage in the
canonical surface vector but left this cached copy unchanged. Consequently,
even with fresh per-commit journal entries, the renderer could continue seeing
Full damage in the active scene. The previous regressions inspected canonical
surfaces and did not cover this additional boundary.

A new regression reads the native composition-plan surface projection. It
failed on d38f1a00: canonical damage was the newer 1x1 region, while native-scene
damage remained Full. The fix synchronizes only the cached damage after
presentation/no-visual settlement, using the existing surface index. It copies
damage only when buffer identity, generation and commit sequence match current
canonical content. Geometry, topology, scene generation and previously frozen
frame snapshots are not rewritten, and the active scene is not rebuilt.

All five pipelined journal/native-scene regressions pass with this change.
The native-scene regression covers both rotated and retained buffers, preserves
the newer commit's damage when an older frame presents, and observes Empty only
after all sampled damage has been presented. This is a code-path correction;
its live performance effect still requires loading the updated compositor.

Surrounding checks: surface/frame tests 91 passed with the same two previously
verified baseline failures; surface-state tests 31 passed; active-scene tests
2 passed. Changed-file rustfmt and `git diff --check` passed. Logs are saved as
`active-damage-*-tests.log` in the stable diagnostics directory. No passing
all-project Clippy result is claimed; the previously documented unrelated
baseline diagnostics remain outside this correction.

The active-scene correction's offline release build and binary `--help` smoke
check passed. The new executable remains at the stable Aether release path;
its checksum and build output are stored as `active-damage-release.sha256` and
`active-damage-release-build.log`. A new native compositor session is required
to measure this correction. The unchanged latency table above describes the
preceding journal-only build, not this active-scene cache correction.


## No-primary-work scheduler ownership regression (2026-10-29)

The first two live validation attempts with the active-scene cache correction
terminated the compositor before a scroll trace could be captured. The running
binary matched the new Aether release checksum. Both session termination
records reported `render attempt started without an active pacing owner`.
The subsequent Vesktop connection failures were consequences of the compositor
exit; there is no new scroll-latency result from these attempts.

Inspection found that `finish_no_primary_work` only retired the scheduler's
visual request when compositor protocol or surface-lineage settlement returned
true. Its caller unconditionally canceled the pacing ACTIVE owner. When the
scene was already current, damage empty and no frame batch remained, settlement
returned false and left scheduler visual work queued with no pacing identity.
The active-scene damage correction makes truly empty damage observable instead
of continually retaining stale Full damage, exposing this ownership mismatch.

The regression uses a real CPU-composition Wayland server, pairs scheduler and
pacing work through `queue_visual_work`, settles an empty cycle without a batch,
and cancels the pacing owner as the runtime does. Before the fix it fails because
visual work remains queued. With the fix it also verifies that new damage can
acquire ownership and begin a subsequent reactive render. The extracted
`settle_no_primary_work` helper initially preserved the old behavior for the
red run; it now completes the scheduler request regardless of the protocol
settlement result. The result still reports only actual protocol/lineage work,
and buffer release, frame callbacks and physical presentation are unchanged.

All eight presentation-cycle tests pass. Runtime-wide validation passed 734
tests with two ignored after excluding
`keyboard_repeat_deadlines_are_counted_in_wake_authority_metrics`. The excluded
test failed with `left: 15, right: 14`, both in the full suite and in isolation.
Restoring the old settlement condition and rerunning that test produced the
same failure, confirming it is independent of this correction. The fixed source
was restored before the final passing runtime run. Changed-file rustfmt and
`git diff --check` passed. Diagnostic logs use the `no-primary-owner-` prefix in
the stable Aether diagnostics directory.

The corrected offline release build and `--help` smoke check passed. The binary
and all build output remain in the stable Aether target directory. The checksum
is saved in `no-primary-owner-release.sha256`. Loading this build still requires
a new compositor session; stability with Vesktop and scroll performance have
not yet been measured live after this ownership correction.


## Live validation after scheduler ownership correction (2026-10-29)

The running compositor PID 28303 matched the corrected release SHA-256
`2de586c4da4e3885a95dffb701dc938ef19fe2bb33158a96f292d14a39f60fcc`.
It remained alive through Vesktop startup, two controlled scroll captures,
diagnostic Vesktop shutdown and restoration of the normal launch (PID 29812).
This validates those exercised operations; it is not a long-duration stability
guarantee. No compositor restart was performed by the diagnostic scripts.

Both runs used the same focused conversation, 1920x929 client area, DPR 1,
AlwaysAnimate enabled, six alternating synthetic mouse scroll gestures and no
injected animation-frame probe. Focus remained true and neither capture
recorded a JavaScript long task. Each run contained 852 presented scroll updates;
only updates carrying a display trace ID enter the latency quantiles.

| Capture | Presented p50 | p95 | p99 | Layerize wall p50 |
| --- | ---: | ---: | ---: | ---: |
| Previous journal-only (`damage-fixed`) | 24.322 ms | 30.421 ms | 36.257 ms | 5.536 ms |
| Cache + ownership correction (`owner-fixed`) | 24.393 ms | 30.497 ms | 36.481 ms | 5.506 ms |
| Repeat (`owner-fixed-repeat`) | 24.362 ms | 30.432 ms | 36.338 ms | 5.491 ms |

The latency result is effectively unchanged. The latest repeat's Chromium
renderer compositor queueing median was 5.912 ms, begin-main-frame-to-commit
7.165 ms, and submit-to-presentation 11.329 ms. Layerize is a measured component
of renderer work; nested stage medians must not be added as independent costs.
These synthetic input traces are not physical mouse-to-panel latency.

The complete performance sampling windows include trace export and time after
the gestures finish. Over these windows, repaint counts changed from 0 partial /
2920 full previously to 1956 partial / 987 full (66.46% partial, 25.06 seconds)
and 1790 partial / 947 full (65.40% partial, about 23 seconds). Registration-to-
fence-signal mean was 2.116 ms previously and 1.106 / 1.087 ms in the new windows.
This shows reduced work over the mixed capture, not that scrolling itself
became 66% partial or that its GPU cost halved.

To distinguish actual scrolling, `owner-fixed-scroll-only.json` uses only
performance snapshots whose monotonic timestamps lie inside the presented
scroll-event interval. This yields coarse interior windows rather than exact
per-frame gesture attribution:

| Capture | Interior duration | Partial / full frames | Partial share | GPU remaining-wait mean |
| --- | ---: | ---: | ---: | ---: |
| Journal-only | 4.009 s | 0 / 621 | 0% | 0.787 ms |
| Cache + ownership | 4.009 s | 30 / 593 | 4.82% | 0.670 ms |
| Repeat | 5.012 s | 31 / 756 | 3.94% | 0.678 ms |

Most partial repaint benefit is therefore outside active scrolling; scrolling
still usually promotes to full repair. This measurement does not establish
whether remaining full repairs originate in fresh client damage, buffer-age
repair expansion, effect dependencies, or a combination. Do not relax the
repair threshold or discard damage based on these counters. Chromium's roughly
5.5 ms Layerize cost and near-refresh queueing remain relevant measured costs.
Further optimization requires attribution at those boundaries.

Artifacts are `owner-fixed*` in the stable Aether diagnostics directory. The
normal Vesktop launch was restored and all six preexisting unrelated source
changes remain untouched. This validation changes documentation only; no new
build or test run is required for the report.


## Blur attribution after cache/ownership corrections (2026-10-29)

A new sequential blur-on/off comparison ran on the corrected compositor,
keeping the same conversation, geometry, animation plugin, synthetic gesture
sequence and no-RAF tracing procedure. Blur was changed only temporarily using
`astreactl blur reload`; a finally block restored the original configuration.
A byte comparison confirmed restoration. Vesktop was restored to its normal
launch and the compositor PID remained 28303.

| Capture | Presented p50 | p95 | p99 | Layerize wall p50 |
| --- | ---: | ---: | ---: | ---: |
| `cache-blur-on` | 24.422 ms | 30.490 ms | 36.435 ms | 5.445 ms |
| `cache-blur-off` | 24.274 ms | 30.401 ms | 48.363 ms | 5.438 ms |

This does not establish a scroll-latency improvement from disabling blur. The
p99 worsened in the off run; a sequential pair does not isolate scheduling/load
noise. In contrast, the repaint policy changed substantially in the interior
scroll windows, bounded by sample timestamps inside presented scroll events:

| Setting | Interior window | Partial / full | Partial share | Area / history full reasons |
| --- | ---: | ---: | ---: | ---: |
| Blur on | 4.010 s | 41 / 582 | 6.58% | 564 / 18 |
| Blur off | 5.012 s | 656 / 136 | 82.83% | 122 / 14 |

Most target buffers had age 2 (567/623 on, 755/792 off); ages above 3 explained
only 18 and 14 full repairs respectively. Thus the large difference primarily
tracks blur-associated area promotion rather than insufficient buffer history.
Whole-window GPU remaining-wait means were 1.149 ms on and 1.610 ms off, again
showing that repaint mode alone must not be equated with measured GPU latency.

The saved previous client protocol log was parsed by pending damage requests
per commit for the known root surface, with rectangles clipped to the settled
1920x929 content extent. Exact rectangle-union area avoids counting overlapping
client requests twice. Approximate realtime/monotonic alignment to the two prior
scroll intervals found 722/720 commits; only six in each exceeded 75% of the
1920x1080 output. Dominant fresh regions covered 1,263,245 pixels (60.92% output)
and 976,325 pixels (47.08%). Alignment is an estimated cross-clock window, not
frame-ID matching; startup damage and older padded buffer sizes are not used
to infer settled per-frame geometry. Files:
`owner-fixed-client-damage-analysis.json` and
`owner-fixed-scroll-client-damage.json`.

Source inspection explains a plausible amplification boundary: physical effect
damage expands source damage by each effect's aggregate sampling footprint,
then intersects the visible influence region. The current material position 0
and no blur override yield semantic blur 0.12; the built-in two-pass Kawase
radius is 1.8 and its conservative aggregate sampling radius computes to 69
pixels. This can substantially enlarge large scrolling regions. Later
buffer-age repair union and effect execution repair can also alter the region.
The exact promotion stage has not yet been identified per live output frame.
Do not shrink a proven sampling bound or remove required repair on this evidence.

Existing diagnostics already provide the missing boundary data: native perf
records include current-damage pixels, final repair pixels, age and fallback;
`TYPHON_EFFECT_EXEC_TRACE=1` adds initial repair, selected presentation output,
capture-only work, merged repair and final promotion outcome. The current
session has `OBLIVION_ONE_PERF_LOG=0` and effect tracing caches its setting with
OnceLock, so these diagnostics require a newly launched compositor. Logging
will be directed to the stable Aether diagnostics directory. This attribution
run will inspect causes; tracing overhead makes its latency unsuitable for a
clean before/after performance claim. No production behavior was changed in
this investigation.


## Separate Kitty background blur from Vesktop (2026-10-29)

The user pointed out that the terminal behind Vesktop has full-background blur.
This distinction matters: prior global blur toggles removed every window's
assignment and did not identify the owner of the costly blur. Do not interpret
those runs as showing a blur requested by Vesktop specifically.

The newly launched compositor PID 33812 has native perf and effect execution
tracing enabled, with logs written to Aether. A short attribution experiment
kept both windows and their geometry unchanged on workspace 1: active maximized
Vesktop occupied (0,71,1920,929), while Kitty occupied (72,72,1163,873) behind it.
A temporary last-matching window rule disabled blur only for app ID `kitty`.
Blur remained globally enabled, and the three client assignments remained;
the one automatic Wayland assignment disappeared. The original configuration
was restored in a finally block and the diagnostic Vesktop launch was restored
to normal. The compositor remained running. These runs use heavy logging and
are for attribution, not latency comparison with earlier clean captures.

Six alternating scroll gestures in each condition yielded:

| Condition | Partial frames | Full frames | Partial share |
| --- | ---: | ---: | ---: |
| Kitty blur on | 34 | 827 | 3.95% |
| Only Kitty blur disabled | 420 | 427 | 49.59% |

The trace contains 860/846 corresponding repaint provenance records (one fewer
than counter deltas at sampling boundaries). With Kitty blur on, 800 full
promotions first occurred in `initial_repaint_plan` due to
`damage_area_threshold`, and 27 due to `insufficient_history`. With only Kitty
blur disabled, these counts were 411 and 16. No observed full promotion first
occurred in final effect execution. Example frames show current logical damage
already reaching about 1.824 million pixels, or age-2/3 repair candidates
combining smaller current damage with prior damage before promoting to the
2.0736-million-pixel output. Thus final effect execution is not the first
promotion boundary in these records. The remaining distinction is upstream
physical damage expansion versus the initial buffer-age repair union.

This confirms that the terminal behind the maximized application affects
repair planning; it does not yet prove which effect or occlusion rule should
change, nor a reduction of input latency from removing that contribution.
Correct handling of opaque coverage, backdrop dependencies and partially
visible windows must be established before changing damage propagation.
Artifacts: `kitty-blur-attribution.json`,
`kitty-blur-attribution-analysis.json`, and the byte ranges recorded there in
`vesktop-attribution-session.log`. Temporary diagnostic scripts and outputs
remain in the stable Aether diagnostics directory. No production code or
persistent visual setting was changed by this experiment.


## Correct stationary effect identity and ordered backdrop damage (2026-10-29)

Two independent defects explain why a stationary blur could enlarge foreground
scroll damage. First, `ResolvedEffectScene.signature` includes the scene
publication generation. `finalize_snapshot` used that signature as the visual
effect identity, so an unrelated surface commit looked like an effect change
and `effect_transition_damage` invalidated the whole effect region. A new
`damage_identity_signature` hashes the same instance visual identities, regions,
coverage, anchors, scopes, reuse and order with a fixed generation seed. Native
visual damage combines it with the frozen program registry generation, so real
program reloads still invalidate effects even when instance parameters stay
fixed. The original scene-generation-bearing signature is retained
for scene identity and existing consumers. Actual effect instance changes still
damage their regions, and registry transitions keep their full-output fallback.

Second, physical effect expansion fed aggregate scene damage into every effect.
The built-in background blur now freezes a trusted BeforeSurface anchor/scope
alongside its immutable footprint. A content-only transition can use ordered
surface damage: each backdrop sees only surfaces below its source boundary,
plus expanded output of earlier backdrop effects. VisualGroup scopes start
before the group's first surface, so the window's own children do not become
its background source. The original base damage remains in the output.

This path requires unchanged surface membership/order, owners, geometry,
decorations, effect identities/recipes, registry, visibility and popup/overlay
membership. It also requires stable cursor/presentation geometry, opacity and
clips, no lifecycle/exit work, and no continuous frame-local effect demand.
Only the built-in BeforeSurface blur with exclusively Backdrop sources is
classified; unknown programs, TargetContent/StaticTexture sources, other anchor
kinds and unresolved source boundaries use the existing conservative expansion.
No sampling radius, repair threshold, buffer-age history, callback, buffer
release or KMS ownership rule was relaxed. Partially visible backdrops and
chains of lower effects keep their required influence.

The original-behavior frame seam produced a proper red regression: a 2x2
own-window/foreground update grew to a 160x160 region through a stationary
background blur. A separate proper red frame-snapshot test showed unchanged
effects becoming SurfaceDamage solely when publication generation advanced.
Both now pass. An additional red/green guard verifies that a real registry
reload still damages unchanged instance regions at initial scene planning,
before any physical pageflip history is updated. Pageflip-path tests also
verify lower-source expansion, group child exclusion and cascading lower blur; seven guard cases retain conservative
fallback for reorder, geometry, unknown recipe, real effect identity, registry,
continuous dirty and visibility changes. Actual effect identity changes remain
damaging. Existing clip, opacity, lifecycle, registry and buffer-history cases
remain in the runtime suite.

Validation: 739 runtime tests passed, two ignored, with only the separately
baseline-confirmed wake-authority count test excluded; 27 compositor effect
tests passed, along with both frozen-recipe output tests. Changed-file rustfmt
and `git diff --check` passed. The existing all-project Clippy baseline issues are not claimed resolved. Logs are saved
as `ordered-backdrop-*` and `stationary-effect-red.log` in stable Aether
diagnostics. The six preexisting unrelated dirty source files were preserved.
Live behavior and performance of this change still require loading the new
release; no numerical latency improvement is claimed from these code tests.

The final offline release build and executable `--help` smoke check passed;
checksum, help output and build log are saved as `ordered-backdrop-release.*`
and `ordered-backdrop-release-build.log` in stable Aether diagnostics. Reloading
the compositor is required to test this binary. For the next latency capture,
explicitly set `OBLIVION_ONE_PERF_LOG=0` and `TYPHON_EFFECT_EXEC_TRACE=0` because
the diagnostic session's child terminal may inherit enabled tracing settings.


## Live validation of ordered stationary blur correction (2026-10-29)

Running compositor PID 53538 matched the corrected Aether release SHA-256
`a0f106d6ededc3dbfa33f34a4b6101f5897557f5861437cf134ff14e912d0511`.
Native perf logging was 0; effect trace was absent (disabled by its parser).
Vesktop was maximized on workspace 1 at (0,71,1920,929), with Kitty behind it
at (72,72,1163,873). Blur remained enabled with three client assignments and
one automatic Wayland assignment; there were no temporary window rules.
Both captures kept focus and visibility, DPR 1 and AlwaysAnimate on, with
six alternating scroll gestures, no injected RAF probe and no JS long tasks.
The compositor survived both captures and Vesktop's normal-launch restoration.

| Capture | Presented updates | p50 | p95 | p99 | Partial share during interior scroll |
| --- | ---: | ---: | ---: | ---: | ---: |
| Earlier `owner-fixed` | 852 | 24.393 ms | 30.497 ms | 36.481 ms | 4.82% |
| Earlier repeat | 852 | 24.362 ms | 30.432 ms | 36.338 ms | 3.94% |
| `ordered-blur-fixed` | 851 | 24.430 ms | 36.286 ms | 42.371 ms | 65.09% |
| New repeat | 852 | 24.333 ms | 35.969 ms | 36.449 ms | 65.99% |

The interior windows contain 399 partial/214 full over 4.009 seconds and
518 partial/267 full over 5.012 seconds. Remaining full reasons were area
threshold 200/246 and insufficient history 14/21. Complete mixed capture
windows had 2553/493 and 2354/429 partial/full frames respectively; these must
not substitute for the scroll-only ratios. Earlier/new captures come from
different compositor sessions, not an interleaved same-session binary A/B.
The gesture and client procedure is the same, but session/load differences
limit causal claims about the smaller timing changes.

During the same coarse interior windows, delta means of the compositor CPU
render counter changed from 1.691/1.705 ms in the earlier runs to 1.327/1.320 ms
in the new runs (about 22% lower). The per-registration remaining GPU fence
wait changed from 0.670/0.678 ms to 0.828/0.842 ms. This wait is not total GPU
execution time and can depend on when registration follows CPU submission.
Ready-release CPU service stayed about 12 microseconds. Layerize medians were
5.554/5.556 ms in the new traces, versus 5.506/5.491 ms earlier.

The correction therefore reduced complete repaints and measured compositor
CPU render duration in these captures, but did not improve presented-scroll
median latency. The p95 is worse in both new captures, near 36 ms instead of
30.5 ms; it must not be presented as an overall latency improvement. Its cause
has not yet been isolated between session/load differences, renderer scheduling
and changed repair/GPU pacing. No additional timing-policy change was made on
these measurements. There is no claim that the remaining input lag is fixed.

Artifacts: `ordered-blur-fixed*`, `ordered-blur-validation-summary.json` and
`ordered-blur-scroll-costs.json` in stable Aether diagnostics. The summary
script records both scroll-bounded counters and trace quantiles. No new code
or build was required for this validation report; the six unrelated dirty
source files remain untouched.


## Vblank-quantized attribution of the remaining scroll latency (2026-10-29)

No new capture was taken for this analysis; it reuses the four comparable
`quietbench` traces and the WAYLAND_DEBUG client logs of `owner-fixed` and
`ordered-blur-fixed-repeat` (the latter preserved as
`ordered-blur-fixed-repeat-client.log`). Presented scroll latency is strongly
quantized to the 6.060637 ms refresh: the 24.4 ms median is four refreshes and
the reported p95 values are five (30.4 ms) or six (36.3 ms) refreshes.

| Capture | 3 V | 4 V | 5 V | 6 V | ≥7 V | Share ≤5 V |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `owner-fixed` | 22 | 451 | 345 | 33 | 1 | 96.0% |
| `owner-fixed-repeat` | 28 | 465 | 337 | 22 | 0 | 97.4% |
| `ordered-blur-fixed` | 43* | 403 | 344 | 49 | 12 | 92.8% |
| `ordered-blur-fixed-repeat` | 32 | 442 | 335 | 43 | 0 | 95.0% |

\* Includes two 2 V events.

The p95 sits on the 5 V/6 V boundary, so the apparent p95 regression is roughly
2–3% of presented updates crossing one extra refresh. In `ordered-blur-fixed`
the twelve 7–9 V events have `SendBeginMainFrameToCommit` of 13–21 ms, i.e.
renderer main-thread stalls, while their submit-to-presentation stays near
12 ms. Buffering mode does not explain the shift either: predictive-triple share
inside the scroll windows was 61%/67% in the earlier runs, 31%/30% in the new
runs, and 18%/14% in the older `damage-fixed`/`release-followup` runs, all with
the 30.4 ms p95 except the two new ones.

Joining each presented Chromium event to its Wayland commit by presentation
timestamp (all 851/852 matched) isolates the compositor share. Commit times are
a lower bound because the client log clock is aligned with the minimum feedback
delivery offset.

| Client log | Scroll commits | Commit→presented p50/mean/p95 | Commit→frame callback p50/p95 |
| --- | ---: | ---: | ---: |
| `owner-fixed` | 723 | 8.56 / 8.78 / 13.71 ms | 3.0 / 7.2 ms |
| `ordered-blur-fixed-repeat` | 705 | 8.02 / 8.33 / 13.81 ms | 2.6 / 7.3 ms |

The compositor share did not get worse with the ordered blur correction; it is
slightly lower. Grouping by total latency, the 4 V→5 V step adds about 2.4 ms
before the commit (input→commit 17.5→19.8 ms) and 3.7 ms after it
(commit→presented 6.8→10.4 ms); the 5 V group's frame callback arrives later
(median 4.4 vs 2.0 ms after commit). About 11% of scroll commits (98/723 and
76/705) are issued within 0.5 ms of the previous commit's frame callback, i.e.
Chromium was still waiting on it; they concentrate in the 5 V group.

A typical 4 V event spends about 5.7 ms in `RendererCompositorQueueingDelay`,
8.7 ms in renderer main-thread work up to commit (Layerize ≈ 5.5 ms), about
2 ms in activation/draw/submit, and 6.8 ms between Wayland commit and
presentation. The queueing delay is nearly constant (p95 6.0 ms) because the
synthetic gesture generator injects input aligned with BeginFrame; physical
input arrives at random phase, so this part of the benchmark overstates real
mouse latency by roughly half a refresh and must not be attributed to Typhon.

As a reference, an idealized compositor that presents each commit at the first
refresh at least L ms after it, in FIFO order, would give commit→presented
mean/p95 of 6.15/9.06 ms for L = 3.0 ms and 6.91/9.77 ms for L = 3.5 ms on the
same commit timeline, versus 8.33/13.81 ms measured. Allowing latest-state
replacement instead of FIFO saves only another ~0.3–0.4 ms, so mailbox-style
replacement of queued frames is not the main lever. The gap of roughly 1.4–2.2
ms mean and 4 ms at p95 is not yet attributable: the compositor waits for the
client's syncobj acquire point before applying a commit, and the client log
cannot separate Chromium GPU completion from Typhon render, READY admission and
KMS timing. Measured compositor render-ready (p50 2.75 ms, ceiling 4.9 ms) and
fence-after-rendered (p50 1.8 ms) figures leave this open.

Artifacts: `commit-latch-analysis.py`/`.log`, `vblank-bucket-stages.py`/`.log`
and `scroll-buffering-modes.py`/`.log` in stable Aether diagnostics. No timing
policy was changed based on this analysis.


## Live per-commit pipeline stream (2026-10-29)

`TYPHON_SURFACE_PIPELINE_TRACE` already records commit capture, acquire
pending/ready, transaction promotion, scene sampling, frame callback admission,
output frame rendered/submitted/presented, presentation feedback and buffer
release with surface, commit and output-frame identities. It only kept a
512-record ring printed at shutdown, with timestamps relative to a process-local
epoch, which cannot cover a scroll capture or be joined with client traces.

The value `stream` now also prints every record as it happens, appending an
absolute `monotonic_ns=` stamp (CLOCK_MONOTONIC, the clock used by
wp_presentation and Chromium trace timestamps). `1`/`true`/`on` keep the
previous ring-only behavior, and the shutdown dump format is unchanged. This is
diagnostic only and is off by default. Unit tests cover mode parsing and the
streamed line format (`pipeline-stream-tests.log`).

Next capture: start the compositor with
`TYPHON_SURFACE_PIPELINE_TRACE=stream OBLIVION_ONE_PERF_LOG=0
TYPHON_EFFECT_EXEC_TRACE=0` and a log file on Aether, run
`capture-with-performance.py`, then split the commit→presented gap per commit
into acquire wait, wait for a render slot, render to READY, READY to KMS
submission, and submission to presentation.


## Per-commit pipeline capture after restart (2026-10-29)

The user restarted the compositor (PID 68699) with the release whose SHA-256 is
`d53df0f11e46169cf37a59aa7d75b89009e7062e5d768aeead97849d47e18e64`,
`TYPHON_SURFACE_PIPELINE_TRACE=stream`, native perf and effect trace disabled,
and `OBLIVION_ONE_DIRECT_SCANOUT=experimental-auto`. This build was made from
the working tree, so it also contains the six unrelated uncommitted files
(direct scanout/dmabuf feedback work); no direct scanout counter changed during
the scroll window. Vesktop ran under the diagnostic supervisor in the same
maximized geometry with Kitty behind and blur unchanged (four assignments).
The streamed trace wrote about 107 MB during the capture, so this session has
logging overhead and is not a clean A/B against earlier captures.

| Capture | Mean | p50 | p90 | p95 | p99 |
| --- | ---: | ---: | ---: | ---: | ---: |
| `owner-fixed` | 27.08 | 24.39 | 30.44 | 30.50 | 36.48 |
| `ordered-blur-fixed-repeat` | 27.03 | 24.33 | 30.42 | 35.97 | 36.45 |
| `pipeline-stream` | 25.87 | 24.22 | 30.35 | 32.99 | 54.43 |

Vblank buckets were 3 V: 82, 4 V: 583, 5 V: 145, 6 V: 9 and 33 events at
7–13 V, so mass moved from 5 V to 4 V (335→145) while a new tail appeared.
The compositor share fell: Wayland commit→presented p50/mean/p95 was
6.38/6.51/10.11 ms (8.02/8.33/13.81 before) and commit→frame callback p50 1.68
ms (2.6 before). Only 10/677 commits waited on the previous frame callback.

The compositor-side stream agrees with the client log (commit→presented p50
6.28 ms) and splits it per commit (677 scroll commits on surface 28):

| Stage | p50 | p95 | max |
| --- | ---: | ---: | ---: |
| Commit → acquire ready (client GPU) | 0.29 | 0.84 | 12.58 |
| Promotion → scene sampled (render slot + render) | 1.10 | 2.86 | 6.32 |
| READY → KMS submission | 0.10 | 0.18 | 10.16 |
| KMS submission → presented | 4.56 | 7.12 | 13.74 |

In the median case Typhon renders and submits about 1.5 ms after the commit
and the rest is waiting for the scanout refresh, close to the idealized
deadline model (mean 6.25 ms at L = 3 ms). The 24 commits over two refreshes
averaged 3.7 ms acquire wait (Chromium GPU work not finished), 3.5 ms waiting
for KMS admission behind an earlier flip and 8.6 ms from submission to
presentation; those stalls produce the 7–13 V Chromium tail together with
renderer main-thread stalls. Their relation to the streaming log I/O is not
established.

Inside the scroll window, predictive-triple frames were 28/505 (5.5%),
against 30%/30% in the ordered-blur captures and 61%/67% in `owner-fixed`.
Across the three captures with client logs, compositor commit→presented means
order the same way (8.78, 8.33, 6.51 ms). This is a correlation over three
sessions, not proof: the render-ready risk estimator uses an evidence ceiling
over its window, and a long-running session can keep render-ahead engaged,
which queues frames one refresh deeper. This matches the earlier observation
that render outliers pin the triple path. No pacing policy was changed.

The remaining 24.2 ms median is now mostly upstream of the commit: for 4 V
events input→Wayland commit is 18.2 ms (one synthetic-input refresh of
queueing, about 9.5 ms renderer main thread including Layerize, and draw).
Artifacts: `pipeline-stream-*` (trace, samples, client log, stage, latch and
bucket logs, byte offsets into `pipeline-stream-session.log`) and
`pipeline-stream-stages.py` in stable Aether diagnostics.


## Clean fresh-session capture and gesture-start transient (2026-10-29)

The compositor was restarted again (PID 84223, 17:09) with the same
`d53df0f1…` release and every trace disabled. The six unrelated dirty files
predate the earlier `a0f106…` release build (last modified 2026-10-23 and
2026-10-28), so both releases contain them and the only code difference between
the two binaries is the disabled stream trace. Vesktop was maximized on
workspace 3 with a Kitty window behind it at (496,102,1163,873) — same size,
different position from earlier captures — and blur unchanged (four
assignments). Capture `fresh-clean` used the usual `quietbench` procedure.

It reproduces the `pipeline-stream` profile without logging overhead:
compositor commit→presented p50/mean/p95 6.59/6.73/11.07 ms, commit→frame
callback p50 1.57 ms, predictive-triple 56/505 frames in the scroll window, and
vblank buckets 3 V: 80, 4 V: 575, 5 V: 154. Both fresh sessions also show a
tail of 33–37 events at 7–10 V that earlier sessions did not. All of them fall
in the first ~230 ms of the benchmark (one in `pipeline-stream` came later).

| Capture | Window | n | Mean | p50 | p95 | p99 |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `owner-fixed-repeat` | after first 250 ms | 810 | 26.68 | 24.36 | 30.43 | 36.30 |
| `ordered-blur-fixed` | after first 250 ms | 809 | 27.33 | 24.41 | 36.28 | 42.37 |
| `ordered-blur-fixed-repeat` | after first 250 ms | 810 | 26.76 | 24.31 | 30.47 | 36.41 |
| `pipeline-stream` | after first 250 ms | 793 | 24.73 | 24.21 | 30.36 | 30.46 |
| `fresh-clean` | after first 250 ms | 810 | 24.87 | 24.28 | 30.40 | 30.48 |
| `fresh-clean` | all | 851 | 26.09 | 24.29 | 33.30 | 54.57 |

Apart from the initial transient, both fresh sessions are about 2 ms better in
mean and one refresh better at p99 than the older long-running sessions, with
the same four-refresh median. The per-commit stream of `pipeline-stream` shows
what happens during the transient: client acquire waits of 5–12.6 ms (Chromium
GPU work not finished when Typhon receives the commit), frames waiting 3–10 ms
for KMS admission behind an earlier flip and 12 ms from submission to
presentation. After that the stage shifts to render-slot waits of ~6 ms that
drain within ~50 ms, and from about 240 ms commits again present 2–7 ms after
arrival. At rest the GPU was in P8 (315 MHz graphics, 405 MHz memory) with
PowerMizer adaptive mode, consistent with a clock ramp after idle slowing both
Chromium and Typhon GPU work. This is inferred from timing and the idle
P-state; the clock trajectory during the gesture was not recorded.

For a physical user this transient would repeat at the start of each scroll
after idle, which matters more for perceived input lag than the steady-state
p95. Whether long-running sessions hide it by keeping predictive-triple
render-ahead engaged (at the cost of the extra refresh measured above) is not
yet established. A capture after several hours of normal use in this same
session will test the session-age hypothesis. No pacing, power or GPU setting
was changed.

Artifacts: `fresh-clean-*` (trace, samples, client log, latch, bucket and
steady-state logs) in stable Aether diagnostics.
