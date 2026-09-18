# Capture GPU Attribution Design

## Goal

Attribute the existing GPU capture timestamp spans to their physical capture
kind, execution mode, checkpoint subset, and bounded execution work so native
qualification can distinguish replay capture from framebuffer blits before any
effects optimization is attempted.

## Design

The executor remains the sole owner of capture execution semantics. At the
existing pass-timing call site it uses `is_direct_framebuffer_capture` and
`pass.checkpoint_dependencies.len()` to attach fixed-size typed metadata to the
already existing pass timestamp span. Non-capture passes receive no capture
metadata.

The profiler aggregates valid resolved spans into separate SceneCapture and
SurfaceCapture, replay and framebuffer-blit, and checkpoint subsets. It also
retains one bounded maximum capture-pass record. Legacy `capture_ns` and
effect-space `capture_pixels` remain derived from the existing capture
categories.

Execution work is recorded in `execute_capture` from the existing materialized
`capture_rects` and replay `indices`. A successful graph attaches the resulting
fixed-size summary to its `GraphAggregate` by `scope_id` before the existing
total span is finalized. Failed executions close timing as before and leave the
execution summary unavailable rather than fabricating one.

## Query and behavior constraints

The timestamp query pool, one begin/end pair per executed pass, asynchronous
collection, invalid/disjoint handling, graph execution, capture policy, damage,
checkpoint order, shaders, presentation, and pacing remain unchanged.

## Verification

GPU aggregation tests cover capture kind, execution mode, checkpoint subset,
maximum pass selection, invalid spans, scope ownership, and fixed query counts.
Executor tests cover authority-derived metadata and physical execution work.
Focused and full Rust gates plus the source-layout gate are run before the
native replay-policy qualification command.
