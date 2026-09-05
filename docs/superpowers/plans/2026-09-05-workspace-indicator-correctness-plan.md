# Eclipse / Typhon Workspace Indicator Correctness Implementation Plan

> **For inline execution:** This plan is executed in the current session with test-first checkpoints. No sub-agents are used.

**Goal:** Publish only active or occupied regular workspaces from Typhon and vertically center Eclipse's production workspace strip without changing logical workspace ownership.

**Architecture:** Add `hidden` to the Typhon workspace protocol snapshot and derive it from active state plus canonical eligible mapped toplevel occupancy. Reuse the existing Astrea structure-dirty publication gate for occupancy refreshes, and add the semantic QML anchor plus production-geometry regression coverage in Eclipse.

**Tech Stack:** Rust, Smithay `ext-workspace-v1`, Qt/QML, Qt Test, CMake, Cargo.

## Global Constraints

- Preserve Typhon's ten logical regular workspaces and stable `typhon.workspace.N` handles.
- Use `CompositorState::astrea_toplevel_snapshot` and `WindowManagementState::regular_workspace` for occupancy authority.
- Keep hidden inactive empty handles allocated; never filter logical workspace numbers in QML.
- Do not use polling, subprocesses, a custom protocol, or the Eclipse `occupied` field as truth.
- Preserve zero-based coordinates, numeric ordering, activation, output enter/leave, and manager `done` semantics.
- Compile and test in the existing Typhon and Eclipse checkout folders.

---

### Task 1: Lock down protocol visibility semantics in Typhon

**Files:**
- Modify: `src/compositor/workspace_protocol_tests.rs`
- Modify: `src/compositor/workspace_protocol.rs`

**Interfaces:**
- `WorkspaceProtocolSnapshot::from_workspace_ids` accepts the regular occupied workspace set in addition to logical workspaces and the active workspace.
- `WorkspaceProtocolSnapshotItem.hidden` records whether an inactive logical workspace is shell-hidden.

- [ ] **Step 1: Add the four failing snapshot cases**

Extend the snapshot test to build active/occupied combinations and assert:

```rust
assert!(!active_empty.hidden);
assert!(!active_occupied.hidden);
assert!(!inactive_occupied.hidden);
assert!(inactive_empty.hidden);
```

Retain assertions for stable IDs, names, zero-based coordinates, numeric order, and exactly one active workspace.

- [ ] **Step 2: Run the focused Rust test and verify the expected failure**

Run:

```bash
cargo test --locked workspace_protocol
```

Expected: compilation/test failure because `hidden` and the occupied input do not yet exist.

- [ ] **Step 3: Implement the minimal snapshot model**

Add `hidden: bool` to `WorkspaceProtocolSnapshotItem`. Compute it as
`workspace != active && !occupied.contains(&workspace)`, preserving the input
iteration order and all existing ID/name/coordinate behavior.

- [ ] **Step 4: Run the focused test and verify it passes**

Run:

```bash
cargo test --locked workspace_protocol
```

Expected: all workspace protocol tests pass.

- [ ] **Step 5: Commit the protocol model change**

```bash
git add src/compositor/workspace_protocol.rs src/compositor/workspace_protocol_tests.rs
git commit -m "fix: model hidden workspace protocol state"
```

### Task 2: Reuse compositor toplevel eligibility for occupancy and publication

**Files:**
- Modify: `src/compositor/workspace_protocol.rs`
- Modify: `src/compositor/server_toplevel.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/toplevel_publication.rs` or `src/compositor/toplevel_publication_state.rs` only if the dirty gate needs a narrowly scoped helper
- Test: `src/compositor/state/desktop_window_tests.rs` or the existing compositor protocol integration fixture

**Interfaces:**
- `CompositorState` provides a private regular-workspace occupancy snapshot built from eligible Astrea toplevels and canonical regular workspace membership.
- Workspace manager binding and publication both consume the same computed snapshot semantics.
- Existing Astrea structure/removal dirtiness marks workspace presence dirty; the existing Astrea reconciliation boundary publishes it and clears the bit.

- [ ] **Step 1: Add failing coverage for occupancy authority and refresh transitions**

Use existing compositor fixtures to cover mapped XDG, unmapped XDG, eligible X11 toplevel/dialog, override-redirect/auxiliary X11, minimized applications, movement between regular workspaces, and special-workspace membership. Assert the derived occupied set and visibility transitions, including stable workspace identity.

- [ ] **Step 2: Run the focused compositor tests and verify the expected failure**

Run:

```bash
cargo test --locked workspace_protocol
cargo test --locked compositor::state::desktop_window_tests
```

Expected: the new occupancy assertions fail or the new API is unavailable.

- [ ] **Step 3: Implement canonical occupancy derivation**

Iterate the existing desktop-window collection, retain only entries for which
`astrea_toplevel_snapshot(window_id)` returns `Some`, then map each entry's
`management` through `regular_workspace()`. Collect a `BTreeSet<WorkspaceId>`.
Do not count special members, unmapped XDG windows, or auxiliary/override-
redirect X11 entries.

- [ ] **Step 4: Apply the same snapshot to initial binding and incremental state**

Have `CompositorState::bind_workspace_manager` compute one current snapshot.
Have `WorkspaceProtocolState::bind_manager` send each item's `Active` bit and
`Hidden` bit from that snapshot. Change `publish_state` to receive the current
snapshot and send the same state calculation before `manager.done`.

- [ ] **Step 5: Add and wire the narrow presence dirty state**

Set the dirty bit from `mark_astrea_toplevel_structure_dirty` and
`mark_astrea_toplevel_removed`. At the existing
`reconcile_astrea_toplevels` publication boundary, publish the current workspace
snapshot when that bit is set. Clear it in the existing immediate active
workspace publication path. Leave metadata-only `mark_astrea_toplevel_dirty`
updates free of workspace publication.

- [ ] **Step 6: Run focused tests and verify green**

Run:

```bash
cargo test --locked workspace_protocol
cargo test --locked compositor::state::desktop_window_tests
cargo test --locked compositor::tests::toplevel_management
```

Expected: all selected tests pass, including map/unmap/move/close visibility transitions.

- [ ] **Step 7: Commit Typhon occupancy/publication changes**

```bash
git add src/compositor/workspace_protocol.rs src/compositor/server_toplevel.rs src/compositor/mod.rs src/compositor/toplevel_publication.rs src/compositor/toplevel_publication_state.rs src/compositor/state/desktop_window_tests.rs
git commit -m "fix: publish occupied workspace visibility"
```

### Task 3: Add Eclipse regression coverage and semantic centering

**Files:**
- Modify: `/home/agony/GitHub/Eclipse/Bar/tests/BarQmlSmokeTest.cpp`
- Modify: `/home/agony/GitHub/Eclipse/shared/tests/TyphonWorkspaceStateTest.cpp`
- Modify: `/home/agony/GitHub/Eclipse/Bar/qml/LauncherSurface.qml`

**Interfaces:**
- The QML smoke test obtains production `launcherPill` and `workspaceStrip` `QQuickItem`s and compares mapped vertical centers.
- The shared-state test proves hidden records are filtered at `manager.done` and later reappear with the same ID/name.

- [ ] **Step 1: Add the production-QRC geometry test**

Instantiate `LauncherSurface.qml` with multiple `WorkspaceItem`s, show the real
window, obtain `launcherPill` and `workspaceStrip`, map the strip center into
the pill, and compare rounded Y centers. Do not inspect QML source text.

- [ ] **Step 2: Run the new Eclipse geometry test and verify the expected failure**

Run the focused Debug target after the existing build is configured:

```bash
cmake --build build/debug --target bar-qml-smoke-test -j2
QT_QPA_PLATFORM=offscreen ctest --test-dir build/debug -R '^bar-qml-smoke-test$' --output-on-failure
```

Expected: the new geometry assertion fails while the strip remains top-aligned.

- [ ] **Step 3: Add the hidden-state contract cases**

Extend `TyphonWorkspaceStateTest` with active state `1`, hidden state `4`,
stable hidden-handle reappearance with state `0`, and an explicit active
workspace visibility assertion.

- [ ] **Step 4: Implement semantic vertical centering**

Add `anchors.verticalCenter: parent.verticalCenter` to the production
`WorkspaceStrip` instance in `LauncherSurface.qml`. Do not change workspace
dimensions, spacing, or reserved width.

- [ ] **Step 5: Run Eclipse focused tests and verify green**

Run:

```bash
cmake --build build/debug --target typhon-workspace-state-test bar-core-test bar-qml-smoke-test astrea-shell astrea-shell_qmllint -j2
QT_QPA_PLATFORM=offscreen ctest --test-dir build/debug -R '^(typhon-workspace-state-test|bar-core-test|bar-qml-smoke-test|bar-qml-legacy-guard)$' --output-on-failure
```

- [ ] **Step 6: Commit Eclipse changes**

```bash
cd /home/agony/GitHub/Eclipse
git add Bar/qml/LauncherSurface.qml Bar/tests/BarQmlSmokeTest.cpp shared/tests/TyphonWorkspaceStateTest.cpp
git commit -m "fix: center and filter workspace indicators"
```

### Task 4: Full verification and final review

**Files:**
- No additional files; inspect the final diffs in both repositories.

- [ ] **Step 1: Run the required Typhon qualification matrix**

```bash
cd /home/agony/GitHub/Typhon
cargo fmt --check
cargo test --locked workspace_protocol
cargo test --locked --lib
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
git diff --check
```

- [ ] **Step 2: Run the required Eclipse build and focused regression matrix**

```bash
cd /home/agony/GitHub/Eclipse
cmake --build build/debug --target typhon-workspace-state-test bar-core-test bar-qml-smoke-test astrea-shell astrea-shell_qmllint -j2
QT_QPA_PLATFORM=offscreen ctest --test-dir build/debug -R '^(typhon-workspace-state-test|bar-core-test|bar-qml-smoke-test|bar-qml-legacy-guard)$' --output-on-failure
git diff --check
```

- [ ] **Step 3: Inspect both final diffs for forbidden shortcuts**

Confirm there is no QML workspace-number filtering, hardcoded occupancy truth,
polling/subprocess fallback, logical-count change, handle recreation, or
pixel-offset alignment. Confirm existing unrelated changes are absent.

- [ ] **Step 4: Commit any formatting-only corrections and report exact evidence**

If verification requires a correction, rerun the affected command, commit the
correction with a focused message, and report any unrelated baseline failures
with their command and output.
