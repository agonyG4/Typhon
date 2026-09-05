# Typhon Keyboard Layout v3 Implementation Plan

> **For agentic workers:** Execute this plan task-by-task in the current checkout. Steps use checkbox (`- [ ]`) syntax for tracking. This implementation is intentionally executed inline; no sub-agents are used.

**Goal:** Add transactional runtime RMLVO/repeat/default-layout configuration with secure persistence, quiescent compositor-thread commit, Wayland publication, startup restoration, `astrea.control` commands, and typed `astreactl` commands.

**Architecture:** Keep all XKB keymap/state objects in the existing compositor-thread TLS owner. Add a sendable keyboard configuration value and a focused secure JSON store plus one-job eventfd worker. Prepare and validate XKB candidates on the compositor thread, persist the desired value asynchronously, and commit only after physical and client key ledgers are empty. Project complete configuration snapshots through the existing control codec and publish keymap/repeat/modifiers with the existing Wayland resource ownership rules.

**Tech Stack:** Rust 2024, existing `xkbcommon = "0.9.0"` bindings with the already-qualified libxkbcommon floor, serde/serde_json, nix/libc filesystem primitives already used by the cursor store, existing native event loop, Wayland server test harness, and `rtk` commands.

## Global Constraints

- Preserve v1/v2 single-XKB authority, compositor-thread TLS ownership, raw evdev keycodes, session semantics, inhibition, repeat filtering, and XWayland sharing.
- Do not add secondary XKB state, `xkb_state_update_mask`, synthetic key/group events, held-key replay, manual keymap text, unsafe XKB `Send`/`Sync`, or runtime fallback.
- Runtime persistence is asynchronous and never performs fsync in the event/render loop; the worker receives only sendable `KeyboardConfig` data.
- Candidate preparation, state migration, and active swap happen on the compositor thread; persistence succeeds before publication.
- A pending mutation is single-flight and commits only when both authoritative physical and compositor client pressed-key sets are empty.
- Keep v2 layout commands ephemeral and generation-neutral. Do not stage or alter unrelated existing worktree changes.
- Build and test in `/home/agony/GitHub/Typhon` so Cargo uses the checkout-local `target` directory. Use `rtk` for every command and do not dispatch sub-agents.

---

## Task 1: Add typed configuration, snapshots, and the secure keyboard store

**Files:** `src/compositor/keyboard.rs`, new `src/keyboard_persistence.rs`, `src/control_snapshots.rs`, `src/lib.rs`, `Cargo.toml` only if an existing dependency is required; tests beside the implementations.

- [ ] Write failing unit tests for the v3 config default/index/nullable-field model, strict version-1 camelCase JSON, unknown fields, NUL rejection, bounded/truncated/oversized reads, and startup source/persistence status projection.
- [ ] Run the focused tests and confirm the intended missing APIs/types fail.
- [ ] Extend `KeyboardConfig` with `default_layout_index` and preserve exact `Some("")` versus `None` behavior. Add bounded structural validation without normalizing RMLVO values.
- [ ] Add a dedicated `KeyboardConfigurationStore` using the existing secure cursor-store principles: XDG path resolution, 0700 directories, 0600 regular file, owner checks, no symlink traversal, bounded reads, temporary atomic publication, file/directory sync, stale temporary cleanup, and previous-file preservation on failure.
- [ ] Add strict typed `KeyboardConfigurationValue`, source/persistence enums, and `KeyboardConfigurationSnapshot`, reusing `KeyboardLayoutSnapshot` and camelCase/`deny_unknown_fields` serde conventions.
- [ ] Run the focused store/schema tests green, `rtk cargo fmt -- --check`, and `rtk cargo test --lib keyboard_persistence`.
- [ ] Commit with `feat: add persistent keyboard configuration model`.

## Task 2: Make XKB candidate preparation and commit transactional

**Files:** `src/compositor/keyboard.rs`, `src/compositor/state/input_resources.rs`, `src/compositor/server_toplevel.rs`, `src/compositor/mod.rs`; tests in the keyboard/compositor modules.

- [ ] Write failing tests for valid candidate replacement, invalid rollback, default index and Text V1 NUL/bounds validation, same-named locked modifier migration (including an extra modifier and absent-name drop), depressed/latched clearing, and no-op detection.
- [ ] Run the focused tests and confirm the intended missing transactional API fails.
- [ ] Add `PreparedKeyboardConfiguration` and TLS methods to prepare candidates without touching active state, inspect physical pressed keys and locked modifier names, construct candidate state, migrate locked names, apply the default locked layout through the v2 bridge, and swap keymap/state/config/bytes together.
- [ ] Keep active generation monotonic only for real configuration changes; expose a complete compositor-owned snapshot and pending marker.
- [ ] Add narrow server/state wrappers for preparing, marking persistence success, testing quiescence, committing, and publishing. Ensure v2 layout mutations remain unchanged and do not alter configuration generation.
- [ ] Run focused XKB/compositor tests green and inspect the authoritative source for forbidden mask-copy/replay/reconstruction paths.
- [ ] Commit with `feat: add transactional xkb configuration commit`.

## Task 3: Add startup precedence and the dedicated persistence worker

**Files:** `src/keyboard_persistence.rs`, `src/native/event_loop.rs`, `src/native_output/runtime/mod.rs`, `src/native_output/runtime/bootstrap.rs`, `src/native_output/runtime/cycle.rs`, `src/native_output/runtime/cycle_dispatch.rs`, `src/native_output/runtime/work_domains.rs`.

- [ ] Write failing tests for default -> persisted -> environment precedence, layout/variant override semantics, valid/invalid default index recovery, invalid persisted-data recovery, worker busy/order/stale completion handling, and persistence failure rollback.
- [ ] Run the focused tests and confirm the worker/startup seams fail before implementation.
- [ ] Add a one-job `KeyboardPersistenceWorker` with eventfd notification, bounded completion, availability/busy state, panic/closed-worker failure handling, and no XKB/Wayland/compositor objects crossing the thread boundary.
- [ ] Add the keyboard worker event source and wakeup collection without disturbing existing cursor/KMS wake handling.
- [ ] Load persisted data during native bootstrap, apply environment overrides after it, compile the active startup candidates with the existing fallback semantics, and record source/persistence/environment-override status.
- [ ] Add runtime dispatch-cycle service for completions, stale job IDs, pending candidate state, quiescence retry, shutdown discard, client disconnect response dropping, and disabling a failed worker.
- [ ] Run focused worker/startup tests, then `rtk cargo test --lib native_output::runtime`.
- [ ] Commit with `feat: add asynchronous keyboard configuration persistence`.

## Task 4: Publish full replacement, repeat-only, and default-only changes

**Files:** `src/compositor/keyboard.rs`, `src/compositor/state/input_resources.rs`, `src/compositor/server_toplevel.rs`, relevant compositor Wayland tests.

- [ ] Write failing real-client tests for full replacement to all live keyboard resources, valid Text V1, no key/enter/leave events, focused modifiers, repeat-only version gating, default-only group/modifier publication, quiescence while A/modifiers are held, and no-op silence.
- [ ] Run the Wayland focused tests and confirm the expected publication behavior is not yet implemented.
- [ ] Refactor initial-state sending into keymap/repeat helpers, add all-live-resource publication helpers, filter repeat to protocol version 4+, and route modifiers only to the focused client.
- [ ] Implement classification so RMLVO, repeat, and default-index differences follow distinct commit/publication paths, with no recompilation for repeat/default-only changes.
- [ ] Make commit retry automatically on later input cycles after release and flush through existing server machinery.
- [ ] Run the compositor client tests green and verify focused-only keymap publication is absent from the replacement path.
- [ ] Commit with `feat: publish transactional keyboard reconfiguration`.

## Task 5: Add strict control protocol and typed client/CLI commands

**Files:** `src/control.rs`, `src/native_output/runtime/cycle_dispatch.rs`, `src/astreactl/client.rs`, `src/astreactl/output.rs`, `src/bin/astreactl.rs`, `tests/astreactl.rs`, `docs/astreactl.md`.

- [ ] Write failing codec, runtime, and CLI tests for exact `{}` get args, full typed set args, unknown/missing/nullable fields, invalid/busy/internal errors, asynchronous response ordering, `keyboard config`, `keyboard configure`, merge-preserve semantics, explicit nullable clears, `--json`, sanitized human output, and preservation of every v2 command/option.
- [ ] Run focused tests and confirm missing command/types/parser behavior fails.
- [ ] Add `keyboard.config.get` and `keyboard.config.set` to the v1 command enum/parser and dispatch strict typed argument/result envelopes through the transactional runtime.
- [ ] Add typed client snapshot/config argument decoding and bounded human formatting with existing sanitizer/no-color behavior.
- [ ] Add CLI `config` and `configure` subcommands with typed flags, optional merge fetch, explicit clear flags, and no arbitrary JSON input.
- [ ] Update `docs/astreactl.md` with the v3 commands while retaining v2 runtime layout documentation and history.
- [ ] Run `rtk cargo test --test astreactl` and `rtk cargo test --bin astreactl` green.
- [ ] Commit with `feat: expose keyboard configuration control`.

## Task 6: Qualify integration, guard invariants, and close documentation

**Files:** v3 design/plan docs, any focused test modules, no unrelated files.

- [ ] Add restart/persistence integration coverage, environment recovery coverage, strict control busy/order coverage, and final source-guard tests/searches for all forbidden patterns listed in the v3 specification.
- [ ] Run the exact required verification from the specification in the checkout-local folder:

  ```text
  rtk pkg-config --modversion xkbcommon
  rtk pkg-config --atleast-version=1.10.0 xkbcommon
  rtk cargo fmt -- --check
  rtk cargo check --all-targets
  rtk cargo clippy --all-targets -- -D warnings
  rtk cargo test --lib
  rtk cargo test --test astreactl
  rtk cargo test --bin astreactl
  rtk cargo test
  rtk git diff --check
  rtk git status --short
  ```

- [ ] Inspect the final diff for unrelated changes, confirm the pre-existing workspace protocol edit remains unstaged, and commit with `docs: close keyboard layout v3 verification`.
