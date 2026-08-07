# M7-A Desktop Interaction Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the approved Typhon M7-A desktop interaction policy: focus follows hover without raising, click activation raises the exact originally hit window, move/resize retains exclusive motion ownership, and committed CSD/shadow extents remain visible during resize preview without adding a normal application border.

**Architecture:** Extend Typhon's existing focus, stacking, pointer-hit-test, interaction, resize-preview, and render-plan seams. Keep pointer focus, keyboard focus, and interaction motion target as separate state. Use exact managed `WindowId` policy for desktop focus and activation; use a conservative root-only visual aperture derived only from the committed root buffer and committed XDG geometry.

**Tech Stack:** Rust, Smithay/Wayland compositor state, existing CPU/GLES render plans, XWayland, deterministic Rust tests, native-session Firefox/Kitty qualification.

## Global Constraints

- Work only in `/home/agony/GitHub/Typhon`, directly on the existing `main` checkout.
- Preserve all pre-existing user changes. Do not reset, clean, discard, branch, create a worktree, detach HEAD, amend, squash, or rewrite history.
- M7-A is Typhon-only. Do not edit Eclipse or implement M7-B until the real Firefox/Kitty M7-A gate has actually passed.
- Follow red/green/refactor for every production slice: add a focused failing regression, run it and record the expected failure, implement the smallest change, rerun focused tests, then run the relevant broader suite.
- Do not change XDG decoration negotiation mode, infer shadows from arbitrary surface trees, or add a visible Typhon-owned normal application border.
- Hover focus never raises. Click activation raises exactly once through the existing family-aware stacking machinery.
- Generic pointer `FocusLoss` never terminates an active move/resize. The captured interaction motion target remains authoritative until an explicit terminal condition.
- Pointer press delivery remains tied to the original hit-test result even when activation changes stacking; no post-activation re-hit-test is allowed.
- Hover advances focus history only when keyboard focus changes to a different managed `WindowId`; repeated hover and pointer refreshes over the same window do not churn the serial.
- Post-interaction pointer refresh is performed exactly once after terminal cleanup.
- Do not claim TTY/DRM, Firefox, or Kitty qualification unless it is observed during this implementation run.

## File Map

Expected production touch points:

- `src/compositor/interaction.rs` — make captured interaction motion ownership explicit and preserve terminal-reason distinctions.
- `src/compositor/state/window_interaction.rs` — begin, update, cancel, release, and exactly-once terminal cleanup.
- `src/compositor/state/hit_testing.rs` — make pointer-focus refresh interaction-safe and centralize the one post-terminal refresh.
- `src/compositor/state/input_resources.rs` — deliver interaction motion to the captured target without requiring current pointer focus to match it.
- `src/native_output/input/routing.rs` — preserve the active-interaction routing order and target identity.
- `src/compositor/state/surface_focus.rs` — add reason-aware desktop focus transition semantics without changing low-level surface focus callers.
- `src/compositor/state/windows.rs` — centralize exact desktop focus and activation policy.
- `src/compositor/state/desktop_windows.rs` — provide family-aware topmost/no-op information required by activation.
- `src/compositor/state/input_dispatch.rs` — capture the press target once, activate the captured `WindowId`, then deliver to the captured surface.
- `src/server.rs` — use the centralized policy where existing shell activation and managed X11 ConfigureRequest handling require it.
- `src/compositor/state/window_resize.rs` — compute committed visual extents and resolve the bounded resize-preview aperture.
- `src/compositor/render.rs` and, if required by the existing representation, `src/compositor/surface.rs` — carry one root-only aperture consistently through CPU/GLES scene plans and damage.

Expected test touch points:

- `src/compositor/state/window_interaction_tests.rs` — interaction ownership, focus-loss suppression, target destruction, stale terminal events, and refresh counts.
- `src/compositor/tests/input_output/window_interaction.rs` — end-to-end pointer crossing and captured motion delivery.
- `src/native_output/tests/input_interaction_liveness.rs` — native input routing while the captured target is no longer pointer-focused.
- `src/compositor/state/desktop_window_tests.rs` — hover focus serials, no-raise hover, exact click activation, no-op activation, minimized restore, and family ordering.
- `src/compositor/tests/input_output/output_keyboard_cursor.rs` or the nearest existing focus test module — pointer/keyboard focus separation.
- `src/compositor/state/window_resize.rs` tests or `src/compositor/tests/xwayland_resize_visual.rs` — `WindowVisualExtents`, aperture, and edge/corner regressions.
- `src/compositor/render.rs` tests — render-plan and damage parity for the resolved aperture.
- `src/compositor/tests/xwayland.rs` — managed X11 border-width normalization and ConfigureNotify behavior.

The exact test module may follow current Typhon placement conventions, but every regression must remain close to the seam it protects and must assert observable state rather than only a boolean active flag.

---

## Task 1: Make move/resize interaction ownership exclusive

**Files:** `src/compositor/interaction.rs`, `src/compositor/state/window_interaction.rs`, `src/compositor/state/hit_testing.rs`, `src/compositor/state/input_resources.rs`, `src/native_output/input/routing.rs`, `src/compositor/state/window_interaction_tests.rs`, `src/compositor/tests/input_output/window_interaction.rs`, `src/native_output/tests/input_interaction_liveness.rs`.

### 1.1 Add failing regressions first

- [ ] Add a test that starts a move or resize on managed window A, crosses managed window B, leaves the desktop, returns over A, and releases. Assert one interaction ID, one captured `WindowId`, and motion delivery to A throughout.
- [ ] Add a test that changes pointer focus to B while A is interacting and asserts that a generic pointer-focus clear does not end A's interaction, clear its cursor override, or clear its pending resize preview.
- [ ] Add a test that delivers interaction motion while the captured interaction surface is no longer the current pointer-focused surface. The current hit-test must not replace the captured recipient.
- [ ] Add a test covering XDG commit, XWayland metadata/configure, scene reconciliation, and explicit pointer refresh during an active interaction; none may transfer ownership or terminate it.
- [ ] Add a test that releases the trigger, completes terminal cleanup, and observes exactly one post-interaction pointer refresh. A repeated/stale release must be a no-op.
- [ ] Add a target-destruction/unmap cancellation test that ends the interaction once, clears cursor and preview state once, and performs one safe refresh when the session is still active.
- [ ] Run the focused tests and record the expected red failures before changing production code.

### 1.2 Implement captured motion ownership

- [ ] Represent the interaction motion target explicitly as the captured root/surface identity plus its `WindowId`; keep it independent from pointer-focus and keyboard-focus state. Capture it once in the begin path and never overwrite it from later hit-tests or focus changes.
- [ ] Preserve the interaction ID, trigger serial/button, and captured root identity in all update and terminal paths so stale updates and stale terminal events can be rejected without affecting a newer interaction.
- [ ] Remove the requirement in interaction motion dispatch that `pointer_surface` equal the captured motion surface. Validate only that the captured target is alive, remains under the captured root, and remains protocol-valid for the interaction, then send motion using the captured target's resources.
- [ ] Keep active-interaction routing ordered as global pointer-position update, captured geometry update, captured-target motion delivery, and interaction cursor preservation. Do not run ordinary hit-test ownership replacement in this path.
- [ ] Make `clear_pointer_focus_state`, crossing refresh, implicit-grab refresh, XDG/XWayland reconciliation, and session cleanup interaction-aware. Pointer focus loss must not call interaction cancellation with `FocusLoss`.
- [ ] Replace generic `FocusLoss` termination with explicit terminal reasons only: trigger release, explicit end/cancel, mode transition, target destruction/unmap, client disconnect, pointer-constraint transition, session suspension, input removal, or state teardown as applicable to the existing call site.
- [ ] Consolidate normal completion and cancellation so each terminal event clears interaction state once, finalizes pending resize/configure state where required, clears cursor override, finishes event delivery, and then performs one post-terminal pointer refresh.
- [ ] Keep release delivery tied to the original implicit-grab/interaction record; never re-target a release from the current pointer location.

### 1.3 Verify the slice

- [ ] Run focused interaction tests serially, including the existing interaction and input-liveness modules.
- [ ] Run `cargo fmt --check` and `cargo test --locked --test <focused-targets>` using the repository's actual test target names.
- [ ] Inspect the diff for any remaining current-hit-test lookup in the active interaction motion path or any `FocusLoss` cancellation.
- [ ] Commit only this slice with a focused message such as `fix: preserve move and resize ownership across focus changes`.

## Task 2: Implement desktop hover focus and exact click activation

**Files:** `src/compositor/state/surface_focus.rs`, `src/compositor/state/windows.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/state/input_dispatch.rs`, `src/server.rs`, `src/compositor/state/desktop_window_tests.rs`, `src/compositor/tests/input_output/output_keyboard_cursor.rs`, and the nearest existing XDG/X11 focus test modules.

### 2.1 Add failing policy regressions first

- [ ] Add overlapping-window coverage where hovering from A to B changes keyboard focus to B but leaves `window_stacking` unchanged.
- [ ] Repeat hover over B and perform pointer-focus refreshes over B; assert the managed focused `WindowId` and desktop focus serial remain unchanged and no duplicate focus transition is emitted.
- [ ] Add the critical click regression: arrange overlapping windows, hit B, make activation reorder the stack, and assert that the button event still arrives at the surface captured before activation. The test must fail if a second hit-test selects the newly topmost surface.
- [ ] Assert that a click focuses and raises exactly once through the existing family-aware path, with no duplicate backend restack or focus command when the target is already focused and topmost.
- [ ] Add minimized-window activation coverage proving restore occurs before focus and raise.
- [ ] Cover XDG and managed X11 normal windows plus X11 transient-family ordering. Verify popup, layer-shell, lock, override-redirect, notification, support, and compositor-owned surfaces do not receive ordinary desktop raise semantics.
- [ ] Run the focused tests and record the expected red failures.

### 2.2 Add reason-aware focus and activation policy

- [ ] Add the reason enum used by the approved design (`PointerEnter`, `PointerPress`, `ShellActivation`, `KeyboardNavigation`, and `Restore`) in the module that owns desktop focus policy.
- [ ] Make desktop focus resolve an exact managed `WindowId`, reject destroyed/unmapped/minimized/ineligible targets, focus the root through existing low-level focus, update active-state publication, and queue existing X11 activation synchronization where applicable.
- [ ] Guard desktop focus serial advancement by managed `WindowId` transition. A surface resource refresh or subsurface transition inside the same window must not advance the serial; low-level protocol surface focus may still change independently.
- [ ] Make hover call only the focus operation with `PointerEnter`; it must never call `raise_window_id`, `raise_root_window`, or activation.
- [ ] Make pointer motion derive an eligible managed root from the already resolved hit target, suppress hover focus during active move/resize, held button/implicit grab, popup grab, pointer lock/confinement, DND, session lock, or exclusive layer-shell interaction, and avoid all ineligible surface classes.
- [ ] Add exact `activate_desktop_window(WindowId, PointerPress)` behavior that resolves, restores if minimized, focuses, and invokes the existing family-aware raise once. Return a no-op outcome when focused/restored/topmost so duplicate backend work is not emitted.
- [ ] In `send_pointer_button`, perform exactly one hit-test, capture the target/root/`WindowId`, establish pointer focus, call exact activation, create the press/implicit-grab record from the captured target, and deliver the button to that captured surface. Do not re-hit-test after activation.
- [ ] Keep low-level `focus_surface` behavior unchanged for popups, layers, pointer constraints, and compositor-owned surfaces; do not let it gain desktop raise/restore semantics.

### 2.3 Verify the slice

- [ ] Run the focused focus, stacking, input-dispatch, XDG, and XWayland tests serially.
- [ ] Run the relevant `cargo fmt --check` and `cargo test --locked` focused filters, then inspect stack snapshots and command counts in the regressions.
- [ ] Review the diff for hover-to-raise calls, duplicate activation calls, and any hit-test after activation.
- [ ] Commit only this slice with a focused message such as `fix: separate desktop hover focus from activation`.

## Task 3: Preserve root visual extents during resize preview

**Files:** `src/compositor/state/window_resize.rs`, `src/compositor/render.rs`, `src/compositor/surface.rs` only if the existing render-plan types require a narrow aperture representation, `src/compositor/state/window_resize_tests.rs` if present, `src/compositor/tests/xwayland_resize_visual.rs`, and the nearest existing render/native-damage test modules.

### 3.1 Add failing geometry and aperture regressions first

- [ ] Add unit coverage for `WindowVisualExtents` using a committed root buffer of `332 x 242` and logical geometries including `(16, 10, 300, 200)`, negative XDG offsets, and geometry larger than or offset within the root buffer. Assert the actual four extents and use signed intermediate arithmetic without unsigned underflow.
- [ ] Add an explicit left-edge resize regression with negative/offset CSD geometry. Assert the changed placement, logical target, preserved left visual extent, and resolved aperture bounds.
- [ ] Add an explicit top-edge resize regression with negative/offset CSD geometry. Assert the changed placement, logical target, preserved top visual extent, and resolved aperture bounds.
- [ ] Add an explicit top-left resize regression that changes both placement axes and asserts both preserved extents, anchor semantics, and aperture strips.
- [ ] Add right-edge, bottom-edge, all-corner, grow, and shrink cases so the three required regressions are not isolated from the existing resize anchoring rules.
- [ ] Add a final-client-commit regression proving the preview aperture clears while the committed root render placement remains correct.
- [ ] Add a stale-content regression proving the aperture does not expose old logical client content outside the desired logical geometry and does not extend the same clip to unrelated subsurfaces.
- [ ] Add CPU/GLES plan parity and native-damage coverage where the repository's test abstractions permit; assert both old and new visible bounds contribute to damage.
- [ ] Run the focused geometry/render tests and record the expected red failures.

### 3.2 Implement signed extents and a bounded root-only aperture

- [ ] Add the approved root-level `WindowVisualExtents { left, top, right, bottom }` value type using the current module visibility and integer conventions. Keep its public values non-negative, but compute from signed coordinates in an intermediate wide integer type.
- [ ] Derive extents only from committed root buffer bounds and authoritative committed `xdg_window_geometry`: treat the buffer as `[0, 0, buffer_width, buffer_height]` and the logical rectangle as `[x, y, x + width, y + height]`; clamp final outside-buffer distances safely after checked/saturating arithmetic.
- [ ] Do not inspect unrelated subsurfaces, alpha, colors, application identity, titlebar content, magic shadow sizes, or surface-tree relationships when deriving extents.
- [ ] Introduce the smallest resolved root aperture representation needed by the current renderer. It must carry the desired logical content aperture plus only the committed root pixels that were outside the prior logical geometry; if one rectangular clip cannot express that union safely, represent non-overlapping root-only content and extent strips rather than widening every surface clip.
- [ ] Apply the extents exactly once when resolving preview placement and target size. Preserve desired logical geometry as the configure/resize-anchor source of truth; never scale or rewrite the committed root buffer.
- [ ] Integrate the aperture into `update_toplevel_visual_render_assignment` and the existing resize-preview path so left/top placement changes and width/height changes use the same resolved target. Keep the aperture independent of focus eligibility and ordinary frame hit-testing.
- [ ] Ensure the logical content remains clipped to the desired logical geometry, the committed CSD/shadow pixels remain visible in the bounded extent strips, and stale old client content cannot leak into those strips.
- [ ] Feed the same resolved aperture to CPU scene plans, GLES scene plans, snapshots, visible bounds, and native damage. Include both old and new aperture bounds in damage when preview geometry changes.
- [ ] Clear the preview aperture on final client commit and restore the ordinary committed render assignment without double-applying the XDG offset.

### 3.3 Verify the slice

- [ ] Run the extents, resize-preview, render-plan, XWayland visual, and native-damage tests serially.
- [ ] Run the focused command set from the handoff and `cargo fmt --check`; inspect actual target rectangles, clips, aperture strips, and damage bounds in test failures/output.
- [ ] Review the diff for generalized surface-tree shadow inference, double-applied XDG offsets, unsigned signed-coordinate arithmetic, and stale-content leaks.
- [ ] Commit only this slice with a focused message such as `fix: preserve committed visual extents during resize`.

## Task 4: Enforce the no-visible-border policy and normalize managed X11 borders

**Files:** `src/server.rs`, `src/compositor/render.rs`, `src/compositor/state/window_resize.rs` only where existing interaction hit thickness is defined, `src/compositor/tests/xwayland.rs`, `src/compositor/tests/xwayland_resize_visual.rs`, and relevant render/state tests.

### 4.1 Add failing border regressions first

- [ ] Add a normal XDG render regression that inspects scene elements/render plans and proves Typhon emits no visible application border, titlebar, separator, or compositor shadow.
- [ ] Add a normal managed X11 render regression with the same assertion; server-frame primitives must not be emitted for ordinary applications.
- [ ] Add a resize-hit-thickness regression proving the existing invisible interaction margin remains usable but contributes no visible render element or border pixels.
- [ ] Add a managed X11 ConfigureRequest regression with a non-zero requested border width. Assert the effective XWM Configure command uses border width zero while preserving the requested geometry and existing ConfigureNotify contract.
- [ ] Run the focused border/X11 tests and record the expected red failures.

### 4.2 Implement the narrow policy change

- [ ] Normalize effective managed-client X11 `ConfigureRequest` border width to zero at the existing request-to-XWM-Configure seam; do not create a second configure path or alter geometry calculations outside the border field.
- [ ] Preserve existing client-side GTK/Qt/Firefox/etc. pixels and XDG decoration negotiation. Do not force server-side decorations or add a Typhon-owned normal frame.
- [ ] Keep server-frame primitives available for their existing non-normal uses, but ensure ordinary XDG and managed X11 composition does not emit them.
- [ ] Keep resize hit thickness as interaction metadata only; do not route it through visual aperture or render-element generation.

### 4.3 Verify the slice

- [ ] Run focused XDG, X11, render, and ConfigureNotify tests serially.
- [ ] Run `cargo fmt --check`, inspect the diff for a second X11 configure/stacking path, and verify the border-width normalization is limited to managed clients.
- [ ] Commit only this slice with a focused message such as `fix: keep normal application borders compositor-invisible`.

## Task 5: Run deterministic M7-A validation and review gates

**Files:** No production files are expected. Update only existing test harness scripts/configuration if a concrete repository-supported stress entry point is missing; do not add arbitrary sleeps or broaden scope.

### 5.1 Run focused and broad automated validation

- [ ] Run `cargo fmt --check`.
- [ ] Run `cargo check --locked --all-targets`.
- [ ] Run `cargo clippy --locked --all-targets -- -D warnings`.
- [ ] Run all focused interaction, pointer/focus, stacking, resize, XDG toplevel, XWayland, render-plan, and native-damage tests serially.
- [ ] Run `cargo test --locked` serially after focused tests pass, then rerun any failure in isolation to distinguish test-environment/FD flakes from product regressions.
- [ ] Run `./bin/check-source-layout` and `git diff --check`.
- [ ] Execute the repository-supported no-sleep stress groups: 100 click-focus-raise cycles, 100 hover-focus-without-raise cycles, 100 mixed hover/click cycles, 100 XDG resize-across-window cycles, 100 X11 resize-across-window cycles, 100 pointer-refresh-during-resize cycles, and 100 CSD extent-resize cycles. Record each result.

### 5.2 Perform the required real-session gate

- [ ] Only after all deterministic tests and stress groups pass, run the real Firefox/Kitty gate in the native Typhon session.
- [ ] Verify hover Kitty while Firefox remains above it focuses Kitty without raising it; click Kitty focuses and raises it exactly once; hover Firefox focuses Firefox while Kitty remains above.
- [ ] Resize Kitty across Firefox, empty desktop, and Kitty until release, repeating left, right, top, bottom, and corner resizes. Verify client-side shadow/CSD extents remain present during preview.
- [ ] Record observed TTY/DRM/native-session results separately from automated tests. Do not treat historical logs or host-compositor runs as this gate.
- [ ] Do not begin M7-B or Eclipse work unless this real-session M7-A gate is actually observed and passes.

### 5.3 Conduct final invariant review

- [ ] Search the final diff for hover paths that raise, click paths that re-hit-test after activation, same-window focus-serial churn, motion ownership derived from current hit-test/pointer focus, generic `FocusLoss` cancellation, and multiple post-interaction refreshes.
- [ ] Search for generalized surface-tree shadow inference, XDG decoration-mode changes, visible normal application borders, duplicate X11 stacking/configure paths, and new `unsafe` without a local precise `// SAFETY:` explanation.
- [ ] Inspect `git status --short --branch`, confirm only intended Typhon files changed, and preserve all unrelated existing changes.
- [ ] Report automated results, stress results, native-session results, and any unrun qualification separately. Do not claim completion of an unobserved gate.

## Completion Checklist

- [ ] Tasks 1 through 4 each have an atomic focused commit and their focused tests pass.
- [ ] The full locked Rust checks, source-layout check, and serial full suite pass.
- [ ] Required 100-cycle stress groups pass without arbitrary sleeps.
- [ ] The real Firefox/Kitty M7-A gate is observed and recorded, or the handoff explicitly says it remains pending.
- [ ] The final diff contains no Eclipse or M7-B changes.
- [ ] The next action is selected only after the evidence review: if M7-A passes, prepare the separate M7-B/Eclipse plan; otherwise keep those areas untouched.
