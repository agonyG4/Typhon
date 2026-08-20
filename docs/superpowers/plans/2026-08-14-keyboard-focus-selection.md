# Typhon Keyboard-Focus Authority and Clipboard Sequencing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Typhon's keyboard focus authoritative and mapped-state-aware, while ensuring core clipboard and primary-selection publication is ordered safely during Qt Wayland bootstrap and remains independent from the data-control protocol.

**Architecture:** Keep `focused_surface` as the compositor's logical desktop/window-management selection, and make `keyboard_surface` the sole authority for `wl_keyboard` focus and core clipboard/primary-selection eligibility. A raw `wl_surface` is registered only; a normal XDG toplevel becomes eligible for automatic focus at its first mapped buffer commit. Core selection publication is targeted explicitly at the keyboard-focused client and occurs before keyboard `enter`; each target device still receives `DataOffer`, MIME offers, then `Selection` in that order. Data-control remains its own protocol and authority path.

**Tech Stack:** Rust, Smithay Wayland server protocol, Cargo test harness, existing `target/` artifacts, persistent `build/qt-wayland-bootstrap-probe` for any Qt probe.

## Global Constraints

- Run every shell command through `rtk`.
- Reuse the existing Cargo `target/` and project build directories; never clean, duplicate, or redirect builds to `/tmp`.
- Do not modify Eclipse unless a new Typhon-side runtime experiment proves an Eclipse defect.
- Do not change data-control authorization or merge it into core keyboard focus.
- Follow test-first order: add and run regression tests before changing production behavior.
- Do not use sleeps to prove protocol ordering; use event state and roundtrips.

---

## 1. Establish the bounded source and test map

- [x] Confirm the crash occurs during Qt Wayland platform bootstrap after Typhon advertises core clipboard, while the same Eclipse binary survives on the host compositor.
- [x] Identify the production paths for raw surface creation, XDG toplevel registration, first-map lifecycle, keyboard registration/focus, core selection, primary selection, and data-control.
- [x] Recheck graph generation and call `check_index_coverage` for every implementation and test path used in this plan. Read any reported partial ranges directly before relying on a negative claim.

## 2. Add test-only observability and RED regressions

- [x] Extend the existing compositor test registry state with a single ordered event timeline covering keyboard leave/enter/modifiers and core data-device offer/MIME/selection events.
- [x] Add a controllable-server snapshot for keyboard focus, or an equivalent test-only assertion path, so tests can distinguish logical desktop focus from actual keyboard focus.
- [x] Add a regression proving raw `wl_surface` creation does not establish logical or keyboard focus and does not emit keyboard enter.
- [x] Add a regression proving an unmapped XDG toplevel does not steal focus, while the first mapped buffer commit applies the normal toplevel autofocus policy.
- [x] Add a regression proving the exact core clipboard wire order is `DataOffer -> Offer* -> Selection`, with clipboard publication before keyboard `Enter`.
- [x] Add regressions for a genuinely keyboard-focused client's late data-device binding, cross-client focus transitions, same-client surface switches, primary-selection focus transitions, and offer replacement/destruction without unbounded growth.
- [x] Run the smallest affected test filters and record the expected RED failures against the current implementation.

## 3. Implement authoritative keyboard focus

- [x] Remove the raw-surface creation side effect that inserts the first surface into `focused_surface`.
- [x] Stop focusing an XDG toplevel during role registration; retain registration and configure behavior without granting focus.
- [x] Make keyboard registration consult `keyboard_surface`, not `focused_surface`, so binding a keyboard cannot manufacture focus.
- [x] Add explicit keyboard-focused client helpers and use them for core clipboard and primary-selection authorization/publication.
- [x] Refactor keyboard focus transitions so the target client receives core selection state before `wl_keyboard.enter`, then receives `Enter` followed by `Modifiers`, with `keyboard_surface` updated consistently only for a surface that actually received enter.
- [x] Preserve explicit layer-shell focus behavior and audit all direct `ensure_keyboard_focus` callers for the new authority/order contract.

## 4. Implement mapped-state autofocus and protocol separation

- [x] Focus a normal XDG toplevel only on its first mapped buffer commit, with the existing window-management policy and without treating popups or layer surfaces as ordinary toplevels.
- [x] Verify XWayland and layer-shell focus paths remain explicit and mapped-state-aware; make no unrelated policy change.
- [x] Keep data-control manager/device/source/selection handling independent from `keyboard_surface` and cover the separation with the existing tests plus any focused regression needed.

## 5. Verify without burning SSD lifetime

- [x] Run focused compositor tests using the existing target directory and no clean rebuild; all affected focus, clipboard, primary-selection, data-control, input, layer-shell, XDG, and XWayland filters pass.
- [x] Run the relevant broader Typhon test suite, formatting/checks, and inspect the final diff/status while preserving unrelated `.codex/` state. The default full library run passed 1,601 tests; 35 AstreaCtl/XWayland tests fail before assertions because the Codex `TMPDIR` produces Unix-socket paths longer than Linux `SUN_LEN`. `cargo fmt --check`, `cargo check --locked --all-targets`, clippy with `-D warnings`, source-layout validation, and `git diff --check` pass.
- [x] Build and run the Qt bootstrap probe in the persistent `build/qt-wayland-bootstrap-probe` directory. It succeeds on the host compositor. Against Typhon's in-process test server, it first reproduced the SIGSEGV with an active clipboard; after gating primary-selection late binding on keyboard focus, it succeeds with `WAYLAND_DEBUG=client` and no startup selection event.
- [x] Qualify the original `start-oblivion-one-tty` command only after Typhon tests pass, capture the resulting runtime/coredump state, and report any environment-specific limitation precisely. The user's fresh run reached Eclipse and reproduced the old crash before the final primary-selection correction. The rebuilt release binary is ready; this agent's bounded native launch is blocked before the shell by the pre-render atomic `TEST_ONLY` `Permission denied` fallback limitation.

## 6. Handoff

- [x] Summarize the root cause, changed files, test commands/results, runtime qualification, and the remaining distinction between Typhon behavior and Eclipse behavior.
