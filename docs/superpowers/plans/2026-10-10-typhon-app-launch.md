# Typhon App Launch Correction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan inline. Steps use checkbox syntax for tracking.

**Goal:** Allow Typhon to launch queued desktop apps during normal operation and fail queued requests only when shutdown begins.

**Architecture:** Correct the shutdown predicate in `NativeRuntime::run_cycle`. Keep the existing launch drain, batching, environment, and shutdown failure helper unchanged. The first build exposed compile errors in the effect module's public exports; make a minimal export-only correction so Cargo can produce the updated Rust binary in the approved Aether target directory.

**Tech Stack:** Rust, Cargo, Typhon native compositor runtime.

## Global Constraints

- Keep the source checkout at `/home/agony/GitHub/Typhon` and work on `main`; do not create a branch.
- Put all Cargo build output under `/mnt/Aether/Desktop/GitHub`; set `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target` explicitly.
- Verify the effective Cargo output directory before building.
- Do not add or run tests unless the user asks.
- Preserve unrelated existing working-tree edits and commit only files changed for this fix.

---

### Task 1: Build the corrected launch path

**Files:**
- Modify: `src/egl_renderer/effects/mod.rs` to merge duplicate resource exports and expose the existing preparation function to the frame module.
- Modify: `src/native_output/runtime/cycle.rs` at the pending launch backlog branch (already committed as `9407019`).

**Interfaces:**
- Consumes: the existing executor preparation function, effect resource types, `NativeShutdownLifecycle::is_running`, the pending process queue, and the current process launch drain.
- Produces: one unambiguous resource export set; the frame module can call the existing preparation function; normal cycles drain queued launches; a cycle that has entered shutdown settles them through `fail_pending_process_launches_for_shutdown`.

- [x] **Step 1: Correct the launch predicate.** Change `if self.shutdown.is_running()` at the launch backlog branch to `if !self.shutdown.is_running()`. Leave both branch bodies unchanged. Commit `9407019` contains this one-line fix.
- [ ] **Step 2: Repair the compile blockers.** In `src/egl_renderer/effects/mod.rs`, merge the duplicate `resources` re-export blocks while retaining `GraphTextureBinding`. Add `prepare_effect_graph_execution_with_external_bindings` to the existing `pub(super) use executor::{...}` list; do not change the function implementation or any runtime behavior.
- [ ] **Step 3: Review the source diff.** Run `rtk git diff -- src/egl_renderer/effects/mod.rs`; confirm it contains only the export cleanup and unrelated dirty files remain untouched.
- [x] **Step 4: Verify the build destination.** `rtk realpath -m /mnt/Aether/Desktop/GitHub/Typhon-target` resolved to `/mnt/Aether/Desktop/GitHub/Typhon-target` before the first build attempt.
- [ ] **Step 5: Build Typhon.** Run `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo build --release --bin oblivion-one` from `/home/agony/GitHub/Typhon`. Confirm the artifact is `/mnt/Aether/Desktop/GitHub/Typhon-target/release/oblivion-one`.
- [ ] **Step 6: Commit the export repair.** Stage and commit only `src/egl_renderer/effects/mod.rs` with message `fix: restore effect exports for release build`.
- [ ] **Step 7: Apply and validate the runtime fix.** The currently running compositor was started from the checkout's old `target/release/oblivion-one`. When the updated Aether binary is running, click one app from Dock and one from Spotlight and confirm neither request returns error code 5.
