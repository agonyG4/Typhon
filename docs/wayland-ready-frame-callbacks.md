# Frame callbacks at bounded READY admission

## Problem and evidence

Chromium's `WaylandFrameManager::MaybeProcessPendingFrame` waits for the previous
`wl_surface.frame` callback before sending another buffer. Typhon previously
marked callbacks rendered after `finish_render_owned`, but did not send them
until the rendered frame could enter the KMS submission lane. A successfully
rendered frame can therefore occupy a bounded READY slot while its client is
unnecessarily blocked by the predecessor's presentation or deferred target
binding.

The measured Vesktop scroll presentation latency was 20.6 ms median on native
Wayland versus 14.2 ms through Xwayland/satellite at 165 Hz. This is evidence to
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

| Chromium-reported scroll latency | Original sample | Follow-up sample |
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
