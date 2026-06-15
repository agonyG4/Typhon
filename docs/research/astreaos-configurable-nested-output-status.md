# AstreaOS Configurable Nested Output Status

Date: 2026-06-15

## Implemented Path

The owned compositor path now accepts nested output configuration on:

```sh
./bin/start-oblivion-one --width 1920 --height 1080 --refresh 165 -- zen-browser
```

The launcher remains a transparent forwarder. The Rust compositor parser
validates `--width`, `--height`, and `--refresh`, builds `NestedOutputConfig`,
and rejects those nested-only flags when output resolves to native. Native KMS
mode selection remains `OBLIVION_ONE_MODE=WIDTHxHEIGHT@HZ`.

## Semantics

- Width and height are the initial logical nested host-window size.
- Manual host-window resize is still respected; it updates output size from the
  actual Winit window and does not force the CLI size back.
- Refresh is advertised through `wl_output.mode` as millihertz and used for the
  nested active wakeup interval with integer nanosecond division.
- The nested event loop remains host-paced and condition-driven. It does not
  repaint unchanged scenes solely because the configured interval elapsed.
- Host monitor refresh is reported when Winit exposes it. If the host monitor is
  lower than the requested nested refresh, startup logs warn that presentation
  will be host-limited.
- With `OBLIVION_ONE_PERF_LOG=1`, nested output periodically emits
  `perf nested.timing` with target refresh, target interval, redraw request,
  presented-frame, coalescing, idle/active wakeup, and host refresh fields.

## Validation Targets

Automated coverage includes parser syntax, app-argument delimiter handling,
invalid nested values, native rejection, nested timing intervals, initial cursor
geometry, minimum host-window sizing, launcher dry-run forwarding, and
`wl_output.mode.refresh` updates.

Manual validation still needs real nested browser runs on 60 Hz and 165 Hz host
monitors. Use `hyprctl monitors` before testing to confirm the monitor refresh.
