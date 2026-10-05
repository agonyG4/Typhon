# F11-D XWayland Interoperability Qualification

**Status: PARTIALLY QUALIFIED — native Typhon session unavailable.** No real-client interoperability case was run or counted as a pass. The active display belongs to Hyprland, so it cannot qualify Typhon's XWayland bridge.

## Qualification record

- Date: 2026-10-05
- Repository: Typhon `main`
- Initial checkout HEAD before the C4 test-only repair: `47b7cae2ed76e32dd0f3b1fb388ffd0dc5ab8bb9`
- C4 test-identity repair commit and initial F11-D source-review HEAD: `749e10f2b05c988fa3b1c12e660b20063d0bd3f1`
- F11-C4 production baseline: present; `e690fe74ccf91752f7e22a8b28acdae933ab998b` is an ancestor of the initial checkout and repair commit.
- Machine: `Kaiser`, x86_64, Linux `7.2.9-1-cachyos`.
- Active session: Hyprland, Wayland, session `2`, seat0, VT1.
- Active output: one `DP-1`, `1920x1080@164.99899`, scale 1.
- GPU and driver: NVIDIA GeForce RTX 3060 Ti; NVIDIA kernel/userspace driver `615.71.09`; Mesa package `3:26.2.4-1`.
- Active display variables: `DISPLAY=:1`; `WAYLAND_DISPLAY=wayland-1`; socket `/run/user/1000/wayland-1`; `XAUTHORITY` is unset. No authentication cookie was read.
- `TYPHON_XWAYLAND` is unset in the active session. The repository parses an unset value as `off`; eager managed startup was not run. No Typhon process, Typhon-managed XWM, Typhon executable on `PATH`, or Typhon native session entry was present.
- Xwayland reported by the active Hyprland session: X.Org Xwayland `24.1.13` (release `12401013`). This is environment inventory, not Typhon-session evidence.
- Typhon launch profile: unavailable in this session. `bin/check-xwayland-session` reported an Xwayland process on `:1`, but `xwm_state`, generation, client count, and selection bridge were `unsupported/not connected`.
- Ordinary X11 connectivity: `xdpyinfo` connected to `:1`. The Xwayland process was a child of Hyprland, so this verifies only that the control display accepts X11 clients.
- Before writing this record, the tracked working tree was clean after the C4 repair commit; unrelated untracked `msg.txt` remained untouched.

The active seat's DRM card is owned by Hyprland. Launching Typhon nested in this session would not meet the qualification rule, and no separate native Typhon session was available. Therefore no real client was launched for an F11-D case, and no endpoint backend was inferred from environment variables.

## Client and tool inventory

Inventory was read from installed packages and local Steam/Wine manifests. Applications were not launched for interoperability testing.

| Area | Available inventory | Qualification use |
| --- | --- | --- |
| Level A tools | `wl-copy`/`wl-paste` 2.3.0, `xprop` 1.2.8, `xdpyinfo` 1.4.0 | Available for later native-session checks. `xclip`, `xsel`, `xwininfo`, and `xdotool` were not found. No packages were installed. |
| GTK | GTK 3 `3.24.52`, GTK 4 `4.22.5`; Firefox `157.0`; Zen Browser `1.22.3b` | Firefox and Zen are candidate real text copy/paste clients. Neither endpoint instance was launched, so no GTK backend or XID was proved. |
| Qt | Qt 5 `5.15.19`, Qt 6 `6.11.2`; Qt 6 Wayland plugin `6.11.2`; Qt 5 Wayland plugin absent; Qt Assistant, Designer, and Linguist tools are installed | Assistant or Designer can serve as Qt application candidates. Neither backend was launched or proved. |
| Electron | Discord package `1:1.0.160`; Codex Desktop `2026.10.02.213647`; Electron 39/40/42 packages installed | Candidate applications exist, but no Electron version-to-app runtime or endpoint backend was established by launching them. |
| Wine | Wine `11.19`; Wine's built-in 64-bit and 32-bit `notepad.exe` are present. No separate user-installed Windows text application was found in the Wine prefix. | Wine Notepad is a candidate clipboard workload. It was not launched. |
| Steam | Steam package `1.0.0.87`; installed manifests include Stardew Valley, Cyberpunk 2077, Crusader Kings III, WorldBox, Farthest Frontier, and two Naruto games. | Steam was not running or launched. No chat/paste or drag-and-drop operation was exercised. |
| Proton | Proton Experimental, GE-Proton 11-5, and CachyOS Proton compatibility-tool directories are installed. | Game manifests and installed compatibility tools do not prove which tool a game uses. No game was launched and no data-interoperation operation was identified or exercised. |

## Backend evidence

No F11-D endpoint backend is proved. The only X11 observation is `xdpyinfo` on Hyprland's `:1`; no candidate application XID, `xprop` client record, or Typhon XWM lifecycle entry was captured. The Wayland socket belongs to the same Hyprland session. No GTK, Qt, Electron, Wine, Steam, or Proton endpoint was launched with a backend control, and no client fallback was assessed.

The following matrix results are therefore environment-blocked even where a candidate application is installed. Repetitions are zero. No payload, drag action, or protocol phase was observed.

## Selection matrix

| Case ID | Channel and direction | Intended operation | Result | Repetitions / backend proof |
| --- | --- | --- | --- | --- |
| `SEL-CB-X11-WL-TEXT` | CLIPBOARD, X11 → Wayland | Unique small UTF-8 payload and application copy/paste | `ENVIRONMENT_BLOCKED` | 0; no Typhon X11 source or native Wayland recipient launched |
| `SEL-CB-X11-WL-INCR` | CLIPBOARD, X11 → Wayland | Large payload exercising incoming INCR | `ENVIRONMENT_BLOCKED` | 0; no X11 selection tool or Typhon session |
| `SEL-CB-X11-WL-CHURN` | CLIPBOARD, X11 → Wayland | Owner replacement, repeated churn, source exit, then independent transfer | `ENVIRONMENT_BLOCKED` | 0; ownership was not exercised |
| `SEL-CB-WL-X11-TEXT` | CLIPBOARD, Wayland → X11 | Unique small UTF-8 payload, application paste, ordinary aliases when requested | `ENVIRONMENT_BLOCKED` | 0; no Typhon Wayland source or X11 recipient launched |
| `SEL-CB-WL-X11-INCR` | CLIPBOARD, Wayland → X11 | Large payload exercising outgoing INCR | `ENVIRONMENT_BLOCKED` | 0; no live SelectionRequest was observed |
| `SEL-CB-WL-X11-CHURN` | CLIPBOARD, Wayland → X11 | Owner replacement, source exit, then independent X11 transfer | `ENVIRONMENT_BLOCKED` | 0; ownership was not exercised |
| `SEL-PRIMARY-X11-WL` | PRIMARY, X11 → Wayland | Select text in an X11 client and paste through Wayland PRIMARY semantics | `ENVIRONMENT_BLOCKED` | 0; PRIMARY was not exercised |
| `SEL-PRIMARY-WL-X11` | PRIMARY, Wayland → X11 | Select text in a Wayland client and paste through X11 PRIMARY semantics | `ENVIRONMENT_BLOCKED` | 0; PRIMARY was not exercised |

CLIPBOARD and PRIMARY remain separate in the test plan. Ctrl+C/Ctrl+V would not count as PRIMARY evidence. No exact-payload or digest evidence was collected.

## XDND matrix

| Case ID | Direction and candidate | Intended coverage | Result | Repetitions / backend proof |
| --- | --- | --- | --- | --- |
| `DND-WL-X11-GTK-COPY-TEXT` | Wayland GTK → X11 GTK | Enter, Position, acceptance/Status, leave/re-entry, Copy Drop, receive, successful Finished; three clean and post-cancellation repetitions | `ENVIRONMENT_BLOCKED` | 0; both endpoints absent from a Typhon-managed session |
| `DND-WL-X11-GTK-COPY-URI` | Wayland GTK → X11 GTK | `text/uri-list` Copy when exposed, including post-Drop receive and completion | `ENVIRONMENT_BLOCKED` | 0; backends not proved |
| `DND-X11-WL-GTK-COPY-TEXT` | X11 GTK → Wayland GTK | v5 admission through canonical Wayland finish and exactly one successful `XdndFinished`; repeat/reject/retry | `ENVIRONMENT_BLOCKED` | 0; backends not proved |
| `DND-X11-WL-GTK-COPY-URI` | X11 GTK → Wayland GTK | `text/uri-list` Copy when exposed, receive/finish, repeat/reject/retry | `ENVIRONMENT_BLOCKED` | 0; backends not proved |
| `DND-WL-X11-QT-COPY-TEXT` | Wayland Qt → X11 Qt | Repeated Copy lifecycle and completed post-Drop receive | `ENVIRONMENT_BLOCKED` | 0; Qt app and backends not launched |
| `DND-WL-X11-QT-COPY-URI` | Wayland Qt → X11 Qt | `text/uri-list` Copy when exposed | `ENVIRONMENT_BLOCKED` | 0; Qt app and backends not launched |
| `DND-X11-WL-QT-COPY-TEXT` | X11 Qt → Wayland Qt | v5 admission, canonical finish, one successful `XdndFinished`, repeat/reject/retry | `ENVIRONMENT_BLOCKED` | 0; Qt app and backends not launched |
| `DND-X11-WL-QT-COPY-URI` | X11 Qt → Wayland Qt | `text/uri-list` Copy when exposed | `ENVIRONMENT_BLOCKED` | 0; Qt app and backends not launched |
| `DND-MIXED-GTK-QT-COPY` | GTK ↔ Qt | At least one completed Copy in each bridge direction | `ENVIRONMENT_BLOCKED` | 0; no endpoint backend proof |
| `DND-ELECTRON-COPY` | Electron ↔ GTK or Qt | Copy in each exposed bridge direction after proving both Electron modes | `ENVIRONMENT_BLOCKED` | 0; Discord/Codex were not launched |
| `DND-X11-WL-MOVE` | X11 source → Wayland target | Real Move and exact DELETE conversion before successful Finished | `ENVIRONMENT_BLOCKED` | 0; no Move operation observed |
| `SEL-CB-WINE-X11-WL` | Wine Notepad X11 ↔ Wayland text client | Meaningful text copy/paste | `ENVIRONMENT_BLOCKED` | 0; Wine Notepad not launched |
| `SEL-CB-STEAM-X11-WL` | Steam client X11 ↔ Wayland text client | Clipboard operation only if Steam exposes one in the available session | `ENVIRONMENT_BLOCKED` | 0; Steam was not launched |
| `DND-PROTON-WORKLOAD` | Installed Proton game | Clipboard or drag-and-drop only if the workload exposes a meaningful operation | `NOT_TESTABLE` | No game operation was identified from the install inventory; no game was launched |

No real `XdndFinished`, canonical Wayland `finish`, MIME receive, Move DELETE, rejection, cancellation, or immediate subsequent drag was observed. The GTK/Qt three-repetition requirement is unmet. No control-compositor comparison was made because no Typhon failure was reproduced.

## Classification and limitations

- `TYPHON_DEFECT`: none established; no real Typhon client operation ran.
- `CLIENT_OR_TOOLKIT_BEHAVIOR`: none observed.
- `EXPECTED_TYPHON_LIMITATION`: reverse productive terminal handling requires XDND v5; pre-v5 sources are rejected. The documented Wayland → X11 path does not advertise or convert DELETE, and its current action policy does not choose Ask. Reverse root-coordinate mapping remains 1:1 for the single-output scope.
- `ENVIRONMENT_BLOCKED`: every installed-client selection and GTK/Qt/Electron/Wine/Steam case above, because no managed Typhon XWayland session was available.
- `NOT_TESTABLE`: installed Proton game workloads, because no clipboard/drag-and-drop operation was identified without launching a game and no Typhon session was available.
- `INCONCLUSIVE`: no client behavior is classified this way; no client protocol operation was attempted.

No trace log was collected. No production workaround or protocol change was made. The only code change in this qualification was the separate C4 test-identity repair in commit `749e10f2`.

## Deterministic validation and next boundary

The focused incoming-terminal command
`CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target cargo test --lib xwayland::xwm::data_bridge::dnd_incoming::tests::terminal`
passed all 50 tests after the C4 fixture identity repair. `cargo fmt --check`
also passed at that point. Later in the same review, unrelated unstaged changes
appeared in decoration/window and native-output files. The final validation
results on that live worktree were:

- `cargo fmt --check`: failed on formatting drift in
  `src/compositor/state/window_decoration_tests.rs`.
- `cargo check --all-targets`: failed while compiling the changed decoration
  tests because `WindowDecorationPolicy` was not exported and referenced
  decoration methods were missing.
- All seven requested `cargo test --lib` filters and all four XWayland
  integration targets were attempted. They failed before running tests due to
  those decoration changes; later builds also reported missing
  `effective_x11_decoration_mode` references and a string `match` inside a
  `const fn` rejected by Rust `1.94.0`.
- `cargo test`: failed at the same unrelated compile errors before running the
  full suite.
- `cargo clippy --all-targets -- -D warnings`: failed at unrelated decoration
  borrow errors and the unsupported `const fn` string match.
- `./bin/check-source-layout`: failed with the repository's known broad set of
  source-size violations, including existing XWayland selection files.
- `git diff --check`: passed.

The live decoration/window and native-output edits were left untouched and were
not staged. These validation failures do not qualify any real-client matrix
cell and are not attributed to the C4 identity repair.

F11-D remains partially qualified until the same cases can be run from the
native, managed Typhon session with `TYPHON_XWAYLAND=eager` (or a launch path
that demonstrably guarantees equivalent eager startup) and X11/Wayland endpoint
backends proved individually. Do not treat this Hyprland inventory as a Typhon
control result.
