# Scheduling QoS Phase 1 and the Phase 2 Render Boundary

Phase 1 keeps the native runtime's current rendering ownership. The opt-in
`OBLIVION_ONE_SCHEDULING_QOS=auto` policy protects the native reactor with
verified `SCHED_RR` priority 1 and `SCHED_RESET_ON_FORK` when direct setup and a
finite soft `RLIMIT_RTTIME` guard are available. It falls back to RTKit
high-priority FAIR, direct negative nice, and ordinary `SCHED_OTHER` in that
order. RTKit realtime grants are intentionally excluded while Typhon's child
limit restoration is not proven for that path. KMS remains a `SCHED_OTHER`
worker and may receive high-priority FAIR scheduling. The default remains
`off` pending hardware qualification.

QoS setup runs at lifecycle boundaries. The reactor does not call D-Bus from
its cycle. Audited helper creation temporarily changes an RT creator to
`SCHED_OTHER` before creating a pthread, then restores the creator's verified
prior state. The new thread raises its own inherited nice value to Typhon's
startup value in its trampoline before role registration or generic work. That
preserves an RTKit FAIR grant on the reactor without letting helper work
inherit it. The KMS thread applies its FAIR policy from its own startup.
Managed child launches restore Typhon's startup nice value and original
`RLIMIT_RTTIME` in the post-fork, pre-exec path and clear ambient capabilities.
The `SIGXCPU` handler only demotes the known reactor TID, records atomic
evidence, and returns after successful demotion. If the kernel refuses the
demotion syscall, it exits the process rather than leave a possibly runaway
RT reactor active.

Session suspend and shutdown demote the reactor before controlled KMS joins.
Session recovery tries direct scheduling only, so the runtime thread never
waits for RTKit over D-Bus. A diagnostic KMS TEST_ONLY geometry probe now
returns a receiver polled by later cycles; it no longer waits synchronously for
KMS completion.

The QoS doctor snapshot refresh uses a nonblocking registry read. Promotion and
session reacquisition take the registry lock while the reactor is still
ordinary, then publish the verified state before entering realtime. If a worker
holds the registry during a doctor request, the response samples the reactor's
kernel state directly and returns a registry-busy reason; worker rows are
omitted for that response instead of waiting on the worker.

## Phase 2 render-worker boundary

The next refactor should split the current `AtomicEglGbmScanout` ownership
across these source boundaries:

- `src/native_output/runtime/presentation_cycle.rs::render_present_and_update_metrics`
  currently executes from the native runtime cycle.
- `src/native_output/scanout/atomic_egl_gbm.rs::AtomicEglGbmScanout::render_to_slot`
  owns `egl.make_current`, scene draw requests, GL scene drawing, and render
  fence creation.
- `src/egl_renderer.rs::GlesSceneRenderer::draw_scene_to_target` owns the GL
  scene-render entry point and renderer resources.
- Presentation planning, output ownership, frame/transaction identity, and
  Wayland state remain with the native reactor.
- EGL/GL contexts, `GlesSceneRenderer`, and render-only GPU resources move to
  one render worker. Timed DRM/KMS submission stays with the existing KMS
  worker.

The render queue must have one executing job and at most one replaceable
pending job. Pending work may be superseded before GPU ownership begins. Once
rendering starts, each frame or transaction must settle its terminal ownership
exactly once. The KMS worker queue, wake deadlines, EBUSY handling, TEST_ONLY
flow, generation checks, and completion ownership remain unchanged.

Phase 1 does not make the reactor a strictly bounded real-time thread: acquire,
prepare, presentation planning, and EGL/GL driver work still execute in the
runtime thread. While RR is active, those GL/driver calls therefore also run
under RR; the desired FAIR render boundary is not established until Phase 2.
Wayland admission rotates over bounded batches of accepts and clients, but the
pinned `wayland-backend` API drains all pending requests for one selected
client inside a dispatch call. Request-count service is therefore not strictly
bounded per client; the RT budget is the fail-safe for a pathological
continuously flooding client until a budgeted backend dispatch API is
available. Hardware qualification must determine whether this Phase 1
ownership split is adequate for the target workload.
