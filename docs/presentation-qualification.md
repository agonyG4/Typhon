# Presentation qualification

Typhon keeps direct scanout disabled by default. Set
`OBLIVION_ONE_DIRECT_SCANOUT=experimental-auto` only for an explicitly labeled
qualification run; `auto` is retained as a compatibility alias and is not a
production default.

The reproducible matrix tool is:

```bash
bin/qualify-presentation --dry-run
```

It prints the sequential matrix across direct policy (`off`,
`experimental-auto`), triple buffering (`off`, `auto`, `force`), and cursor
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
that metric. The tool does not enable VRR or tearing and does not change the
default direct policy.

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

The required application matrix for a real TTY/DRM qualification is Palworld,
Steam UI/popups/fullscreen launch, Firefox, Kitty move/resize, and one
additional Vulkan game. Unit tests and `--dry-run` output are not real hardware
qualification.
