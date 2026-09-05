# astreactl

`astreactl` is Typhon's local control client. Each invocation sends
one versioned request to the per-instance Unix socket and accepts exactly one
newline-terminated response. Responses are capped at 1 MiB; the client never
reads an unbounded stream or waits for server EOF after a complete frame.

## Commands

```text
astreactl version
astreactl status
astreactl doctor
astreactl outputs
astreactl windows
astreactl activewindow
astreactl keyboard config
astreactl keyboard layout
astreactl keyboard next
astreactl keyboard previous
astreactl keyboard set INDEX
astreactl keyboard configure [--rules VALUE|--model VALUE|--layout VALUE|--variant VALUE|--options VALUE|--repeat-rate RATE|--repeat-delay MS|--default-layout INDEX|--default-layout-index INDEX|--clear-rules|--clear-model|--clear-variant|--clear-options]
astreactl cursor get
astreactl cursor set-theme THEME
astreactl cursor set-size PIXELS
astreactl cursor set --theme THEME --size PIXELS
astreactl cursor reload
```

`astreactl --version` reports the client build. `astreactl version` queries the
running compositor.

## Options

Global options are `--json`, `--instance NAME`, `--socket ABSOLUTE_PATH`, and
`--timeout 250ms|2s`. The default timeout is two seconds and the maximum is
sixty seconds.

Keyboard configuration options are typed flags on `keyboard configure`. The
command first reads the active configuration, merges only supplied flags, and
then submits one complete configuration transaction. Nullable RMLVO fields can
be cleared explicitly; a field cannot be set and cleared in the same command.

Socket discovery uses explicit `--socket`, then `--instance`, then
`WAYLAND_DISPLAY`, then a single valid Typhon instance under
`$XDG_RUNTIME_DIR/astrea/typhon`. Multiple instances require explicit
selection. Automatic runtime, Astrea, Typhon, and instance directories are
validated as non-symlink, effective-user-owned `0700` directories. The
Astrea, Typhon, and instance directories must be effective-user-owned,
non-symlink directories with mode `0700`; the runtime directory must be
effective-user-owned, non-symlink, and not group- or world-writable.
The `WAYLAND_DISPLAY` instance directory receives the same instance-directory
validation before its socket is accepted. The control socket must be an
effective-user-owned, non-symlink Unix socket with mode `0600`. Automatic
discovery inspects only bounded direct children, fails closed when the entry
limit is exceeded, and sorts valid instance names lexicographically; temporary
socket files and lock files are not instance directories. Instance names such
as `attempt-1` are valid. An explicit `--socket` path validates its parent
components for directory and symlink safety and validates the socket itself.

Every invocation uses one monotonic total deadline for connect, request write,
write-half-close, and response read. The default is two seconds and the maximum
is sixty seconds. A slow-drip peer cannot extend the deadline with repeated
small reads.

## Output and Exit Codes

`--json` writes exactly one JSON value to stdout. Human-readable output is
written as concise text without JSON string quoting. Exit codes are 0 for
success, 1 for a server command error, 2 for usage errors, 3 for an unavailable
endpoint, 4 for transport errors, 5 for timeouts, 6 for protocol or response
errors, and 7 for a successfully decoded unhealthy doctor report. An unhealthy
doctor report is still printed normally; it does not produce a synthetic error
diagnostic.

Each command has an exact result schema. A successful response must contain a
result object with every required field and the type required by that command;
`version`, `status`, `doctor`, `outputs`, `windows`, and `activewindow`
are decoded into their corresponding typed snapshots before either output mode
is run. Snapshot objects reject unknown fields. A missing, null, incorrectly
typed, or command-incompatible result is a malformed response and exits `6`.
Duplicate global options, including `--json`, `--timeout`, `--instance`,
and `--socket`, are usage errors and exit `2`.

Human output sanitizes all client-controlled labels before writing them to a
terminal. Newlines, tabs, carriage returns, and other control characters
become spaces; ESC and C1 terminal escape introducers are removed. JSON output
is serialized from the validated typed snapshot and is not sanitized or
mutated.

## Snapshot Semantics

Window identity is the compositor's `WindowId`; PID is metadata only. Titles and
app IDs are UTF-8 bounded, mapped and minimized are reported separately, and
full counts are calculated before the bounded window list is truncated.

Output, KMS, renderer, cursor, Direct Scanout, VRR, worker, triple-buffering,
session, shutdown, and XWayland fields are sourced from the native runtime.
`configured`, `available`, and `active` are not interchangeable: Direct Scanout
is only `active` when the presented primary plane is direct, and VRR is not
reported active merely because policy or capability exists. Hardware EDID
serial and physical dimensions are currently unavailable and remain `null`.

`doctor` reports stable checks for `control.endpoint`, `session.state`,
`output.available`, `kms.backend`, `renderer.backend`, `output.mode`,
`cursor.backend`, `xwayland.state`, `shutdown.state`, `kms_worker.state`,
`direct_scanout.state`, `triple_buffering.state`, and `vrr.state`. `healthy` is
true only when every check is `ok`.

The session vocabulary is `active`, `suspended`, `recovering`, and
`failed`; a failed native session is never presented as recovering. Optional
feature policies are reported separately from runtime degradation:
intentionally disabled KMS worker, VRR, triple-buffering, or Direct Scanout
policies are healthy, while an explicitly forced feature that cannot be
honored is a warning or error. Automatic KMS worker startup degradation is a
warning only when the startup outcome proves that degradation; legacy
synchronous fallback is informational/healthy. VRR is not reported active
merely because it is requested or supported, and Typhon reports it active only
after an actual KMS enable.

Window collection uses an explicit serialized-byte budget below the 1 MiB
protocol cap. It reserves response/envelope headroom, accounts for every
serialized window object, preserves the authoritative `total`, and sets
`truncated` before adding a window that would exceed the budget. XWayland
states use the stable strings `disabled`, `armed`, `starting`,
`running_base`, `running`, `backoff`, and `failed`.

Cursor commands are typed M4 commands. `cursor get` and every successful cursor
mutation return a `CursorSnapshot` containing desired and active theme/size,
the generation, backend (`hardware`, `software`, `hidden`, or `unavailable`),
configuration source (`default`, `config`, or `control`), and persistence state.
It also reports `asset_source` as `system_theme` or `builtin_fallback`. The
snapshot distinguishes the desired logical XCursor configuration from the
actually loaded fallback asset.
The client validates the complete result before formatting it. A valid envelope
with a missing, null, incompatible, or malformed result is a protocol error and
exits `6`; it is never treated as an empty successful snapshot.

Themes are logical XCursor names, not paths. They are 1–128 UTF-8 bytes made
only from ASCII letters, digits, `.`, `_`, and `-`; path separators,
whitespace, controls, traversal, and non-ASCII characters are rejected locally
with exit `2`. Cursor sizes are integers from 8 through 256 pixels. Syntactically
valid but unavailable themes are server errors and exit `1`. Only one cursor
mutation may be active at a time. A second mutation returns the stable
`cursor_generation_busy` detail and exit `1`; `cursor get` remains immediate
while a mutation is loading or persisting.

Human output sanitizes all client-controlled labels before writing them to a
terminal. Newlines, tabs, carriage returns, and other control characters become
spaces; ESC and C1 terminal escape introducers are removed. JSON output is
serialized from the validated typed snapshot and is not sanitized or mutated.

Cursor persistence and runtime behavior are documented in
[`cursor-control.md`](cursor-control.md). Existing client-owned Wayland cursor
surfaces are outside the compositor-owned cursor authority and may not reload.

Keyboard layout commands query or change the compositor's ephemeral locked XKB
layout for the current seat. `keyboard layout`, `keyboard next`, `keyboard
previous`, and `keyboard set INDEX` use the exact wire commands
`keyboard.layout.get`, `keyboard.layout.next`, `keyboard.layout.previous`, and
`keyboard.layout.set`. Successful responses contain `effectiveIndex`,
`lockedIndex`, `layoutCount`, and the configured layout names with stable
numeric indices. Layout changes do not recompile or resend the keymap, and are
not persisted across compositor restarts. Invalid or extra arguments are local
usage errors for the CLI and `invalid_argument` responses for the control
protocol.

Human keyboard output shows effective and locked indices plus a sanitized list
of numeric layout identities; JSON output preserves the validated names.

`keyboard config` returns the complete typed runtime configuration, including
RMLVO, repeat settings, default layout index, generation, source, persistence
state, pending state, and the effective/locked layout projection. The exact
wire commands are `keyboard.config.get` and `keyboard.config.set`; the latter
requires every typed field in the request and rejects unknown fields.

Configuration changes are transactional. Typhon validates and compiles a real
XKB candidate on the compositor thread, persists the desired configuration in
`$XDG_CONFIG_HOME/AstreaOS/input/keyboard.json` (or
`$HOME/.config/AstreaOS/input/keyboard.json`) using a bounded versioned JSON
document, and publishes only after persistence completes and all physical and
compositor key state is quiescent. A pending configuration is not reported as
active, and a successful response is not sent before commit. Restart restores
the last successfully persisted candidate; invalid, missing, insecure, or
unavailable persistence falls back safely to the compiled default chain.
Environment overrides are applied at startup and reported explicitly in the
configuration snapshot. Repeat-only and default-layout-only changes avoid
keymap recompilation; full RMLVO changes migrate same-name locked modifiers,
clear transient depressed/latched state, and publish the appropriate Wayland
keymap/repeat/modifier events.

## Scope

Window commands remain read-only: M3 does not activate, minimize, restore, or
close windows; or provide subscriptions, remote access, DBus, or Dock
integration. Runtime keyboard layout control remains available for ephemeral
locked-layout changes, while v3 adds typed transactional RMLVO/repeat
configuration. It does not add window mutation, process launch, or streaming
events.

## Packaging

The repository installer installs launcher scripts, not built Rust binaries.
Build the client with `cargo build --release --bin astreactl`; installing that
artifact is currently external to `bin/install-start-oblivion-one`.
