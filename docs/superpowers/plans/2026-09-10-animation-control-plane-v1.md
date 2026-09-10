# Typhon Animation Control Plane v1 Implementation Plan

## Constraints and checkpoints

- Preserve the dirty `src/effects/render_graph.rs` change.
- Reuse the existing `target/`; never run `cargo clean` or create another build directory.
- Use `rtk run --` for Cargo, source-layout, and test commands.
- Keep animation semantics separate from `TrustedEffectRegistry`.
- Add tests before each production seam and run the narrow test target before proceeding.

## 1. Add typed catalog and user configuration

Create `src/animation_control/{mod,catalog,config}.rs` and export the module
from `src/lib.rs`. Define typed slot, effect, availability, and preset enums
with stable string conversion, fixed deterministic catalog arrays, compatibility
validation, and the v1 defaults. Define strict serde wire types for the
versioned user document, finite speed validation, override preservation, and
requested/effective resolution. Add unit tests for defaults, all presets,
Astrea Lamp reservation, planned fallback, override precedence, clearing, and
invalid selections.

## 2. Add secure persistence and snapshots

Create `src/animation_control/persistence.rs` by following the existing
Astrea configuration hierarchy and secure atomic replacement conventions. Keep
the document bounded, validate directory/file identity and permissions, handle
missing/invalid/unsupported input without panicking, and expose an injectable
store/failure path for transaction tests. Create `snapshot.rs` with bounded
camel-case JSON types, deterministic slot maps, catalog capability entries,
source/startup-override metadata, and requested/effective fields. Test missing,
round-trip, malformed, unsupported-version, bounded, and atomic-failure cases.

## 3. Integrate the runtime policy

Add a compositor-owned animation control state containing the validated user
configuration, resolved typed policy, generation, source metadata, and
persistence store. Load defaults, persisted intent, and legacy environment
overrides in that order. Add typed `curve_for` resolution that delegates
geometry effects to the existing `PresentationAnimationPolicy`, applies speed
scaling without changing base constants, and returns no curve for disabled or
non-geometry effects. Add exact speed-1 and spring/easing equivalence tests.

Update the geometry transition entry point to resolve semantic slots instead of
treating `PresentationAnimationStyle` as the complete user model. Capture the
curve at transition start and retain it for retargeting. Route enabled changes
through the existing animator cancellation behavior; preset, speed, and slot
changes affect only future transitions. Add live geometry tests for Astrea/KDE
identity, macOS/KDE runtime switching, disabled cancellation, and transition
curve stability.

## 4. Extend control commands and CLI

Add `animation.config.get` and `animation.config.set` to the strict additive
control command parser. Implement validate-persist-publish-generation-return
transaction ordering in the compositor control server, with unchanged runtime
and generation on rejected or failed writes. Add the snapshot result to
`control_snapshots`, client decoding, and human CLI formatting. Add
`astreactl animation get` plus a bounded full-document set form consistent with
the existing command parser. Test codec bounds, command dispatch, requested /
effective / catalog fields, invalid generation behavior, and successful single
increments.

## 5. Add v3 minimize-anchor ownership

Extend `protocols/astrea-toplevel-management-v1.xml` to version 3 with the two
anchor requests, and update the compositor dispatch to require the existing
authenticated shell authority and protocol version. Add bounded positive
dimensions, negative-coordinate support, and resource/client ownership keyed by
`WindowId` in `AstreaToplevelPublisher`. Clear only matching owners on resource
teardown, clear on client/window teardown, and preserve all existing limits.
Add protocol contract and compositor tests for v2 compatibility, v3 access,
authorization, validation, replacement, stale teardown, current-owner teardown,
and window removal.

## 6. Verify and commit

Run targeted Rust tests after each subsystem, then the required closure checks:

```text
rtk run -- cargo fmt --check
rtk run -- cargo check --locked --all-targets
rtk run -- cargo clippy --locked --all-targets -- -D warnings
rtk run -- cargo test --locked
rtk git diff --check
rtk run -- bash bin/check-source-layout
```

Review the diff for unrelated files, commit the Typhon implementation as one
focused change, and report any pre-existing failures separately.
