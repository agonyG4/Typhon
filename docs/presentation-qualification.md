# Presentation regression and requalification

Direct Scanout has completed project hardware qualification and is qualified
for production automatic use; `OBLIVION_ONE_DIRECT_SCANOUT=auto` is the default.
Runtime qualification is separate: a new compositor starts without a proven
current content/KMS candidate, and every candidate must still pass the exact
scene, device, plane,
format/modifier, generation, synchronization, cursor, presentation-state, and
KMS `TEST_ONLY` checks before it can be submitted. A failed proof falls back to
composition.

This procedure is for regression and hardware requalification of that
production default. The canonical matrix values are `off` and `auto`.
`experimental-auto` remains a deprecated compatibility alias for existing
launch scripts; new procedures should use `auto`.

Direct Scanout presentation-property qualification uses the stable WindowGroup
`SceneNodeId` for active, canonical, and physically presented Geometry/Opacity
state. `root_surface_id` remains the current render/input/frame adapter and is
preserved in immutable physical evidence, so an XWayland backing replacement
does not make previously presented nonidentity state disappear before the new
identity frame is physically shown.

The reproducible matrix tool is:

```bash
bin/qualify-presentation --dry-run
```

It prints the sequential matrix across direct policy (`off`, `auto`), triple
buffering (`off`, `auto`, `force`), and cursor
scheduling (`auto`, `piggyback`, `software`). Every combination has a distinct
phase label. No result is considered a qualification until it has been run on
a real TTY with the same hardware and driver.

For a live run, provide the command that owns one session:

```bash
OBLIVION_ONE_QUALIFY_COMMAND="$PWD/bin/start-oblivion-one-tty" \
  bin/qualify-presentation
```

Each phase writes bounded, labeled artifacts under
`~/.local/state/oblivion-one/qualifications/<timestamp>/`, including the
session log, trace placeholder, metrics placeholder, environment snapshot,
and summary. The summary reports trace drops when the running compositor emits
that metric. Each phase explicitly selects its Direct Scanout policy. Adaptive
Sync and tearing qualification remain independent and must be selected
explicitly for the live phase being run.

## Adaptive Sync Phase 1 qualification

Adaptive Sync uses `OBLIVION_ONE_VRR=off|auto|on`. `off` never requests it;
`auto` requires the compositor's solitary-fullscreen candidate; `on` requests
it for a capable Atomic output path. Capability comes from the live DRM
connector `vrr_capable` property being nonzero and the CRTC `VRR_ENABLED`
property being present. Sysfs may be recorded as a cross-check but cannot
override those DRM properties. Legacy KMS is not VRR capable.

For each live TTY/DRM run, record the configured policy, connector and CRTC
property observations, output/DRM generation, effective mode and blockers,
submitted mode, and matching pageflip-confirmed mode. Confirm that the initial
state sets `VRR_ENABLED=0`, that every Adaptive transaction includes the exact
requested VRR state in both TEST_ONLY and real requests, and that shutdown and
session recovery restore the value captured during discovery. A pageflip
confirms only that the KMS request was committed; it does not prove that a
physical monitor changed its refresh interval on that frame.

Unit tests cover property discovery, request parity, transaction ownership,
fallback, confirmation, and restoration. They do not qualify a real driver or
monitor. A real TTY/DRM qualification must separately exercise `off`, `auto`,
and `on`, fullscreen entry/exit, modes with a cursor and plane activity,
TEST_ONLY and real-submit rejection fallback, session suspend/resume, and
shutdown restore on the target kernel, driver, connector, and monitor.

Phase 1 deliberately keeps Adaptive Sync on conservative `ReactiveDouble`
pacing. Further VRR scheduler and final hardware qualification work is
intentionally deferred while the project waits for the required NVIDIA driver
support on the Astrea/Typhon target hardware. This target-hardware qualification
hold does not disable or remove Phase 1. Fixed-refresh VSync, Direct Scanout,
Async/tearing, FIFO, Commit Timing, Predictive O1, and presentation-feedback
ownership remain independent and must continue to pass their existing tests.

Each live phase must exercise and inspect:

- an idle-to-visual frame and sustained fullscreen cadence;
- `kernel-submitted + prepared`, and in worker mode
  `kernel-submitted + worker-queued-next`;
- Direct Scanout steady state, rejection, exit to composition, and re-entry;
- hardware cursor piggyback, cursor-only commits, and software cursor damage;
- callback, feedback, release, queue-overflow, duplicate-settlement, and
  invariant counters;
- render/GPU-ready, scheduler wake, worker queue residency, worker submit-wake,
  Atomic ioctl, submission-budget, ready-wait, submit-to-pageflip, and target
  slip timing.

The authoritative pipeline log is `native.presentation_pipeline`. It reports
the configured policy, effective mode, exact capability/blocker, current
primary, kernel-submitted owner, worker-queued-next owner, prepared owner,
future-primary depth, free compositor slots, direct state, force-unavailable
blocker, and ledger ownership cross-check. Future-primary depth must never
exceed two and explicit compositor slot capacity must remain three.

For explicit Atomic output, the shutdown summary separates total Atomic
submissions from the synchronization path: `atomic_in_fence_submissions`
counts Vsync submissions carrying the primary-plane `IN_FENCE_FD`, while
`async_userspace_fence_submissions` counts Async submissions whose render fence
was waited in userspace and was intentionally omitted from the Atomic request.

The required application matrix for a real TTY/DRM qualification is Palworld,
Steam UI/popups/fullscreen launch, Firefox, Kitty move/resize, and one
additional Vulkan game. Unit tests and `--dry-run` output are not real hardware
qualification.
