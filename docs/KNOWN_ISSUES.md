# Known Issues

## Effects hardware qualification is deferred

The effects engine has deterministic model, graph, damage, resource, protocol,
and renderer-path coverage, but no real TTY/DRM performance run has been
recorded for this checkout. GPU timing, native presentation combinations, and
the 1920x1080@165 qualification matrix remain deferred. The procedure and
required evidence are in `docs/EFFECTS_QUALIFICATION.md`.

The first effects milestone is linear-sRGB-only. HDR, wide-gamut color
management, and trusted static texture assets are not advertised or accepted.

## Direct Scanout remains opportunistic

Direct Scanout is project-qualified for production automatic use and defaults
to `OBLIVION_ONE_DIRECT_SCANOUT=auto`. This does not qualify every buffer or turn
the feature into arbitrary hardware-plane composition. Runtime eligibility and
KMS proof remain exact for each candidate; an ineligible or rejected candidate
returns to composition.

The current path assigns the primary plane on the selected output. It does not
provide generalized overlay/underlay allocation, multi-output scanout,
cross-device or multi-GPU scanout, HDR/color-equivalence handling, arbitrary
scaling or transforms, arbitrary primary-plane blending, or unrestricted
formats and modifiers. Presentation mode and content type are part of the
Direct Scanout validation identity, so VRR and tearing are not blanket
exclusions: each still depends on its own eligibility and exact KMS validation.

The current candidate attempt is available only on the explicit Atomic
EGL/GBM path when effective KMS worker transport is active and a worker handle
is present. The runtime feature state also requires a healthy worker and a
native session that permits output. With worker `off`, Legacy KMS,
or `auto` falling back to synchronous submission, ordinary composition
continues safely and Direct Scanout stays configured without being attempted.
This path limitation is not a compositor failure. Each attempted candidate
still needs exact scene, device, plane, format/modifier, synchronization,
cursor, generation, presentation-state, `DirectPlaneValidationKey`, and KMS
validation evidence; rejection returns to composition. Runtime state, exact
blockers, validation-cache counters, and TEST_ONLY/submission counters report
that evidence without a global sticky candidate-qualified state.

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

## XWayland is opt-in and not broadly real-application-qualified

The managed XWayland/XWM path is implemented but defaults off;
`TYPHON_XWAYLAND=off` is the default, with managed lazy and eager modes
available by opt-in. The managed XWM lifecycle, bidirectional CLIPBOARD and
PRIMARY transfer, and XDND in both directions are implemented. Runtime RandR
output publication and X11 cursor ownership integration remain incomplete,
and broad real-application interoperability qualification is still pending.
See [XWayland](XWAYLAND.md) and
[XWayland interoperability qualification](XWAYLAND_INTEROP_QUALIFICATION.md)
for the implemented scope and qualification boundary.

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
