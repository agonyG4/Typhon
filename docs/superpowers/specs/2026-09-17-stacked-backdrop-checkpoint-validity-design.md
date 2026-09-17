# Stacked Backdrop Checkpoint Validity Design

## Goal

Prove or disprove that replay-plus-partial stacked backdrop rendering can execute a framebuffer-backed checkpoint capture from pixels that are not semantically valid for that checkpoint. If the RED evidence confirms the defect, make the smallest pass-driven scene-work correction and retain the existing graph-texture validity checks.

## Scope and constraints

- Keep the existing capture-coordinate and fullscreen-visibility fixes unchanged.
- Do not add `PersistentBackdropCache`, a new production backdrop architecture, global framebuffer capture, global full repaint, Full Kawase production behavior, or global framebuffer replay capture.
- Keep checkpoint dependencies and the ordering `base scene -> A -> A Composite -> advance -> B capture -> B blur -> B Composite`.
- Keep presentation damage and final Composite clipping unchanged.
- Do not change alpha, shell shape, or blend semantics as part of the primary experiment.
- Compile and test in the repository directory, using the existing pooled resources only as allocation reuse.

## Evidence-first design

The test fixture will model the native geometry with a Dock-sized checkpoint domain and a smaller TopBar-sized domain. It will use a non-uniform background, a translucent shell-like surface, two ordered backdrop effects, `TopLeftScanout`, replay capture policy, partial Kawase, and partial repaint.

The first RED layer is a bounded, pure region calculation. For each selected direct framebuffer SceneCapture it compares the authoritative capture domain against the exact region reconstructed to the checkpoint, not a bounding-box approximation. Trace fields remain bounded and include pass, instance, anchor, checkpoint count, required/valid rectangle counts and bboxes, missing counts and bbox, and missing pixel count.

The second RED layer is a real-GLES two-frame test. Frame 1 renders the full graph and saves the framebuffer. Frame 2 changes a small source region, renders a partial candidate while B remains a checkpoint-dependent framebuffer blit, and independently renders a full Frame 2 reference. The candidate must equal the previous framebuffer outside the repair and the full reference inside it. Two recognizable poison patterns are applied to unproven pooled framebuffer/capture contents; both candidates must match each other and the full reference inside the repair. The test also asserts B's actual capture mode and dependency count.

## Isolation variants

- Variant A is the current behavior: replay policy, partial Kawase, checkpoint capture as framebuffer blit, ordinary presentation scene work only. It must remain RED if the semantic-validity hypothesis is true.
- Variant B derives internal scene work from the same `is_direct_framebuffer_capture` authority and capture graph texture domains used by execution. It unions presentation work with checkpoint work, preserves `extra_internal_work`, reconstructs the checkpoint state, and restores only the extra region after graph execution. It must turn the RED GREEN if the hypothesis is the root cause.
- Variant C keeps the current checkpoint source behavior and runs Full Kawase only as a control. It is not a production fix.

## Production correction after confirmation

Only after Variant B turns the deterministic RED GREEN, replace the global-policy-driven scene-work decision with a pure shared plan containing:

- `presentation_work`: exactly ordinary presentation repair;
- `framebuffer_checkpoint_work`: the union of domains for selected passes that actually use direct framebuffer capture;
- `internal_work`: the bounded union of the two;
- `extra_internal_work`: `internal_work - presentation_work`.

The same plan drives `SurfaceConsumerPlan`, scene clear, replay scissors, checkpoint-source validity, preservation, restore, and trace diagnostics. Dependency demand must prove that every earlier effect whose output influences a later checkpoint is executed over the required source region; the correction must not force an earlier effect to full-domain execution.

Direct framebuffer capture gets a distinct checkpoint-source semantic-validity assertion before its physically-written graph texture is marked valid. The existing graph-texture current-execution validity invariant remains unchanged.

## Verification

Run focused renderer/effects/compositor tests, formatting/checking/linting gates, and the existing coordinate-space, fullscreen, ordering, diagnostic-configuration, and Full-Kawase-control tests. Then run the native workload at 1920x1080@165 with replay capture and partial Kawase, requiring multiple partial frames with checkpoint dependencies, actual framebuffer blits, and zero missing semantic source pixels before claiming resolution.
