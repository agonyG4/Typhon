# Oblivion One Nested Lab Design

## Goal

Create a safe realtime lab for the future Astrea compositor, named Oblivion One.
The first deliverable must run inside the current Hyprland session and avoid
installing or selecting an SDDM session.

## Scope

This slice provides a native Rust visual prototype, a CLI wrapper around the
available nested backend `gamescope`, and a session environment file that later
commands can reuse. It is a testing scaffold for compositor work, not the
compositor core itself.

## Commands

- `oblivion-one doctor` reports whether `gamescope`, `kitty`, Rust, and DBus
  helpers are present.
- `oblivion-one smoke` prints a non-GUI launch preview.
- `oblivion-one prototype` opens a native visual shell mockup with topbar, dock,
  workspaces, simulated app windows, focus cycling, click activation, and real
  app launchers.
- `oblivion-one nested` starts the nested lab and writes
  `~/.local/state/oblivion-one/session.env`.
- `oblivion-one run <command>` launches another app into the active nested
  session.
- `oblivion-one env` prints shell exports for manual debugging.

## Safety Boundary

The lab does not write `/usr/share/wayland-sessions`, does not modify SDDM, and
does not take over DRM/libinput. Those steps belong to later TTY work after the
nested loop is useful.

## Validation

Use `cargo test`, `cargo fmt -- --check`, `cargo check`, a short `timeout`
prototype smoke, and the CLI `doctor`, `smoke`, and `prototype` commands before
trying the GUI nested run.
