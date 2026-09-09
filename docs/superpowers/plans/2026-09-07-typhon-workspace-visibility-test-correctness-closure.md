# Typhon Workspace Visibility Test Correctness Closure Implementation Plan

> **For agentic workers:** Execute this plan inline in the current workspace; the user explicitly requested no sub-agents.

**Goal:** Correct the workspace wire-test identity assertion and add regression coverage for an atomic regular-workspace 2 → 3 occupancy migration.

**Architecture:** Preserve the accepted production pipeline and touch only the compositor workspace wire tests. The existing same-client `ObjectId` checks remain the protocol-resource stability invariant; the second client is checked only for semantic workspace metadata and state.

**Tech Stack:** Rust, Cargo, Wayland client test bindings, the existing controllable compositor test server, and `rtk` for repository commands.

## Global Constraints

- Preserve the current production workspace visibility architecture and accepted XDG/X11 eligibility semantics.
- Do not add production APIs solely for this test.
- Build and test in `/home/agony/GitHub/Typhon` so Cargo reuses the existing local target directory.
- Keep the existing accepted workspace, XDG, X11, special-workspace, dirty-state, and handle-stability coverage.
- Run the complete verification matrix from the approved specification before reporting completion.

### Task 1: Replace the invalid cross-client identity assertion

**Files:**
- Modify: `src/compositor/tests/workspace.rs`
- Modify: `src/compositor/tests/support/server_runtime.rs`

- [x] **Step 1: Write the semantic second-client assertions**

Keep the original client’s saved `ObjectId` and all existing same-client stability checks. Replace the second client’s cross-client `ObjectId` equality with assertions that its initial snapshot contains ten logical workspaces and that `typhon.workspace.3` has logical metadata `id = typhon.workspace.3`, `name = 3`, `coordinates = [2]`, and a non-Hidden visible state; also assert workspace 1 is Active.

- [x] **Step 2: Run the focused test**

Run `cargo test --locked wire_visibility_is_atomic_and_independent_of_astrea_manager` and record the result before making the migration addition.

### Task 2: Add the inactive regular-workspace migration regression

**Files:**
- Modify: `src/compositor/tests/workspace.rs`

- [x] **Step 1: Add the wire-level scenario**

Use one mapped `LiveTestClient` surface and the existing controllable-server workspace movement infrastructure. Keep workspace 1 active, move the only eligible application to workspace 2, verify workspace 2 is visible and workspace 3 is Hidden, save both original same-client handle `ObjectId`s, then use a test-only window-targeted movement command to move that same application to workspace 3 after focus has been cleared by leaving the active scene.

- [x] **Step 2: Assert atomic completion semantics**

After the second move, assert exactly one additional `manager.done` snapshot, workspace 1 is Active, workspace 2 is Hidden, workspace 3 is visible, both saved handle `ObjectId`s are unchanged, and `removed_count` remains zero.

- [x] **Step 3: Run the focused workspace tests**

Run `cargo test --locked compositor::tests::workspace` and inspect failures for test setup or protocol-event ordering issues.

### Task 3: Run the complete accepted verification matrix

**Files:**
- Verify: repository working tree and the modified test file

- [x] **Step 1: Run formatting, compilation, lint, focused tests, full library tests, all targets, and diff checks**

Run each command in the repository root:

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked workspace_protocol
cargo test --locked wire_visibility_is_atomic_and_independent_of_astrea_manager
cargo test --locked compositor::tests::workspace
cargo test --locked compositor::tests::xwayland
cargo test --locked --lib
cargo test --locked --all-targets
git diff --check
```

- [x] **Step 2: Confirm scope and report results**

Verify the diff contains only the approved test-correctness changes and this plan record, with no production workspace redesign. Report the replaced assertion, semantic second-client checks, migration test, preserved same-client stability checks, each verification result, and any remaining real-session qualification requirement.
