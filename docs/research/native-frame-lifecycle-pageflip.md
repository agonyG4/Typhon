# Native Frame Lifecycle and Pageflip Research

Date: 2026-06-11

Scope: `src/native_output.rs`, `src/compositor/server.rs`,
`src/compositor/output.rs`, frame callbacks, presentation feedback, buffer
release, explicit sync release, and KMS pageflip completion.

Update 2026-06-11: the first remediation slice landed. The native GBM loop now
finishes compositor frame work from DRM pageflip completion, exposes a separate
`finish_frame()` server step, blocks new native client dispatch/repaint while a
pageflip is pending, and logs `native.finish_frame reason=pageflip_complete`.
Remaining work from this note still applies to DRM-fd/libinput-driven wakeups,
precise pageflip timestamp/sequence propagation, and the future EGL/GLES render
path.

Update 2026-06-19: DRM, Wayland, and input fds now drive an epoll reactor and a
monotonic absolute timerfd supplies protocol-only deadlines plus a bounded
pageflip watchdog. The scheduler never completes an asynchronous frame from a
timer.

Update 2026-06-19 (presentation metadata): legacy pageflip events are now
length-validated and matched through unique user-data tokens. Kernel seconds,
microseconds, and `u32` sequence values reach `wp_presentation` unchanged apart
from exact microsecond-to-nanosecond conversion. The advertised clock follows
`DRM_CAP_TIMESTAMP_MONOTONIC`, and feedback conservatively reports `VSYNC`
without `HW_CLOCK`, `HW_COMPLETION`, or `ZERO_COPY`.

Non-goals:

- Do not patch production compositor code in this document.
- Do not redesign rendering, damage tracking, or input dispatch here.
- Do not change Wayland protocol, CLI, JSON, or service contracts.

## Summary

The native backend now uses GBM/KMS pageflips and can move the pointer through a
hardware cursor, but the client pacing boundary is still not the real scanout
boundary.

Main finding: `server.present_frame()` is called from the native loop after a
repaint attempt, not from DRM pageflip completion. For a freshly rendered frame,
`scanout.present()` has either only queued a pageflip or was blocked by an
already pending pageflip. Immediately after that, `present_frame()` releases
buffers, signals explicit-sync release points, completes `wl_surface.frame`
callbacks, and sends `wp_presentation_feedback.presented`.

That means Oblivion can tell clients "the frame is done/presented/released"
before the KMS pageflip that displays that content has completed. In one common
triple-buffer case, it can do this before the repaint has even been scheduled
for pageflip.

The native loop also samples the DRM fd with `poll(..., 0)` and then sleeps from
an internally calculated timer. The DRM fd is not the wake source for the loop,
so the compositor is still timer-paced rather than vblank/pageflip-paced.

## Current Oblivion Lifecycle

### Native loop ordering

In the native loop:

1. `server.tick()` accepts and dispatches Wayland clients.
2. `scanout.drain_page_flip_events()` samples DRM completion.
3. `scanout.present()` tries to submit any previously ready buffer.
4. Input is drained and applied.
5. If clients, render generation, pending frame work, or redraw require it,
   `paint_server_frame()` renders into a GBM buffer.
6. `scanout.present()` tries to submit that repaint.
7. If `server.has_pending_frame_work()` is still true, `server.present_frame()`
   completes all frame work.
8. The thread sleeps through `native_wakeup_interval()`.

Evidence:

- `src/native_output.rs:727-763` dispatches clients, drains pageflip events, and
  tries an early `present()`.
- `src/native_output.rs:825-852` repaints and tries another `present()`.
- `src/native_output.rs:881-884` calls `server.present_frame()` whenever frame
  work remains after repaint.
- `src/native_output.rs:893-902` sleeps after the loop iteration.

### Pageflip state

The GBM path tracks only whether a pageflip is pending:

- `NativePageFlipState` stores a boolean `pending`.
- `NativeGbmScanout::present()` returns early when a flip is pending.
- On successful `drmModePageFlip(... DRM_MODE_PAGE_FLIP_EVENT ...)`, the buffer
  index is stored as `pending_index`.
- `NativeGbmScanout::drain_page_flip_events()` updates `current_index` and
  clears `pending` only after `drain_drm_page_flip_events()` reports completion.

Evidence:

- `src/native_output.rs:3601-3622` defines the pending-only pageflip state.
- `src/native_output.rs:3742-3771` schedules the legacy KMS pageflip event.
- `src/native_output.rs:3774-3783` applies the completed pending buffer.
- `src/native_output.rs:3879-3915` counts `DRM_EVENT_FLIP_COMPLETE` events.

Two important details follow from this:

- `present()` is a submission call, not a presentation-completion call.
- The DRM event reader currently counts events only. It discards vblank
  timestamp, sequence, and event metadata that would be needed for precise
  presentation feedback.

### Compositor completion work

`OwnCompositorServer::present_frame()` currently performs several logically
different lifecycle transitions at once:

- commits ready explicit-sync buffers;
- flushes pending color and resize configure work;
- releases pending buffers;
- completes frame callbacks;
- sends presentation feedback;
- flushes clients.

Evidence:

- `src/compositor/server.rs:222-229` is the whole method.
- `src/compositor/mod.rs:2631-2659` sends presentation feedback with a software
  timestamp sampled when `present_frame()` runs.
- `src/compositor/mod.rs:2675-2687` releases normal buffers and explicit-sync
  releases.
- `src/compositor/state_data.rs:536-543` maps explicit-sync release to
  `point.signal()`.

This shape is acceptable for simple tests that explicitly call `PresentFrame`,
but it is not a native KMS presentation boundary.

### Pending frame work

`has_pending_frame_work()` returns true for:

- pending resize configure;
- pending frame callbacks;
- pending presentation feedback.

Evidence:

- `src/compositor/mod.rs:2625-2629`.
- Tests assert this model for frame callbacks and presentation feedback in
  `src/compositor/tests/surface_frames.rs:30-44` and
  `src/compositor/tests/lifecycle.rs:70-85`.

This is useful for deciding that clients need progress, but it does not tell the
native backend that a pageflip completed.

### Output refresh

The selected KMS refresh is now carried into output state:

- `src/native_output.rs:531-532` sets output size and refresh on the server.
- `src/compositor/mod.rs:241-249` stores the refresh and resends output mode.
- `src/compositor/output.rs:81-87` converts refresh to `wl_output` mHz and
  presentation refresh nanoseconds.
- `src/native_output.rs:4928-4932` has a 165 Hz test expecting a `6060 us` active
  interval.

So the main lifecycle issue is not "all clients see 60 Hz" anymore. The issue is
that frame completion is still driven by the native loop timer and
`present_frame()`, not by the pageflip event carrying the true presentation
boundary.

## Protocol Boundary

Wayland core frame callbacks are a throttling hint. The protocol says the server
must send callbacks so clients avoid excessive updates while still allowing the
highest possible rate, and should give clients time to draw for the next
refresh. See `/usr/share/wayland/wayland.xml:1611-1646`.

`wp_presentation` is stricter: feedback is associated with a surface commit, and
`presented` should be sent when final realized presentation time is available,
for example after framebuffer flip completion. See
`/usr/share/wayland-protocols/stable/presentation-time/presentation-time.xml:39-52`.

The same protocol says `hw_completion` means the display hardware signaled that
it started using the new image, as opposed to a timer guess. See
`/usr/share/wayland-protocols/stable/presentation-time/presentation-time.xml:180-186`.

For explicit sync, the release point is the point the compositor signals when it
has finished using the buffer for the relevant commit. See
`/usr/share/wayland-protocols/staging/linux-drm-syncobj/linux-drm-syncobj-v1.xml:42-47`
and `:210-219`.

Oblivion currently sends software-timed presentation feedback and release from
`present_frame()`, before the corresponding KMS completion is known.

## Reference Lifecycle

### Hyprland

The local Hyprland reference no longer uses wlroots, but its output lifecycle is
the same class of design: output presentation events advance presentation
feedback and frame scheduling.

Evidence:

- `WM para Referencia/Hyprland-main/src/output/Monitor.cpp:112-140` listens for
  the backend output present event and sends `PROTO::presentation->onPresented`
  with the backend timestamp, refresh, sequence, and flags.
- `WM para Referencia/Hyprland-main/src/output/Monitor.cpp:163-175` sends frame
  events in no-damage cases and then calls `m_frameScheduler->onPresented()`.
- `WM para Referencia/Hyprland-main/src/protocols/PresentationTime.cpp:113-143`
  drains queued presentation feedback when a monitor reports presentation.
- `WM para Referencia/Hyprland-main/src/output/MonitorFrameScheduler.cpp:55-83`
  uses `onPresented()` as the point to commit pending work after missed timing.

Takeaway for Oblivion: presentation feedback and pending-frame scheduling should
be downstream of output `present`, not downstream of the function that submits a
pageflip.

### wlroots-like / Smithay reference

The local ShojiWM reference is Smithay-based, but it demonstrates the same
wlroots-like lifecycle shape:

- DRM events are inserted into an event loop source.
- A `DrmEvent::VBlank` calls `frame_finish()`.
- `frame_finish()` uses DRM metadata for presentation clock, sequence, flags,
  next frame target, and presentation feedback.
- Render time queues output presentation feedback into the DRM frame, then vblank
  completion presents it.

Evidence:

- `WM para Referencia/ShojiWM-main/src/shojiwm/src/backend/tty.rs:503-508`
  dispatches DRM vblank to `frame_finish()`.
- `WM para Referencia/ShojiWM-main/src/shojiwm/src/backend/tty.rs:527-604`
  marks the frame submitted and sends presentation feedback from DRM metadata.
- `WM para Referencia/ShojiWM-main/src/shojiwm/src/backend/tty.rs:4625-4629`
  queues output presentation feedback into the DRM frame.
- `WM para Referencia/ShojiWM-main/src/shojiwm/src/presentation.rs:466-483`
  sends frame callbacks by output and throttling state.

Takeaway for Oblivion: even if the first implementation remains simple, the
state transition should be "pageflip event completed -> finish frame -> release
and feedback", not "sleep-loop decided work was pending -> finish frame".

## Main Suspicions

### 1. Frame callbacks are completed too early

Confirmed.

When a repaint occurs and `scanout.present()` succeeds, the pageflip is only
queued. `server.present_frame()` can run immediately afterward and send all
pending frame callbacks before the DRM event for that flip.

Worse, if an older pageflip is still pending, the repaint can produce a new
`ready_index`, `scanout.present()` will no-op, and `server.present_frame()` can
still complete callbacks for a frame that has not been pageflipped at all.

Expected symptom:

- clients draw ahead of real vblank;
- browsers can submit another frame before the previous visible result is on
  screen;
- compositor CPU/GPU work stacks up and feels worse at high refresh.

### 2. Presentation feedback is completed too early and with guessed metadata

Confirmed.

`complete_pending_presentation_feedbacks()` samples `CLOCK_MONOTONIC` at
`present_frame()` time and uses `render_generation` as sequence. It does not use
DRM flip timestamp, MSC/sequence, or `Vsync/HwClock/HwCompletion` flags.

Expected symptom:

- clients that use presentation feedback for pacing see approximate timings;
- feedback can report a frame as presented before it is visible;
- feedback cannot distinguish real hardware completion from timer guesses.

### 3. Buffer release and explicit-sync release can be early

Likely for native KMS.

The compositor is CPU-compositing SHM content into its own scanout buffer, so
early `wl_buffer.release` for SHM may be acceptable once the copy is complete.
For dmabuf handles and explicit sync, however, release is semantically tied to
when the compositor has finished using the buffer for the commit.

Today `release_pending_buffers()` runs inside `present_frame()`. If a dmabuf is
still part of the renderable scene or a future direct-scanout/overlay path
depends on it, the current boundary is too early.

Expected symptom:

- explicit-sync clients may reuse buffers sooner than intended;
- future direct scanout will become unsafe unless releases are moved to the
  backend presentation lifecycle.

### 4. The loop can repaint while a pageflip is pending

Confirmed.

Triple buffering allows a new render into a free buffer while a previous
pageflip is pending. That is not inherently wrong. The bug is that lifecycle
completion is not associated with the eventual flip that uses the new buffer.

Expected symptom:

- under load, Oblivion may accumulate "rendered but not presented" state;
- `pending_frame_work` can cause more rendering and more callbacks without a
  matching completed output frame.

### 5. Wakeup is timer-based, not vblank-based

Confirmed.

The loop samples the DRM fd with timeout `0` and then sleeps. Active wakeup is
derived from refresh, and active surfaces can wake every `4 ms`, but neither path
waits on DRM fd readiness. At 165 Hz the intended frame interval is about
`6060 us`, but timer jitter and CPU render time are still part of the pacing.

Expected symptom:

- input, Wayland dispatch, and pageflip completion can wait for the next sleep
  wake;
- the compositor can wake before vblank and do no useful present work;
- frame callback timing drifts from actual output cadence.

## Correction Plan

### Step 1: Split submission from completion in code structure

Introduce a small native frame-completion boundary without changing protocols:

- keep `scanout.present()` as "try to submit ready buffer";
- make `drain_page_flip_events()` return completion records instead of only
  clearing state;
- call a new server method only when a completion record exists, for example
  `server.finish_presented_frame(completion)`;
- keep the old `present_frame()` only for tests or non-native fallback paths.

Validation:

- add native perf logs for `pageflip.scheduled`, `pageflip.completed`, and
  `frame.finished`;
- assert in logs that `frame.finished` follows a real completion for GBM/KMS.

### Step 2: Associate compositor frame work with submitted scanout frames

Track which compositor lifecycle work belongs to the buffer submitted to KMS.
The minimal version can be coarse:

- when repaint produces a buffer, mark that buffer as carrying pending callbacks,
  presentation feedback, and release work;
- when that buffer's pageflip completes, finish only that work;
- if a newer frame supersedes a ready-but-not-submitted buffer, discard or
  reassign feedback according to protocol expectations.

Validation:

- add tests around "repaint while pageflip pending must not complete callbacks";
- add tests around "feedback for superseded ready buffer is discarded or delayed,
  not presented early";
- add a native-only log field `submitted_frame_id` and `completed_frame_id`.

### Step 3: Use DRM event metadata for presentation feedback

Parse the DRM event as a vblank/pageflip event, not just a generic event count.
Capture:

- timestamp;
- sequence;
- whether the timestamp is hardware sourced when available;
- refresh interval from the current output mode.

Then send:

- `Vsync` when using normal vsync pageflip;
- `HwClock`/`HwCompletion` only when the backend metadata supports them;
- `refresh_nsec` from the selected mode, or `0` when unknown/VRR-like.

Validation:

- compare logged `presentation_ts` against kernel pageflip event timestamps;
- verify `wp_presentation_feedback.presented` does not occur before
  `pageflip.completed`;
- use `WAYLAND_DEBUG=1` on a small client to inspect feedback order.

### Step 4: Move release to the correct backend boundary

Separate release policies:

- SHM copied into compositor-owned memory can still release after the copy is
  complete.
- DMABUF composited through GL should release after GPU sampling is complete.
- DMABUF direct scanout or overlay must release after the output no longer uses
  it.
- Explicit-sync release points should be signaled according to that selected
  path, not blindly in `present_frame()`.

Validation:

- keep the existing syncobj test but add a native-lifecycle equivalent that does
  not signal release until pageflip completion;
- add perf/debug logs for `buffer.release` with `reason=copy_complete`,
  `reason=gpu_complete`, or `reason=pageflip_complete`.

### Step 5: Replace sleep pacing with fd-driven wakeups

Incrementally replace the sleep tail with a wait over:

- Wayland client/socket readiness;
- input backend fd or libinput dispatch fd;
- DRM fd readiness;
- timerfd only for fallback repaint deadlines or no-damage frame callbacks.

Validation:

- log `wake.reason=drm|input|wayland|timer`;
- measure time from DRM event readiness to `pageflip.completed` handling;
- compare idle wakeups per second before and after.

## Logging and Perf Validation Plan

Enable existing perf:

```sh
OBLIVION_ONE_PERF_LOG=1 OBLIVION_ONE_CURSOR=hardware OBLIVION_ONE_MODE=1920x1080@165 ./bin/oblivion-one --output native
```

Add or inspect these fields:

- `pageflip_scheduled_at_us`
- `pageflip_completed_at_us`
- `present_submit_us`
- `frame_finish_us`
- `submitted_frame_id`
- `completed_frame_id`
- `ready_buffer_index`
- `pending_buffer_index`
- `current_buffer_index`
- `callbacks_completed`
- `feedback_presented`
- `buffer_releases`
- `syncobj_releases`
- `wake_reason`

Useful checks:

```sh
rg "perf native\\.(frame|present_frame|pageflip|wake|buffer)" ~/.local/state/oblivion-one/session.log
```

Expected after Step 1:

- every native `frame.finished` follows a `pageflip.completed`;
- no `native.present_frame` equivalent occurs while `pageflip_pending=true`;
- callbacks and feedback counts line up with completed output frames, not render
  attempts.

Expected after Step 5:

- `wake_reason=drm` appears at the output refresh cadence while frames are
  pending;
- timer wakeups are fallback/deadline events, not the primary frame clock;
- p95 input-to-present and callback-to-next-commit timing should shrink.

## Risk Notes

- Moving callbacks too late can stall clients if no-damage frames do not produce
  pageflips. Hyprland handles this with no-damage frame events from the output
  present path; Oblivion will need a similar fallback.
- Associating all callbacks with one output is acceptable for the current
  single-output native backend. Multi-output will need per-output feedback.
- Current tests are written around `PresentFrame` as an abstract completion
  command. Keep that command for unit tests, but add native lifecycle tests so
  the real backend cannot regress.
- Buffer release policy must be path-specific before direct scanout or overlays
  are added.

## Bottom Line

The native backend is using KMS pageflip for scanout, but client lifecycle is
still completed by a loop-level timer approximation. The highest-value
incremental fix is to move frame callbacks, presentation feedback, and native
release decisions behind the DRM pageflip completion event, then make the DRM fd
a real wake source. Only after that boundary is correct does it make sense to
spend more effort tuning repaint heuristics around frame callbacks.
