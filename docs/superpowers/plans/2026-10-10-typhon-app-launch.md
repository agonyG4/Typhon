# Typhon App Launch Correction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan inline. Steps use checkbox syntax for tracking.

**Goal:** Allow Typhon to launch queued desktop apps during normal operation and fail queued requests only when shutdown begins.

**Architecture:** Correct the shutdown predicate in `NativeRuntime::run_cycle`. Keep the existing launch drain, batching, environment, and shutdown failure helper unchanged. Build the updated Rust binary in the approved Aether target directory.

**Tech Stack:** Rust, Cargo, Typhon native compositor runtime.

## Global Constraints

- Keep the source checkout at `/home/agony/GitHub/Typhon` and work on `main`; do not create a branch.
- Put all Cargo build output under `/mnt/Aether/Desktop/GitHub`; set `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target` explicitly.
- Verify the effective Cargo output directory before building.
- Do not add or run tests unless the user asks.
- Preserve unrelated existing working-tree edits and commit only files changed for this fix.

---

### Task 1: Route launches by shutdown state

**Files:**
- Modify: `src/native_output/runtime/cycle.rs` at the pending launch backlog branch.

**Interfaces:**
- Consumes: `NativeShutdownLifecycle::is_running`, the existing pending process queue, and the current process launch drain.
- Produces: normal cycles drain queued launches; a cycle that has entered shutdown settles them through `fail_pending_process_launches_for_shutdown`.

- [ ] **Step 1: Correct the predicate.** Change `if self.shutdown.is_running()` at the launch backlog branch to `if !self.shutdown.is_running()`. Leave both branch bodies unchanged.
- [ ] **Step 2: Review the source diff.** Run `rtk git diff -- src/native_output/runtime/cycle.rs`; confirm the only source change is the predicate inversion and unrelated dirty files remain untouched.
- [ ] **Step 3: Verify the build destination.** Run `rtk realpath -m /mnt/Aether/Desktop/GitHub/Typhon-target` and confirm it resolves under `/mnt/Aether/Desktop/GitHub` before compiling.
- [ ] **Step 4: Build Typhon.** Run `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo build --release --bin oblivion-one` from `/home/agony/GitHub/Typhon`. Confirm the artifact is `/mnt/Aether/Desktop/GitHub/Typhon-target/release/oblivion-one`.
- [ ] **Step 5: Commit the fix.** Stage and commit only `src/native_output/runtime/cycle.rs` with message `fix: launch apps while compositor is running`.
- [ ] **Step 6: Apply and validate the runtime fix.** The currently running compositor was started from the checkout's old `target/release/oblivion-one`. When the updated Aether binary is running, click one app from Dock and one from Spotlight and confirm neither request returns error code 5.
