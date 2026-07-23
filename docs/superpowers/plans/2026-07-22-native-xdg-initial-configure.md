# Native GTK XDG Activation Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure native Wayland clients such as GTK4 Zenity do not stall when they request an activation token with an invalid input serial.

**Architecture:** Reproduce GTK startup by creating an xdg-toplevel and committing an activation token with serial zero. Complete the token request with an invalid empty token, as required by xdg-activation-v1, without recording a usable compositor activation token.

**Tech Stack:** Rust, wayland-server, wayland-client, Cargo tests.

## Global Constraints

- Preserve existing xdg-shell ordering and configure-ack validation.
- Do not alter Steam or force applications onto Xwayland.
- Do not modify or discard unrelated working-tree changes.

---

### Task 1: Reproduce and fix invalid activation-token completion

**Files:**
- Modify: `src/compositor/tests/xdg.rs`
- Modify only the production file proven responsible by the failing test.

**Interfaces:**
- Consumes: `OwnCompositorServer::tick`, the existing test server harness, and `RegistryTestState` xdg event handling.
- Produces: a regression test proving GTK-style startup receives both its xdg configure and activation-token completion without `wl_display.sync`.

- [x] **Step 1: Write the failing integration test**

Create an xdg toplevel, make the initial empty commit, request an activation token with serial zero, and dispatch events without a client roundtrip. Assert that configure events and activation-token `done` all arrive.

- [x] **Step 2: Run the focused test to verify it fails**

Run: `cargo test compositor::tests::xdg::invalid_activation_serial_still_completes_gtk_toplevel_startup -- --exact --nocapture`

Observed before the fix: FAIL because `activation_token_done` remains `None`.

- [x] **Step 3: Implement the minimal production correction**

When activation-token serial validation fails, send `done("")` and return without storing a valid token.

- [x] **Step 4: Verify focused and neighboring tests**

Run the focused test, then all `compositor::tests::xdg` tests. Observed: PASS (28 tests).

- [x] **Step 5: Verify the broader suite and build the release binary**

Run `cargo fmt --check`, `git diff --check`, `cargo test`, and `cargo build --release`. Restart the live compositor session before repeating the native Zenity reproducer because an already-running compositor cannot load the new code.
