# Native termination reliability review

Review date: 2026-10-08. The current working tree is the implementation source of truth. The October 8 termination cause remains unknown; the repeated effect-owner warnings are not treated as causal evidence.

## Failure-path map

- **Startup and command exit:** [`src/main.rs`](../src/main.rs) parses the command, binds the Wayland socket in `own_compositor`, returns early for `--check`, then starts native bootstrap. Startup errors now print one bounded JSON error-chain record. The `NativeStartupError` source carries the bootstrap phase.
- **Requested shutdown:** input dispatch sets `NativeCycleState::shutdown_requested` in [`src/native_output/runtime/cycle_dispatch.rs`](../src/native_output/runtime/cycle_dispatch.rs). [`cycle.rs`](../src/native_output/runtime/cycle.rs) forwards it to `request_native_shutdown`. [`shutdown_cycle.rs`](../src/native_output/runtime/shutdown_cycle.rs) stops KMS admission, records the expected pageflip token, and advances the lifecycle. [`shutdown.rs`](../src/native_output/runtime/shutdown.rs) makes duplicate requests idempotent. Shutdown drains a pending pageflip with a bounded timeout, stops session children, then restores KMS.
- **Reactor, seat, and session failures:** [`cycle/pageflip.rs`](../src/native_output/runtime/cycle/pageflip.rs) waits for reactor events, checks KMS worker health, processes worker events, dispatches seat events, and continues suspend/resume recovery. Errors propagate to `NativeRuntime::run`; session suspension by itself is a recoverable state transition, not proof that the compositor should exit.
- **KMS worker failures:** the worker health and event paths in [`cycle/pageflip.rs`](../src/native_output/runtime/cycle/pageflip.rs) propagate fatal worker conditions. [`kms_worker_teardown.rs`](../src/native_output/runtime/kms_worker_teardown.rs) joins the worker, settles or quarantines submitted ownership, and classifies restoration proof. `SafeDisable` is a cleanup boundary, not an explanation for why shutdown began.
- **Child process exits:** [`runtime/xwayland.rs`](../src/native_output/runtime/xwayland.rs) reaps supervised children. The supervisor in [`src/process.rs`](../src/process.rs) writes each exit and restart result to stderr; runtime performance logging adds optional fields. A critical shell exit can transfer authorization to a restarted shell. An exit alone does not request compositor shutdown. Errors while handling the exit can propagate through the runtime path.
- **Runtime error and emergency cleanup:** `NativeRuntime::run` in [`runtime/mod.rs`](../src/native_output/runtime/mod.rs) now records the original error and phase before attempting XWayland cleanup, reactor synchronization, and session-child termination. Cleanup failures are separate bounded fields. `Drop` then quiesces workers, drains or quarantines ownership, attempts the existing KMS safety boundary, and emits a summary on both the proven and unproven cleanup paths.

## Where the cause was obscured

Before this change, the runtime returned its original error, but the CLI displayed only the outer `Display` text after cleanup had already produced KMS, child, and renderer logs. Several emergency cleanup results were discarded. KMS teardown logs describe the cleanup outcome (`Restored`, `SafeDisable`, or unproven ownership); they do not identify the initial failure. Detailed performance output is optional and therefore could not serve as the only termination record.

The new journal is fixed-size: it keeps the first typed cause, current runtime phase, one bounded cleanup error, and a count of later cleanup failures. It stores no per-frame event history and writes only from process exit/`Drop`, never in frame processing. The summary includes latest submitted and presented frame IDs, pageflip token/state, KMS worker state, seat state, active GPU release leases, output transactions, submitted/quarantined KMS ownership, and final KMS teardown safety. JSON is bounded; the file is written through a same-directory temporary file and rename with private permissions. Stderr remains an independent sink.

There is no safe in-process final event after `SIGKILL`; the summary may therefore be absent or describe the preceding termination if the process is killed before `Drop` runs. No signal handler was added.

## Scene ownership and damage findings

`freeze_presentation_effect_influences()` in [`runtime/frame.rs`](../src/native_output/runtime/frame.rs) builds only presentation-animation influence records, which require a window-group `SceneNode`. The physical effect-damage snapshot is frozen separately for every resolved effect. A layer or overlay may therefore lack a presentation owner while still contributing physical damage. The existing `effect_without_presentation_owner_keeps_physical_damage` test covers that invariant.

The ownership path is consistent with that split:

- [`state/hit_testing.rs`](../src/compositor/state/hit_testing.rs) resolves popup ownership to its popup owner and follows subsurface ancestry to its root.
- [`state/active_scene.rs`](../src/compositor/state/active_scene.rs) resolves presentation `SceneNode` IDs only for window groups. Non-window surfaces intentionally have no such node.
- Frame preparation separately maps retained exit-animation surfaces to their retained `SceneNode` and root before resolving ordinary live surface ownership.
- [`physical_effect_damage_tests.rs`](../src/native_output/runtime/physical_effect_damage_tests.rs) covers introduced/removed effects, registry-generation transitions, delayed old-generation submissions, rejected candidates, and damage-history repair. [`scene_history.rs`](../src/native_output/runtime/scene_history.rs) promotes the submitted snapshot matching the pageflip token and rejects stale tokens.

The aligned surface/node/owner projections were traced through construction and presentation. Some helpers use debug assertions before `zip`, but snapshot construction has unconditional length assertions and `PresentedCanonicalSceneSnapshot::capture` validates lengths and identities before producing evidence. No silent truncation path was demonstrated, so the parallel collections were not broadly refactored. No rendering, buffer-age, DMA-BUF explicit-sync, pageflip ownership, or direct-scanout behavior was changed.

## Classification

### Confirmed issues addressed

- **Termination observability gap:** runtime errors and cleanup events had no independent, structured final summary; cleanup errors could be ignored. The first runtime cause is now preserved separately from cleanup outcomes, including when a shortcut initiated shutdown first.
- **Activation-environment gap:** [`bin/start-oblivion-one`](../bin/start-oblivion-one) intentionally clears inherited display variables, while D-Bus/systemd activation previously received no replacement `WAYLAND_DISPLAY` after the socket became available. `main.rs` now calls `dbus-update-activation-environment --systemd` after binding the socket and before native bootstrap. Failure is reported but does not abort compositor startup.

### Plausible risks

- A failure in the activation-environment helper leaves user services with stale variables; Typhon logs the failed status and continues. The code does not restart or activate the portal service.
- A runtime error occurring after a normal shutdown request is retained as a cleanup error, preserving the initiating user request as the first cause. This summary records the first cause and the first cleanup error, not an unbounded sequence of every later cleanup event.
- Termination summaries cover orderly error returns and Rust unwinding while `Drop` runs. They cannot report a final in-process event after forced process termination.
- No hardware A/B measurement was run for logging, frame CPU time, wake frequency, or GPU behavior. The new runtime-phase writes are one enum-field update at phase boundaries; no performance claim is inferred from that design.

### Unsupported hypotheses

- The missing presentation-owner warnings caused the October 8 termination. They are rate-bounded in the current source, but missing owners can be valid, and physical effect damage is independent.
- `SafeDisable`, XWayland termination, or client disconnects caused the compositor exit. They can all follow cleanup after the cause.
- The absence of a matching coredump, SIGSEGV, OOM-kill, or NVIDIA Xid establishes a particular alternative cause.

The supplied incident report gives approximately 74,021 owner warnings. A separate local CPU qualification note records a different warning count from its capture. Neither count identifies a termination cause.

## Desktop integration and configuration findings

The GTK portal startup failures were reviewed separately from compositor termination. The user-session journal showed earlier `xdg-desktop-portal-gtk` starts failing with an empty display and a later successful start; the failures predate the available Typhon session log. This establishes an activation/display-environment issue to investigate independently, not a cause of the compositor exit.

The current blur policy contains `layers.default: "enabled"`, which is not accepted by the Rust schema (`client_only` or `disabled`). The loader logs the validation error and uses built-in defaults. No user configuration was edited, and `enabled` was not silently reinterpreted. The material configuration passed its schema validation.

## External design references

These references informed invariants only; no code was transplanted:

- wlroots scene nodes have explicit node/tree lifecycle and surface damage handling: [wlroots scene API](https://github.com/swaywm/wlroots/blob/master/include/wlr/types/wlr_scene.h) and [surface API](https://github.com/swaywm/wlroots/blob/master/include/wlr/types/wlr_surface.h).
- Smithay render elements use identity and commit counters to determine damage across committed content: [Smithay renderer elements](https://docs.rs/smithay/latest/smithay/backend/renderer/element/index.html).
- KWin effects contribute repaint damage through effect repaint planning: [KWin blur effect source](https://github.com/KDE/kwin/blob/master/src/plugins/blur/blur.cpp).
- XDG Desktop Portal activation needs the Wayland display in the D-Bus/systemd activation environment: [XDG Desktop Portal system integration](https://flatpak.github.io/xdg-desktop-portal/docs/system-integration.html) and [`dbus-update-activation-environment`](https://dbus.freedesktop.org/doc/dbus-update-activation-environment.1.html).

## Retrieving diagnostics

The last orderly termination summary is written to:

```text
${OBLIVION_ONE_STATE_DIR}/diagnostics/last-termination.json
```

When that override is absent, `XDG_STATE_HOME/oblivion-one/diagnostics/last-termination.json` is used; otherwise the default is `~/.local/state/oblivion-one/diagnostics/last-termination.json`. The same JSON record is written to stderr and therefore appears in the launcher's `session.log` when started through `bin/start-oblivion-one`.

For a future exit, inspect the summary and matching boot/session logs:

```sh
cat "${OBLIVION_ONE_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/oblivion-one}/diagnostics/last-termination.json"
journalctl --user -b -u xdg-desktop-portal-gtk.service
journalctl -b --since '10 minutes ago' | rg -i 'typhon|oblivion-one|drm|seat|oom|xid'
```

If the process was forcibly killed, use the preceding summary only as last-known state and inspect the boot journal/kernel log for the kill event. Do not treat the preceding summary as proof of the forced-kill cause.

## Rollback

Review the commit history and working tree first, then revert the task commit with `git revert <commit>`. This leaves later work intact and creates a normal rollback commit on `main`; do not reset the branch or discard the unrelated working-tree changes.
