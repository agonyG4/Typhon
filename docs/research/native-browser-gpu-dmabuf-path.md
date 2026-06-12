# Native Browser GPU and DMABUF Path

Source snapshot: 2026-06-11. This note covers Oblivion One's owned compositor
browser path for Zen/Firefox/Gecko and Brave/Chromium, with emphasis on the
native KMS backend.

## Current Verdict

The native session is not exposing real browser GPU composition yet. It can use
GBM/KMS pageflips for scanout, but the compositor scene is still rendered into a
CPU `Vec<u32>`, copied into an XRGB staging buffer, written into a writable GBM
scanout BO, and then pageflipped. That is why the native path intentionally
prints:

```text
gpu buffer protocols: disabled for native CPU scanout
```

and binds the Wayland server with GPU buffer globals disabled.

This is the right guard for the current renderer. `zwp_linux_dmabuf_v1`,
`wl_drm`, and `wp_linux_drm_syncobj_manager_v1` exist in the compositor, but if
native advertised them today, clients could commit dmabuf buffers that the
native CPU renderer cannot blit. `RenderableSurface::cpu_pixels()` is empty for
dmabuf-backed surfaces, and `DesktopSceneRenderer` skips surfaces without CPU
pixels. The likely failure mode would be invisible or stale client content, not
GPU acceleration.

## Current App Spawn Policy

Common app environment, from `src/launch_env.rs`:

- apps receive `WAYLAND_DISPLAY=<oblivion socket>`;
- host `DISPLAY` is removed unless an Oblivion-owned XWayland bridge is
  explicitly selected;
- GTK/Qt/SDL/Clutter are biased to Wayland;
- `MOZ_ENABLE_WAYLAND=1` and `ELECTRON_OZONE_PLATFORM_HINT=wayland` are set;
- host activation, portal, accessibility, GVFS/FUSE, and LSFG routes are
  stripped or disabled with `GTK_USE_PORTAL=0`, `GIO_USE_PORTALS=0`,
  `QT_NO_USE_PORTAL=1`, `GTK_A11Y=none`, `NO_AT_BRIDGE=1`,
  `GIO_USE_VFS=local`, `GVFS_DISABLE_FUSE=1`, and `DISABLE_LSFG=1`.

There are two app GPU policies:

| Policy | Used by | Effect |
| --- | --- | --- |
| `Accelerated` | nested output and generic `spawn_compositor_app` | Wayland-only environment, isolated browser profiles, no software-rendering env guards. |
| `CpuOnly` | native output and native Spotlight | Adds CPU/software env and Chromium GPU-disabling switches. |

The native path calls `spawn_cpu_compositor_app()` from `src/native_output.rs`,
so every browser launched from native Spotlight is currently tagged in perf logs
as `app_policy=cpu-compositor`.

### Gecko: Zen, Firefox, LibreWolf, Floorp, Waterfox

The Gecko path is light on command-line flags:

- `--no-remote`
- `--profile ~/.local/state/oblivion-one/app-profiles/<browser>`
- `MOZ_ENABLE_WAYLAND=1`

Under native CPU policy it additionally inherits:

- `OBLIVION_ONE_CPU_COMPOSITION=1`
- `MOZ_WEBRENDER_SOFTWARE=1`
- `LIBGL_ALWAYS_SOFTWARE=1`
- `WEBKIT_DISABLE_DMABUF_RENDERER=1`
- `GSK_RENDERER=cairo`

So Zen/Firefox are explicitly kept on the Wayland socket, but WebRender is
forced to software in native. The current `surfaces=0` value on
`app.first_toplevel` is a timing observation rather than proof that the browser
never drew: the latest log shows Zen creating the first toplevel with
`surfaces=0`, then the next repaint moves to `surfaces=2` and starts full
CPU-frame rendering.

### Chromium: Brave, Chromium, Chrome, Vivaldi, Edge

The accelerated Chromium path currently adds:

- `--user-data-dir=~/.local/state/oblivion-one/app-profiles/<browser>`
- `--ozone-platform=wayland`
- `--enable-features=UseOzonePlatform`
- `--use-gl=egl-angle`
- `--use-angle=opengles`
- `--disable-features=Vulkan`
- `--disable-vulkan`

The native CPU path replaces those GL choices with:

- `--disable-gpu`
- `--disable-gpu-compositing`
- `--disable-gpu-rasterization`
- `--disable-zero-copy`
- `--disable-features=Vulkan,DefaultANGLEVulkan,VizDisplayCompositor`
- `--disable-vulkan`

That is conservative and explains why real Chromium GPU acceleration is not
expected in native today. Earlier logs, before the current native guard was
tightened, showed repeated Brave GPU-process initialization failures. The
current CPU path avoids that class of failure by not asking Chromium to keep a
GPU compositor alive while Oblivion cannot import/render its dmabufs natively.

## Protocols That Already Exist

The compositor plan includes these browser-relevant globals:

- `zwp_linux_dmabuf_v1`
- `wl_drm`
- `wp_linux_drm_syncobj_manager_v1`
- `wp_presentation`
- `wp_viewporter`
- `wp_fractional_scale_manager_v1`
- `wl_subcompositor`
- `wl_data_device_manager`

`OwnCompositorServer::bind()` enables GPU buffer globals. The native path uses
`OwnCompositorServer::bind_cpu_composition()`, which passes
`gpu_buffers_enabled=false`; `src/main.rs` also filters those protocol names
from the native printed list. This is the native-only kill switch.

The dmabuf implementation is more than a registry stub:

- `zwp_linux_dmabuf_v1` v3/v4 is implemented;
- v3 clients receive `format`/`modifier` events;
- v4 clients can request default and surface feedback;
- feedback includes a format table, main device, target device, tranche formats,
  and `done`;
- `wl_drm` compatibility can send device, capabilities, formats, and accept
  authenticated prime buffers;
- `create` and `create_immed` produce `wl_buffer` resources containing
  `DmabufBufferHandle`;
- committed surfaces preserve `ShmSnapshot` and `DmabufHandle` as different
  buffer types;
- explicit sync protocol state can import timelines, set acquire/release
  points, defer unsignaled commits, and signal release points after presentation.

The missing piece is not basic protocol parsing. The missing piece is a native
renderer that can import those handles and present them on the KMS path.

## Nested GPU Path Versus Native CPU Path

Nested output has the real EGL/GLES import path:

- `EglGlesFrameRenderer` creates a Wayland EGL window;
- it queries EGL dmabuf formats/modifiers;
- it detects the EGL main DRM/render device;
- it sends renderer-derived dmabuf feedback into `OwnCompositorServer`;
- it imports dmabuf surfaces through `EGL_LINUX_DMA_BUF_EXT`;
- it binds EGLImages with `glEGLImageTargetTexture2DOES`;
- it can use `EGL_KHR/EXT_swap_buffers_with_damage` for output damage.

The NVIDIA-specific feedback handling is also already in the nested EGL code:
when block-linear NVIDIA modifiers are detected, the feedback path keeps usable
tranche formats and appends known unindexed NVIDIA table entries. That is much
closer to what Brave/Chromium need on NVIDIA than a hand-written linear-only
format list.

Native output is different:

- it opens KMS through libseat when possible;
- it selects a DRM mode, currently `1920x1080@165` in the latest TTY run;
- it creates writable `XRGB8888` GBM scanout buffers with
  `SCANOUT | WRITE | LINEAR`;
- `NativeFrameRenderer` composes the entire desktop through the CPU scene
  renderer;
- GBM scanout copies ARGB words into an XRGB staging buffer and calls
  `gbm_bo.write()`;
- dumb framebuffer fallback mmap-copies the same CPU-rendered frame;
- dmabuf-backed client surfaces have no CPU pixels, so the native CPU renderer
  cannot draw them.

This is why the latest log can show a KMS/GBM/NVIDIA scanout backend and still
not mean browser GPU acceleration:

```text
native scanout: GBM write/pageflip buffers ready: 1920x1080, 3 buffer(s), backend nvidia
perf app.spawn ... app_policy=cpu-compositor
perf native.frame ... bytes=8294400 ... surfaces=2
```

The GPU is used as a scanout/display mechanism. Client composition is still CPU.

## Gecko Versus Chromium Impact

Gecko currently has fewer compositor-forced command-line knobs. It is mostly
controlled by environment and by the Wayland globals it sees. In native today,
`MOZ_WEBRENDER_SOFTWARE=1` and `LIBGL_ALWAYS_SOFTWARE=1` are the direct
software forcing points. Once native can safely expose dmabuf and import it,
Gecko should be allowed to use normal Wayland/WebRender behavior first, with
diagnostic prefs such as `widget.dmabuf.force-enabled` reserved for manual
experiments.

Chromium is more sensitive to launch switches and advertised buffer support.
The accelerated path still disables Vulkan and nudges ANGLE toward GLES, while
the CPU path fully disables GPU compositing, GPU rasterization, zero-copy, and
Viz display compositor. On NVIDIA, Chromium/Brave should not be given optimistic
dmabuf globals unless native feedback is derived from the actual EGL/GBM render
device; incorrect modifiers or incomplete import support can cause GPU-process
restart loops or black client content.

Practical difference:

- Gecko can probably be moved from native CPU to native accelerated by removing
  the software env guards after dmabuf/native EGL import is real.
- Chromium needs both the env/args relaxation and trustworthy dmabuf feedback,
  because its GPU process is more likely to fail loudly when GL/Vulkan/ANGLE
  selection and compositor capabilities disagree.

## Current Bottlenecks

1. Native app launches use `spawn_cpu_compositor_app()`.

   This forces CPU/software env and, for Chromium, disables GPU paths at the
   command-line level.

2. Native server binding disables GPU buffer protocols.

   `zwp_linux_dmabuf_v1`, `wl_drm`, and `wp_linux_drm_syncobj_manager_v1` are
   intentionally hidden because the native renderer cannot import/render those
   buffers.

3. Native rendering is CPU-to-scanout, not EGL-to-GBM.

   Every repaint still creates a full CPU frame, copies it, writes about
   `8.29 MB` at 1920x1080, and schedules a pageflip. That cost is visible in
   `paint_us`, `copy_us`, and `write_us`.

4. Dmabuf surfaces cannot be drawn by the CPU scene renderer.

   The compositor correctly preserves dmabuf handles without CPU pixels. That is
   good for zero-copy, but it makes the current native CPU renderer unable to
   display them.

5. Explicit sync is protocol-level only for this path.

   Syncobj acquire/release is parsed and tested, but native presentation is
   still coordinated through `present_frame()` and pageflip bookkeeping rather
   than a renderer-owned GPU fence and vblank completion model.

6. Portal and ScreenSaver noise is real but secondary.

   Logs show xdg-desktop-portal activation, document portal FUSE warnings, and
   repeated `org.freedesktop.ScreenSaver` warnings. These add session noise, but
   they are not the reason browser GPU acceleration is disabled.

## Gradual Enablement Proposal

### Phase 0: Keep the Current Guard

Keep native CPU mode as the default until the renderer can import dmabufs. The
current guard prevents clients from choosing buffers that native cannot draw.

Useful invariant: if native uses `NativeFrameRenderer`, keep
`gpu_buffers_enabled=false` and keep `spawn_cpu_compositor_app()`.

### Phase 1: Add a Native EGL/GLES Render Target Behind a Flag

Introduce a native GPU render path separate from the nested Wayland EGL window.
It should render into GBM-backed targets suitable for KMS pageflip, not into the
current CPU staging buffer.

The first flag should be explicit, for example:

```sh
OBLIVION_ONE_NATIVE_RENDERER=egl-gles
```

Acceptance for this phase:

- render wallpaper/shell/client `wl_shm` surfaces through GLES;
- pageflip rendered GBM buffers;
- preserve hardware cursor behavior;
- retain CPU scanout fallback by default.

### Phase 2: Feed Native Renderer Capabilities Into the Server

Only after the native EGL renderer is alive, query:

- supported dmabuf formats;
- supported modifiers;
- main render device path/rdev;
- `GL_OES_EGL_image` availability;
- swap-with-damage availability;
- whether an internal dmabuf import smoke succeeds.

Then call `set_dmabuf_feedback()` with native renderer data before launching
apps. Do not reuse static linear-only feedback for NVIDIA browser tests.

### Phase 3: Enable `zwp_linux_dmabuf_v1` and `wl_drm` for Native GPU Mode

Make GPU buffer globals conditional on the native renderer, not on the output
backend alone.

Suggested rule:

- native CPU renderer: no dmabuf, no wl_drm, no syncobj global;
- native EGL renderer with import support: dmabuf + wl_drm enabled;
- native EGL renderer with real acquire/release integration: syncobj enabled.

If import fails for a submitted format/modifier, prefer failing the dmabuf
create path or refusing unsupported feedback over accepting a buffer that later
renders black.

### Phase 4: Relax Browser CPU Guards

Switch native GPU mode from `spawn_cpu_compositor_app()` to
`spawn_compositor_app()`.

For Gecko:

- remove `MOZ_WEBRENDER_SOFTWARE=1`;
- remove `LIBGL_ALWAYS_SOFTWARE=1`;
- keep `MOZ_ENABLE_WAYLAND=1`;
- keep isolated profiles.

For Chromium:

- remove `--disable-gpu`;
- remove `--disable-gpu-compositing`;
- remove `--disable-gpu-rasterization`;
- remove `--disable-zero-copy`;
- keep `--ozone-platform=wayland`;
- initially keep Vulkan disabled and ANGLE/GLES forced if that is the safer
  NVIDIA bring-up path;
- later test Brave/Chromium with fewer compositor-injected GL flags, because
  production compositors usually expose capabilities and let Chromium choose.

### Phase 5: Wire Sync and Presentation to Real Completion

Enable `wp_linux_drm_syncobj_manager_v1` only when acquire/release points are
ordered around actual GPU import/render and KMS presentation. The current code
can defer commits until acquire is signaled and signal release later, but native
GPU mode should eventually tie release and frame callbacks to renderer/pageflip
completion rather than CPU paint completion.

### Phase 6: Optimize After Correctness

After browsers are using accelerated buffers safely:

- reduce full-output repaint/write work;
- use damage-aware GBM/EGL rendering;
- move presentation feedback to pageflip/vblank completion;
- keep hardware cursor as the default path;
- add direct scanout eligibility only after normal compositing is correct.

## Manual Test Plan

### Current Native CPU Baseline

Run from a TTY or SDDM path:

```sh
OBLIVION_ONE_OUTPUT=native \
OBLIVION_ONE_PROFILE=release \
OBLIVION_ONE_PERF_LOG=1 \
OBLIVION_ONE_MODE=1920x1080@165 \
./bin/start-oblivion-one-tty
```

Expected today:

- log contains `gpu buffer protocols: disabled for native CPU scanout`;
- launched browser logs `app_policy=cpu-compositor`;
- native frames show `scanout="GBM/KMS pageflip"` and `bytes=8294400`;
- first Zen toplevel may show `surfaces=0`, followed by later frames with
  `surfaces=2`;
- Brave/Chromium should not be judged accelerated while CPU policy is active.

### Nested EGL/DMABUF Sanity

From an existing graphical session:

```sh
./target/release/oblivion-one compositor --renderer=gpu --output=nested -- wayland-info
```

Expected:

- output renderer is `gpu`;
- registry exposes `zwp_linux_dmabuf_v1`, `wl_drm`, and possibly syncobj if the
  device supports it;
- logs include EGL/GLES renderer details and, where available, EGL dmabuf main
  device information.

### Native GPU Bring-Up Smoke

After adding a native EGL renderer flag:

```sh
OBLIVION_ONE_NATIVE_RENDERER=egl-gles \
OBLIVION_ONE_OUTPUT=native \
OBLIVION_ONE_PROFILE=release \
OBLIVION_ONE_PERF_LOG=1 \
./bin/start-oblivion-one-tty -- wayland-info
```

Expected before browsers:

- `wayland-info` sees dmabuf only in native EGL mode;
- perf logs no longer show a full CPU `gbm_bo.write()` as the primary render
  path;
- `wl_shm` clients still render;
- exiting the session does not leave KMS state broken.

### Gecko Browser Check

Launch Zen/Firefox in native GPU mode:

```sh
MOZ_LOG=Widget:5,WidgetPopup:5 \
MOZ_LOG_FILE=/tmp/oblivion-gecko-widget.log \
./bin/start-oblivion-one-tty -- /opt/zen-browser-bin/zen-bin
```

Check:

- `about:support` reports Wayland window protocol;
- WebRender is not forced to software by Oblivion env;
- browser surfaces render after first toplevel;
- no persistent `GFX1` protocol errors before compositor shutdown;
- frame timings improve without full-frame CPU write cost.

### Chromium Browser Check

Launch Brave/Chromium in native GPU mode with conservative EGL/GLES flags first:

```sh
./bin/start-oblivion-one-tty -- brave --enable-logging=stderr --vmodule='*wayland*=2,*gpu*=2'
```

Check:

- `chrome://gpu` does not report global software-only compositing caused by
  Oblivion flags;
- GPU process stays alive;
- no repeated `viz_main_impl.cc` GPU-process exits;
- NVIDIA modifiers in feedback match the renderer query;
- zero-copy/GPU raster flags are not disabled by the native launcher once the
  renderer is ready.

### Regression Checks

- `cargo test` for protocol and launch-env contracts.
- `wayland-info` against nested and native modes.
- `OBLIVION_ONE_DRY_RUN=1 ./bin/start-oblivion-one` to verify launcher output
  selection and environment behavior.
- Native session exit through `Alt+P`, confirming Gecko/GTK broken-pipe logs
  only happen after compositor shutdown.

## Do Not Do

- Do not enable dmabuf globals in native while `NativeFrameRenderer` is still
  the compositor renderer.
- Do not advertise hard-coded linear-only feedback as the NVIDIA browser path.
- Do not force browser GPU flags as a substitute for compositor buffer import.
- Do not remove the CPU policy until there is an explicit fallback switch for
  debugging and for machines where EGL/GBM import fails.
- Do not treat portal or ScreenSaver warnings as the core GPU blocker.

## Files To Read Before Implementing

- `src/launch_env.rs`
- `src/main.rs`
- `src/native_output.rs`
- `src/nested_renderer.rs`
- `src/egl_renderer.rs`
- `src/egl_renderer/dmabuf.rs`
- `src/compositor/server.rs`
- `src/compositor/dmabuf.rs`
- `src/compositor/protocols/buffers.rs`
- `src/compositor/protocols/globals.rs`
- `src/compositor/protocols/syncobj.rs`
- `src/compositor/render.rs`
- `src/compositor/surface.rs`
- `src/render_backend/`
- `docs/research/native-session-log-analysis-2026-06-11.md`
- `docs/NATIVE_SESSION.md`
