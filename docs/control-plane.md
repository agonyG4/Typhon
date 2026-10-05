# Typhon Control Plane

Typhon exposes one local control socket for each Wayland instance:

```text
$XDG_RUNTIME_DIR/astrea/typhon/<instance>/control.sock
```

The instance is the Typhon Wayland socket name. The `astrea`, `typhon`, and
instance directories are created with mode `0700`; the socket is created with
mode `0600`. The instance directory also contains `control.lock` with mode
`0600`. `XDG_RUNTIME_DIR` must be an absolute directory owned by the compositor
user and must not be group- or world-writable.

The native runtime owns the listener and every client registration through its
generation-safe epoll reactor. Socket creation, reads, writes, and accepted
connections use nonblocking close-on-exec descriptors. The server authenticates
each accepted stream with `SO_PEERCRED` and requires the peer UID to equal the
compositor effective UID. PID names, executable paths, claimed JSON fields, and
filesystem ownership are not authentication mechanisms.

Protocol version 1 accepts one bounded NDJSON request per connection. Requests
are limited to 64 KiB, responses to 1 MiB, simultaneous clients to 32, and
control operations to 16 per compositor cycle. A request-reading client and a
response-writing client each have a 10-second idle timeout, refreshed only by
successful byte progress or a completed state transition. The closest control
deadline is merged into the native timerfd deadline, and expiration is bounded
per cycle.

Readable bytes are processed before `EPOLLRDHUP` or `EPOLLHUP` cleanup. A
complete newline-terminated request therefore survives a peer write-half
close. An incomplete request at EOF receives one bounded `invalid_request`
response when the peer can still receive it. Partial reads and writes,
hangups, resets, malformed messages, slow peers, and stale reactor tokens are
client-local failures. Client admission, registration, and interest-modify
failures are also client-local; listener descriptor invariant failures remain
fatal. They do not terminate the compositor. The reactor continues to service
DRM, timer, input, Wayland, explicit-sync, XWayland, child, KMS-worker, and
render-fence sources normally.

Typhon takes an exclusive nonblocking lifetime lock on `control.lock` before
socket cleanup and retains it until the server is dropped. A second compliant
instance for the same Wayland name fails with an instance-locked error; the
kernel lock is released automatically if Typhon crashes. When an existing
socket is found, Typhon refuses symlinks and non-socket objects. A same-UID
live listener is an address-in-use failure. A same-UID socket is removed only
after a nonblocking probe returns a definitive dead listener result and its
file type, owner UID, device, and inode all still match the original probe.
Every post-bind failure is guarded by the bound socket identity and removes
only that socket. On shutdown Typhon unregisters every client and listener
token, closes every client and listener descriptor, and removes only the
socket inode created by that server instance; the lock file and parent runtime
directories remain. The server tracks bounded saturating counters for accepts,
rejections, malformed and oversized requests, timeouts, stale tokens, and
client I/O failures rather than logging every connection or client string.

M3 adds bounded read-only `version`, `status`, `doctor`, `outputs`, `windows`,
and `active-window` snapshots. The output snapshot carries a semantic
`configurationGeneration`, current and available opaque mode IDs, explicit
mutation capabilities, and a nullable transaction projection. The generation
is distinct from frame, render, swapchain, and KMS presentation IDs. Mode IDs
refer to exact entries in the selected connector's native mode inventory and
are valid only with that output configuration generation. M4 adds typed
`cursor.get`, `cursor.set-theme`,
`cursor.set-size`, `cursor.set`, and `cursor.reload` commands. Snapshot data is
copied into owned values before encoding, and the client decodes each success
into the exact command-specific result type. Missing, null, incompatible, or
malformed results are protocol errors (client exit `6`). Snapshot objects use
strict version-one schemas and stable string vocabularies.

Cursor mutation validation and final publication are dispatched on the native
runtime thread. The transport only decodes bounded arguments and queues a
response; it does not load cursor assets, access configuration files, create
framebuffers, or submit KMS state. One bounded cursor I/O worker owns complete
candidate loading and persistence. A second mutation is rejected as
`cursor_generation_busy`, while M3 reads and `cursor get` remain responsive.
After the worker completes, the runtime publishes a new cursor generation,
schedules ordinary repaint and cursor-plane replanning, and returns the
snapshot. Accepted jobs still publish after a client disconnects; only the
stale response token is ignored. Shutdown does not cancel an accepted job; it
drops the worker without waiting indefinitely for filesystem I/O, so a blocked
job may finish independently while the process tears down. Failed validation, loading, or pre-commit
persistence leaves the active generation unchanged. Old cursor generations
remain retained while KMS transactions, worker jobs, cursor-plane owners, or
software frames still reference them.

The v1 Display request shapes are `outputs.configure`,
`outputs.configure.confirm`, and `outputs.configure.revert`. Configure carries
the output ID, base configuration generation, opaque native mode ID, scale,
and transform. Confirm and revert carry only the server-issued transaction ID.
The server owns transaction deadlines; clients cannot select the timeout. A
successful future transaction must return a complete output snapshot, Keep
must persist before clearing the deadline, and Revert or expiry must restore
the captured exact configuration. Structured rejection codes distinguish
unknown outputs, stale generations, unknown modes, unsupported dimensions,
and transaction or persistence failures.

Runtime Display mode mutation is qualified only for Atomic KMS and a scanout
backend with a same-kind replacement path. Mode-selection capability is
published only when the runtime has an exact native mode inventory, Atomic
`TEST_ONLY`, the replacement scanout path, and the persistence worker. Legacy
KMS remains unsupported. Scale and transform must retain their authoritative
current values; output enable/disable, topology, positioning, and VRR mutation
remain unsupported. This is for Typhon's currently owned output and does not
claim general multi-output support.

Mode IDs are opaque IDs into the current connector's exact DRM mode inventory
and are valid only with the matching semantic output configuration generation.
They are never persisted. A successful temporary apply advances that
generation; a successful rollback advances it again; Keep does not. Requests
with stale generations are rejected before an ID is resolved. The public mode
list may be truncated, but every published ID remains unique and resolves to
one retained `drm_mode_modeinfo`; modes omitted from the public list cannot be
selected through the control protocol.

A changed configure request is queued as the runtime's single bounded Display
mutation. It stops new frame admission and waits for pageflip and KMS worker
ownership, the Atomic commit arbiter, presentation transactions, Direct
Scanout, cursor work, deferred completion events, and explicit-sync watches to
reach a safe boundary. READY work is retired through its existing abandonment
path. Typhon prepares a full-frame replacement scanout at the candidate
dimensions, then sends one stored Atomic request through `TEST_ONLY` and the
real modeset. Runtime state and the scanout swap are published only after the
real submit succeeds. A synchronous modeset disables VRR and uses Graphics
content type. Old Direct Scanout ownership is released only after that commit
proves KMS switched away from its buffer.

After temporary apply, Typhon owns one confirmation transaction with a
15-second monotonic deadline. `outputs` refresh returns its authoritative
transaction ID, state, applied generation, remaining time, and rollback error
if present. Settings can disconnect without affecting the deadline. Explicit
Revert and timeout use the same exact-mode reconfiguration path. A failed
rollback remains a terminal `rollback_failed` transaction and blocks another
Display mutation until an explicit recovery succeeds.

Keep writes before it clears rollback protection. The versioned strict
configuration is stored at
`$XDG_CONFIG_HOME/AstreaOS/typhon/output.json`, or
`$HOME/.config/AstreaOS/typhon/output.json` when `XDG_CONFIG_HOME` is unset.
It uses Typhon's private configuration file infrastructure, bounded reads and
writes, private directory and file modes, no-follow validation, and atomic
replacement. The document stores the connector identity Typhon owns, optional
physical-size evidence, and a stable native timing fingerprint containing the
pixel clock, active dimensions, sync and total timing, skew, scan multiplier,
and timing flags. DRM names and preferred-type metadata are excluded from the
timing identity. The fingerprint must resolve to exactly one current mode.
Typhon does not currently own EDID identity, so connector name is the strongest
persisted identity; physical-size evidence is an additional qualifier when
available.

Persistence runs on one bounded worker and does not block the compositor event
loop. A write failure returns `output_persist_failed` and leaves the
confirmation deadline armed. If expiry wins while a Keep write is outstanding,
the transaction remains rollback-owned; a late completion cannot confirm it,
and a successful late write is followed by a compensating write of the
restored mode. A failed compensation is logged and disables further mode
mutation for the runtime.

Startup chooses an explicit non-`auto` `OBLIVION_ONE_MODE` first, then a valid
persisted connector and exact timing match, then normal auto/preferred
selection. Persisted mode IDs are never consulted. Missing, ambiguous,
disconnected, or invalid persisted selections produce one bounded diagnostic
and fall back to normal selection without rewriting the file. Settings' Display
page remains read-only; Apply, Keep, and Revert controls are not enabled yet.
Deterministic tests qualify the state machine and injected Atomic submission
candidate lifecycle. Live hardware mode-mutation qualification has not been
performed.

Cursor configuration is persisted at
`$XDG_CONFIG_HOME/AstreaOS/input/cursor.json`, or
`$HOME/.config/AstreaOS/input/cursor.json` when `XDG_CONFIG_HOME` is unset.
AstreaOS-owned directories are private `0700` directories and the file is a
regular `0600` file. Reads and writes are relative to validated no-follow
directory descriptors. Publication uses a private unpredictable temporary
file, flush, file `fsync`, `RENAME_NOREPLACE` for first publication, and
`RENAME_EXCHANGE` plus exact old/new inode verification for replacement. The
parent-directory `fsync` is part of the persistence commit point where
supported. Symlinks, foreign ownership,
unexpected node types, malformed documents, and unsupported versions are
rejected. A missing standard `$HOME/.config` is securely created as `0700`; a
missing explicit `XDG_CONFIG_HOME` is not created implicitly. Startup falls
back to the existing default or builtin cursor, keeps the desired logical
configuration, and emits one bounded warning when persisted configuration or
assets are invalid or unavailable.

Cursor file input is bounded to 16 MiB; frames are limited to 1024 pixels per
dimension, 1,048,576 pixels, and 256 frames per file. A candidate retains at
most six unique selected images and caches repeated source aliases for the
duration of one load. Post-commit cleanup failures are reported as bounded
degradation and never cause runtime/disk divergence. Verified transaction
debris with `.cursor.json.tmp-*` or `.cursor.json.quarantine-*` prefixes may be
cleaned by a later write; replacement nodes are never removed.

Cursor backend values are `hardware`, `software`, `hidden`, and `unavailable`.
The compositor-owned shape values are `pointer`, `move`,
`resize_horizontal`, `resize_vertical`, `resize_diagonal_nw_se`, and
`resize_diagonal_ne_sw`. The pointer uses `left_ptr`, `default`, and `arrow`;
optional interaction aliases fall back to the pointer. The active shape bundle
is shared by software, atomic hardware, and legacy hardware paths. Hardware
update failure falls back to software without making a valid logical
configuration fatal. Future supervised children receive the desired logical
`XCURSOR_THEME` and `XCURSOR_SIZE` in their command-local environment; Typhon
does not mutate its process-global environment, and already-running clients
that own their own cursor surfaces are not forced to reload. The snapshot
`asset_source` value is `system_theme` for a loaded system theme and
`builtin_fallback` for Typhon's fallback; the latter is a doctor warning.

Window snapshots use an explicit serialized-byte budget below the 1 MiB
response cap. If the complete authoritative window set cannot fit, the list is
truncated before adding another object and its full `total` is retained. A
recognized command whose response serialization still fails is reported as a
bounded internal error, not `invalid_command`.

## Material configuration

`material.config.get` returns the authoritative `MaterialSnapshot`.
`material.config.set` accepts one complete version-one `MaterialConfiguration`
and returns the resulting authoritative snapshot. Its request and response
share the normal 64 KiB and 1 MiB control bounds. The candidate is fully
validated and resolved by Typhon's `MaterialCurveV1` before persistence. A
failed save leaves the active configuration and generation unchanged.

The snapshot's three capability booleans are runtime-confirmed permission for
Advanced override mutations. They do not report persistence support,
client-requested blur support, or raw renderer features. `AtomicEglGbm` and
`NativeEglGbm` currently enable Blur, Saturation, and Noise overrides; `Gbm`
and `Dumb` report them unavailable. An unsupported persisted override may be
kept at its current value or cleared, while introducing or changing that
override is rejected with `invalid_argument`. Position-only updates remain
allowed, including when they preserve an unsupported stored override.

Typhon persists the material document at
`$XDG_CONFIG_HOME/AstreaOS/typhon/material.json`, or
`$HOME/.config/AstreaOS/typhon/material.json` when XDG_CONFIG_HOME is unset.
AstreaOS and Typhon directories are owner-private `0700` directories, and the
regular material file is owner-private `0600`. The shared secure store rejects
symlinks, wrong ownership or modes, unsupported versions, malformed JSON, and
documents over 16 KiB. Writes use a private temporary file, flush and fsync it,
rename it atomically, and fsync the parent directory. No material file access
occurs in frame rendering.

A successful update publishes one material-aware canonical background effect
generation, advances compositor render state, and requests an ordinary redraw.
The material configuration itself never creates a visible effect instance;
only an existing surface assignment can require composition.

## Material program selection

The additive `astrea.control` v1 commands are:

- `material.program.catalog.get` with `{}` returns the bounded qualified
  Material Program Catalog snapshot.
- `material.program.get` with `{}` returns the requested/effective selection
  snapshot.
- `material.program.set` accepts a complete version-one object containing
  `version` and `requestedProgram`, and returns the authoritative selection
  snapshot.
- `material.program.state.get` with `{}` returns the catalog and selection
  captured from the same trusted registry generation.
- `material.program.describe` accepts `{"name":"glass.liquid"}` and returns
  the current qualified program's schema and typed parameter descriptors.
- `material.program.parameters.set` accepts one complete version-one
  per-program override configuration and returns the authoritative parameter
  configuration snapshot.

The catalog contains the valid `OnDamage`/`Backdrop` subset of the Trusted
Effect Registry that has only `UniformOnly` parameters and no
`TargetContent`/`StaticTexture` source. The built-in
`system.background_blur` is first and is the default and fallback. A set request
must name a currently registered qualified program. It may be remembered when
the active renderer cannot execute effects; renderer availability is reported
separately from Advanced override capabilities.

Selection is persisted separately at
`$XDG_CONFIG_HOME/AstreaOS/typhon/material-program.json`, or
`$HOME/.config/AstreaOS/typhon/material-program.json` when
`XDG_CONFIG_HOME` is unset. This private file stores requested intent only. It
does not store the effective fallback, registry generation, renderer
availability, or catalog contents. Missing or newly unqualified requested
programs fall back to `system.background_blur` without rewriting the request;
they become effective again automatically when a later successful registry
generation qualifies them. Failed saves publish no selection change, and an
identical set is a no-op.

This selection is independent of `material.config.get` and
`material.config.set`. Phase 2A `MaterialConfiguration` remains persisted in
`material.json` and continues to update `system.background_blur` while a
trusted-local program is selected.

`material.program.describe` exposes one descriptor per parameter in stable
name order. Values identify their own type (`float`, `vec2`, `vec3`, `vec4`, or
`int`). A trusted range is reported when present; an unbounded parameter has a
`null` range. Descriptions expose no renderer IDs, uniform locations, shader
source, filesystem paths, or effect graphs. The built-in
`system.background_blur` is describable and currently has no public parameters.

Parameter overrides are submitted as a complete desired set:

```json
{
  "version": 1,
  "program": "glass.liquid",
  "schemaSignature": 123456,
  "overrides": {
    "intensity": {"type": "float", "value": 0.72}
  }
}
```

Typhon validates names, types, finite values, ranges, and the current schema
before saving. A changed non-empty configuration with an old schema signature
is rejected. Parameter intent is stored separately at
`$XDG_CONFIG_HOME/AstreaOS/typhon/material-program-parameters.json`, or under
`$HOME/.config/AstreaOS/typhon/` when `XDG_CONFIG_HOME` is unset. The strict,
bounded private document stores only program names, schema signatures, and
named overrides. It does not store defaults, effective values, registry or
parameter generations, renderer state, or internal effect IDs.

Overrides remain stored if their program disappears or its schema changes.
They apply only while the current program has the same schema signature and
the complete override set still validates. A missing or changed schema uses
trusted manifest defaults without deleting the saved intent; restoring the
original schema makes the saved overrides effective again. Each accepted
changed parameter configuration advances `parameterGeneration` once. An
identical replay and a failed mutation do not advance it or request redraw.

The generations describe separate changes. The selection snapshot's
`generation` (`selectionGeneration`) tracks requested Material Program
changes. `registryGeneration` tracks trusted effect registry and schema
changes. `parameterGeneration` tracks accepted persistent parameter-override
changes. None substitutes for another; registry reloads do not advance
`parameterGeneration`, and parameter updates do not advance either selection
or registry generation.

The persistence domains remain separate: `material-program.json` stores the
requested program, `material-program-parameters.json` stores per-program
parameter intent, `effects.json` contains trusted effect definitions, and
`material.json` stores Phase 2A System Material configuration.

Cursor changes, wallpaper commands, window actions, and shell protocol remain
unavailable.

For a live socket conflict, inspect the instance-specific path above and stop
the owning Typhon instance before starting another one. An insecure or missing
`XDG_RUNTIME_DIR` is a bootstrap error; Typhon does not chmod or replace the
user's runtime directory.
