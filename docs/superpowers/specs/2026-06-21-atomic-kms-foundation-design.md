# Atomic KMS Foundation Design

## Scope

Add a conservative single-output Atomic KMS backend while retaining the current
legacy path as an explicit startup-only fallback. The change does not implement
VRR, direct scanout, atomic cursor planes, overlays, fences, damage clips,
hotplug, or multi-output policy.

## Existing path

The native runtime selects a connector, CRTC, and mode, renders an initial
GBM/dumb framebuffer, calls legacy `set_crtc`, and promotes that framebuffer to
current. Later EGL/GBM and CPU GBM frames reserve the existing process-wide
pageflip token, call the legacy pageflip ioctl with `PAGE_FLIP_EVENT`, retain the
buffer as pending, and promote it only after the matching DRM event. The event
timestamp and sequence drive presentation feedback. A drop guard restores the
captured legacy CRTC state. Hardware cursor updates use legacy cursor ioctls.

## Architecture

Create `src/native/kms/` with focused responsibilities:

- `properties.rs`: typed DRM object/property identifiers, exact-name property
  discovery, required and optional property caches, and captured values.
- `atomic.rs`: plane discovery/selection, checked fullscreen geometry, mode
  blob ownership, deterministic atomic request construction, raw ioctl packing,
  initial test/real commits, steady-state flips, and restore/safe-disable.
- `state.rs`: explicit one-pending-commit state using the existing nonzero
  token identity and backend generation.
- `legacy.rs`: the existing set-CRTC/pageflip/restore behavior behind the same
  display backend interface.
- `mod.rs`: `auto|atomic|legacy` policy, effective backend kind, startup-only
  fallback decisions, and shared submission/completion types.

Raw atomic ioctl storage owns its vectors for the whole ioctl call and carries
the full `u64` user data. The render loop sees typed requests and a selected
display backend, not raw property IDs.

## Startup data flow

1. Parse `OBLIVION_ONE_KMS_MODE`.
2. On `auto` or `atomic`, request `DRM_CLIENT_CAP_ATOMIC` on the KMS card FD.
3. Select connector/CRTC/mode and open the scanout producer.
4. Discover connector, CRTC, and primary-plane properties; snapshot current
   property values; select a compatible primary plane for the producer format.
5. Create and retain the selected-mode blob.
6. Build the complete connector/CRTC/plane state and submit
   `TEST_ONLY|ALLOW_MODESET`.
7. Submit the same state with `ALLOW_MODESET` and promote the initial buffer.
8. In `auto`, capability/discovery/plane/test failures before takeover may use
   legacy. Forced `atomic` fails. A real atomic takeover failure never silently
   downgrades.

## Runtime and completion

A regular frame changes only primary-plane `FB_ID` and submits
`NONBLOCK|PAGE_FLIP_EVENT` with the existing pageflip token. Submission failure
keeps the ready buffer retryable. Exactly one pending state owns the token,
framebuffer ID, submission time, and backend generation. Only a matching DRM
event clears it and promotes pending to current. Event parsing, scheduler
completion, kernel timestamp/sequence handling, frame callbacks, explicit sync,
and presentation feedback remain shared and unchanged.

## Restoration

The atomic backend snapshots connector, CRTC, and primary-plane values before
takeover. Orderly shutdown first attempts a tested exact restore. If captured
resources are no longer usable, it commits one atomic safe-disable transaction.
Only after restore/disable may compositor-owned mode blobs and scanout
framebuffers be released. Drop is best-effort and never panics. Legacy policy
retains the existing CRTC restore behavior.

## Hardware cursor

Primary-plane requests never contain cursor-plane properties. Existing legacy
cursor enable/move/disable ioctls and software/client-cursor fallback remain
unchanged. Driver rejection is reported and follows the existing explicit
software fallback; this task does not migrate the cursor plane.

## Testing

Pure tests cover policy, property discovery, plane selection, geometry,
request serialization/flags/token, commit state, retryable buffers, blob
lifecycle abstraction, restore/safe-disable construction, and absence of cursor
properties. Existing native scheduler, DRM event, cursor, explicit-sync, and
damage tests remain authoritative. Real DRM takeover is reported separately
from unit verification.
