# AstreaOS native one-shot screenshot design

## Scope

Typhon owns an authenticated, private `astrea-screen-capture-v1` request and
services it in the native capture work domain. Eclipse consumes the result on
the already authenticated `TyphonSharedConnection`, freezes it in a shell
overlay, and performs crop, PNG, and clipboard work in Qt.

## Authority and ordering

The request is accepted only for the exact authenticated Wayland `ClientId`
used by `astrea_shell_mutation_allowed`. A request is queued after Wayland
dispatch. Native work reclassification marks capture and acquire/prepare due,
but never presentation due. The cycle executes acquire/prepare first, then
services the request, then independently reclassifies physical presentation.
Inactive sessions fail before EGL/KMS access. Pending resources are removed on
resource/client/output/session/compositor teardown.

## Rendering and transport

The atomic and compatibility GLES backends make their ordinary EGL context
current, construct the ordinary `EglSceneDrawRequest`, and invoke one shared
`GlesSceneRenderer` capture primitive. The primitive renders a full-damage,
age-zero scene into a temporary `GL_RGBA8` texture/FBO with the backend's
normal framebuffer origin, omits the cursor, and synchronously reads RGBA8.
Rows are normalized to displayed top-first order and alpha is forced to 255.
Capture uses `discard_rendered` and restores presentation-oriented renderer
state (timing, diagnostics, origin, and damage/presentation history); it never
calls a presented-commit path or allocates a physical scanout slot. CPU paths
return `unsupported`.

Normalized bytes are checked against a 256 MiB limit, written to a CLOEXEC,
sealable memfd, and sealed with shrink/grow/write/seal seals before the fd is
sent in `ready`. The only v1 format is RGBA8888.

## Eclipse flow

`TyphonScreenCaptureClient` binds the capture manager and `wl_output` through
the shared connection and tags every proxy/event with its connection
generation. `ScreenshotController` owns the request, frozen image, overlay,
selection mapping, cancellation, atomic PNG save, and clipboard publication.
The QML overlay is hidden until `ready`, uses the captured image provider with
a generation query, maps logical selection coordinates to pixels with clamping,
and applies the legacy tiny-selection full-image fallback. The shell shortcut
router maps `screenshot_capture` independently of Alt+Tab and Spotlight gates.

## Verification

Tests cover exact authorization, output/lifecycle/busy handling, capture-only
work classification, inactive-session short-circuiting, GLES capture request
parameters and normalization, state neutrality, bounded sealed transport,
Direct Scanout neutrality, PrintScreen press-only binding, protocol/client
transport, reconnect generations, shell routing, crop/save/provider behavior,
and overlay cancellation/flow. Native GPU/KMS qualification is reported only
if actual hardware is exercised.
