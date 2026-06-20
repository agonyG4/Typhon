# Native performance and mouse latency research

Date: 2026-06-11

Scope: native session input, render, pageflip, and frame pacing behavior for high-refresh displays, especially 165 Hz. This document focuses on low-risk and medium-risk improvements that do not require rewriting the native loop around epoll.

Update 2026-06-19: the epoll rewrite described as future work below is now
implemented. Input and DRM readiness wake the compositor directly, frame
deadlines are absolute timerfd deadlines, and mapped unchanged surfaces do not
cause periodic wakeups. The bottleneck analysis remains useful historical
context for renderer and cursor work.

## Summary

The native session currently has a working transitional scanout path, but mouse latency and high-refresh pacing are limited by three confirmed design costs:

- Pointer motion always requests a compositor redraw.
- The GBM/KMS pageflip path still uses CPU composition and CPU upload into GBM buffers.
- Frame callbacks and presentation feedback are not completed from real pageflip completion.

The fastest practical improvements are to avoid full redraws for plain pointer motion, advertise the real KMS refresh rate to Wayland clients, coalesce motion events per loop, and connect native frame completion to DRM pageflip events. A full event-driven loop would be cleaner long-term, but a small `poll()`-based wait over the current fds can reduce wake latency without forcing an epoll rewrite.

## Confirmed bottlenecks

### Pointer motion forces full redraw

`NativeInputState::handle_pointer_motion_delta()` updates cursor position, optionally forwards pointer motion, and then unconditionally calls `effect.request_redraw()`:

- `src/native_output.rs:900-917`

That redraw flag flows into the native loop. If `redraw_requested` is true, the loop calls `scanout.paint_server_frame()` and then `scanout.present()`:

- `src/native_output.rs:263-284`

This means a high-rate mouse can drive full desktop composition even when only `wl_pointer.motion` needs to reach the focused client. The nested output already avoids this by using a host cursor style path, documented in:

- `README.md:126-128`
- `docs/ARCHITECTURE.md:85-89`

The native path does not yet have the same cursor separation.

### GBM scanout is CPU-rendered and CPU-uploaded

The native GBM path creates writable XRGB8888 scanout buffers:

- `src/native_output.rs:2617-2655`

But `NativeGbmScanout::paint_server_frame()` still renders through `NativeFrameRenderer`, copies the frame into a staging byte buffer, and writes it into the GBM buffer:

- `src/native_output.rs:2658-2681`

The per-pixel conversion loop is in:

- `src/native_output.rs:3009-3043`

The docs already call this out as transitional:

- `docs/NATIVE_SESSION.md:41-44`
- `docs/NATIVE_SESSION.md:80-83`
- `docs/KNOWN_ISSUES.md:24-33`
- `README.md:264-268`

At 2560x1440, one full XRGB frame is about 14 MiB before considering staging, cache effects, and GBM write cost. At 165 Hz, a full-frame CPU path is not a viable steady-state target.

### Native loop uses sleep-based pacing

The main native loop processes Wayland, drains DRM pageflip events, attempts present, drains input, maybe redraws, maybe completes frame work, then sleeps:

- `src/native_output.rs:252-299`

The active interval is derived from the selected KMS refresh rate:

- `src/native_output.rs:331-351`

At 165 Hz, tests expect a 6060 us active interval:

- `src/native_output.rs:3591-3595`

This is better than a fixed 60 Hz sleep, but it still means input can wait up to the sleep interval if it arrives just after the loop goes idle. A 1 ms input interval exists:

- `src/native_output.rs:449-452`

But it only applies after the loop has already observed input events.

### Pageflip completion is not the frame-completion boundary

GBM pageflips are scheduled with `DRM_MODE_PAGE_FLIP_EVENT`:

- `src/native_output.rs:2697-2727`

Pageflip events are drained with a zero-timeout `poll()` and raw `read()`:

- `src/native_output.rs:2729-2738`
- `src/native_output.rs:2834-2865`

However, the main loop completes compositor frame work with `server.present_frame()` based on `server.has_pending_frame_work()`, not based on the pageflip event that actually completed scanout:

- `src/native_output.rs:286-288`

`present_frame()` completes pending frame callbacks and presentation feedback:

- `src/compositor/server.rs:212-220`

This can make client pacing approximate rather than presentation-driven.

### Wayland clients are advertised 60 Hz

The native output size is updated from KMS, but `wl_output.mode.refresh` is hardcoded to `60_000`:

- `src/compositor/output.rs:106-112`

Presentation feedback also uses a fixed 60 Hz refresh duration:

- `src/compositor/explicit_sync.rs:19`
- `src/compositor/mod.rs:2601-2630`

For a 165 Hz native session, this can cause clients to self-pace incorrectly even if the compositor loop itself targets about 6.06 ms.

### Flush pressure on input path

Every high-level input send flushes clients immediately:

- `src/compositor/server.rs:123-141`

For pointer motion, `apply_native_input_effect()` calls `server.send_pointer_motion()` for each motion effect:

- `src/native_output.rs:2137-2150`

This is correct and simple, but it may increase syscall pressure during high-rate mouse motion. It should be measured before changing because immediate flush also helps latency.

## Hypothetical bottlenecks to measure

These are plausible costs but were not measured in this pass:

- `gbm::BufferObject::write()` may be the largest single cost during full-frame redraw.
- The ARGB-to-XRGB conversion loop may consume visible CPU at high resolution.
- Immediate `flush_clients()` per pointer event may create syscall overhead under 1000 Hz mouse input.
- `server.tick()` dispatch cost may grow when clients are busy or when many surfaces have pending frame callbacks.
- The raw evdev fallback loops over devices and reads until empty; with many devices this may matter, though it is capped at 256 events per drain:
  - `src/native_output.rs:2050-2063`
- `NativeLibinputBackend::drain_events()` allocates a fresh `Vec` each call; likely minor compared with full redraw, but easy to confirm:
  - `src/native_output.rs:1624-1642`

## Reference patterns

### Hyprland

Hyprland separates frame scheduling from direct pointer/cursor handling. It schedules monitor frames through the backend output rather than repainting blindly:

- `WM para Referencia/Hyprland-main/src/Compositor.cpp:2099-2110`

Its output present event is the point where presentation protocol state is updated and frame scheduler state advances:

- `WM para Referencia/Hyprland-main/src/output/Monitor.cpp:112-178`

For mouse movement, Hyprland only schedules cursor-move frames when software cursor rendering is required:

- `WM para Referencia/Hyprland-main/src/managers/input/InputManager.cpp:270-276`

It tries to use a hardware cursor buffer when backend capabilities allow it, and only schedules a frame for cursor shape when needed:

- `WM para Referencia/Hyprland-main/src/managers/PointerManager.cpp:397-415`

Relevant takeaway for Oblivion: plain pointer motion should not imply full desktop redraw. Cursor image updates and software-cursor fallbacks should be separate redraw reasons.

### KWin

KWin has a render loop that estimates the next presentation target from refresh rate and recent render time:

- `WM para Referencia/kwin-master/src/core/renderloop.cpp:44-112`

Refresh changes reschedule repaint if a composite timer is active:

- `WM para Referencia/kwin-master/src/core/renderloop.cpp:238-249`

It also limits pending frames and handles VRR/tearing modes explicitly:

- `WM para Referencia/kwin-master/src/core/renderloop.cpp:256-275`

Relevant takeaway for Oblivion: even without adopting KWin's full render loop model, native should use real output refresh consistently and complete client frame pacing from presentation events when possible.

### Shoji

Shoji's TTY backend uses calloop DRM events. A DRM vblank event calls `frame_finish()`:

- `WM para Referencia/ShojiWM-main/src/shojiwm/src/backend/tty.rs:502-508`

`frame_finish()` marks the frame submitted, builds presentation feedback from DRM metadata, clears `frame_pending`, and schedules follow-up redraw only when needed:

- `WM para Referencia/ShojiWM-main/src/shojiwm/src/backend/tty.rs:527-682`

When a rendered frame has no damage, Shoji arms an estimated-vblank callback so visible clients still receive frame callbacks without forcing a full redraw:

- `WM para Referencia/ShojiWM-main/src/shojiwm/src/backend/tty.rs:4693-4720`
- `WM para Referencia/ShojiWM-main/src/shojiwm/src/backend/tty.rs:10345-10480`

Relevant takeaway for Oblivion: pageflip/vblank completion should be a first-class state transition. No-damage callback progress can be maintained without repainting everything.

## Low-risk fixes

### 1. Do not redraw for plain pointer motion

Risk: low.

Change `NativeInputState::handle_pointer_motion_delta()` and `handle_pointer_absolute()` so they only request redraw when:

- Spotlight is visible and cursor affects shell UI.
- A native window interaction is active.
- The native compositor is using a software cursor.
- Cursor shape or shell hover state changes.
- A future hardware cursor update fails and falls back to software cursor.

Forward `wl_pointer.motion` immediately, but keep `redraw_requested` false for normal client pointer motion.

Expected result:

- Empty-desktop mouse motion no longer drives `paint_server_frame()`.
- Client pointer responsiveness remains intact.
- CPU and memory bandwidth drop during mouse movement.

### 2. Coalesce pointer motion per native loop tick

Risk: low.

During `input_devices.drain_events()`, accumulate relative motion and keep only the latest absolute cursor position for client motion unless buttons, axes, or interactions require ordering. Send one pointer motion/frame per loop.

This is especially useful for high polling rate mice. It reduces client flush pressure and redundant hit-testing while preserving the final pointer position for the frame.

Expected result:

- Fewer `wl_pointer.motion` sends per loop under bursty input.
- Lower syscall pressure from repeated `flush_clients()`.

### 3. Advertise real native refresh

Risk: low to medium.

Store the selected KMS refresh rate in compositor output state and use it for:

- `wl_output.mode.refresh`
- `wp_presentation_feedback.presented(refresh_nsec)`
- Any future output description updates after mode change

The current hardcoded values are 60 Hz:

- `src/compositor/output.rs:106-112`
- `src/compositor/explicit_sync.rs:19`

Expected result:

- 165 Hz clients see 165000 mHz output refresh.
- Client-side animation and frame callback pacing better match native output.

### 4. Complete frame callbacks on pageflip event

Risk: medium.

Make `NativeGbmScanout::drain_page_flip_events()` return whether a pending flip completed. In the main loop, call `server.present_frame()` only after a completed pageflip for GBM. Keep a fallback for dumb framebuffer, where there is no async pageflip path.

Expected result:

- Frame callbacks better represent displayed frames.
- Presentation feedback sequence/timestamp can move toward real DRM completion.
- Less chance of clients pacing against a frame that was prepared but not scanned out.

### 5. Replace blind sleep with small `poll()` wait

Risk: medium.

Without a full epoll rewrite, replace the final `thread::sleep()` with a small poll wait over the fds already available:

- DRM fd for pageflip events.
- libinput fd when the libinput backend is active.
- raw evdev fds when raw fallback is active.
- Wayland display/socket fd if available through the current server abstraction.

Use the existing `native_wakeup_interval()` result as the timeout. This keeps the current loop structure but wakes early on input or DRM completion.

Expected result:

- Lower worst-case input latency.
- Fewer unnecessary wakeups when idle.
- Same state machine, easier rollback.

## Medium-risk fixes

### Hardware cursor plane

Risk: medium.

Implement a native hardware cursor path where possible:

- Keep cursor position updates on the cursor plane.
- Fall back to software cursor only when cursor image/size/backend constraints require it.
- Avoid full scene redraw for cursor-only movement.

The project docs already list this as future native work:

- `docs/NATIVE_SESSION.md:86-87`

This is the best mouse-latency fix, but it touches DRM cursor plane behavior and needs careful fallback handling.

### Direct EGL/GLES into GBM

Risk: medium to high.

Move from CPU-filled GBM buffers to EGL/GLES rendering into GBM render targets. This is already the documented next performance milestone:

- `docs/NATIVE_SESSION.md:80-83`
- `docs/KNOWN_ISSUES.md:33-35`

This removes the biggest architectural bottleneck, but it is broader than the immediate low-risk mouse fixes.

## Measurement plan

Run these from a real native session or a controlled TTY where taking DRM ownership is expected.

### CPU and wakeups

```bash
cd /home/agony/Projetos/Oblivion
pid="$(pgrep -n oblivion-one)"
sudo perf stat -p "$pid" -e cycles,instructions,context-switches,cpu-migrations,page-faults -I 1000
```

Watch for:

- CPU cycles during idle.
- CPU cycles during fast mouse motion over empty desktop.
- Context switches per second.
- Difference between pointer motion and active window drag.

### Hot functions

```bash
pid="$(pgrep -n oblivion-one)"
sudo perf top -p "$pid"
```

Expected current hot areas during mouse-driven redraw:

- Frame composition.
- `copy_argb_frame_to_xrgb_mapping`.
- GBM buffer write or DRM/ioctl adjacent paths.
- Wayland client flush/send paths.

### Syscalls

```bash
pid="$(pgrep -n oblivion-one)"
sudo strace -ttT -p "$pid" -e poll,read,write,ioctl,nanosleep,clock_nanosleep
```

Watch for:

- Sleep intervals during mouse input.
- Repeated writes/flushes per pointer event.
- DRM pageflip ioctls and event reads.

### Scheduler latency

```bash
pid="$(pgrep -n oblivion-one)"
sudo perf sched record -p "$pid" -- sleep 10
sudo perf sched latency
```

Watch for:

- Wakeup latency under high mouse input.
- Whether the process misses the 6.06 ms frame budget at 165 Hz.

### Optional eBPF counters

```bash
sudo bpftrace -e 'tracepoint:syscalls:sys_enter_nanosleep /comm=="oblivion-one"/ { @sleep=count(); }'
```

```bash
sudo bpftrace -e 'tracepoint:syscalls:sys_enter_ioctl /comm=="oblivion-one"/ { @[args->cmd]=count(); }'
```

## Before and after criteria

### Idle

Before:

- Native loop wakes at roughly 4 ms when surfaces exist.
- No input may still keep active-ish polling behavior.

After:

- Idle wakeups drop when no frame/input/DRM work exists.
- No visual or client callback starvation.

### Plain mouse movement

Before:

- Pointer motion requests redraw.
- Full CPU compose and scanout upload can happen at mouse polling cadence.

After:

- Plain pointer motion forwards `wl_pointer.motion` without full redraw.
- CPU usage during mouse movement over empty desktop drops significantly.
- Pointer movement still feels immediate to clients.

### 165 Hz frame pacing

Before:

- Native loop active pacing computes about 6.06 ms.
- Wayland output and presentation feedback still advertise 60 Hz.

After:

- Wayland clients see 165000 mHz for the native output.
- Presentation refresh duration matches the selected KMS mode.
- Frame callbacks are completed after pageflip where the backend supports it.

### Active window drag/resize

Before:

- Drag/resize causes redraw on every motion.

After:

- Drag/resize may still redraw, but motion coalescing limits redundant work.
- No regression in `Alt` drag/resize behavior.

### Pageflip correctness

Before:

- `server.present_frame()` is not tied to pageflip completion.

After:

- GBM path completes frame callbacks after DRM pageflip event.
- Dumb framebuffer path keeps a documented immediate/fallback completion rule.
- No overlapping pageflips; `NativePageFlipState` remains the guard.

## Implementation priority

1. Dynamic output refresh and presentation refresh duration.
2. Avoid redraw for plain pointer motion.
3. Coalesce pointer motion within one loop drain.
4. Return pageflip completion from `drain_page_flip_events()` and complete frame callbacks there.
5. Replace final sleep with `poll()` timeout over current fds.
6. Add hardware cursor plane support.
7. Move native GBM from CPU-filled buffers to EGL/GLES render targets.

## Validation checklist

- `cargo test native_wakeup_uses`
- `cargo test native_frame_pacing_uses_kms_refresh_rate`
- Add or update tests for:
  - pointer motion that forwards client motion without requesting redraw;
  - pointer motion during active window interaction still requesting redraw;
  - output refresh set to selected KMS mode;
  - pageflip completion triggering frame callback completion.
- Native smoke on TTY:
  - launch native session at 165 Hz;
  - move mouse over empty desktop;
  - drag/resize a window;
  - run a Wayland client that waits on frame callbacks;
  - collect perf/strace before and after.

## Evidence

Primary Oblivion files:

- `src/native_output.rs`
- `src/compositor/server.rs`
- `src/compositor/output.rs`
- `src/compositor/mod.rs`
- `src/compositor/explicit_sync.rs`
- `docs/NATIVE_SESSION.md`
- `docs/KNOWN_ISSUES.md`
- `README.md`

Reference files:

- `WM para Referencia/Hyprland-main/src/Compositor.cpp`
- `WM para Referencia/Hyprland-main/src/output/Monitor.cpp`
- `WM para Referencia/Hyprland-main/src/managers/input/InputManager.cpp`
- `WM para Referencia/Hyprland-main/src/managers/PointerManager.cpp`
- `WM para Referencia/kwin-master/src/core/renderloop.cpp`
- `WM para Referencia/ShojiWM-main/src/shojiwm/src/backend/tty.rs`

Changes made for this research file:

- Created `docs/research/native-performance-mouse.md`.

Validation performed:

- Static source inspection only.
- Native session was not launched because it can take over DRM/TTY.
