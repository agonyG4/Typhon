# Display Phase 2A: Atomic Runtime Mode Transactions

## Goal

Complete one-output runtime resolution and refresh-rate changes on a qualified
Atomic KMS path, with exact native mode selection, safe scanout replacement,
server-owned rollback, persistence on Keep, and startup restore. Scale, transform,
output enablement, topology, positioning, and VRR mutation remain unsupported.

## Current source and boundary

Typhon already has typed configure/confirm/revert protocol shapes,
generation-qualified mode snapshots, structured rejection codes, and a
`NativeRuntime` that owns KMS, scanout, presentation, input, cursor, and control
state. Today its output capability snapshot deliberately reports mode mutation
unsupported, its mode list discards native mode structs after projection, and
its output-list snapshot publishes no transaction. The new Display transaction
must remain independent from the frame-presentation `OutputTransactionLedger`.

Eclipse already preserves generation-keyed drafts, transaction recovery from
ordinary refresh, stale-completion rejection, and read-only `Display.qml`.
Only backend compatibility fixes and documentation changes are in scope there.

## Architecture

### Applied state and exact mode inventory

Retain every connector mode as an internal `(opaque id, drm_mode_modeinfo)`
entry. IDs keep the existing inventory-position identity and are qualified by
the semantic output configuration generation. Public mode projection remains
bounded and may be truncated; mutation validates both that the ID is in the
current public snapshot and that the generation-qualified inventory resolves
it to one exact native mode.

Represent the applied output as one runtime-owned configuration containing the
selected target and exact native mode, mode ID, scale 1000, and normal
transform. Existing mode label, refresh, server geometry, scanout geometry,
input bounds, cursor bounds, and timing must be derived or updated from this
configuration at the same successful commit boundary.

### Request and KMS lifecycle

Validate request shape, output identity, generation, mode, scale, and transform
at dispatch. Exact-current requests return a complete snapshot immediately.
One changed request may be pending; retain its control token and request ID and
return the result asynchronously. While pending, stop starting new presentation
work, but continue servicing terminal pageflip, worker, cursor, and sync events.

Prepare dimension-correct candidate scanout resources without dropping the
active scanout. Render a full compositor frame and obtain its framebuffer.
After existing ownership has reached a proven safe boundary, build one Atomic
runtime candidate. Its exact request and mode blob are used for both
`TEST_ONLY` and real submission. Keep the active backend mode/blob until real
submission succeeds; then adopt the candidate and publish the new runtime
configuration. A failed validation, candidate render, test, or submit leaves
the old applied configuration authoritative.

Factor the common post-modeset ownership transition from session recovery:
retire old KMS ownership only after successful synchronous modeset, terminalize
any prior direct primary exactly once, update the compositor-owned baseline,
invalidate mode/dimension-dependent proofs and damage, reset timing/adaptive
state, and request a full repaint. Do not fake session suspend/resume.

### Confirmation transaction, persistence, and startup

After a temporary apply, create one nonzero server-owned Display transaction
containing the previous exact configuration, temporary applied configuration,
applied generation, state, and a monotonic deadline of about 15 seconds.
Expose its remaining time as informational state through every complete output
snapshot. Add its deadline to the existing native wake planner. Explicit revert
and timeout use the same output reconfiguration engine and exact captured old
mode. Failed rollback remains an explicit blocking `rollback_failed` state.

Keep writes a strict versioned private record to
`$XDG_CONFIG_HOME/AstreaOS/typhon/output.json` (or `$HOME/.config` fallback)
using `PrivateConfigFile` and the bounded persistence-worker pattern. Persist
connector identity, available physical-size evidence, and exact stable native
timing fields; never persist generation-local mode IDs, DRM object IDs, or
preferred/type metadata as timing identity. Persistence must succeed before
the transaction is cleared. If expiry wins an in-flight persistence race,
rollback remains authoritative; a late completion cannot confirm the expired
transaction and must be compensated before another Display mutation is
accepted.

Load persistence before startup target/modeset construction. Resolve only
against the current connected connector and exact current inventory. Startup
precedence is explicit non-auto `OBLIVION_ONE_MODE`, valid persisted mode, then
normal auto/preferred selection. Invalid or absent persisted hardware falls
back without preventing startup or rewriting the file.

## Capability and wire behavior

Advertise `modeSelectionSupported` only when Atomic KMS, exact inventory,
candidate scanout preparation, TEST_ONLY, and rollback support are active.
Legacy KMS and unqualified scanout backends report false. Scale, transform,
enable/disable, topology, positioning, and VRR mutation remain unsupported.
Configure, confirm, and revert return full authoritative output-list snapshots;
ordinary `outputs` refresh recovers the transaction after Settings restarts.
No QML mutation controls are added.

## Verification

Use fake time and an injected KMS submitter to test the transaction and Atomic
candidate state machines without physical DRM. Cover generation and exact-mode
resolution, bounded async gating and ownership quiescence, identical test/real
candidate, failure non-publication, successful adoption/retirement, coordinated
geometry/timing/cache updates, apply/revert/expiry/Keep/persistence races,
startup precedence, and Legacy-vs-Atomic capability advertisement. Run the
relevant Typhon and existing Eclipse Settings suites with build output only in
`/mnt/Aether/Desktop/GitHub`. Live hardware qualification is separate and must
be reported only if actually performed.

## Known limitation

Typhon owns connector presentation identity and optional physical-size
evidence, but not authoritative EDID identity. Persistence therefore uses the
strongest identity currently available and may safely fall back if connector
identity changes. This milestone remains single-output and does not claim
general topology or live hardware qualification from deterministic tests.
