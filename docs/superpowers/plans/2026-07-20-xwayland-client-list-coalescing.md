# Xwayland Client-List Coalescing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent short-lived Steam popup windows from terminating managed Xwayland when stale intermediate EWMH client-list snapshots are superseded in one reactor batch.

**Architecture:** Add one pure batch-normalization helper beside native Xwayland dispatch. It preserves every non-list command in order, retains only the final `SyncClientLists` snapshot, and dispatches that normalized batch through the existing strict XWM command path.

**Tech Stack:** Rust, x11rb/XWM command model, Cargo unit and integration tests.

## Global Constraints

- Keep strict generation and registry validation for all executable window operations.
- Do not weaken `XwaylandService` failure handling for genuine XWM errors.
- Treat only `XwmCommand::SyncClientLists` as a supersedable declarative snapshot.
- Preserve the original relative order of all non-list commands.

---

### Task 1: Coalesce EWMH client-list snapshots per reactor batch

**Files:**
- Modify: `src/native_output/runtime/xwayland.rs`
- Test: `src/native_output/runtime/xwayland.rs`

**Interfaces:**
- Consumes: `Vec<oblivion_one::xwayland::xwm::XwmCommand>` collected by `NativeRuntime::dispatch_xwayland_window_events`.
- Produces: `fn coalesce_client_list_sync(commands: Vec<XwmCommand>) -> Vec<XwmCommand>` and a normalized command vector containing at most one final `SyncClientLists`.

- [ ] **Step 1: Write failing same-batch popup regression tests**

Add a test module that constructs generation-bound parent and popup handles. The first test supplies a stale `SyncClientLists { parent, popup }`, an unrelated `Focus { parent }`, and a final `SyncClientLists { parent }`; it asserts the result is exactly `Focus { parent }` followed by the final parent-only snapshot. The second test supplies only non-list commands and asserts the vector is unchanged.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test --locked --bin oblivion-one coalesce_client_list_sync
```

Expected: compilation fails because `coalesce_client_list_sync` does not exist.

- [ ] **Step 3: Implement minimal batch coalescing**

Implement the helper by iterating once, storing the latest `SyncClientLists` command separately, preserving every other command in a result vector, and appending the latest snapshot after the loop. Call the helper after compositor backend commands are collected and before command execution.

- [ ] **Step 4: Verify focused and related runtime suites**

Run:

```bash
cargo test --locked --bin oblivion-one coalesce_client_list_sync
cargo test --locked --bin oblivion-one native_output::tests::
cargo test --locked --lib xwayland::xwm::
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
git diff --check
```

Expected: all commands exit zero with no warnings.

- [ ] **Step 5: Commit the isolated repair**

```bash
git add src/native_output/runtime/xwayland.rs
git commit -m "fix(xwayland): coalesce client-list snapshots"
```

### Task 2: Full verification, release, and live Steam popup validation

**Files:**
- Verify: repository-wide Rust sources and integration tests
- Build: `target/release/oblivion-one`

**Interfaces:**
- Consumes: committed coalescing repair.
- Produces: clean verified release binary for the next Typhon session.

- [ ] **Step 1: Run the complete quality gate**

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -- --test-threads=1
./bin/check-source-layout
git diff --check
```

Expected: zero failures and zero warnings.

- [ ] **Step 2: Build a provably fresh release**

```bash
cargo clean --release -p oblivion-one
cargo build --locked --release
sha256sum target/release/oblivion-one
```

Expected: release build exits zero and its hash differs from the currently running pre-fix compositor.

- [ ] **Step 3: Validate live Steam popup lifecycle after restart**

Launch Steam in the managed Xwayland display, open Settings and Friends via Steam URIs, and sample `_NET_CLIENT_LIST`, `_NET_CLIENT_LIST_STACKING`, and `_NET_ACTIVE_WINDOW`. Confirm that live dialogs remain mapped, destroyed popup XIDs disappear, and the Xwayland PID/generation remains stable.
