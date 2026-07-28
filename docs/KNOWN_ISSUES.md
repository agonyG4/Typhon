# Known Issues

## Direct Scanout 2.0 remains experimental

Direct Scanout 2.0 is implemented as a conservative primary-plane assignment
inside the output transaction and KMS worker pipeline. It remains disabled by
default: `OBLIVION_ONE_DIRECT_SCANOUT=off` and
`OBLIVION_ONE_KMS_COMMIT_WORKER=off`. Experimental qualification requires the
Stage 3 KMS worker and the explicit `experimental-auto` direct policy. Real
TTY/DRM qualification is still pending and belongs to the next task.

The supported Stage 4 candidate is limited to one opaque solitary fullscreen
surface on one output: XRGB dmabuf content on the selected DRM device, an
exactly supported primary-plane format/modifier, identity geometry, identity
scaling, identity transform, and a supported explicit-synchronization
contract. Hardware cursor composition is allowed only when the complete
primary-plus-cursor atomic assignment validates. Any rejection falls back to
composition.

Stage 4 explicitly excludes overlay planes, VRR, tearing, multi-output,
hotplug, scaling, transforms, color conversion, HDR, cross-device or
multi-GPU scanout, primary-plane blending, and broad format expansion.

## Native SDDM and TTY validation is incomplete

The native session is implemented and deterministic tests cover the planning,
KMS, input, rendering, presentation, recovery, and shutdown seams. Real TTY
and SDDM runs are still required across the supported DRM drivers before the
session can be considered production-ready.

## Native backend fallbacks remain conservative

The default scanout policy attempts native EGL/GBM and can fall back to CPU GBM
or dumb framebuffer paths. The fallback paths are useful for recovery and
diagnostics but do not provide the same performance envelope as the normal
GPU path. Hardware cursor policy can similarly fall back to software unless
hardware cursor use was explicitly required.

## X11 compatibility is not enabled

Clients launched by Typhon receive the Typhon Wayland environment. An X11-only
client fails instead of opening on an inherited host display. A Typhon-owned
XWayland bridge remains an architectural boundary, not an enabled fallback.

## Application-specific graphics warnings

Some Chromium-based clients may choose a Vulkan path and print a client-side
warning while Typhon continues using native EGL/GLES. The compositor preserves
application arguments and does not silently rewrite that client policy.

## Native input fallback permissions

Without a working seat-managed libinput path, Typhon may use a direct libinput
or raw evdev fallback when permissions permit. The launcher warns when the
configured input group is unavailable. Physical device permissions and seat
ownership still need validation on the target session manager.

## Clipboard and driver coverage

Protocol coverage and DRM-driver behavior continue to require validation on
real hardware. These are native compositor issues and do not have a separate
host-window execution mode.
