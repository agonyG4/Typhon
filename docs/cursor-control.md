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
```

The desired state is the requested configuration. The active state is the last
fully loaded and published configuration. They may differ briefly when a
candidate cannot be loaded. A generation changes only after publication;
identical set values are a no-op, while `reload` reloads assets even when the
values are unchanged.

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

The configuration path must be absolute. Typhon rejects symlinked directories
and files, foreign ownership, non-regular nodes, and incorrect modes. It
creates only its own `AstreaOS` and `input` directories as `0700`; it does not
chmod `XDG_CONFIG_HOME`, `$HOME`, or their existing parents. The file is
`0600`. A complete document is written to a private unpredictable temporary
file, flushed, `fsync`ed, atomically renamed, and followed by a parent-directory
`fsync` where supported. Cleanup removes only the temporary file created by
that write attempt.

Set and reload operations validate and load the complete candidate before
persistence and publication. Any failure preserves the active cursor, active
generation, and previously valid persisted file. Startup uses a valid and
loadable persisted configuration; a missing, malformed, insecure, or unavailable
configuration falls back to the existing default and produces one bounded
warning without preventing compositor startup.

## Rendering and generations

The software renderer and hardware cursor-plane path consume the same active
generation. A successful change invalidates cursor-related software state,
damages old and new visible bounds, requests a normal repaint, and requests a
normal cursor-plane replan. Hardware buffers continue through the existing
ownership and in-flight settlement rules. Old generations remain alive until
all owners release them; repeated replans do not double-release resources. If a
hardware update cannot use an otherwise valid theme, Typhon keeps the logical
configuration and uses the software fallback.

The requested size is passed to the XCursor loader, which selects an available
frame for that requested size. Typhon does not treat scaling a low-resolution
already-rasterized cursor as the authoritative size change.

## Child processes and client-owned cursors

Typhon does not modify its process-global environment. Future children launched
through the existing supervisor receive command-local:

```text
XCURSOR_THEME=<active theme>
XCURSOR_SIZE=<active size>
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
cursor_config_missing
cursor_config_invalid
cursor_config_insecure
cursor_config_write_failed
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
