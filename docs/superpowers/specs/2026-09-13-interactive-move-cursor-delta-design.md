# Interactive move cursor-delta design

## Evidence

During a predictive-triple-buffered interactive move, a primary frame can be
worker-queued while the worker still exposes it as an attachable primary. The
next pointer-only cursor state has the same hardware delivery, framebuffer,
dimensions, hotspot, and image generation; only its position and motion
revision change.

The current runtime adapter nevertheless computes:

```text
validation_base_unchanged = !atomic_commit_pending && presented_cursor == cursor.current
```

`atomic_commit_pending` includes the worker-queued phase. The classifier then
turns every such motion into `CursorDeltaClass::Visual`. This is conservative
for an independent cursor commit, but it is incorrect for a sidecar that will
be merged into the still-mutable primary before the worker freeze. The sidecar
already receives the primary's immutable `validation_base`, and the worker
requires that exact base when claiming it.

## Smallest correction

Treat the validation base as unchanged for cursor classification when:

1. the presented cursor still matches the cursor's physical current state;
2. no Atomic commit is pending, or an exact attachable primary exists.

The attachable-primary condition only relaxes classification while the worker
can still merge the cursor state into that primary. A frozen or kernel-submitted
commit remains conservative and continues to classify the motion as `Visual`.

## Regression coverage

Add a policy regression for an attachable primary with a position-only state
change while the Atomic worker lane is pending. It must select `PositionOnly`
and preserve hardware cursor delivery.

Add a bounded scripted move/worker model that repeatedly coalesces pointer
motion, admits a primary, offers a cursor sidecar, submits the exact bundle,
acknowledges its pageflip, and checks terminal ownership, cursor pin lifetime,
swapchain progress, and latest-geometry progress. The same model must retain
the conservative `Visual` result after the primary passes the attachable/freeze
boundary.

No cursor mode, KMS worker default, buffering policy, renderer effect, or input
rate is changed by this design.
