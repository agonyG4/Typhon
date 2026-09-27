# Scaled Primary Plane Capability Probe

## Goal

Add an opt-in diagnostic that asks the active DRM primary plane whether it accepts a fullscreen client DMA-BUF scaled from its full source dimensions to the full output mode. A positive or negative answer is telemetry only; the current frame continues through normal composition.

## Design

### Scene eligibility

Keep `direct_scanout_scene_candidate()` and `DirectScanoutSceneAnalysis::candidate` as the accepted direct-scanout API. Add a separate probe-candidate path that reuses the same scene analysis and candidate materialization. The probe path may ignore only the buffer/output-size mismatch. It still requires the existing opaque DMA-BUF, covering application, no-content-above, no-overlay, no-popup, no-decoration, no-effect, no-clip, no-animation, no-resize-preview, no-pending-work, and full-output placement checks. Probe viewport validation requires scale 1, Normal transform, full-source identity semantics, and full-output destination semantics when viewport metadata exists. The probe candidate records actual source dimensions separately from mode dimensions.

### Atomic request and worker ownership

Add `AtomicPlaneGeometry::full_source_to_output(source_width, source_height, output_width, output_height)`, reusing the existing unsigned 16.16 source validation. Build the full primary-plane state through `AtomicRequest` for FB_ID, CRTC_ID, all SRC fields, all CRTC fields, and rotate-0 when the primary exposes a rotation property. Keep the same request builder usable by later real presentation, while exposing a submitter method that can only perform TEST_ONLY for the diagnostic path.

Run the ioctl on the existing KMS worker. The worker accepts a synchronous diagnostic request only while its lifecycle is Running and its queue, reservations, execution, kernel in-flight state, cursor sidecar, and phase are all idle. It rechecks before execution and returns a busy skip if work arrived meanwhile. Once executing, the worker thread serializes this ioctl with normal submission ioctls. The probe request has no output transaction, frame token, direct lease, frame batch, feedback, or commit operation. TEST_ONLY leaves current physical KMS state untouched.

### Cache and telemetry

Use a bounded, probe-only cache keyed by output identity, output and DRM generations, connector, CRTC, primary plane, mode dimensions, DMA-BUF format/modifier, source dimensions, stable plane-layout hash, connector content-type property/value, input-fence property contract, primary rotation-property presence, current hardware-cursor assignment, and any other request state that affects TEST_ONLY. Cache accepted and rejected TEST_ONLY results; import failures are cached separately to prevent repeated import work. A busy skip is not cached so a later idle cycle may try once. Never consult this cache from production direct-scanout validation.

Gate candidate inspection, probe attempts, and probe telemetry behind `TYPHON_SCALED_DIRECT_PROBE=1`. Report candidate observations, attempts, accepted/rejected results, cache hits, busy skips, and size-mismatch-only direct-scanout rejections. With the feature disabled, do not derive probe candidates or change emitted telemetry. When `TYPHON_DIRECT_SCANOUT_DEBUG=1`, log one concise result per uncached completed attempt.

## Validation

Unit tests cover geometry construction and overflow; complete atomic assignments; separation of accepted and probe candidates; viewport and scene blockers; source/output dimensions; cache positive and negative behavior and key identity; and worker idle admission. The worker tests verify that either TEST_ONLY result produces no real submit or direct ownership. Tests use recording/fake worker executors and do not require DRM hardware.

## Follow-up invariant for real scaled direct scanout

Primary-plane geometry is persistent KMS state. If real scaled direct scanout later programs source 1600x900 to destination 1920x1080, every transition back to composition must explicitly restore full-output primary geometry, or authoritative physical plane assignment state must include geometry. A framebuffer-only update is not sufficient. This diagnostic patch does not change production physical plane state.
