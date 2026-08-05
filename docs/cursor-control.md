# Runtime Cursor Control

Typhon owns the compositor cursor theme and size through the local
`astreactl` control socket. Changes take effect on the next normal eligible
frame and do not restart Typhon.

## Commands

```text
astreactl cursor get
astreactl cursor set-theme Bibata-Modern-Ice
astreactl cursor set-size 32
astreactl cursor set --theme Bibata-Modern-Ice --size 24
astreactl cursor reload
```

All commands accept the existing global `--json`, `--instance`, `--socket`,
and `--timeout` options. `cursor set` requires both options. Duplicate global,
theme, or size options are usage errors. Every successful command returns one
typed `CursorSnapshot` JSON value followed by one newline when `--json` is
used.

## Validation and result state

Themes are logical XCursor theme names. They must be 1–128 UTF-8 bytes and may
contain only ASCII letters, digits, `.`, `_`, and `-`. Empty values, path
separators, traversal, absolute paths, whitespace, NUL, controls, and
non-ASCII characters are rejected. Sizes must be integers in the inclusive
range 8–256 pixels. The same validator is used by CLI arguments, protocol
requests, persisted files, and startup configuration.

`CursorSnapshot` reports:

```text
desired_theme, desired_size_px
active_theme, active_size_px
generation
backend: hardware | software | hidden | unavailable
source: default | config | control
persistence: saved | missing | invalid | insecure | write_failed
asset_source: system_theme | builtin_fallback
```

The desired state is the logical requested configuration, including when its
system theme is unavailable. The active state describes the fallback
configuration that is actually loaded. `asset_source` distinguishes a loaded
system theme from Typhon's bounded builtin fallback; the builtin fallback is
reported as a doctor warning and is never advertised to child processes as a
theme named `builtin`. A generation changes only after publication; identical
set values are a no-op, while `reload` reloads assets even when the values are
unchanged.

## Compositor cursor shapes

Typhon selects one authoritative shape for all compositor-owned cursors:
`pointer`, `move`, `resize_horizontal`, `resize_vertical`,
`resize_diagonal_nw_se`, and `resize_diagonal_ne_sw`. The pointer is required
and is loaded through `left_ptr`, `default`, and `arrow`. Interaction shapes
use bounded standard XCursor alias lists: `move`, `fleur`, `all-scroll`;
`ew-resize`, `size_hor`, `sb_h_double_arrow`, `left_side`, `right_side`;
`ns-resize`, `size_ver`, `sb_v_double_arrow`, `top_side`, `bottom_side`;
`nwse-resize`, `size_fdiag`, `top_left_corner`, `bottom_right_corner`; and
`nesw-resize`, `size_bdiag`, `top_right_corner`, `bottom_left_corner`. A
missing or malformed optional shape falls back to the loaded pointer; a
missing or malformed pointer rejects the candidate. Shape transitions
invalidate software cursor bounds and cause the atomic or legacy hardware path
to rebuild its image even when the theme generation is unchanged.

## Persistence

The file is:

```text
$XDG_CONFIG_HOME/AstreaOS/input/cursor.json
```

or, when `XDG_CONFIG_HOME` is unset:

```text
$HOME/.config/AstreaOS/input/cursor.json
```

Its version-one schema is:

```json
{
  "version": 1,
  "theme": "Bibata-Modern-Ice",
  "sizePx": 24
}
```

The configuration path must be absolute. Typhon opens and validates the
`AstreaOS` and `input` directory descriptors with no-follow operations, then
opens `cursor.json` relative to the validated `input` descriptor. It rejects
symlinked directories and files, foreign ownership, non-regular nodes, and
incorrect modes. It creates only its own `AstreaOS` and `input` directories as
`0700`; it does not chmod `XDG_CONFIG_HOME`, `$HOME`, or their existing
parents. For the standard `$HOME/.config` fallback, a missing `.config` is
created as `0700`; a missing explicit `XDG_CONFIG_HOME` is a persistence error.
The file is `0600`, and reads are bounded before allocation beyond the document
cap. A complete document is written to a private unpredictable temporary
file, flushed, `fsync`ed, and published relative to the validated directory.
First publication uses atomic no-replace. Replacement exchanges the temporary
entry with the exact validated destination inode, verifies both identities, and
exchanges back on failure when both entries remain proven; unexpected
replacements are never overwritten or removed. The parent directory is
`fsync`ed where supported.

Set and reload operations validate on the native thread, then submit at most
one bounded mutation job to the cursor I/O worker. The worker owns persisted
configuration reads, bounded XCursor file reads and parsing, complete shape
bundle construction, temporary writes, file and directory synchronization, and
atomic publication. The native reactor never waits for those operations. While
one job is active, another mutation returns `cursor_generation_busy`; a second
Typhon instance that holds the persistence lock returns
`cursor_persistence_busy`. `get` and all M3 read-only commands remain
immediate. A disconnected or timed-out client does not cancel an accepted job:
the worker result is still published, while a stale response token is ignored.
Shutdown uses the deterministic policy that an already accepted job is allowed
to finish only its owned persistence work; runtime publication is stopped once
shutdown tears down the compositor. The worker never accesses compositor
state. Shutdown unregisters the worker notification and does not wait
indefinitely for a blocked filesystem call.

The worker and reactor share one `O_CLOEXEC` nonblocking `eventfd`. Notification
writes retry `EINTR`, treat `EAGAIN` as already notified, and reject short
writes. The reactor drain retries `EINTR` and continues until `EAGAIN`; notification
failures make the worker terminal and are counted. A panic in loading, parsing,
or persistence produces one bounded terminal completion, releases the mutation
slot, unregisters the worker source, and makes later mutations return
`cursor_io_unavailable`; read-only commands remain available. Cursor-worker
readiness is kept separate from control-client readiness, including terminal
`HUP` and `ERR` events.

The native compositor entry blocks `SIGCHLD` with `pthread_sigmask` before
constructing the Wayland server or entering output discovery, EGL/GBM probing,
DRM opening, renderer or input setup. `NativeRuntime::bootstrap` repeats this
idempotently, and the child supervisor repeats it immediately before creating
the signalfd. The signalfd remains the single child notification mechanism;
every later KMS, cursor, XWayland, and session-owned child path inherits the
blocked mask, and session-owned children are launched only after signalfd
ownership is valid.

The loader caps each cursor file at 16 MiB, each frame dimension at 1024,
each frame at 1,048,576 pixels, each file at 256 frames, and each candidate
load at six unique source images. A per-load source-path cache parses repeated
aliases once and retains only the selected bounded image. Overflow, malformed
frames, and unsafe dimensions reject the required pointer or fall back for an
optional shape.

Persistence has a global advisory lock at
`$XDG_CONFIG_HOME/AstreaOS/input/cursor.lock` (or the corresponding
`$HOME/.config` path). It is a descriptor-relative, owned regular file with
exact mode `0600`, opened with no-follow and held with nonblocking exclusive
`flock` through stale cleanup, publication, rollback, and cleanup. Typhon
captures the opened descriptor's device, inode, type, owner, and mode, then
compares the descriptor-relative `cursor.lock` entry with that exact identity
immediately after locking, before stale cleanup, and immediately before
publication. A replacement, symlink, or mode/ownership change fails closed as
`cursor_config_insecure`; it never cleans transaction files, publishes the
configuration, removes, or chmods the replacement lock. The lock file remains
after shutdown. Reads do not acquire it because canonical publication is
atomic; compliant Typhon instances serialize all writes. Advisory locking is
not a privilege boundary against arbitrary same-UID filesystem interference;
identity checks make such interference fail closed.

Persistence has a commit point: the exact new `cursor.json` identity has been
verified, its contents synchronized, and the parent directory synchronization
has completed or been accepted as unsupported. First publication uses
`renameat2(RENAME_NOREPLACE)`. Replacement uses
`renameat2(RENAME_EXCHANGE)`, verifies both exact identities, and exchanges back
only when both entries are still proven. Every pre-commit failure rolls back
and preserves the prior canonical file. After commit, old-inode cleanup is
best effort; cleanup or its directory synchronization can produce a bounded
degradation counter but cannot fail the mutation or leave runtime using the
old configuration while disk contains the new one. Verified stale files with
the `.cursor.json.tmp-*` and `.cursor.json.quarantine-*` prefixes are cleaned
on a later write; unexpected nodes are never removed.

Any pre-commit failure preserves the active cursor, active generation, and
previously valid persisted file. Startup uses a valid and loadable persisted
configuration; a missing, malformed, insecure, or unavailable configuration
falls back to the existing default and produces one bounded warning without
preventing compositor startup.

## Rendering and generations

The software renderer and hardware cursor-plane path consume the same active
shape bundle and generation. A successful change invalidates cursor-related
software state, damages old and new visible bounds, requests a normal repaint,
and requests a normal cursor-plane replan. Hardware buffers continue through
the existing ownership and in-flight settlement rules. Old complete bundles
remain alive until all shape images have no external owners; shared pointer
fallback aliases are counted once. Collection also runs at normal presentation
settlement points. If a hardware update cannot use an otherwise valid theme,
Typhon keeps the logical configuration and uses the software fallback. Legacy
disable and replacement failures are nonfatal and leave a valid software path.

The requested size is passed to the XCursor loader, which selects an available
frame for that requested size. Typhon does not treat scaling a low-resolution
already-rasterized cursor as the authoritative size change.

## Child processes and client-owned cursors

Typhon does not modify its process-global environment. Future children launched
through the existing supervisor receive command-local:

```text
XCURSOR_THEME=<desired logical theme>
XCURSOR_SIZE=<desired logical size>
```

Already-running Wayland clients that own their own cursor surfaces may continue
to draw those surfaces and are not guaranteed to reload. M4 does not implement
XSettings, desktop portals, wallpaper, Dock integration, window mutation,
remote control, subscriptions, or multi-output cursor policy.

## Errors and qualification

Locally invalid theme or size syntax is exit `2`. A syntactically valid but
unavailable theme, persistence failure, or other command failure is a server
error and exit `1`. Transport, timeout, endpoint, and malformed-response exit
codes remain `4`, `5`, `3`, and `6` respectively. The stable cursor detail
identifiers include:

```text
invalid_cursor_theme
invalid_cursor_size
cursor_theme_not_found
cursor_theme_load_failed
required_pointer_missing
cursor_file_read_failed
cursor_file_invalid
cursor_config_missing
cursor_config_invalid
cursor_config_insecure
cursor_config_write_failed
cursor_persistence_busy
cursor_io_unavailable
```

On a real native session, qualify with an installed theme:

```text
astreactl cursor get
astreactl cursor set-theme <installed-theme>
astreactl cursor set-size 32
astreactl cursor set --theme <installed-theme> --size 24
astreactl cursor reload
```

Verify movement, resize cursors, hide/show, hardware presentation when
available, software fallback, suspend/resume, and persistence across a
compositor restart. A qualification result should only be reported when a
real running Typhon session was actually queried.
