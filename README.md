# Oblivion One

Oblivion One is the Rust lab for a first-party Wayland compositor, window
manager, and desktop environment.

The main path is now our own compositor. Hyprland and Gamescope are kept only as
legacy lab backends while the owned Wayland server grows.

## Quick Start

```sh
./bin/oblivion-one doctor
./bin/oblivion-one compositor --check
```

`compositor --check` binds an Oblivion-owned Wayland socket and exits. It does
not call Hyprland, Gamescope, or any external compositor.

To keep the server running for early client experiments:

```sh
./bin/oblivion-one compositor
```

That opens a nested Oblivion output window. It is still running inside your
current desktop session, but the pixels in that window come from the Oblivion
renderer: a simple procedural wallpaper, the Oblivion-rendered mouse cursor, and
any committed client surfaces. Clients launched through the command below target
the Oblivion Wayland socket, not Hyprland.

The nested output uses the native EGL/GLES GPU renderer by default. The
EGL/GLES path uses a Wayland `wl_egl_window`, composites wallpaper, cursor, and
client surfaces as GL textures, uploads only damaged `wl_shm` rectangles on
same-size commits, and imports `linux-dmabuf` buffers as EGLImages when clients
send GPU buffers. CPU rendering remains available only as a fallback/debug path:

```sh
./bin/oblivion-one compositor --renderer=gpu
./bin/oblivion-one compositor --renderer=cpu
./bin/oblivion-one compositor --width 1920 --height 1080 --refresh 165 -- zen-browser
```

On `oblivion-one compositor --output nested`, `--width` and `--height` are the
initial logical nested-output size and `--refresh` is the requested nested
output refresh advertised to Wayland clients. The nested window remains
host-paced: requesting `165` Hz advertises `165000` mHz and uses a 6.06 ms
active scheduling interval, but physical presentation is still limited by the
host compositor and the monitor where the window lives.

To launch a client against the Oblivion-owned socket:

```sh
./bin/oblivion-one compositor -- wayland-info
./bin/oblivion-one compositor -- kitty --single-instance=no --session=none --class OblivionOneKitty
```

That command keeps the compositor alive and spawns `wayland-info` with
`WAYLAND_DISPLAY` pointing at the Oblivion socket. It is the current best smoke
test because it proves a real Wayland client can see our registry, including the
core output and seat globals that real toolkits expect.

For the current rendering milestone, the automated tests create a real Wayland
client, attach a `wl_shm` buffer to a `wl_surface`, commit it, receive paced
`wl_surface.frame.done`, and verify the compositor has a renderable pixel
snapshot. `kitty`, `nautilus`, and `brave` can connect to the Oblivion socket,
create real xdg toplevels, and commit buffers into the owned compositor path.
The nested output now forwards basic keyboard and pointer events to the focused
xdg surface.

The owned compositor app launcher is Wayland-only by default. It removes the
host `DISPLAY`, sets `WAYLAND_DISPLAY` to the Oblivion socket, and treats
fallback to the host X11/XCB session as a bug. Future X11 compatibility will go
through an Oblivion-owned XWayland bridge, not the host display. The initial
architecture has a separate rootless XWayland launch plan with compositor-owned
`-listenfd`, `-wm`, and `-displayfd` file descriptors. Runtime XWayland app
hosting still needs an XWM implementation before it should be enabled for real
apps.

The first real milestone is:

```text
owned Wayland socket -> accept clients -> xdg-shell surfaces -> render real buffers -> basic input
```

## Architecture

```text
src/core/        shared geometry, paths, and platform helpers
src/render_backend/ renderer capability contract, buffer model, and GPU-real backend roadmap
src/compositor/  owned Wayland display/socket, protocol state, and buffer lifecycle
  plan.rs        public compositor architecture/protocol plan
  surface.rs     renderable surface, damage, and placement model
  interaction.rs floating-window hit-testing and move/resize geometry
  render.rs      desktop scene composition helpers and CPU render fallback
src/wm/          focus, floating placement, move, resize, maximize, close policy
src/shell/       dock/topbar/launcher/settings surfaces, mostly deferred
src/session/     nested first, then TTY/SDDM lifecycle
src/xwayland.rs  future isolated XWayland launch and XWM boundary
```

Topbar, dock polish, and full DE surfaces are intentionally later. We first make
apps run inside our compositor.

## Commands

- `compositor`: starts the Oblivion-owned Wayland server path.
- `compositor --check`: binds the Wayland socket and exits for validation.
- `doctor`: checks local tools useful for the lab.
- `bin/install-start-oblivion-one --sddm-session`: installs the experimental
  native SDDM Wayland session entry.
- `prototype`: opens the native Rust visual shell prototype.
- `de --backend oblivion`: alias path for the owned compositor backend.
- `de --backend hyprland`: legacy nested Hyprland lab backend.
- `nested`: legacy Gamescope lab backend.
- `run`: launches a command into the active legacy nested session env.
- `env`: prints shell exports for the active legacy nested session.

## Current Status

Done:

- Project split into explicit architecture modules.
- Owned Wayland server path using `wayland-server`.
- `compositor --check` validates socket binding without external compositors.
- Real registry globals for `wl_compositor`, `wl_subcompositor`,
  `wl_data_device_manager`, `wl_shm`, `zwp_linux_dmabuf_v1`, `xdg_wm_base`,
  `wl_output`, and `wl_seat`.
- A Wayland client test creates an `xdg_toplevel` inside the Oblivion server.
- `xdg_toplevel.configure` + `xdg_surface.configure` are sent in the correct
  order for real xdg-shell clients.
- Attached `wl_shm` buffers are copied into renderable surface snapshots.
- `wl_surface.frame.done` is sent after commits so clients can continue drawing.
- The compositor opens a nested output window and composites committed snapshots.
- The nested output has a simple desktop wallpaper and uses the host cursor as a
  hardware-cursor-style path, so mouse movement does not force a full desktop
  recomposite.
- The nested output has a native EGL/GLES scene renderer, with automatic fallback
  to the previous CPU `softbuffer` renderer when GPU startup is not strict.
- The GPU renderer is the default compositor renderer. It caches wallpaper,
  cursor fallback, and per-client-surface textures,
  then reuploads only damaged rectangles when a same-size client surface commits
  new `wl_shm` pixels.
- The `egl-gles` renderer uses a Wayland EGL window, keeps scene and cursor
  vertex uploads separate, and imports `linux-dmabuf` client buffers through
  EGLImage/`GL_OES_EGL_image` instead of reading them through the CPU.
- The EGL/GLES renderer detects `EGL_KHR/EXT_swap_buffers_with_damage` and uses
  output damage when available, falling back to normal `eglSwapBuffers` when the
  driver does not expose it.
- Compositor-owned window shadows are intentionally disabled in active render
  paths for now. Interactive resize uses only a neutral preview backdrop and a
  simple outline; full shadows, rounded-corner masking, blur, and server-side
  decoration polish are deferred to the window-decoration milestone.
- Runtime Vulkan/WGPU support was removed from the current product path. The
  renderer architecture still routes through backend capability profiles so a
  future Vulkan backend can be added without moving WM policy into graphics code.
- `render_backend` tracks the official `egl-gles` dmabuf target and keeps
  committed surface buffers typed as `wl_shm` snapshots or `linux-dmabuf`
  handles.
- The EGL/GLES target has a tested dmabuf import contract: it builds
  modifier-aware EGL import attributes without requiring CPU pixels.
- Dmabuf `wl_buffer.release` is delayed until a GPU-imported buffer is replaced
  or the surface is destroyed, so clients cannot recycle the active video buffer
  while Oblivion still samples it.
- Wallpaper rendering is cached per output size, and the nested output redraws
  on resize or client render generation changes instead of recomputing the
  whole scene in an unbounded loop.
- The event loop idles at a lower cadence without mapped client surfaces and
  switches back to the configured refresh-derived cadence while apps,
  interactions, pending frame work, or redraws are active.
- Buffer frame callbacks are completed after the nested output presents a frame,
  which paces clients instead of letting them redraw in a tight loop.
- `compositor -- app args...` launches a process on the Oblivion socket instead
  of the host compositor.
- App launch env is Wayland-only for the owned compositor path; fallback to host
  X11/XCB is treated as a bug during testing.
- X11 compatibility is scoped to a future Oblivion-owned XWayland bridge; the
  current code has the isolated launch/env contract but does not expose host
  `DISPLAY` to compositor-launched apps.
- The nested output forwards basic physical keyboard events and pointer
  enter/motion/button events into the focused xdg surface.
- Basic real-client floating move/resize: hold `Alt` and drag with the left
  mouse button to move a mapped root window, or the right mouse button to resize
  it. Resize visual geometry follows pointer motion immediately while
  `xdg_toplevel.configure` and browser commits catch up asynchronously. Stale
  client content is cropped or left at committed size instead of being stretched.
- Basic real-client window state: xdg-toplevel minimize, maximize, fullscreen,
  and restore are tracked by the compositor and exposed through temporary
  `Alt` keyboard lab shortcuts.
- Basic real-client popups: xdg-popup menus receive configure events and render
  as child surfaces, so browser menus/settings popovers can map inside the
  compositor.
- A temporary compositor-rendered dock shows open real app toplevels. A simple
  Spotlight lab overlay opens with `Super+Space` (`Ctrl+Space` fallback) and launches commands on the
  Oblivion Wayland socket. Browser suggestions such as Brave, Zen Browser,
  Firefox, and Chromium use isolated lab profiles so they do not reuse a host
  browser process.
- Initial `linux-dmabuf` v3 path: Oblivion advertises ARGB/XRGB modifiers,
  keeps async `create` on the safe `failed()` path, accepts `create_immed` into
  typed `DmabufBufferHandle` surfaces, and the native EGL/GLES renderer imports
  those buffers as GPU textures.
- The experimental SDDM launcher can install an `oblivion-one.desktop` Wayland
  session entry, runs native sessions from the release binary by default,
  publishes session activation environment, writes a native session log, and
  uses an ABNT2 XKB keymap by default.
- Legacy Hyprland/Gamescope paths isolated behind explicit backend choices.

Next:

- Add compositor-side titlebars/decorations so move/resize no longer depends on
  the temporary `Alt` drag lab gesture.
- Add optional toolkit protocols only when a real app needs them.
- Expand dmabuf feedback/explicit-sync support beyond the initial import path.

## Prototype Controls

`prototype` is still a visual shell mockup:

If you see the dock and fake windows shown by `prototype`, you are not testing
the real compositor path yet. Use `./bin/oblivion-one compositor` for the owned
Wayland server, or `./bin/oblivion-one compositor -- app args...` for real apps.

Real compositor path:

- Hold `Alt` and drag with the left mouse button to move the real app window.
- Hold `Alt` and drag with the right mouse button to resize the real app window.
- Press `Alt+M` to minimize the focused real app window.
- Press `Alt+R` to restore the next minimized real app window.
- Press `Alt+F` to maximize/restore the focused real app window.
- Press `Alt+Enter` or `Alt+F11` to toggle fullscreen for the focused real app window.
- Press `Super+Space` (`Ctrl+Space` fallback) to open the simple Spotlight,
  type an app/command, use `Up`/`Down` to select, and press `Enter` to launch
  it inside Oblivion.
- Click a dock item to focus or restore that open real app window.

- Click a simulated window to focus it.
- Drag a titlebar to move a simulated window.
- Drag the lower-right corner to resize a simulated window.
- Click the red, yellow, or green window buttons to close, minimize, or maximize.
- Press `Tab` to cycle focus.
- Press arrow keys to move the active simulated window.
- Press `[` or `]` to shrink or grow the active simulated window.
- Press `M` to minimize, `F` to maximize/restore, `R` to restore a minimized window.
- Press `Delete` or `Backspace` to close the active simulated window.
- Press `Esc` or `Q` to close.

## Recommended Local Packages

Useful for the current lab:

```sh
sudo pacman -S kitty brave-bin spotify wayland-utils xorg-xwayland
rustup component add clippy
```

`kitty`, `quickshell`, `brave`, `spotify`, `Xwayland`, `cargo`, `rustc`, `rustup`,
`clippy`, and `dbus-run-session` are already present on this machine.

## Experimental SDDM Session

The SDDM entry is now installable for native-session testing:

```sh
cargo build --release
./bin/install-start-oblivion-one --sddm-session
```

The launcher defaults native sessions to `target/release/oblivion-one`, logs to
`~/.local/state/oblivion-one/session.log`, publishes `WAYLAND_DISPLAY` and
desktop activation variables, and keeps `DISPLAY` unset so X11 apps cannot leak
onto the host session. Use `OBLIVION_ONE_DRY_RUN=1 ./bin/start-oblivion-one` to
inspect the exact command before selecting the SDDM entry. `oblivion-one doctor`
also reports native-session readiness: runtime dir, KMS/render devices,
connected output, seat/libinput/GBM/EGL prerequisites, and whether the current
raw input fallback can open `/dev/input/event*`.

For nested development from an existing desktop, the launcher forwards nested
output sizing directly to the compositor:

```sh
./bin/start-oblivion-one --width 1600 --height 900 --refresh 165 -- zen-browser
```

Those flags configure only the nested host window. Native SDDM/TTY mode
selection remains under `OBLIVION_ONE_MODE`, for example
`OBLIVION_ONE_MODE=1920x1080@165`. Use `hyprctl monitors` on Hyprland hosts to
check the monitor refresh that can physically pace the nested window.

This path is still experimental. The native backend now prefers `libseat` for
both DRM device ownership and libinput keyboard/pointer input, with direct DRM,
direct libinput, and raw evdev kept as fallback/debug paths. In `auto` scanout
mode it now tries `native-egl-gbm` first: the shared EGL/GLES scene renderer
draws into a GBM-backed EGL surface, locks the rendered front buffer, caches a
DRM framebuffer ID for that BO, and presents it through KMS pageflip. Set
`OBLIVION_ONE_SCANOUT_BACKEND=auto` for startup fallback from GPU to CPU/dumb,
`gpu` or `native-egl-gbm` to require the GPU path, `gbm-cpu-write` or `cpu` to
force the GBM CPU-write fallback, and `dumb` for the KMS dumb framebuffer
fallback. Legacy scanout values such as `gbm` and `gbm-egl` still select the
CPU-write fallback for compatibility.

Native app GPU policy is derived after the active scanout backend is known:
`OBLIVION_ONE_NATIVE_APP_GPU=auto` or unset accelerates apps only on
`native-egl-gbm`, `gpu` requires that compatible backend, and `cpu` forces the
software recovery profile. The startup command now runs after the socket,
backend, feedback, initial modeset, and input backend are ready:

```sh
./bin/start-oblivion-one-tty -- kitty
```

Startup fallback is limited to backend creation and initial paint. Runtime GPU
failures are fatal with structured diagnostics and a CPU restart command, for
example `OBLIVION_ONE_SCANOUT_BACKEND=cpu OBLIVION_ONE_NATIVE_APP_GPU=cpu`.
Real TTY/KMS hardware validation is still required before treating the native
session as production-ready.
