# Wayland Core + XDG Advertised-Contract Compliance Implementation Plan

**Goal:** Make Typhon's advertised Wayland Core and stable xdg-shell surface truthful, version-gated, protocol-error-correct, and ownership-safe without changing native KMS or adaptive-buffering behavior.

**Architecture:** Keep protocol dispatch thin and move shared behavior into version constants, a real wire-error test harness, permanent/live role lifecycle, one surface commit transaction, XDG lifecycle/configure ledger, request-time SHM validators, typed input serials, focused data-device state, and output membership. Renderer/KMS consume only validated published state.

**Tech Stack:** Rust 2024, wayland-backend 0.3.15, wayland-server 0.31.13, wayland-client 0.31.14, wayland-scanner 0.31.10, wayland-protocols 0.32.12, existing compositor test runtime, no new framework dependency.

## Global Constraints

- Preserve the complete existing tree and the native Atomic EGL/GBM, explicit-fence, framebuffer-origin, frame-owned release, callback ownership, and adaptive-buffering paths.
- Do not commit, reset, clean, restore, rebase destructively, lower advertised versions, or implement Direct Scanout, VRR, tearing, XWayland, multi-output, or unrelated cleanup.
- Use the exact locked XML/generated sources and record ambiguity decisions in `docs/wayland/PROTOCOL_SOURCE_MANIFEST.md`.
- Every supported request is classified, tested, and delegated; supported wildcard arms cannot silently absorb requests.
- Work is sequential by checkpoint: baseline/docs, wire errors and gates, roles, surface transactions, XDG, subsurfaces, SHM, input serials, data-device/DnD, output lifecycle, adversarial/native verification.

## Task List

### Task 1: Baseline and normative inventory

**Files:**
- Create: `docs/wayland/PROTOCOL_SOURCE_MANIFEST.md`
- Create: `docs/wayland/CORE_COMPLIANCE_MATRIX.md`
- Create: `src/compositor/protocols/versions.rs`
- Modify: `src/compositor/server.rs`, `src/compositor/protocols.rs`
- Test: existing compositor protocol tests plus a manifest/registration test

**Acceptance criteria:** actual branch/HEAD and dirty-tree state are recorded; all registered globals consume constants; locked crate checksums and exact XML paths are recorded; every covered request/event has a matrix row and a classification.

**Verification:** run the mandated `rg` audits, `cargo fmt --check`, `cargo check --all-targets`, and the new version/manifest tests.

### Task 2: Wire-level protocol-error harness and version gates

**Files:**
- Modify: `src/compositor/tests/support/server_runtime.rs`, `src/compositor/tests/support/registry_state.rs`, `src/compositor/tests/support/mod.rs`
- Create: focused protocol compliance test module under `src/compositor/tests/`

**Interfaces:** helpers `expect_protocol_error`, `expect_request_ignored_without_disconnect`, `expect_roundtrip_alive`, `expect_object_destroy_order_error`, and `expect_client_state_scrubbed` start the existing real server and use locked wayland-rs client APIs.

**Acceptance criteria:** malformed client A receives the exact interface/error code and disconnects; independent client B remains usable; bootstrap cases cover invalid scale, invalid sibling, and invalid SHM pool size; event/request version gates are exercised for every advertised version.

**Verification:** focused `protocol_error` and version-gate tests, then all existing tests single-threaded.

### Task 3: Permanent roles and live instances

**Files:**
- Create: `src/compositor/state/role_lifecycle.rs`
- Modify: `src/compositor/state/roles.rs`, `src/compositor/state/mod.rs`, `src/compositor/state/client_lifecycle.rs`, `src/compositor/protocols/core.rs`, `src/compositor/protocols/xdg.rs`, `src/compositor/protocols/layer_shell.rs`, `src/compositor/protocols/input.rs`, `src/compositor/protocols/data_device.rs`
- Test: role lifecycle and client lifecycle tests

**Interfaces:** `reserve_role`, `activate_same_role_instance`, `deactivate_role_instance`, `reserve_xdg_association`, `construct_xdg_role`, `destroy_xdg_association`, `validate_surface_destroy`, and `scrub_surface_lifecycle` become the sole role authority.

**Acceptance criteria:** role identity survives live-object destruction; only `wl_surface` destruction removes the permanent record; all providers reject switching and enforce version-applicable destroy ordering.

**Verification:** role-switch, same-role recreation, destroy-order, cursor/drag-icon, failed-creation, and disconnect scrub tests.

### Task 4: Atomic `wl_surface` transaction

**Files:**
- Create: `src/compositor/state/surface_state.rs`
- Modify: `src/compositor/surface.rs`, `src/compositor/state/surface_commits.rs`, `src/compositor/state/surfaces.rs`, `src/compositor/protocols/core.rs`, `src/compositor/protocols/viewport.rs`, renderer-facing validated snapshot consumers
- Test: `core_surface_compliance` and surface frame tests

**Acceptance criteria:** one commit extracts one coherent pending snapshot; request-time copies and commit-time transform/scale/viewport/damage validation are atomic; exact invalid scale/transform/size/offset errors are posted; regions, callbacks, feedback, releases, and explicit sync share commit ownership; preferred events are version-gated; native output orientation is unchanged.

**Verification:** red/green focused surface tests, frame ownership tests, native-path unit tests, and diff/layout audits.

### Task 5: Strict xdg-shell lifecycle and configure ledger

**Files:**
- Create: `src/compositor/state/xdg_lifecycle.rs`
- Modify: `src/compositor/protocols/xdg.rs`, `src/compositor/state/windows.rs`, `src/compositor/state/resize.rs`, `src/compositor/state/surface_commits.rs`, `src/compositor/popup.rs`, test support helpers
- Test: `xdg_compliance` and corrected XDG integration tests

**Acceptance criteria:** association/construction/role/map states are explicit; initial empty commit, configure, ack, and buffer mapping ordering is enforced; configures are an ordered ledger; invalid/duplicate/future acks, invalid geometry/constraints/edges/parents/positioners/grabs, topmost popup destruction, and wm-base destruction use exact errors; version-gated events are truthful.

**Verification:** all listed positive/negative XDG cases, existing valid toolkit helper flows, and full suite single-threaded.

### Task 6: Subsurface transaction integration

**Files:**
- Modify: `src/compositor/state/subsurfaces.rs`, `src/compositor/subsurface.rs`, `src/compositor/state/surface_commits.rs`, role lifecycle and output membership modules
- Test: `subsurface_compliance`, existing subsurface and frame tests

**Acceptance criteria:** effective synchronization is inherited; cached subtree commits publish atomically; position/restack/state/callback/feedback/release/sync ownership follows the same transaction; invalid restack and cycles leave the old tree unchanged; role destruction unmaps but does not clear permanent identity.

**Verification:** focused subsurface tests and native/frame ownership regression tests.

### Task 7: Request-time SHM validation

**Files:**
- Create or extend: `src/compositor/state/shm_validation.rs`
- Modify: `src/compositor/protocols/buffers.rs`, `src/compositor/shm.rs`, shared pool/buffer state
- Test: `shm_compliance` and buffer protocol tests

**Acceptance criteria:** pool FD, size, format, dimensions, offset, stride, overflow, final-row bound, and advertised-format checks happen before rendering with exact locked XML errors; pool growth is visible as required; valid padded and exact-end buffers remain accepted.

**Verification:** all listed SHM cases, renderer boundary assertions, and full suite.

### Task 8: Typed seat serials and input contract

**Files:**
- Create: `src/compositor/state/input_serials.rs`
- Modify: `src/compositor/state/input_dispatch.rs`, `src/compositor/state/input_resources.rs`, `src/compositor/state/surfaces.rs`, `src/compositor/state/resize.rs`, `src/compositor/protocols/input.rs`, activation/data-device consumers
- Test: `input_serial` and input/output tests

**Acceptance criteria:** serials carry typed provenance and purpose-specific validators; timestamps are monotonic wrap-safe u32; keyboard enter contains held keys/modifiers; keymap FD/payload/repeat/name/capability generations are correct; pointer v7 grouping and axis metadata are preserved where available.

**Verification:** focused serial/input tests plus move/resize, relative pointer, constraints, cursor, and Astrea regressions.

### Task 9: Core data-device and DnD state machine

**Files:**
- Create or extract: `src/compositor/state/data_device.rs`
- Modify: `src/compositor/protocols/data_device.rs`, clipboard/selection bridge, hit testing/input routing, role lifecycle, client teardown
- Test: `data_device`, clipboard, and client lifecycle tests

**Acceptance criteria:** v3 source/offer lifecycle, one-time use, action masks, implicit-grab start, permanent drag-icon role, target events, MIME/action negotiation, receive/drop/finish ordering, cancellation, destruction, disconnect, and selection focus behavior are explicit and exact.

**Verification:** each six-step DnD slice is red/green before the next; clipboard copy/paste stays green; client isolation and leak counters remain zero.

### Task 10: Output membership and cross-protocol teardown

**Files:**
- Create: `src/compositor/state/output_membership.rs`
- Modify: `src/compositor/output.rs`, `src/compositor/protocols/input.rs`, surface mapping/teardown and preferred-event consumers
- Test: output membership, lifecycle, and version-gate tests

**Acceptance criteria:** enter/leave is emitted once per deterministic membership transition; unmap/remap and subsurface-tree membership are correct; output and preferred events never exceed bound versions; resource/client teardown scrubs membership.

**Verification:** focused output/lifecycle tests and full protocol matrix coverage.

### Task 11: Adversarial and final verification

**Files:**
- Create or extend: model/property-style state tests under `src/compositor/tests/`
- Modify: diagnostics/metrics and final compliance docs only as evidence requires

**Acceptance criteria:** bounded randomized sequences cannot publish illegal state or leak client-owned objects; supported-request-unhandled and client-state-leak counters are zero; protected native counters remain zero; final report distinguishes automated, native, and unrun validation.

**Verification commands:**

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test -- --test-threads=1
./bin/check-source-layout
git diff --check
cargo build --release
```

## Checkpoints

After Tasks 1-2: version constants, source manifest, matrix, real protocol errors, client isolation, and event gates are green.

After Tasks 3-5: role, surface, and XDG state machines are green without touching native output ownership.

After Tasks 6-9: subsurface, SHM, input serial, clipboard, and DnD tests are green.

After Tasks 10-11: complete suite and audits are green; native validation is run only on the actual available NVIDIA TTY session and reported honestly.

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Actual HEAD differs from supplied baseline | High | Preserve actual tree, record divergence, re-audit before changing behavior. |
| Generated protocol enums differ from assumed names | High | Read locked generated sources/XML before each error implementation and record exact ownership. |
| Unified surface state touches frame ownership | High | Add failing ownership tests first and stop before changing native release/pacing paths. |
| Existing helpers encode permissive invalid XDG order | Medium | Correct helpers only when the XML-backed test proves the old expectation invalid. |
| DnD policy is underspecified | Medium | Implement protocol state and choose/document deterministic compositor policy; stop if shell policy becomes necessary. |

## Scope Stop Conditions

Stop and report evidence if a required change would alter Atomic KMS, adaptive buffering, frame-owned release, explicit-sync ownership, or accepted native pacing; if locked generated code materially contradicts the requested contract; or if valid DnD/XDG behavior requires an unapproved shell policy.

## Execution Note

The user explicitly requested no commit and no sub-agents. The plan is therefore saved as an uncommitted review artifact and will be executed inline with test-first red/green cycles.
