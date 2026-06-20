# AstreaOS Wayland Gaming Input and Frame Pacing Status

Date: 2026-06-15

Update 2026-06-19: native frame pacing is now event driven. DRM, Wayland,
libinput/raw evdev, and timerfd readiness feed the runtime scheduler; pageflip
completion remains the only asynchronous finish boundary, and no-damage frame
callbacks use refresh-aligned absolute deadlines.

This note records the implemented subset of Plan 02 and the remaining runtime
validation boundary.

## Implemented

- Input protocol advertisement is capability gated. The default public registry
  no longer advertises `zwp_relative_pointer_manager_v1`,
  `zwp_pointer_constraints_v1`, or `zwp_idle_inhibit_manager_v1` as unsupported
  desktop-baseline globals.
- Native libinput relative motion is preserved as a timestamped
  `PointerMotionSample` with separate accelerated and unaccelerated deltas.
  Cursor clamping updates absolute output position without rewriting relative
  deltas.
- Consecutive compatible motion samples coalesce without crossing
  button/key/axis ordering boundaries.
- The compositor has an explicit `send_pointer_motion_sample` path carrying
  timestamp, optional absolute output position, and optional relative motion.
- Relative-pointer protocol resources are tracked and receive
  `relative_motion` events for the client owning the focused pointer surface
  when the capability is enabled in tests.
- A locked pointer-constraint state suppresses absolute `wl_pointer.motion`
  while still delivering relative pointer motion.
- Idle inhibit protocol resources can be capability-enabled, tracked, and wired
  into `IdleManager`.
- Native keyboard shortcut inhibition policy is implemented in the native input
  router: compositor window shortcuts are forwarded to the focused client while
  inhibition is active, with `Alt+P` retained as the emergency session escape.
- Native Wayland dispatch is no longer blocked merely because a KMS pageflip is
  pending. The loop logs `pageflip_pending_at_tick` separately from
  `tick_blocked_by_pageflip`.

## Still Gated

- `zwp_pointer_constraints_v1` remains disabled in normal sessions until the
  full protocol path validates client ownership, lifetime cleanup, activation
  and deactivation events, and confinement regions.
- `zwp_keyboard_shortcuts_inhibit_manager_v1` is not advertised yet. The native
  routing policy exists, but protocol resource ownership/focus activation still
  needs to be connected end to end.
- Nested mode still needs an explicit raw-motion capability decision. It must
  not advertise raw relative motion if host raw device motion is unavailable.
- The safe `libseat` crate API still exposes no independently registerable seat
  event fd. Seat lifecycle work is dispatched on other reactor wakeups without
  restoring an idle polling timer.
- Explicit-sync acquire points are still nonblocking readiness checks scheduled
  after client activity or at refresh-aligned deadlines. Eventfd-backed fence
  readiness remains future work.
- Gaming readiness still requires real hardware validation with native
  libinput devices, fullscreen focus transitions, KMS pageflip logs, and at
  least one native Wayland game or relative-pointer test client.
