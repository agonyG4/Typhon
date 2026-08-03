# AstreaOS Control and Desktop Integration Program Board

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement each implementation plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a safe AstreaOS control plane, runtime cursor configuration, authoritative Typhon window integration for the Dock, and a standalone wallpaper service without coupling desktop presentation to the compositor renderer.

**Architecture:** Typhon remains the authority for compositor state, input cursor ownership, outputs, and windows. Eclipse owns desktop presentation processes such as the Dock and wallpaper. `astreactl` is a small CLI client that talks to versioned local services; it never embeds compositor runtime code.

**Tech Stack:** Rust 2024, Linux Unix sockets and epoll, Wayland private protocols, C++20, Qt 6.6+, QML, LayerShellQt, libwayland-client, CMake 3.24+.

## Global Constraints

- Preserve the current Typhon `main` worktree, which is one commit ahead of its bundled `origin/main`, and do not add `artifacts/` to commits.
- Preserve the current Eclipse `feature/settings-foundation` worktree and every existing modified, deleted, and untracked Settings file.
- Do not copy Hyprland or KWin source code. Reuse architectural ideas only.
- Keep all control sockets under `$XDG_RUNTIME_DIR`; never place control endpoints in `/tmp`.
- Every runtime socket directory must be mode `0700`; every socket must be mode `0600`.
- Authenticate local control clients with `SO_PEERCRED` and require the compositor user's UID.
- Do not block Typhon's compositor thread on socket reads, writes, process execution, image decoding, or filesystem persistence.
- Keep protocol version 1 backward-compatible after release. Add capabilities or a new protocol version instead of changing existing meanings.
- No new general-purpose async runtime is allowed for the first implementation. Use Typhon's existing epoll reactor and Qt's existing event loop.
- Keep image wallpaper loading outside Typhon. Typhon retains its current gradient as a failure-safe background.
- Do not infer Dock state from process existence. Dock state must come from Typhon's authoritative window model.
- A failed cursor or wallpaper update must preserve the last working visual state.
- All new Rust unsafe blocks require a local `// SAFETY:` explanation.
- All plans, protocol descriptions, and Markdown documentation are written in English.

---

## Source Review and Corrections

The reviewed `Typhon(6)` snapshot is newer than the July 13 audit. It already contains a supervised XWayland runtime, XWM integration, X11 scene participation, stable `WindowId` values, and minimize/restore state. The implementation must use that current architecture rather than the audit's obsolete statement that XWayland is absent.

Typhon currently has no runtime control socket. Its primary CLI in `src/main.rs` only handles `help`, `doctor`, `compositor`, and `portal`. The native reactor already has generated `ReactorToken` identities and bounded readiness processing, which is the correct integration point for a nonblocking control server.

The cursor theme is currently loaded once from environment variables. `src/cursor_theme.rs` stores the shared image in a `OnceLock<Arc<CompositorCursorImage>>`, and `NativeRuntime` keeps a fixed `Arc`. Runtime theme changes therefore require an explicit manager and transactional propagation into software and hardware cursor paths.

The Eclipse Dock already exposes `running`, `active`, and `windowCount` roles. Its documentation intentionally leaves them false because Typhon has no public window protocol. This is a prepared extension seam, not a model redesign.

Hyprland's useful ideas are a compact command registry, human and JSON output, instance selection, and a distinct event stream. Its loose text protocol and compositor-thread blocking reads are not adopted.

KWin's useful ideas are per-window protocol objects, stable identities, explicit state events, server-authoritative requests, batched updates, and minimized geometry. Astrea's first protocol remains much smaller than Plasma Window Management.

## Reviewed Decision

Implement the program in this order:

1. **Control Plane Foundation** — versioned Typhon control socket and read-only `astreactl` commands.
2. **Runtime Cursor Control** — transactional cursor theme/size changes through the control plane.
3. **Toplevel Protocol and Dock Integration** — real-time window state and minimize/restore actions.
4. **Wallpaper Service** — `astrea-desktop` on the Layer Shell background layer, with `astreactl` routing.
5. **Hardening and Shell Integration** — event watching, completions, Settings adapters, packaging, and qualification.

This order is deliberate. Cursor management needs a safe command path; Dock integration needs a stable Typhon window API; wallpaper does not need compositor renderer changes and can be developed independently after the control CLI can route to services.

## System Boundary

```text
                        +----------------------+
                        |      astreactl       |
                        | human / JSON output  |
                        +----------+-----------+
                                   |
                    service discovery + NDJSON
                 +-----------------+------------------+
                 |                                    |
      +----------v-----------+              +---------v----------+
      | Typhon control.sock  |              | desktop control.sock|
      | status/cursor/window |              | wallpaper commands  |
      +----------+-----------+              +---------+-----------+
                 |                                    |
      +----------v-----------+              +---------v----------+
      |  Typhon NativeRuntime|              |  astrea-desktop     |
      | cursor/output/window |              | Layer Shell bg      |
      +----------+-----------+              +--------------------+
                 |
        private Wayland protocol
                 |
      +----------v-----------+
      | Eclipse Typhon client|
      +-----+------------+---+
            |            |
        +---v---+    +---v----+
        | Dock  |    | AltTab |
        +-------+    +--------+
```

## Stable Contracts

### Typhon control endpoint

```text
$XDG_RUNTIME_DIR/astrea/typhon/<instance>/control.sock
```

`<instance>` is the Wayland socket name, sanitized to ASCII letters, digits, `.`, `_`, and `-`. The default instance remains `oblivion-one-0` until a separate naming migration is approved.

Request envelope:

```json
{"protocol":"astrea.control","version":1,"id":7,"command":"status","args":{}}
```

Successful response:

```json
{"protocol":"astrea.control","version":1,"id":7,"ok":true,"result":{}}
```

Error response:

```json
{"protocol":"astrea.control","version":1,"id":7,"ok":false,"error":{"code":"invalid_argument","message":"cursor size must be between 16 and 256"}}
```

Initial limits:

- request: 64 KiB
- response: 1 MiB
- simultaneous clients: 32
- requests per connection: 1 in protocol version 1
- accepted/read clients serviced per compositor cycle: 16
- write queue per client: one bounded response

### Cursor configuration

```text
${XDG_CONFIG_HOME:-$HOME/.config}/AstreaOS/input/cursor.json
```

```json
{"schemaVersion":1,"theme":"Bibata-Modern-Ice","size":32}
```

Supported size range is 16 through 256. Images that do not fit the active hardware cursor plane use the existing software cursor path; they do not fail solely because of hardware plane dimensions.

### Wallpaper configuration

```text
${XDG_CONFIG_HOME:-$HOME/.config}/AstreaOS/appearance/wallpaper.json
```

```json
{
  "schemaVersion": 1,
  "default": {"path":"/home/user/Pictures/wallpaper.jpg","mode":"fill"},
  "outputs": {}
}
```

The `outputs` object is included in schema version 1 so multi-output support can be added without replacing the file format. The first implementation applies `default` to every available output.

### Private window protocol

The XML source of truth is:

```text
Typhon/protocols/astrea-toplevel-management-v1.xml
```

The global is available only to authorized Astrea shell clients. The protocol publishes managed XDG and X11 toplevels, not popups, notifications, override-redirect windows, or windows marked `skip_taskbar`.

Stable window identity is Typhon's session-scoped `WindowId`, encoded as a decimal string. PID is metadata, never identity.

## Milestone Board

| Milestone | Repository | Depends on | Deliverable | Gate |
|---|---|---|---|---|
| M1 Control primitives | Typhon | none | bounded v1 codec and errors | codec tests pass |
| M2 Native control server | Typhon | M1 | nonblocking authenticated socket | reactor and socket stress tests pass |
| M3 Read-only astreactl | Typhon | M2 | status/version/doctor/windows/outputs | human and JSON golden tests pass |
| M4 Cursor manager | Typhon | M3 | runtime theme/size apply and persistence | software/atomic/legacy regressions pass |
| M5 Window action API | Typhon | M3 | ID-based activate/minimize/restore/close | XDG and X11 tests pass |
| M6 Toplevel protocol | Typhon | M5 | authoritative window enumeration and deltas | protocol lifecycle tests pass |
| M7 Shared Typhon client | Eclipse | M6 | reusable C++ protocol client | fake-server tests pass |
| M8 Dock integration | Eclipse | M7 | real indicators and click policy | controller/model/QML tests pass |
| M9 astrea-desktop | Eclipse | M3 | standalone background layer service | config/image/surface tests pass |
| M10 Unified astreactl routing | Typhon + Eclipse | M9 | wallpaper commands and service discovery | route/error tests pass |
| M11 Qualification | both | M1-M10 | real TTY session acceptance | 100-cycle soak passes |

## User-Visible Command Surface

```bash
astreactl status
astreactl version
astreactl doctor
astreactl outputs
astreactl windows
astreactl active-window
astreactl cursor get
astreactl cursor set --theme Bibata-Modern-Ice --size 32
astreactl cursor reload
astreactl window activate 42
astreactl window minimize 42
astreactl window restore 42
astreactl window close 42
astreactl wallpaper get
astreactl wallpaper set /home/user/Pictures/space.jpg --mode fill
astreactl wallpaper reload
```

Every query supports `--json`. Every command supports `--instance <wayland-socket-name>`. `wallpaper` subcommands route to `astrea-desktop`; they do not enter Typhon's renderer.

## Dock Click Policy

For a pinned application:

1. No matching windows: launch the desktop file.
2. One minimized window: restore and activate it.
3. One visible inactive window: activate it.
4. One visible active window: minimize it.
5. Multiple windows: activate the most recently focused non-minimized window; if all are minimized, restore the most recently focused one.

The Dock model resolves Typhon `app_id` and X11 startup class through `DesktopEntryCatalog`. Exact desktop filename and desktop ID matches win before `StartupWMClass`. Unresolved windows remain available to Alt+Tab but do not attach to an unrelated Dock pin.

## Non-Goals for This Program

- No image wallpaper decoding inside Typhon.
- No DBus control API in version 1.
- No network-accessible control endpoint.
- No scripting language embedded in the compositor.
- No workspaces or multi-output window migration added by the window protocol task.
- No live cursor-theme override guarantee for already-running third-party Wayland clients; they own their cursor surfaces.
- No process-based approximation of window count.
- No Dock window preview UI in the first integration.
- No protocol compatibility with Hyprland or KDE clients.

## Program Acceptance Gate

The program is complete only when all of the following are true:

- malformed or oversized control input cannot stall or terminate Typhon;
- unauthorized UIDs cannot issue commands;
- `astreactl --json` emits exactly one valid JSON document on stdout and diagnostics only on stderr;
- cursor updates preserve the previous cursor after load, upload, or persistence failure;
- an oversized cursor automatically uses software composition without losing pointer visibility;
- Dock state reflects XDG and XWayland windows without polling `/proc`;
- minimize/restore/activate/close target the exact `WindowId` once;
- closing or disconnecting a client emits one terminal window removal to Eclipse;
- a broken wallpaper path leaves the previous wallpaper visible;
- stopping `astrea-desktop` reveals Typhon's fallback background rather than a black or undefined frame;
- Typhon and Eclipse source-layout, formatting, static analysis, unit tests, sanitizer tests, and release builds pass;
- a real 100-cycle launch/minimize/restore/close soak passes for one XDG app and one XWayland app;
- original uploaded worktrees remain unchanged by plan generation.


---

# AstreaOS Integration Plan Review Report

**Review date:** 2026-08-02

## Reviewed Inputs

- `Typhon(6).zip`
- `Eclipse(3).zip`
- `Hyprland-main(1)(5).zip`
- `kwin-master(1)(5).zip`
- `TYPHON_CURRENT_AUDIT_2026-07-13(1).md`

## Source-State Safety

The plan generation process inspected extracted copies only. It did not edit either source repository.

Observed Typhon state:

```text
main...origin/main [ahead 1]
?? artifacts/
```

Observed Eclipse state:

```text
feature/settings-foundation
modified/deleted/untracked Settings migration files present
```

The implementation plans explicitly preserve both states and exclude unrelated Settings work and Typhon qualification artifacts from commits.

## Architecture Review

### Control plane

**Approved.** A Unix-domain control socket integrated with Typhon's existing epoll reactor is lower-risk than DBus or an embedded scripting system. The plan keeps reads/writes bounded and nonblocking, unlike the part of Hyprland's command path that can wait on client input in the compositor loop.

### Cursor

**Approved with transactional requirement.** The current `OnceLock` architecture cannot support safe runtime replacement. The plan replaces it without putting a lock in the frame hot path: renderers and cursor backends receive complete `Arc` snapshots, and the shared lock is used only to acquire a snapshot.

Large cursor images use software composition rather than being rejected because a hardware plane is 64x64. This is required for accessibility-sized cursors and preserves pointer visibility.

### Dock protocol

**Approved.** Typhon already has stable `WindowId`, XDG/X11 metadata, minimize/restore state, and exact backend ownership. The protocol therefore publishes compositor truth and does not reproduce Hyprland address identifiers or process heuristics.

A private Wayland protocol is preferable to making the Dock poll `astreactl`: it provides lifecycle ordering, per-window state, immediate deltas, and request objects tied to exact windows.

### Wallpaper

**Approved outside Typhon.** Image decoding, filesystem watching, transitions, and presentation policy do not belong in the KMS/compositor renderer. A Layer Shell background process can fail independently while Typhon's existing fallback remains visible.

## Dependency Review

The dependency graph contains no cycle:

```text
Control Plane -> Cursor
Control Plane -> Window Action API -> Wayland Protocol -> Eclipse Client -> Dock
Control Plane -> astreactl Router -> Wallpaper Service
Wallpaper Service does not depend on Dock or Toplevel Protocol
```

The program can stop after any milestone with independently useful software.

## API Consistency Review

The following names are consistent across plans:

- `ControlRequest`, `ControlResponse`, `ControlCommand`, `ControlResult`, `ControlError`
- `ControlRuntimePaths`, `NativeControlServer`, `ControlReadyEvent`
- `CursorSettings`, `CursorSettingsStore`, `CursorThemeSnapshot`, `CursorThemeManager`
- `ToplevelSnapshot`, `ToplevelStateFlags`, `ToplevelCapabilities`
- `TyphonDisplayConnection`, `TyphonToplevel`, `TyphonToplevelRegistry`
- `DockWindowTracker`, `DockAppWindowState`
- `WallpaperSelection`, `WallpaperConfig`, `WallpaperController`

## Security Review

The plans cover:

- runtime-directory ownership and permissions;
- same-UID peer authentication;
- bounded request, response, client, and per-cycle work;
- no network listener;
- no subprocess execution through the control socket;
- strict JSON fields and protocol version checks;
- stale reactor token handling;
- authorization on every private Wayland request;
- exact `WindowId` action targeting;
- absolute wallpaper paths and bounded decode dimensions;
- symlink and atomic-persistence behavior;
- no silent credential fallback.

## Lifecycle Review

The plans explicitly test:

- partial socket reads/writes and early disconnect;
- compositor shutdown with connected clients;
- stale epoll generations;
- window destruction before state flush;
- XWayland restart;
- Dock reconnect and stale target retry;
- client-owned cursor active during theme change;
- KMS cursor resource rollback;
- stale asynchronous wallpaper decode;
- screen surface recreation;
- service stop revealing compositor fallback.

## Scope Review

The program is split into four independently reviewable implementation plans. The Settings UI is intentionally excluded because the uploaded Eclipse worktree is in an active source-preserving Settings migration. Settings adapters should be added only after these backend contracts are stable and the Settings branch is clean.

The first release also excludes workspaces, Dock previews, wallpaper blur/slideshow, public third-party window management, DBus, and live replacement of third-party client-owned Wayland cursors.

## Self-Review Result

- Specification coverage: complete for the requested wallpaper, `astreactl`, cursor theme/size, Dock window display, minimize/restore, and useful diagnostics.
- Placeholder scan: no unresolved markers or undefined implementation placeholders remain.
- Type consistency: shared interfaces use the same names and ownership semantics across all plans.
- Source accuracy: the plans use the current Typhon XWayland implementation and do not rely on the obsolete XWayland status in the July 13 audit.
- Repository safety: no implementation or source commit was performed while generating these documents.

## Recommended First Execution

Start with `01-control-plane-implementation-plan.md`. Complete and review its six tasks before beginning cursor or protocol work. This produces the foundation used by every CLI-driven feature while remaining independently testable and useful.
