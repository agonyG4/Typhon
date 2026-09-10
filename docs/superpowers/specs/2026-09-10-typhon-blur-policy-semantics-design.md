# Typhon Blur Policy Semantics Repair

## Goal

Repair the existing Astrea blur policy without changing the Effects v1 renderer, preserving transactional reloads, client exact regions, precedence, capability handling, and trusted scene ordering.

## Design

`BlurPolicyConfig` becomes the only persistent representation. It contains only the version, enabled flag, approved Wayland/XWayland/layer modes, fullscreen option, and bounded named rules. Window and layer rules use the public `name`/`match`/`blur` schema. Separate mode enums make unsupported v1 combinations (`xwayland: auto` and `layers.default: auto`) unrepresentable during deserialization.

`BlurPolicySnapshot` becomes a runtime status representation. It reports the requested status fields, including current renderer capability, generation, rule counts, active assignment counts by source, bounded path, and bounded reload error. It is never used as the JSON load target. Invalid startup config leaves the built-in config installed, records the error, emits a warning, and allows compositor startup; a failed reload leaves the previous valid generation installed.

The resolver keeps compact decision data in the frame path. A `ResolvedBlurAssignment` carries only `BlurAssignmentSource`, `EffectAnchorScope`, target surface ID, and bounded `EffectRegion`; no rule strings enter an effect instance. Client assignments use `Surface` scope, desktop-window synthesized assignments use `VisualGroup`, and explicit layer-rule synthesized assignments use `Surface`. The resolved scene consumes this one canonical assignment path, so direct scanout sees the same real effect instances as rendering.

Automatic synthesized regions start as the visible surface candidate and subtract the committed, bounded opaque rectangles. Full opacity suppresses automatic assignment; partial opacity subtracts exact rectangles; no opacity metadata keeps the full candidate. Client-provided regions bypass this optimization unchanged. The subtraction operation is added to the existing bounded `EffectRegion` algebra and is deterministic.

## Error and bounds policy

The loader rejects files over 256 KiB, more than 128 window rules or 128 layer rules, duplicate/empty/overlong rule names, unknown fields, invalid modes, and regex patterns over 256 bytes. Regexes compile only during load/reload. Runtime diagnostic strings are truncated to a fixed bound before entering status output.

## Tests

Add policy-unit coverage for schema rejection, precedence and rule matching, alpha capability, opaque full/partial/multi-rectangle subtraction, scope by source, and reload failure preservation. Add an integration test through `resolved_effect_scene()` and `direct_scanout_scene_candidate()` for fullscreen automatic blur, explicit enable, and removal after reload. Existing client exact-region, renderer capability, effects ordering, and trusted binding tests remain authoritative.

