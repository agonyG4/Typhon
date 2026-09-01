# Typhon Native Pointer Routing Transition Latency Closure v2 Plan

> Execute inline in the current checkout. Do not dispatch sub-agents.

## Phase 1: honest observer RED -> GREEN

- Add a failing timing test proving wake time and actual input-service time are
  distinct (`transition=100`, wake=`200`, service=`450`).
- Add a failing test proving empty input service does not complete an
  observation and a later non-empty batch does.
- Replace synthesized activation phase fields with one committed transition
  boundary and add dispatch/queue-drain/checkpoint fields.
- Remove wake-time population of `next_input_service`.
- Keep the fixed ring, disabled fast path, and one summary per completed
  transition.

## Phase 2: explicit ingress RED -> GREEN

- Add failing API/tests for `begin_semantic_epoch` and
  `drain_epoch_chunk_into`.
- Split libinput dispatch from queue draining; preserve exact queue-presence
  detection and the 256-event bound.
- Keep raw evdev dispatch as a no-op and retain bounded raw continuation.
- Time dispatch and queue drain independently.

## Phase 3: bounded guard RED -> GREEN

- Add a production-owned guard state machine with a fixed maximum of four
  checkpoints.
- Add deterministic checkpoint tests for input arriving after checkpoint zero,
  no-input bounded completion, no-transition zero checks, and exactly-once
  fresh service.
- Integrate the guard at the real cycle boundaries before XWayland/client
  scene, acquire/prepare, and render/presentation/KMS. Keep checkpoint zero
  immediately after real transition settlement.
- Preserve active-epoch ownership and merge fresh microturn state explicitly.

## Phase 4: readiness and integration coverage

- Make the input peek recognize only `EPOLLIN`; terminal flags remain normal
  reactor lifecycle/error handling.
- Test input + control, DRM-like eventfd, and runtime continuation without
  consuming non-input readiness.
- Add/retain real compositor/server ordering coverage for old `D`, Wayland lock
  request, and fresh `D2`; >256 continuation; raw evdev; repeated transitions;
  and wake-authority non-regression.
- Audit epoch-owned deferred Wayland progression and multi-cycle continuation;
  change only with a deterministic regression if still present.

## Verification and handoff

- Run each focused test immediately after its RED/GREEN slice and record the
  actual RED reason.
- Run `rtk cargo fmt --check`, locked check, locked clippy with `-D warnings`,
  locked tests, and `rtk git diff --check`.
- Commit implementation slices and report starting/ending HEAD, exact tests,
  remaining dirty user changes, reference comparisons, and the manual primary
  timing command. Do not claim Sober closure; the user performs that run.
