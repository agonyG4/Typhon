# KMS Commit Worker Stage 3 Qualification

- Commit: `3fcb18f0c004197ba6baa5d875bd02bba0dbe53c`
- Date: 2026-07-26
- GPU and driver: `01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GA104 [GeForce RTX 3060 Ti Lite Hash Rate] [10de:2489] (rev a1); Kernel driver in use: nvidia`
- Connected DRM path: `/sys/class/drm/card1-DP-1`
- Advertised mode: `1920x1080`
- Worker policy: `force` (not launched)
- Direct scanout policy: `off` (not launched)

## Gate status

The software gate passed: `cargo fmt --check`, `cargo test --locked` (536
passed, 1 ignored), `cargo check --locked --all-targets`,
`cargo clippy --locked --all-targets -- -D warnings`, `cargo build --locked
--release`, `./bin/check-source-layout`, `git diff --check`, and
`bash -n bin/qualify-kms-worker` all exited successfully.

The native gate failed before launch. `tty` reported `not a tty`, and
`loginctl show-session self ...` reported that the caller does not belong to
any known session. The dry-run launcher check was successful, but it does not
acquire a seat, DRM device, or pageflip stream. No real TTY launch was
attempted from this process.

## Application matrix

| Application | Fullscreen | Alt+Tab | Cursor | Overlays | Result |
|---|---|---|---|---|---|
| Palworld | FAIL | FAIL | FAIL | FAIL | FAIL |
| Steam UI, context menus, and popups | FAIL | FAIL | FAIL | FAIL | FAIL |
| Firefox browsing and fullscreen video | FAIL | FAIL | FAIL | FAIL | FAIL |
| Kitty typing, moving, and resizing | FAIL | FAIL | FAIL | FAIL | FAIL |
| one additional Vulkan game | FAIL | FAIL | FAIL | FAIL | FAIL |

The matrix was not run because the required native TTY/DRM session was
unavailable. Each `FAIL` records the unmet qualification requirement, not an
observed application defect.

## Shutdown and recovery

- shutdown while a game is presenting: FAIL
- shutdown while a cursor update is queued: FAIL
- VT switch away and back: FAIL
- session suspend and resume: FAIL
- restart Typhon after the previous cases: FAIL

These cases were not run because the native session could not be launched.

## Qualification counters

No valid worker perf log was produced. `bin/qualify-kms-worker` was not run
against synthetic, empty, or unrelated session data, so no worker counter is
claimed or copied as a hardware result.

Required counters: not measured.

## Verdict

FAIL

Per the implementation plan, the Direct Scanout 2.0 plan stops here. Task 1
and all later tasks were not started.
