# Display Phase 2A live hardware qualification

This document is a manual NVIDIA/Atomic qualification procedure for the
already-implemented output mode mutation path. The deterministic coordinator
scenarios run in Typhon tests; this hardware matrix is never started by
`cargo test` or normal session startup.

`OUTPUT_MODE_SELECTION_RUNTIME_QUALIFIED` remains `false`. The
`qualification` boot policy is for explicit target-hardware qualification and
is not an end-user feature. Scale, transform/orientation, VRR, color depth,
output enablement, topology, positioning, HDR/color pipeline, Night Shift, and
saturation mutation remain unsupported.

## Start a qualification session

Launch Typhon with the explicit qualification policy and Atomic native EGL/GBM
scanout:

```bash
OBLIVION_ONE_OUTPUT_MODE_MUTATION=qualification \
OBLIVION_ONE_KMS_MODE=atomic \
OBLIVION_ONE_SCANOUT_BACKEND=native-egl-gbm \
./bin/start-oblivion-one-tty
```

Before changing a mode, inspect the authoritative outputs snapshot and verify
that it advertises:

```text
modeSelectionSupported = true
```

Do not click Apply while this value is false. Stop the matrix and diagnose the
`native display mutation` startup diagnostic, especially its `blocker` field.
Qualification policy bypasses only the project-level production qualification
gate. Atomic KMS, exact native mode inventory, a live supported scanout,
persistence availability, compensation state, and rollback health still decide
whether the capability is exposed.

Record the GPU, connector, kernel and driver versions, starting exact native
mode, output configuration generation, and startup blocker/capability
diagnostic with the run. Select alternate modes by their opaque native mode
IDs and record each generation and transaction phase.

## Recovery before the first mode change

Keep a recovery terminal available. A confirmed `Keep` may persist a test mode
as the next startup mode. The native startup mode override takes precedence
over persisted output selection, so launch with the machine's verified
known-good mode and the mutation kill switch if recovery is needed:

```bash
OBLIVION_ONE_MODE=1920x1080@165 \
OBLIVION_ONE_OUTPUT_MODE_MUTATION=off \
./bin/start-oblivion-one-tty
```

Replace `1920x1080@165` with the actual known-good mode recorded for the target
machine. This recovery does not delete persisted configuration.

## Manual matrix

Run all cases on the designated output after the deterministic qualification
tests pass and the outputs snapshot reports mode mutation enabled.

| Case | Action | Required result |
| --- | --- | --- |
| A. Refresh revert | Current resolution/current refresh → alternate refresh → Apply → Revert | The alternate exact mode is active during confirmation; Revert restores the exact starting native mode and refresh. |
| B. Refresh keep | Current resolution/current refresh → alternate refresh → Apply → Keep | The alternate exact mode remains active, persistence completes, and the compositor restarts in that mode. |
| C. Resolution revert | Current resolution → alternate resolution → Apply → Revert | Scanout and output geometry change to the alternate dimensions; Revert restores the exact starting mode and geometry. |
| D. Resolution keep | Current resolution → alternate resolution → Apply → Keep | Scanout, server geometry, pointer mapping, cursor, and persisted startup selection use the alternate dimensions. |
| E. Settings exits | Apply a temporary mode, then kill/close Settings and wait for the server deadline | Typhon rolls back without a Settings client. The rollback deadline remains server-owned. |
| F. Settings reconnects | Apply a temporary mode, close Settings, then reopen it | The transaction and phase recover from the authoritative outputs snapshot; reconnect does not confirm, cancel, or restart the deadline. |

For every case, verify and record:

- cursor remains usable and pointer mapping matches the active output bounds;
- XWayland remains alive and usable;
- Wayland clients receive the correct output geometry;
- scheduler and frame pacing follow the new refresh;
- the compositor remains responsive;
- rollback leaves no persistent black screen;
- stale pageflip or worker completion cannot corrupt the restored mode;
- Direct Scanout safely falls back and can requalify after the generation change;
- the server-owned rollback deadline continues while Settings is closed.

Do not claim hardware qualification unless the complete A–F matrix passes and
the recorded evidence covers each required check. Any failure, unavailable
capability, or ambiguous output snapshot stops the matrix for diagnosis.

## READY ownership failure discovered during qualification

The first NVIDIA apply attempt reached Settings draft, `outputs.configure`,
runtime output mutation, and Apply, then terminated with
`overtaken READY has no pacing owner`. The explicit swapchain still held the
expected physical READY identity, but frame pacing no longer reported the
matching logical READY owner. This exposed a real ownership divergence; the
failure must remain an invariant error until every sidecar is settled together.

The runtime fix binds physical rendered-frame identity to pacing ownership and
retires an unsubmitted READY frame through one reconfiguration operation. That
operation validates the swapchain, pacing, scene history, scheduler, output
transaction, and protocol batch before retiring them. Structural modesets,
including Revert rollback, publish a new output epoch after the old epoch is
quiesced. DRM session and buffer-pool generations remain separate lifetimes.

After deterministic regression coverage passes, rerun this exact NVIDIA case
first and capture the termination journal plus Display transaction state:

```text
1920x1080@165 → 1920x1080@60 → Apply → Revert
```

Use the explicit qualification session above. Continue the larger matrix only
after this case completes and restores the exact starting mode without an
ownership invariant failure. The production qualification constant remains
`false`; this regression and a single successful retry do not qualify the
Display mode-selection path.
