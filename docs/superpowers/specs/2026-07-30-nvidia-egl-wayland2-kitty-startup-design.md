# NVIDIA egl-wayland2 Kitty Startup Design

## Problem

On NVIDIA 610.43.03 with `egl-wayland2` 1.0.1, a native Kitty client aborts
inside `libnvidia-egl-wayland2.so.1` while creating its first EGL window when
Typhon advertises the render node as the DMA-BUF main device and the primary
node as the scanout tranche target. The two nodes belong to the same physical
GPU but have different `dev_t` values.

Typhon already has a guarded same-device normalization policy that prevents
the crash. Live A/B qualification proved that `off` reproduces SIGABRT while
`auto` lets Kitty create a toplevel and submit buffers. The policy nevertheless
defaults to `off`, so the normal SDDM session does not benefit from it.

The dedicated qualification helper also reports a false failure because it
waits only for `app.first_toplevel`, while its externally launched client emits
`app.toplevel`.

## Design

`NvidiaEglWayland2CompatPolicy::from_env` will default to `Auto`. Explicit
`off`, `auto`, and `force` values retain their current meaning, so operators
keep a deterministic rollback. The existing resolver remains the safety
boundary: automatic normalization is effective only for an NVIDIA EGL vendor,
DMA-BUF feedback version 4 or newer, a differing scanout target, and primary
and render nodes proven to share one physical GPU.

The normal launcher documentation will describe `auto` as the default and
`off` as the rollback. No session-specific environment override is needed;
the behavior belongs to the GPU feedback policy and therefore applies
consistently to SDDM, TTY, and other native launch paths.

The qualification helper will accept either `app.first_toplevel` or
`app.toplevel app_id=kitty` as proof that its client created a toplevel. It
will keep the liveness and coredump checks and will wait for the client
observation window before terminating the compositor.

## Testing

1. A Rust unit test will remove the environment variable and assert that
   `from_env()` returns `Auto`.
2. Existing parsing and topology tests will continue covering explicit
   `off`, `auto`, `force`, and rejection on unsafe device topology.
3. A launcher test will assert that help text documents `auto` as the default.
4. A shell-level qualification regression will feed representative log lines
   to the helper's toplevel detector, covering both brokered and external
   clients.
5. The release build and a native A/B run will verify:
   - explicit `off`: Kitty aborts in `libnvidia-egl-wayland2`;
   - default/`auto`: Kitty creates a toplevel without a coredump.

## Scope

This change does not disable explicit synchronization, replace
`egl-wayland2`, broaden direct-scanout eligibility, or alter multi-GPU
feedback. It does not change unrelated Direct Scanout 2.0 worktree content.
