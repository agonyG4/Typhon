# Opaque RGB Direct Scanout and Doctor Diagnostics Design

**Goal:** Qualify explicitly supported opaque RGB8888 client buffers, including XBGR8888, without weakening composition rules, and make the remaining overlay/effect blockers actionable in the one-shot doctor snapshot.

## Architecture

`DrmFormat` is the single format-opacity authority. It will represent XBGR8888 distinctly, round-trip its exact FOURCC, and expose `is_opaque_rgb8888()`. Buffer alpha capability, presentation coverage, scene qualification, and fullscreen opacity checks will call that helper; no caller will infer opacity from an arbitrary FOURCC.

Presentation coverage will use the semantic `OpaqueRgb8888` state. Direct scene qualification will require the source buffer to be explicitly opaque RGB8888, while the native path will retain the exact source FOURCC and modifier in the candidate key, validation key, framebuffer descriptor, capability lookup, and KMS transaction. Atomic bootstrap capability discovery will filter explicit opaque formats, then require the existing exact EGL feedback, GBM allocation, and primary-plane checks before advertising each exact pair. Normal compositor render-target negotiation remains unchanged.

The doctor path will derive bounded diagnostics only when `ControlCommand::Doctor` is dispatched. Layer-shell entries will be built from authoritative committed role state plus active render targets and will report root, namespace, layer, mapped state, configured geometry, active surface IDs/details, intersection, and truncation. Effect entries will be built from the authoritative resolved effect scene and will report visible count, instance ID, registered program identity, anchor, output-space region, and target surface where available. Existing `overlay_visible` and `effect_requires_composition` qualification behavior remains intact.

## Error handling and bounds

Unsupported or unknown formats remain non-opaque. Native structural validation keeps its existing plane, stride, offset, modifier, and contiguous-index requirements, with format-neutral opaque-RGB messages. Capability absence rejects before import and falls back to composition. Doctor output exposes truncation rather than emitting unbounded metadata, and never includes shader source or frame-loop logging.

## Testing

Tests will be written red-first for FOURCC parsing/round-trip and opacity, XBGR coverage and scene qualification, native structural validation and exact AddFB2 identity, exact capability acceptance/rejection, Atomic capability discovery filtering, and doctor formatting/metadata/truncation. Focused tests will run before the full locked Cargo checks, clippy, source-layout check, diff check, and hardware retest.
