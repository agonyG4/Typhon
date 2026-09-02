# Typhon Native Input Readiness Authority Closure v1

## Revision and scope

Starting HEAD recorded before this task: `6e24da9b0ce2e673795f01727572a6a5907e8b54` (`native: attribute wake deadline anomalies by owner`).

The checkout advanced during the work through the already-present content-cadence commits `0e3a96c` and `5726c4e`. The focused implementation commit is `916c12a806a76bd95811db1923fa7b1d3194c4d2` (`native: use targeted input readiness authority`). The checkout was subsequently observed at `77df0650e783446dbd9a63e007aa44a052b92912`, an unrelated root-level content-frame-clock report commit; no native-input source changed after `916c12a`.

This closure preserves semantic epochs, explicit ingress, the pre-read gate, the post-transition guard, epoch-owned Wayland progression, bounded continuation, exact motion, coalescing, raw evdev fallback, and Native Wake Authority.

## Runtime evidence and diagnosis

The supplied latest Sober qualification showed the post-transition guard reaching input in approximately 5–8 us: 37/37 transitions had `pre_read_probe=true`, 0/37 promoted input at that probe, and 37/37 became serviceable at checkpoint 0. Nevertheless, 27/37 locked activations had hardware spans of at least 24,000 us; the median was 26,989 us with a median raw count of 26. The supplied trace also included libinput's approximately 27 ms lag warning.

That evidence makes the post-transition guard healthy and places the remaining defect before or during the client read that mutates input-resource topology. It does not reopen activation-anchor causality: the supplied traces continue to show cursor/anchor equality and matching native/compositor anchors.

The source defect was `NativeEventLoop::input_ready_nonblocking()` using the global epoll instance and its `MAX_READY_EVENTS = 64` buffer as an exact input readiness oracle. A ready native input registration can be omitted from a saturated bounded batch. Linux documents that up to `n` ready descriptors are returned and that successive calls round-robin through a larger ready set ([epoll_wait(2)](https://man7.org/linux/man-pages/man2/epoll_wait.2.html)). The helper therefore both produced false negatives and changed the global ready-list delivery position for the subsequent reactor wait. It was not observer-neutral even though it did not read fd payloads.

The libinput contract matches the fix: libinput exposes one event fd, and `libinput_dispatch()` should be called immediately when that fd is readable ([libinput API](https://wayland.freedesktop.org/libinput/doc/latest/api/group__base.html)).

## Reference comparison

- KWin gives acquisition explicit ownership: its `LibinputBackend` owns a `libinput-connection` thread ([backend](https://raw.githubusercontent.com/KDE/kwin/master/src/backends/libinput/libinputbackend.cpp)), its connection owns a `QSocketNotifier`, dispatches, and appends to an explicit queue ([connection](https://raw.githubusercontent.com/KDE/kwin/master/src/backends/libinput/connection.cpp)). Typhon adopts the explicit source/snapshot boundary without copying the thread.
- Aquamarine exposes the exact libinput fd as a poll source and its callback dispatches libinput ([Session.cpp](https://raw.githubusercontent.com/hyprwm/aquamarine/main/src/backend/Session.cpp)); Hyprland uses that short event-loop path. This supports source-specific readiness, not a global-batch sample.
- wlroots registers `libinput_get_fd()` directly with its Wayland event loop and dispatches from the readable callback ([backend/libinput/backend.c](https://raw.githubusercontent.com/swaywm/wlroots/master/backend/libinput/backend.c)).
- Weston likewise registers the libinput fd and calls `libinput_dispatch()` from its event source ([libinput-seat.c](https://raw.githubusercontent.com/wayland-mirror/weston/main/libweston/libinput-seat.c)).

A dedicated input thread was rejected because Typhon already has direct epoll ownership and semantic epochs, while Wayland resource topology, focus, constraint state, and client delivery remain compositor-thread responsibilities. A thread would add cross-thread queue/session lifetime complexity without making a blocked compositor thread deliver relative-pointer events.

Unconditional `libinput_dispatch()` was rejected because the fd is the authority and the libinput API requires prompt dispatch after readability, not dispatch without readiness. Drain-until-quiescent was rejected because a continuously readable 1000 Hz device could starve a pending Wayland client read. Increasing `MAX_READY_EVENTS` was rejected because it does not make a bounded global batch a complete source-specific query.

## Selected architecture

`NativeEventLoop` remains the global wake collector. Exact input readiness now belongs to `NativeInputBackend::ready_nonblocking()`:

- libseat/direct libinput use one cached `libc::pollfd` for the single libinput fd and one `poll(..., 0)` syscall per targeted probe;
- raw evdev builds one reusable pollfd vector when the stable device set is constructed and polls all current raw fds in one syscall;
- only `POLLIN` without `POLLERR`, `POLLHUP`, `POLLNVAL`, or `POLLRDHUP` is healthy serviceability;
- the probe does not read, dispatch, mutate epochs, mutate Wayland state, or touch the global epoll ready list;
- suspended backends report no healthy readiness.

The old global `input_ready_nonblocking()` production helper is removed. The only remaining `epoll_wait()` in the source is the normal global reactor wait. `MAX_READY_EVENTS = 64` is unchanged.

The final semantic cut is: input readable at the last targeted pre-read probe belongs to the old topology and is processed in exactly one new semantic epoch before the Wayland client read; input arriving after that probe belongs to a later epoch. Hardware timestamps remain diagnostic only.

The exact pre-read gate remains inside `dispatch_wayland_and_input()`, after initial native constraint settlement and cursor synchronization and immediately before the Wayland-only `dispatch_wayland_with_outcome()` call. It probes only when `dispatch_wayland && !service_input`; input-only and combined-input turns perform zero extra probes. A positive result promotes one epoch, allows all of that epoch's `>256` continuation chunks to finish without another probe or client read, then permits the originally requested Wayland read. The existing post-transition guard uses the same targeted backend probe at its bounded checkpoints, with no guard checks on ordinary motion.

## Timing truth

`NativePointerPreReadObservation` now carries the pre-read probe timestamp and the actual Wayland read start/end timestamps. When the client read queues a pointer-routing transition, those timestamps are copied into the transition record. Summaries therefore report `wayland_read_duration_ns` for that causal read and `pre_read_probe_to_transition_ns` when available. The timing trace remains a fixed-capacity ring, emits only transition summaries, and does not add clock reads or formatting when disabled. Empty input attempts still do not complete an observation; only a real nonempty batch does.

## RED evidence and GREEN coverage

The targeted raw-readiness test was written before the API implementation and first ran RED with the expected missing `NativeInputDevices::from_devices` and `NativeInputBackend::ready_nonblocking` APIs (the then-current checkout also exposed unrelated presentation-worktree compiler errors). The timing-carry test was likewise run RED on the missing observation fields. The bounded global-batch test is a deterministic algorithm seam: a full 64-source non-input prefix with input after it demonstrates why the former inspection cannot answer targeted readiness without depending on kernel ordering.

Focused GREEN results:

- native input tests: 77 passed;
- cycle-dispatch tests: 16 passed;
- transition-guard tests: 4 passed;
- pointer-timing tests: 12 passed;
- native event-loop tests: 34 passed;
- real relative/constraint integration tests: 38 passed;
- targeted raw readiness covers unreadable, repeated readable, terminal-only, terminal-plus-healthy, multiple raw fds, non-consuming behavior, and suspension;
- the real `OwnCompositorServer` relative-pointer/locked-pointer fixture now explicitly covers the protected pre-read semantic cut, old-delta exclusion, exact post-activation D2 delivery, and locked anchor preservation;
- the timing regression verifies transition 100 → wake 200 → actual service 450 remains 350 ns to service, while the Wayland-read carry test reports the measured interval.

The post-transition guard, anchor, >256 epoch, exact-256, deferred Wayland progression, fairness, coalescing, and raw-evdev drain tests remain green. The epoch-owned deferred progression implementation was audited and left unchanged because its existing tests are green; no speculative refactor was introduced.

## Verification

Results actually run:

- `rtk cargo fmt --check`: passed;
- `rtk cargo check --locked --all-targets`: passed;
- `rtk git diff --check`: passed;
- `rtk cargo clippy --locked --all-targets -- -D warnings`: the first run stopped on two unrelated presentation-pacing diagnostics in the concurrent checkout (`classify_content_frame` had 9 arguments and a test used field assignment after `Default`); a fresh rerun at the subsequently observed `77df065` HEAD passed with no issues;
- `rtk cargo test --locked`: one unrelated XWayland geometry test failed in the initial run; its exact isolated rerun passed;
- `rtk cargo test --locked -- --test-threads=1`: passed, 3,225 passed, 5 ignored, 40 filtered.

No native DRM/KMS compositor runtime or Sober/Roblox run was performed by the agent. The required user qualification command is:

```bash
TYPHON_POINTER_TIMING_TRACE=1 \
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
./bin/start-oblivion-one-tty
```

Optional semantic-debug command:

```bash
TYPHON_POINTER_DEBUG=1 \
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
./bin/start-oblivion-one-tty
```

If the user's timing-neutral run still shows `pre_read_input_promoted=false` and a large first Locked batch, the remaining falsifiable hypothesis is H2: input became readable only after the targeted pre-read cut and accumulated during a long `dispatch_wayland_with_outcome()` call. Its newly truthful `wayland_read_duration_ns` distinguishes that case; it is outside this closure.

## Files changed by this closure

- `src/native/event_loop.rs`
- `src/native_output/input/routing.rs`
- `src/native_output/runtime/cycle.rs`
- `src/native_output/runtime/cycle_dispatch.rs`
- `src/native_output/runtime/pointer_timing.rs`
- `src/native_output/tests/input.rs`
- `src/compositor/tests/input_output/relative_and_constraints.rs`
- this report
