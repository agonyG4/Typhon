# Neutral Output Background Closure Report

Date: 2026-08-20
Repository: Typhon
Baseline: `0ef9f7b99fa38d0fc04bf5ffa8f494db5a6eade6`

## Outcome

Typhon no longer renders an artistic wallpaper gradient as compositor-owned fallback. The CPU scene starts from `OUTPUT_BACKGROUND`, full rebuilds fill the scene with that solid color, and partial repair clips and fills only the damaged output rectangle before ordinary client redraw. The lower Paper/background surface is still restored through normal renderable-surface ordering.

The EGL scene now begins with `Solid(ServerFrameColor::OutputBackground)`. The output-sized wallpaper cache, gradient generation, wallpaper texture resource, resize upload path, and wallpaper draw layer were removed. Generic layer-shell semantics, buffer age, damage history, render-ahead, fullscreen handling, Direct Scanout, and presentation history were not changed.

## Root cause and architecture

The previous compositor fallback combined gradient constants and interpolation helpers with a CPU output-sized cache and an EGL output-sized wallpaper resource. That duplicated output-sized work even though Paper is the artistic wallpaper owner. The new neutral fallback is a bounded solid base; Paper remains an ordinary client surface and Typhon does not read Paper state or special-case its namespace.

The new `ServerFrameColor::OutputBackground` uses the existing solid-resource architecture. Its 1x1 resource is the first EGL scene command and is stretched by the existing geometry path. CPU partial repair uses the same output color only in clipped damaged regions, preserving client redraw and lower-surface restoration.

## Changed files

- `src/compositor/render.rs`: neutral CPU base, clipped partial repair, removal of gradient/cache/draw path, renderer regressions.
- `src/compositor/mod.rs`: removal of the obsolete wallpaper renderer export.
- `src/egl_renderer/geometry.rs`: removal of the wallpaper draw-layer variant.
- `src/egl_renderer.rs`: dedicated solid output-background command/resource path and structural regression.
- `src/compositor/tests/layer_shell.rs`: inclusion of the split regression module.
- `src/compositor/tests/layer_shell_full_output.rs`: generic layer-shell creation-order and resize regression.
- `docs/superpowers/specs/2026-08-20-neutral-output-background-design.md`: design/evidence record.
- `docs/superpowers/plans/2026-08-20-neutral-output-background-closure.md`: implementation plan.

## Verification

Focused locked suites passed during the closure pass:

- `cargo test --locked compositor::render --lib` — **52 passed**.
- `cargo test --locked --bin oblivion-one egl_renderer` — **74 passed**.
- `cargo test --locked --bin oblivion-one damage` — **66 passed**.
- `cargo test --locked layer_shell --lib` — **51 passed**.
- Full-output layer-shell regression — **1 passed**, covering both creation orders, positive Topbar/Dock reservations, and resize.
- `cargo test --locked --bin oblivion-one fullscreen_frame_scene` — **3 passed**.
- `cargo test --locked --bin oblivion-one scanout` — **196 passed**.
- `cargo check --locked --all-targets` — passed.

The forbidden renderer-symbol search found no legacy gradient/wallpaper symbols in the compositor/EGL renderer. Remaining matches are external `astreactl` wallpaper timeout/command concepts and remain intentionally unchanged.

## Repository-baseline limitations

The full binary suite reached **922 passed, 1 failed, 2 ignored**. The failure was the pre-existing native-input resize test at `src/native_output/tests/input.rs:1502`, ending in a spawned-thread `RecvError` and an expected cursor count of 1 versus 0.

The full library suite reached **1690 passed, 35 failed, 2 ignored**. The failures were existing `astreactl::discovery`/Xwayland environment tests caused by `InvalidInput: path must be shorter than SUN_LEN`, followed by poisoned locks.

`cargo clippy --locked --all-targets -- -D warnings` reported four existing issues outside this closure: mutable key type (`src/compositor/state/frame_callbacks.rs:28`), large enum variant (`src/xwayland/xwm/event_types.rs:22`), unnecessary cast (`src/compositor/state/task_05_8_tests.rs:134`), and single-element loop (`src/xwayland/xwm/events.rs:1416`).

The source-layout check no longer reports `src/compositor/tests/layer_shell.rs`; it still reports pre-existing oversized files: `src/compositor/tests/windows.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/state/windows.rs`, `src/compositor/server.rs`, `src/compositor/mod.rs`, and `src/xwayland/xwm/events.rs`.

## Qualification limits and final status

No native Astrea session was available. This report makes no native screenshot, scanout, or visual-acceptance claim. Deterministic CPU/EGL/layer-shell closure is covered by the focused tests above; native qualification remains pending.

Both repositories contained substantial unrelated dirty changes before this task; they were preserved. No commit or staging operation was requested or performed.
