# Native Atomic KMS Foundation

## Previous pipeline

Native startup rendered the first scanout buffer and programmed it with legacy
`set_crtc`. EGL/GBM and CPU-GBM frames then reserved a nonzero process-wide
pageflip token, called legacy `page_flip`, retained the submitted buffer as
pending, and promoted it to current only after the matching DRM event. The dumb
buffer path was immediate. Shutdown attempted a legacy CRTC restore.

## Atomic pipeline

The card FD requests `DRM_CLIENT_CAP_ATOMIC` before plane enumeration. Discovery
caches exact required properties for the selected connector (`CRTC_ID`), CRTC
(`ACTIVE`, `MODE_ID`), and a compatible primary plane (`FB_ID`, `CRTC_ID`,
source/destination geometry, and `type`). Optional VRR, in-fence, out-fence, and
damage-clip properties are recorded for diagnostics only.

The compositor selects a primary plane compatible with the selected CRTC and
scanout format, preferring the plane already attached to that CRTC. Source
geometry uses checked unsigned 16.16 values and destination geometry uses
integer pixels. An RAII mode blob owns the exact selected mode.

Initial state binds connector, CRTC, mode, primary framebuffer, and 1:1
fullscreen geometry in one request. The request must first pass
`TEST_ONLY | ALLOW_MODESET`; the real takeover uses `ALLOW_MODESET` and is
blocking. Steady-state requests set only primary-plane `FB_ID` and use
`NONBLOCK | PAGE_FLIP_EVENT` without `ALLOW_MODESET`.

## Completion and ownership

Atomic and legacy submissions share the existing process-wide `u64` token and
DRM event parser. Each EGL/GBM or CPU-GBM scanout backend owns an explicit
`Idle`/`Pending` commit state tagged with its backend generation. A failed ioctl
returns the buffer to ready state. Only the matching token and generation can
promote pending to current; timers and watchdogs cannot fabricate presentation.
Kernel sequence and timestamp remain authoritative for protocol completion.

## Policy and restoration

`auto` falls back to legacy only for atomic capability, discovery, or test-only
failure before hardware takeover. `atomic` fails startup in those cases, while
`legacy` never probes atomic state. A real initial-commit or runtime failure
never silently downgrades an owned atomic pipeline.

The pre-takeover connector, CRTC, and selected primary-plane values are captured.
Shutdown test-validates and commits an exact restore where possible. If saved
external objects are no longer valid, one complete atomic transaction detaches
the plane and connector and clears CRTC mode/active state. Scanout buffers stay
alive until restoration or safe disable finishes; the compositor mode blob is
destroyed afterward.

The hardware cursor deliberately remains on legacy cursor IOCTLs. Atomic
primary-plane requests contain no cursor-plane assignments.

## Validation status and non-goals

Pure request, property, policy, geometry, mode-blob, and commit-state tests run
without a DRM device. Physical native takeover was not performed in the
development container. The required RTX 3060 Ti TTY matrix therefore remains
outstanding.

This foundation does not implement VRR, direct scanout, atomic cursor planes,
overlay assignment, KMS in/out fences, framebuffer damage clips, hotplug,
multi-output transactions, HDR, or color management.
