# Native Session Log Analysis - 2026-06-11

Source: `~/.local/state/oblivion-one/session.log`, latest TTY run starting at
`2026-06-11T17:13:47-03:00`.

## Baseline

- Session entry: `start-oblivion-one output=native renderer=gpu profile=release socket=oblivion-one-tty`
- KMS mode: `1920x1080@165`, exact policy
- DRM backend: `libseat DRM`
- Scanout backend: `GBM/KMS pageflip`
- GBM backend: `nvidia`
- Frame budget at 165 Hz: about `6060 us`

## App Startup

- Zen spawned in `652 us`; first xdg toplevel appeared after about `1.04 s`.
- Brave spawned in `217 us`; first xdg toplevel appeared after about `379 ms`.
- Brave still logs KDE Wallet and profile-component noise in the isolated
  profile path. That is app/session integration noise, not a compositor crash.
- The `Pipe quebrado` and Gecko `Compositor crashed` lines at the end followed
  `native input exit requested; shutting down cleanly`, so they are currently
  expected client disconnect fallout when the compositor exits through `Alt+P`.

## Performance Counters

From `1537` native frame samples:

- Average paint: `7949 us`
- p95 paint: `14450 us`
- p99 paint: `15780 us`
- Max paint: `67515 us` on the initial frame
- Frames over 165 Hz budget: `587` (`38.2%`)
- Frames over 12 ms: `339` (`22.1%`)
- Average render: `3762 us`
- p95 render: `10159 us`
- Average ARGB/XRGB copy: `1114 us`
- p95 ARGB/XRGB copy: `1560 us`
- Average GBM write: `3072 us`
- p95 GBM write: `3173 us`

## Root Causes

1. The native path is using KMS/GBM pageflip, but composition is still CPU-side.
   Every repaint still produces a full 1920x1080 frame, copies it into a staging
   buffer, writes about `8.3 MB` into the GBM BO, then pageflips.

2. Browser frame callbacks keep the compositor busy while Zen/Brave animate or
   repaint. Those frames frequently land around `13-16 ms`, so the compositor
   behaves closer to 60-75 FPS than 165 FPS during active browser rendering.

3. Pointer motion is already coalesced, but software cursor movement still
   requires repainting and writing a full scanout buffer. This makes the mouse
   feel tied to the CPU scanout path.

4. The log showed repeated `libinput error: client bug: value requested for unset axis`.
   The cause was reading modern libinput scroll axes without checking
   `has_axis` first.

## Changes Landed From This Capture

- Modern libinput scroll events now check `has_axis` before reading an axis
  value, removing the `value requested for unset axis` client-bug path.
- The ARGB-to-XRGB copy path now uses native row copies instead of per-pixel
  masking. XRGB ignores the high byte, so preserving it is valid for scanout and
  should reduce the measured `copy_us` portion.

## Next Optimization Targets

1. Implement the real EGL/GLES native renderer into GBM buffers or an importable
   dmabuf path. This is the main requirement for Hyprland/KWin-class browser
   performance.
2. Move pointer rendering to a hardware cursor plane where possible. Until then,
   mouse movement keeps forcing full-buffer CPU writes.
3. Add damage-aware native scanout writes for the dumb/mapped path and evaluate
   whether a mapped GBM path is faster than `gbm_bo_write` on NVIDIA.
4. Add app/session integration cleanup for portals, wallet fallback, and isolated
   Brave component profile noise after the core compositor frame path is faster.
