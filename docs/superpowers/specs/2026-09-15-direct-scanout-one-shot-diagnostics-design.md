# Typhon Direct Scanout One-Shot Diagnostics

## Goal

Make `astreactl doctor` explain the current Direct Scanout failure stage without adding work to the frame loop or changing the Direct Scanout policy, Presentation Coverage, fullscreen, tearing, VRR, or physical KMS authorities.

## Design

The existing `ControlCommand::Doctor` branch will perform one scene-analysis query and format its result into the existing `DoctorCheck.detail` field for `direct_scanout.state`. The detail will combine scene evidence, informational semantic-solitary-fullscreen state, current feature state, live runtime gates, and the latest Atomic Direct Scanout counters. No other control command, output snapshot field, or protocol version changes.

The diagnostic will use the existing `OwnCompositorServer::direct_scanout_scene_analysis()` result. The covering root comes from Presentation Coverage, opacity is rendered-coverage proof, blockers are serialized with `DirectScanoutSceneRejection::as_str()`, and semantic solitary fullscreen is queried only for the candidate root as metadata. This keeps physical scene coverage separate from fullscreen presentation policy.

`DirectScanoutCounters` will retain the historical `first_blocker`, add `last_blocker`, and keep `blocker_set` as the union of observed classes. Atomic blocker recording will update the latest blocker on every observation without resetting the first blocker.

Human doctor output will render a sanitized indented detail line. JSON remains the authoritative structured response because it carries the unchanged `DoctorCheck` shape.

## Testing

Tests will cover first/latest blocker semantics, no-candidate scene blockers, candidate-plus-counter detail formatting, human detail sanitization, and the doctor-only placement of scene derivation. Focused tests will run RED before implementation and GREEN after implementation, followed by the repository verification commands.
