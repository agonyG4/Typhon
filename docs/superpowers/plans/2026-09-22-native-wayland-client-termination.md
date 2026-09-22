# Native Wayland client termination fix plan

## Scope

Implement and verify the design in
`docs/superpowers/specs/2026-09-22-native-wayland-client-termination-design.md`.
Do not modify application-specific behavior or create a second xdg
transaction system.

## 1. Establish the exact audit baseline

1. Re-check the Codebase Memory project/generation and coverage for every
   source path touched by the audit.
2. Trace fatal error emission and client-disconnect paths for xdg-shell,
   xdg-decoration, wl_surface/subsurface, dmabuf, explicit synchronization,
   and input serial validation.
3. Record whether a live native-Wayland reproducer is available and preserve
   the baseline focused test results.

Acceptance: the implementation notes identify all reachable fatal paths and
distinguish a reproduced sequence from a source-backed potential violation.

## 2. Add failing decoration regressions first

Files: `src/compositor/tests/xdg_decoration.rs` and, if needed, the test
support client helpers.

1. Parameterize the decoration test client by manager bind version.
2. Add a v1 mapped-content creation test that expects the protocol error.
3. Add a v2 mapped-content creation test that succeeds.
4. Add v2 destroy/recreate-before-commit and destroy/commit/recreate tests,
   asserting the wire/applied mode rather than merely object existence.
5. Add mixed configure ownership assertions and fullscreen/repeated-preference
   coverage where the existing helper does not already cover them.

Run only the decoration tests with a verified Aether target and confirm the
new tests fail for the current implementation before production edits.

## 3. Implement xdg-decoration v2 lifecycle

Files: `src/compositor/protocols/versions.rs`,
`src/compositor/state/window_decoration.rs`,
`src/compositor/state/xdg_lifecycle.rs`,
`src/compositor/state/surface_transactions.rs`, and affected xdg/window code.

1. Advertise manager version 2; do not change `wayland-protocols` because the
   0.32.13 XML already supports v2.
2. Preserve v1 `unconfigured_buffer` rejection based on the manager resource
   version.
3. Add explicit pending-destroy/commit state to the existing decoration state.
4. Make destruction retain the preference and defer visible/applied state.
5. Make recreation before a commit retain the preference.
6. Finalize destruction through the acknowledged configure plus surface commit;
   after a commit while absent, recreate with client-side semantics.
7. Preserve repeated-preference suppression and the existing decoration mode
   stored on configure records.
8. Clear stale acknowledged decoration state on unmap/remap if required by the
   lifecycle ledger.

Run decoration tests again; all old and new tests must pass.

## 4. Add failing popup serial regressions

Files: `src/compositor/tests/xdg.rs`, popup/input test support, and the
validation helper if extraction improves unit coverage.

1. Add pointer, keyboard, and touch grab acceptance tests.
2. Add pointer-enter, unknown, stale-focus, wrong-client, and wrong-root
   rejection tests.
3. Add the mapped-toplevel → keyboard press → popup creation → keyboard grab →
   popup configure/ack/commit sequence.
4. Preserve and extend nested, reposition, parent teardown, and rapid-cycle
   assertions where needed.

Run the popup-focused tests and confirm the keyboard/touch acceptance tests
fail before changing the validator.

## 5. Implement narrow popup validation correction

Files: `src/compositor/state/surfaces.rs` and relevant tests.

1. Separate the popup-grab accepted-kind predicate from drag validation.
2. Accept exactly button press, keyboard key press, and touch down.
3. Leave client, root, seat, serial, and focus-generation checks untouched.
4. Keep invalid-grab fatal behavior and popup lifetime/topmost checks strict.

Run popup and protocol-error tests.

## 6. Audit and harden configure serial ownership

Files: `src/compositor/state/xdg_lifecycle.rs`,
`src/compositor/state/windows.rs`, resize/activation paths, and tests.

1. Enumerate every `xdg_surface.configure` emission.
2. Route every emission through one send-and-record invariant or add a
   narrowly scoped assertion that proves the immediate ledger record exists.
3. Cover initial, decoration, resize, maximize, fullscreen, activation,
   popup initial/reposition, and remap sequences.
4. Keep unknown and consumed acknowledgements fatal.

Run the mixed configure tests and the full existing xdg suite.

## 7. Improve fatal protocol diagnostics

Files: `src/compositor/protocol_error_trace.rs`,
`src/compositor/state/client_lifecycle.rs`, and protocol-error tests.

1. Record the human-readable reason in `ProtocolErrorRecord`.
2. Emit a debug-only fatal line containing client id, peer pid when available,
   precise interface, resource id, protocol error code, surface id, and reason.
3. Preserve the centralized raw `post_error` path and negligible normal-path
   overhead.
4. Add regression assertions for the trace record and debug-format data.

## 8. Verify broader protocol paths and live behavior

1. Re-run the Codebase Memory coverage checks and source inspection for core
   surface/subsurface, dmabuf, explicit sync, xdg-shell, decoration, and input
   error paths.
2. Run `cargo fmt --check`.
3. Run `cargo check --all-targets`.
4. Run `cargo clippy --all-targets -- -D warnings`.
5. Run `cargo test`.
6. Run the repository source-layout gate.
7. If a live native-Wayland environment is available, exercise Firefox/Zen,
   OBS, and a simple native client across menus, autocomplete, popup nesting,
   resize, maximize, fullscreen, new windows, and rapid dismissal while
   watching fatal protocol diagnostics and window/commit traces.

Every Rust command must print and verify a target directory under
`/mnt/Aether/Desktop/GitHub` before it runs.

## 9. Handoff

Review `git diff --check`, inspect the final status, commit the implementation
as a logical change, and report exact test/live results plus remaining
compatibility risks. Do not claim a reproduced fatal sequence unless the live
trace actually contains one.
