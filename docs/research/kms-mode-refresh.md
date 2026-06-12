# KMS Mode, Refresh, and Native Input Research

Status: research note for the native SDDM backend

Audience: agents working on `src/native_output.rs`

Goal: make the native backend reliably select `1920x1080@165`, pace frames from
the chosen mode, advertise the correct refresh to clients, and reduce mouse
latency toward Hyprland/KWin-class behavior.

## Summary

The native backend already derives frame pacing from the selected KMS mode, so a
true `165 Hz` mode becomes an active interval of about `6060 us`. The missing
piece is mode selection: the current code chooses the first mode from the first
connected connector with a usable CRTC. That can be `1920x1080@165`, but only if
the kernel reports it first.

The other major limitation is event-loop shape. Input dispatch and DRM pageflip
drain are checked from a sleeping compositor loop. Mature compositors wire
libinput and DRM fds into an event-driven loop, then drive frame completion from
real presentation/pageflip events.

## Current Behavior

### KMS Target Selection

`select_kms_target()` is the current native mode selection entry point in
`src/native_output.rs`.

Current behavior:

- query KMS resources;
- iterate connector ids in kernel order;
- skip disconnected connectors;
- take `modes.first()` from the first connected connector;
- find the current encoder or first compatible encoder;
- return the first usable connector, CRTC, and mode.

Important code points:

- `run()` calls `select_kms_target()` before setting output size and frame
  pacing.
- `select_kms_target()` currently uses `modes.first().copied()`.
- the sysfs diagnostic helper reads `/sys/class/drm/.../modes`, but that helper
  is not used for scoring or choosing the actual KMS mode.

Implication: there is no explicit policy for `1920x1080@165`, `highres`,
`highrr`, `preferred`, or exact user requested modes.

### Frame Pacing

`NativeFramePacing::from_mode()` reads `mode.vrefresh`.

Current behavior:

- missing refresh falls back to `60 Hz`;
- refresh is clamped to `30..=360`;
- active interval is `1_000_000 / refresh_hz` microseconds;
- tests already assert that `165 Hz` maps to `6060 us`.

This is directionally correct, but the compositor still sleeps at the end of the
loop. Refresh-derived sleep is a useful fallback, not a replacement for DRM
pageflip driven scheduling.

### Input Polling

`NativeLibinputBackend::drain_events()` calls `input.dispatch()` only when the
main native loop wakes. The loop then sleeps according to activity state.

Current behavior:

- input is drained once per compositor loop iteration;
- input events set a short `1 ms` follow-up wakeup;
- no libinput fd is registered as the primary wake source;
- libseat lifecycle dispatch also happens from the input drain path.

Implication: mouse motion can be delayed by loop sleep, CPU rendering, scanout
buffer writes, and late libinput timer servicing.

### DRM Pageflip Drain

The GBM scanout path schedules legacy pageflips with
`DRM_MODE_PAGE_FLIP_EVENT`, then `drain_page_flip_events()` calls a helper that
does `poll(fd, ..., 0)` and reads available events.

Current behavior:

- DRM fd readiness is sampled, not used to wake the compositor;
- pageflip completion clears pending state;
- `server.present_frame()` is still called from pending frame work in the main
  loop instead of being directly tied to pageflip completion.

Implication: Wayland frame callbacks and presentation feedback are still paced by
the loop approximation, not by real presentation timing.

## Desired Mode Policies

The mode policy should be explicit and testable. Recommended policy order:

1. `exact`
2. `preferred`
3. `highrr`
4. `highres`

The implemented first pass exposes the policy through `OBLIVION_ONE_MODE`
itself:

```sh
OBLIVION_ONE_MODE=1920x1080@165
```

### exact

Use when the user has a known target such as `1920x1080@165`.

Rules:

- parse `WIDTHxHEIGHT@REFRESH`;
- require exact width and height;
- choose the nearest refresh if the connector reports `164.99`, `165`, or mHz
  style values through a rounded representation;
- fail loudly or fall back based on an explicit fallback policy.

Recommended default for this host goal:

```sh
OBLIVION_ONE_MODE=1920x1080@165
```

### preferred

Use the connector preferred mode if the DRM mode flags expose it. If preferred
flags are not available through the current wrapper, preserve the current
`modes.first()` behavior as the compatibility fallback.

This should be the conservative default when no user policy is set.

### highrr

Choose the highest refresh mode. When multiple modes have the same refresh,
prefer the larger resolution or the current connector native/preferred size.

This policy is useful when the user asks for the smoothest pointer/frame feel and
accepts a non-native resolution if it gives the highest refresh.

### highres

Choose the largest pixel area. When multiple modes share the largest size, choose
the highest refresh.

This policy is useful for workstation/default desktop behavior where visual
fidelity matters more than refresh.

## Recommended Selection Algorithm

1. Enumerate all connected connectors and all modes.
2. Build candidate records:
   - connector id;
   - connector name when available;
   - encoder id;
   - CRTC id;
   - mode name;
   - width;
   - height;
   - refresh Hz;
   - preferred flag when available;
   - exact match score.
3. Apply user connector preference if provided.
4. Apply mode policy.
5. Log the selected mode and the reason:

```text
native KMS mode selected: DP-1 1920x1080@165 policy=exact source=OBLIVION_ONE_MODE
```

6. If no exact mode is found, log all candidate modes for the selected connector.

For the immediate goal, add tests around pure scoring logic before touching KMS
ioctl code. The scoring function should accept plain structs so it can be tested
without a DRM device.

## Refresh Announced to Clients

The compositor should keep one refresh source of truth:

```text
selected KMS mode -> output state -> wl_output mode refresh -> frame pacing -> presentation feedback
```

Today, `server.set_output_size(width, height)` is called after KMS target
selection. The next hardening step is to carry the selected refresh into the
Wayland output state as well, so clients see the actual current mode:

```text
1920x1080@165000 mHz
```

Client-facing requirements:

- `wl_output.mode` should mark the selected mode as current.
- refresh should be advertised in milli-Hz if the protocol path expects it.
- if the mode reports no refresh, use the normalized pacing fallback internally
  but log that the advertised refresh was unavailable.
- presentation feedback should eventually use the actual DRM pageflip timestamp,
  not the time when the loop happened to call `present_frame()`.

Hyprland's output path reports the monitor pixel size and refresh to clients
from monitor state. Oblivion should mirror that shape once native output state
has refresh data.

## Event Loop and Frame Pacing Plan

The long-term fix is an event-driven native loop:

- Wayland server/client fd readiness wakes client accept/dispatch;
- libinput fd readiness wakes input dispatch;
- libseat fd readiness wakes session lifecycle handling when available;
- DRM fd readiness wakes pageflip event drain;
- timerfd is used only for scheduled repaint deadlines or fallback pacing.

Recommended ordering per wake:

1. Drain libseat state changes.
2. Drain libinput until empty and coalesce pointer motion to the latest position.
3. Dispatch Wayland clients and collect commits.
4. If scene or cursor damage exists and no pageflip is pending, render a frame.
5. Schedule pageflip.
6. On DRM pageflip event, mark the frame presented and complete frame callbacks
   and presentation feedback with the real timestamp.

The current `thread::sleep()` loop should remain only as a temporary fallback or
as a development path while the fd-driven loop is introduced.

## Mouse Latency Notes

The current pointer path requests redraw for mouse motion, and the native scanout
path still renders through CPU memory before writing a GBM buffer. At
`1920x1080@165`, the frame budget is about `6.06 ms`, so full-frame CPU compose,
copy, and `bo.write()` on cursor-only motion will quickly consume the budget.

Recommended order:

1. fd-driven libinput dispatch;
2. coalesce pointer motion per event-loop turn;
3. hardware cursor plane where possible;
4. software cursor damage only for old and new cursor rects;
5. EGL/GLES rendering directly into GBM buffers;
6. pageflip timestamp driven frame callbacks.

The first two steps improve latency without changing renderer architecture. The
last four are needed for performance closer to Hyprland/KWin.

## Validation

### Static and Unit Validation

Add deterministic tests for:

- parsing `1920x1080@165`;
- `exact` selecting the matching mode;
- `highrr` selecting the highest refresh;
- `highres` selecting the largest resolution then highest refresh;
- `preferred` selecting a preferred mode when present;
- fallback logging when `1920x1080@165` is unavailable;
- `165 Hz` pacing remaining `6060 us`;
- selected refresh being carried into output state.

Useful existing checks:

```sh
cargo test native_frame_pacing --bin oblivion-one
cargo test native_wakeup_uses --bin oblivion-one
cargo test native_input_backend_plan --bin oblivion-one
```

### Native Dry Run

Use dry-run checks before entering real SDDM/VT paths:

```sh
OBLIVION_ONE_DRY_RUN=1 ./bin/start-oblivion-one
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u DISPLAY \
  OBLIVION_ONE_DRY_RUN=1 ./bin/start-oblivion-one-tty
./bin/oblivion-one doctor
```

### Real Native Session Telemetry

For real native testing, capture:

- selected connector, CRTC, mode name, width, height, refresh;
- frame pacing interval;
- libinput event age from device timestamp to dispatch;
- render duration;
- GBM staging copy duration;
- `bo.write()` duration;
- pageflip schedule-to-complete duration;
- time from pageflip complete to frame callback completion;
- whether clients receive `1920x1080@165000 mHz`.

Suggested acceptance criteria for the `1920x1080@165` target:

- selected mode is exactly `1920x1080@165` when available;
- active frame interval is about `6060 us`;
- clients see the selected mode and refresh;
- input p95 latency is below one 165 Hz frame;
- no repeated libinput debounce/timer expiry warnings under continuous mouse
  movement;
- pageflip completion drains from DRM fd readiness, not from a later sleep wake.

## Reference Points

Current Oblivion code:

- `src/native_output.rs`: native KMS selection, GBM scanout, input dispatch, and
  frame pacing.
- `src/compositor/server.rs`: `present_frame()` completes pending frame
  callbacks and presentation feedback.
- `docs/NATIVE_SESSION.md`: current native session architecture and production
  gaps.
- `docs/KNOWN_ISSUES.md`: native SDDM limitations.

Local compositor references:

- `WM para Referencia/kwin-master/src/backends/libinput/connection.cpp`: libinput
  fd notifier pattern.
- `WM para Referencia/kwin-master/src/backends/drm/drm_gpu.cpp`: DRM fd notifier
  and `drmHandleEvent()` pageflip handling.
- `WM para Referencia/ShojiWM-main/src/shojiwm/src/backend/tty.rs`: exact-size
  mode filtering and highest-refresh fallback.
- `WM para Referencia/Hyprland-main/tests/config/MonitorParser.cpp`: explicit
  monitor mode parsing examples such as `2560x1440@144`.
