# Typhon Oversized Resize Presentation-Ghosting Report

Date: 2026-08-17

## Outcome

The oversized/offscreen resize repair is implemented and deterministic model
coverage is green.

Baseline HEAD was `a08a480fb552e9d26f907390964930d2fdebd698`. The worktree was
already dirty with unrelated tracked changes, a deleted prior report, and
untracked plans, logs, and `.codex/` artifacts; the final status was preserved
without cleanup.

Defect A was reproduced with a pixel-reference test: a framebuffer containing
presented frame A was repaired from rendered-but-unpresented B to current C,
which left A-only titlebar/frame pixels behind. The fix replaces the two
independently advanced native scene histories with one compact
`NativeSceneHistory`. Ready and submitted snapshots are keyed by the existing
output token; only the exact completed pageflip promotes the matching snapshot
to presented history. Stale, replaced, rejected, and recovered submissions do
not promote.

Defect B was reproduced with SSD preview tests: committed 1800x1000 geometry
continued to drive SSD layout while a narrower resize preview was active. SSD
render instances and hit testing now use
`current_visual_root_window_geometry`, with committed geometry only as the
fallback. Right-edge and left-edge preview parity now pass.

No clipping rewrite, forced full repaint, buffer-age disable, render-ahead
disable, triple-buffer disable, or Direct Scanout disable was added.

## Implementation

- Added compact scene metadata in
  `src/native_output/output/damage.rs`; snapshots contain surface IDs,
  mapped damage/bounds, decoration metadata, and cursor state, not client
  pixel buffers or theme assets.
- Added bounded ready/submitted/presented ownership in
  `src/native_output/runtime/scene_history.rs`.
- Captured candidates at render/worker-queue boundaries and promoted only from
  the exact completed pageflip path in
  `src/native_output/runtime/cycle/pageflip.rs`.
- Connected worker rejection and session quarantine to snapshot discard.
- Made native damage use presented scene history and logical bounds before
  output clipping.
- Made SSD render and hit-test layout use the active visual resize geometry in
  `src/compositor/state/window_decoration.rs`.
- Added pixel, shrink-sequence, exact-token, stale-token, right-edge, and
  left-edge regressions, including 31 oversized shrink states and all eight
  resize-edge logical-bound transitions.
- Corrected damage ordering so full logical old/new bounds are compared before
  each repair rectangle is clipped to the output. This is required when two
  oversized frames expose the same output slice but have different hidden
  right-side geometry.

## Verification

Passed:

- `rtk cargo fmt --check`
- `rtk cargo check --locked --all-targets`
- Native output tests: 34 passed
- Scene-history tests: 3 passed
- SSD decoration tests: 11 passed
- Full `oblivion-one` binary suite: 900 passed
- `rtk git diff --check`

The full library suite reached 1,664 passed and 20 failed. The failures are
environment-only: Astrea control-entry-point discovery is unavailable, and
several XWayland integration tests cannot create their socket paths because
the test workspace path exceeds the Unix socket-name limit. No changed native
output or SSD test failed.

Expected pre-existing checks remain:

- Clippy reports the existing large `XwmEvent` enum variant in
  `src/xwayland/xwm/event_types.rs:21`.
- Source-layout reports existing limits in
  `src/compositor/state/windows.rs`, `src/compositor/mod.rs`, and
  `src/compositor/server.rs`. The modified
  `src/native_output/runtime/presentation.rs` was kept within its limit.

## Native qualification

`/dev/dri/card0` and `/dev/dri/renderD128` exist and the user is in the
`video`/`render` groups, but `target/debug/astreactl status` reported that no
Typhon instance is running. Therefore no live DRM/KMS resize observation or
bounded native rendered/queued/submitted/pageflip trace is claimed.

The deterministic tests prove the presented-history and visual-geometry
defects at the model/output-damage boundary; live native confirmation remains
blocked until a Typhon native session is available.

Move impact is not native-qualified. The existing deterministic 30-step
movement damage test passes, but no live move observation was possible.

## Worktree and integration

The worktree was already dirty at baseline with unrelated tracked changes,
untracked plans/logs, and a deleted prior report. Those changes were preserved.
No files were staged, committed, or pushed.

Follow-up coverage still recommended: explicit buffer-age 1/2/3 schedule
tables, broader XWayland/CSD/fullscreen mode combinations, and live native
MacTahoe-Dark qualification when a Typhon instance can be started safely.
