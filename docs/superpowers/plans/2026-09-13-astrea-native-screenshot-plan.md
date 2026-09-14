# AstreaOS native screenshot implementation plan

## Working rules

- Preserve the pre-existing dirty paths, especially compositor subsurface and
  native pacing edits. Do not reset, clean, broad-checkout, mass-format, or
  commit.
- Use the existing `target/` directory and run commands through `rtk`.
- Follow TDD: add a deterministic failing test, implement the smallest seam,
  run the focused test, then continue.

## Typhon tasks

1. Add and test `protocols/astrea-screen-capture-v1.xml`, generated Rust
   bindings, server global registration, exact ClientId authorization, output
   identity validation, one-request-per-client state, and lifecycle cleanup.
2. Add bounded RGBA8888 normalization and sealed memfd helpers. Test checked
   arithmetic, exact bytes/size, CLOEXEC, all four seals, and failure mapping.
3. Add `KEY_SYSRQ` and the reserved, bypassed, non-repeating
   `astrea-shell/screenshot_capture` default binding. Test one pressed action
   per physical press and no repeat/release action.
4. Extend native work domains with a first-class capture bit and operation
   plan. Test capture => acquire/prepare, capture !=> presentation, and
   inactive-session short-circuiting.
5. Add one shared origin-aware offscreen primitive to `GlesSceneRenderer`.
   Test the request contract (full damage, age zero, cursor excluded), both
   origin row paths, alpha normalization, presentation-history neutrality,
   effect/frame timing restoration, and non-poisoning capture failures.
6. Add only the two GLES backend adapters. They make their normal EGL context
   current, build the ordinary request, invoke the primitive, and return a
   capture result. Atomic adapters must not acquire slots, touch Direct
   Scanout, KMS, pageflips, swapchain ownership, or TEST_ONLY. CPU adapters
   return unsupported. Test direct-scanout and import-failure neutrality.
7. Insert capture service after `process_acquire_and_prepare` and before the
   independent presentation reclassification. Add tests proving a request
   received during Wayland dispatch is serviced in that cycle and only after
   legal publication.
8. Run focused input/runtime/renderer/protocol tests and inspect the diff for
   capture/presentation separation before moving to Eclipse.

## Eclipse tasks

1. Copy the exact capture XML and add generated client/server build rules plus
   a drift test against Typhon's XML.
2. Extend `TyphonShortcutClient` registration with screenshot capture and add
   `TyphonScreenCaptureClient` using the shared connection, shared display,
   output binding, generation tagging, metadata validation, mmap/deep-copy,
   close/unmap, and reconnect/rebind behavior. Add focused tests.
3. Move all cross-feature mapping into `ShellShortcutDispatcher` with the
   requested shell action enum; reduce Alt+Tab mapping to Alt+Tab actions and
   test independent feature gates including screenshot routing.
4. Add `Shell/screenshot/` controller and image provider. Test ready/failure,
   hidden-before-ready, generation invalidation, crop mapping/clamping,
   tiny-selection full-image fallback, cancellation, deterministic collision
   naming, QSaveFile PNG, and clipboard image publication.
5. Add the layer-shell QML overlay and wire it through `ShellRuntime` and
   `AstreaShellApplication`. Test the unified shortcut/request/ready/overlay
   flow plus Escape and right-click cancellation.
6. Run focused Eclipse tests, then the existing configure/build/CTest workflow.

## Final verification

Run fresh Typhon format, locked all-target check, locked clippy, locked tests,
source-layout gate, focused capture/input/runtime/renderer tests, and diff
checks. Run Eclipse configure/build/CTest and focused protocol/controller/
shortcut/QML tests. Report exact pass/fail counts and distinguish unavailable
native GPU/KMS execution from deterministic test results. Audit all requested
invariants before reporting completion.
