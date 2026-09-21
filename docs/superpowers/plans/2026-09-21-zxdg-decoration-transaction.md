# zxdg-decoration Transaction Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Typhon's xdg-decoration mode changes transactional with the existing xdg-shell configure/ack/commit lifecycle so native SSD changes are atomic with client content.

**Architecture:** Store requested decoration preference separately from applied mode. Attach the effective configured mode to the exact `XdgConfigureRecord` serial emitted by the existing toplevel configure sender, retain the latest acknowledged record, and apply it at surface-tree commit publication. Render and hit-test only from applied mode.

**Tech Stack:** Rust, Smithay/Wayland server and client protocol bindings, Cargo tests, existing compositor integration-test harness.

## Global Constraints

- Keep the advertised `zxdg_decoration_manager_v1` version at 1.
- Do not add application-specific behavior, renderer ghost suppression, duplicate scene nodes, timing delays, or a second configure queue.
- Preserve XWayland decoration behavior and existing interaction ownership.
- All build/test output must be under `/mnt/Aether/Desktop/GitHub`; set and verify `CARGO_TARGET_DIR` before every Cargo build/test command.
- Use RTK for Cargo/test output where applicable.
- Do not overwrite the existing unrelated working-tree changes.

### Task 1: Add the failing protocol regression harness

**Files:**
- Modify: `src/compositor/tests/support/registry_state.rs`
- Create: `src/compositor/tests/xdg_decoration.rs`
- Modify: `src/compositor/tests/mod.rs`

**Interfaces:**
- `RegistryTestState` exposes decoration configure modes/counts, xdg surface serials, event ordering, and opt-out flags for automatic xdg ack/buffer commit.
- The new tests use real `zxdg_toplevel_decoration_v1`, `xdg_surface`, and `wl_surface` client objects.

- [ ] **Step 1: Extend the test state with observable decoration events.**
  Add fields for decoration configure count/modes and keep the existing xdg
  surface serial vector and event log. In the decoration dispatch handler,
  record `ClientSide`/`ServerSide` values and append a decoration event marker.
  Add flags that let focused tests suppress automatic ack and initial-buffer
  commit while keeping the default behavior unchanged for current tests.

- [ ] **Step 2: Add the first failing dynamic ClientSide -> ServerSide test.**
  Map a toplevel with an initially applied client-side mode, issue
  `set_mode(ServerSide)`, assert that a decoration and xdg configure arrive
  but the server still reports no SSD, then ack the serial and commit a new
  buffer and assert one SSD instance and one visible-generation change.

- [ ] **Step 3: Run only the new test to verify the failure is the current bug.**
  Run:
  `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo test --lib compositor::tests::xdg_decoration::dynamic_client_to_server_waits_for_commit -- --exact --nocapture`
  Expected: the pre-fix implementation fails because the SSD appears or the
  visible generation advances before the acknowledged commit.

- [ ] **Step 4: Commit the red test and harness only.**
  Run `git add` for the three test files and commit:
  `test: expose xdg decoration configure transactions`

### Task 2: Make xdg configure records own decoration payloads

**Files:**
- Modify: `src/compositor/state/xdg_lifecycle.rs`
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/resize.rs`

**Interfaces:**
- `XdgConfigureRecord` carries `Option<DecorationMode>` for the exact serial.
- `XdgSurfaceLifecycle` retains and consumes the latest acknowledged record.
- The existing configure sender remains the only path that emits paired
  toplevel/xdg-surface configure events.

- [ ] **Step 1: Add a typed decoration payload to configure records.**
  Keep the existing `record_configure(serial)` API for popup/non-decoration
  callers and add a decoration-aware variant used by toplevel configure
  emission. Add `last_acked_configure: Option<XdgConfigureRecord>` and make
  acknowledgement save the selected record after retiring older records.

- [ ] **Step 2: Add the central paired toplevel configure helper.**
  Refactor `send_configure_root_window_to` and the initial configure path to
  use one helper that, in order, computes the requested effective decoration
  mode, sends decoration configure when a resource exists, sends the toplevel
  configure, sends `xdg_surface.configure`, and records the mode under the
  serial. Preserve popup configure behavior with a decoration-free record.

- [ ] **Step 3: Apply only the latest acknowledged payload at surface commit.**
  Add a compositor-state method that takes the lifecycle's acknowledged record
  during the actual cached surface-tree commit. Leave records untouched until
  that point so a request or ack without commit cannot change the visible mode.

- [ ] **Step 4: Run xdg lifecycle and configure tests.**
  Run the focused lifecycle and xdg tests with the Aether target directory and
  fix only compile/test failures caused by the record API.

- [ ] **Step 5: Commit the transaction identity change.**
  Commit the lifecycle and configure sender changes with:
  `fix: attach xdg decoration state to configure serials`

### Task 3: Separate preference, applied mode, and decoration-object lifetime

**Files:**
- Modify: `src/compositor/state/window_decoration.rs`
- Modify: `src/compositor/protocols/xdg.rs`
- Modify: `src/compositor/state/subsurfaces.rs`
- Modify: `src/compositor/state/windows.rs`

**Interfaces:**
- `WindowDecorationState` exposes requested preference, applied mode, and
  object-presence transitions without making preference the render authority.
- `CompositorState` applies an acknowledged decoration mode and reconciles
  decoration-owned input captures on an applied transition.

- [ ] **Step 1: Add a failing unit test for applied-only rendering.**
  Update the decoration state fixture so it can set applied mode explicitly,
  then add a test proving a changed preference alone leaves render instances,
  hit testing, and scene generation at the old applied state.

- [ ] **Step 2: Implement typed state transitions.**
  Add `applied_mode` and explicit object-presence state. Make preference
  changes return whether the request changed. Make renderer, hit testing, and
  `surface_uses_server_side_decorations` read applied mode for native Wayland
  surfaces; leave X11 policy calculation independent.

- [ ] **Step 3: Apply acknowledged mode atomically and invalidate once.**
  On commit, compare old and new applied modes, clear stale decoration button
  captures/hover/titlebar interaction when leaving SSD, and advance render
  generation only when the applied mode changed and the commit did not already
  publish a visual generation.

- [ ] **Step 4: Fix protocol request and destroy behavior.**
  Remove immediate render-generation changes from `set_mode`/`unset_mode`.
  Suppress identical preference requests. Do not send a standalone decoration
  configure from object creation; route initial and dynamic changes through the
  paired xdg configure helper. On destroy, remove the resource but retain the
  current state until the next commit queues/applies client-side mode. Enforce
  the advertised v1 rule for creating an object after committed content.

- [ ] **Step 5: Preserve recreation-before-commit behavior for v1.**
  If a decoration object is destroyed before a permitted initial commit,
  cancel the uncommitted destroy transition when a replacement object is
  created. Do not add any v2 behavior or version advertisement.

- [ ] **Step 6: Run the new dynamic test and the decoration unit tests.**
  Verify the test is green and that the server-side frame appears/disappears
  only after the relevant ack plus commit.

- [ ] **Step 7: Commit the state ownership fix.**
  Commit with:
  `fix: make xdg decoration state commit-atomic`

### Task 4: Complete regression coverage

**Files:**
- Modify: `src/compositor/tests/xdg_decoration.rs`
- Modify: `src/compositor/tests/support/registry_state.rs`
- Modify: `src/compositor/state/window_decoration_tests.rs`
- Modify: `src/compositor/state/xdg_lifecycle.rs`

- [ ] **Step 1: Add ClientSide -> ServerSide, ServerSide -> ClientSide, and unset tests.**
  Assert event order, no pre-commit visual change, atomic post-commit change,
  and one visible generation per applied transition.

- [ ] **Step 2: Add repeated-preference and multiple-outstanding tests.**
  Prove three identical `set_mode(ServerSide)` requests produce one relevant
  configure, while two different outstanding transactions apply only the mode
  attached to the newest acknowledged serial before commit.

- [ ] **Step 3: Add destroy, initial-map, and close-ownership tests.**
  Verify destroy retains the old frame until commit, initial mapping creates
  one desktop window and one coherent frame, and a close-button hit resolves
  to the exact root `WindowId`.

- [ ] **Step 4: Add fullscreen wire/visual tests.**
  Assert internal `None` suppresses native chrome only after the fullscreen
  transaction commits and the wire decoration configure is `ServerSide`; assert
  leaving fullscreen restores the configured/applied normal mode transactionally.

- [ ] **Step 5: Run focused compositor coverage.**
  Run the xdg-decoration module, existing `window_decoration` unit tests,
  xdg tests, input/output interaction tests, and XWayland decoration tests.

- [ ] **Step 6: Commit regression coverage.**
  Commit with:
  `test: cover transactional xdg decoration state`

### Task 5: Full verification and live validation

**Files:**
- No production files; inspect the final diff and test artifacts only.

- [ ] **Step 1: Run formatting and build checks with verified output paths.**
  Before each command, print `realpath` for
  `/mnt/Aether/Desktop/GitHub/Typhon-target` and confirm it is not under the
  source checkout. Run `rtk cargo fmt --check`, `rtk cargo check --all-targets`,
  and `rtk cargo clippy --all-targets -- -D warnings` with
  `CARGO_TARGET_DIR` set to that path.

- [ ] **Step 2: Run the complete test suite.**
  Run `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo test`
  and record all failures, distinguishing pre-existing failures from regressions.

- [ ] **Step 3: Inspect source-layout gates and final diff.**
  Run the repository's source-layout check if present, `git diff --check`, and
  review `git status` to ensure unrelated pre-existing edits were not reverted.

- [ ] **Step 4: Perform live native-Wayland validation if the environment permits.**
  Under the running Typhon session, launch Zen Browser, OBS Studio, and one
  simple native Wayland client. Confirm one visual window per logical toplevel,
  no ghost SSD, working move/resize/close/maximize/fullscreen, no configure
  loop/protocol error, and no surface-tree duplication. Capture protocol trace
  ordering when a client exercises decoration transitions.

- [ ] **Step 5: Commit the final verified logical change.**
  If prior task commits exist, create a final verification commit only for any
  necessary corrections. Otherwise commit the complete implementation with a
  focused message and report every verification result honestly.
