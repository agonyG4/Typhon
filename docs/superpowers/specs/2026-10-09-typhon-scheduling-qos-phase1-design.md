# Typhon Scheduling QoS Phase 1 Design

## Goal

Provide an opt-in, source-auditable scheduling policy that protects the native runtime thread from ordinary fair-scheduler starvation while keeping GPU, KMS, and background work out of realtime policy. The production default remains `off` until the target desktop completes the same-binary hardware A/B.

## Source boundaries

`NativeRuntime::run_cycle()` owns Wayland and XWayland dispatch, input routing, controller work, acquire/prepare, presentation decisions, and `render_present_and_update_metrics()`. The presentation path reaches `AtomicEglGbmScanout::render_to_slot()`, which makes the EGL context current, prepares the scene draw request, calls `GlesSceneRenderer::draw_scene_to_target()`, and creates the render fence. Phase 1 may protect this existing thread, but it does not make it a bounded realtime reactor.

The existing `typhon-kms-commit` thread retains its bounded queue and timed submission ownership. It receives high-priority FAIR scheduling only. No KMS ioctl or worker thread receives realtime policy.

## Policy and evidence

`OBLIVION_ONE_SCHEDULING_QOS=off|auto|require` is parsed once at native bootstrap; absent means `off`. Requested policy is stored separately from the kernel-observed policy. Diagnostics are keyed by Linux TID and include role, requested/effective class and priority, backend, degradation reason, reset-on-fork state, RT budget evidence, and demotion count.

`auto` first requests `SCHED_RR` at priority 1 with `SCHED_RESET_ON_FORK`. It installs a finite soft `RLIMIT_RTTIME` and an async-signal-safe `SIGXCPU` demotion path before enabling RR. The hard limit is retained. If direct RR is unavailable, RTKit realtime is deliberately excluded because it would need process-wide RT-time-limit changes; fallback tries RTKit high-priority FAIR, then direct negative nice, then normal scheduling. `require` needs a kernel-verified RR policy and otherwise returns a typed startup error. KMS requests high-priority FAIR and always fails open to normal scheduling.

RTKit setup runs on a normal-priority broker created before reactor promotion. It uses the existing zbus dependency, targets Linux TIDs, clamps to `MinNiceLevel`, and is never called from a cycle. Child launches restore the original nice and `RLIMIT_RTTIME` in their pre-exec setup and clear ambient capabilities. The compositor does not change its hard RT-time limit or add capabilities.

All explicit Typhon helper-thread creation uses one guarded wrapper. In `auto` and `require`, an RT creator is temporarily normalized to `SCHED_OTHER` before pthread creation, so the child cannot inherit RT while its Rust closure starts. The creator's prior state is restored only after creation unless the RT safety handler has demoted it. The child trampoline raises inherited nice to Typhon's startup value before role work. A `SCHED_OTHER` high-priority FAIR creator is left unchanged so a restricted RTKit grant is not lost. Existing bootstrap workers are created before reactor promotion. Application-scope registration runs in the separately exec'd launch helper; `SCHED_RESET_ON_FORK` and child-state restoration keep that path ordinary.

## Internal service and diagnostics

The existing input epoch, bounded raw-input drain, and routing checkpoints remain authoritative. Wayland accepts, process-launch work, and screen-capture service are capped per cycle and use the existing eventfd continuation source to preserve FIFO/request ownership while yielding to later input and control service. The Rust Wayland backend's per-client API drains all messages currently pending from one selected client, so the client batch is bounded but an individual client's request count is not. The diagnostic scaled-primary TEST_ONLY probe now returns a receiver that later cycles poll without blocking on KMS. Existing controller, control, and system-action budgets remain unchanged.

Scheduling state is added to the native doctor snapshot. Existing pointer timing, slow-cycle phase, and KMS worker timing traces remain the latency evidence; no per-cycle `/proc` polling or periodic repaint is added.

## Phase 2 rendering boundary

Phase 2 will split `NativeRuntime::render_present_and_update_metrics()` in `src/native_output/runtime/presentation_cycle.rs` from the EGL/GL work in `src/native_output/scanout/atomic_egl_gbm.rs::AtomicEglGbmScanout::render_to_slot()` and `src/egl_renderer.rs::GlesSceneRenderer::draw_scene_to_target()`. The reactor will retain Wayland state, semantic state, frame/transaction identity, presentation planning, and output ownership. The render worker will own EGL/GL contexts, `GlesSceneRenderer`, and render-only resources; KMS remains with the existing worker.

The future render queue is bounded to one executing job and one replaceable pending job. A pending job may be superseded only before GPU ownership starts; each started job must settle terminal ownership exactly once. Phase 1 does not change these ownership paths.

## Qualification boundary

The same binary must be compared under `off` and `auto` with identical display, VRR, cursor, KMS, effects, and CPU saturation settings. Kernel policy must be inspected by TID. Source and automated tests can establish the policy and fail-safe behavior, but only the target 165 Hz stress run can establish whether the reported desktop stalls are fixed.
