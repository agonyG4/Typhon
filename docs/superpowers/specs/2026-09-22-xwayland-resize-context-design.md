# XWayland Resize Context Design

## Goal

Preserve valid non-resizing X11 configure work after the resize epoch captured at enqueue time retires, while keeping resize-owned work strictly bound to its original epoch.

## Design

`WindowBackendCommand::Configure.resize_epoch` remains captured for every X11 configure, including `resizing: false`. The field has two meanings at dequeue time:

- `resizing: true` uses the epoch as strict ownership. Missing, retired, or different epochs discard the command; matching epochs translate to `BeginResizeSync`.
- `resizing: false` uses the epoch as enqueue-time resize context. No epoch becomes ordinary `ConfigureFrame`; a matching live epoch becomes a position-only `XwmCommand::Configure`; a retired epoch becomes ordinary `ConfigureFrame`; a different live epoch discards the command without rebinding it.

`FinalizeResize` keeps strict ownership and continues to require an exact live epoch. No enqueue-time capture is removed, no native XWayland event ordering changes, and no broad field rename is needed.

## Observability

The existing `xwayland_resize_backend_command` trace remains conditional and gains distinct reasons for matching position-only translation, conversion after resize-context retirement, and discard after a newer resize supersedes the captured context. The retirement fallback identifies `translation=configure_frame`.

## Verification

Add compositor regressions covering both late `ResizeSyncPresented` and `ResizeSyncTimedOut` retirement after a post-release move. Each test holds the backend queue until after retirement, verifies canonical and visual geometry remain at M, and asserts the drained command is `ConfigureFrame(M)`. Preserve the existing E1→E2 ownership tests and run the focused and full Typhon checks with all build output under `/mnt/Aether/Desktop/GitHub`.
