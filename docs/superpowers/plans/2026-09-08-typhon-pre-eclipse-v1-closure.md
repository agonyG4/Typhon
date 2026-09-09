# Typhon Pre-Eclipse V1 Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the pre-Eclipse Typhon effects v1 correctness gates without redesigning the renderer, then produce fresh deterministic evidence and report native qualification only if a controlling TTY/DRM session is available.

**Architecture:** Preserve the existing `LegacyScene`, render graph, Dual Kawase passes, visual-group scheduling, region-local captures, liveness pool, trusted registry/runtime reload path, protocols, and Direct Scanout integration. Make only bounded fixes at the normalization, trusted ABI, reload scheduling, schema, parameter-policy, mask, and built-in shader numerical-contract boundaries.

**Tech Stack:** Rust, GLES 3.00, existing Cargo `target/` directory, `rtk` command wrapper, renderer/effects/native-output tests.

## Global Constraints

- Treat `TYPHON_EFFECTS_PRE_ECLIPSE_V1_CLOSURE_PROMPT_2026-09-08.md` as the execution contract and the fifth checkpoint review as context.
- Do not implement the trusted-asset `StaticTexture` table; retain the internal type only as needed and keep its typed v1 rejection.
- Write tests before implementation changes for each behavioral fix; use the existing build directory and never create a second target tree.
- Use codebase-memory graph discovery for structure, then direct reads/searches for literals and non-code files; check coverage for every source/doc path relied upon.
- Do not use subagents. Preserve unrelated user changes and untracked artifacts.

## Tasks

- [ ] **1. Establish the red tests and exact source boundaries.** Inspect the current graph normalization, damage/footprint, trusted wrapper, reload publication, schema reconciliation, mask, and built-in shader code. Add focused failing tests for non-zero output-domain normalization, transparent pooled-target clearing, footprint-required normalized validity, compositor output-size semantics, parameter-impact rejection, complete schema compatibility, reload redraw scheduling, mask parity, and built-in finite/premultiplied outputs.

- [ ] **2. Repair NormalizeInput coordinate mapping.** Add a separate output-domain uniform and upload input/output domains from texture plans while retaining physical texture dimensions as physical dimensions. Replace local-target/global-domain mixing with output-space mapping and add graph plus real GLES pixel coverage for identical/different non-zero origins, auxiliary domains, differing physical scale, and stage offsets.

- [ ] **3. Make NormalizeInput deterministic and footprint-complete.** Replace discard outside source coverage with explicit transparent black output. Compute normalized pass damage/validity from the consuming stage source query/declared footprint, bounded by the normalized domain and preserving region-local behavior. Add pooled-target reuse and partial-damage auxiliary-footprint regressions.

- [ ] **4. Correct the trusted ABI output dimensions.** Keep `ctx.texture_size` physical to the primary effect input, set `ctx.output_size` from the renderer’s actual compositor output dimensions, preserve actual scale, and document/test the output-space `content_rect` meaning.

- [ ] **5. Enforce v1 parameter-impact policy.** Add a typed configuration error and reject trusted manifest `Footprint` and `Structure` parameters while retaining the internal enum for future versions. Add focused parser/registry tests and update the effects documentation.

- [ ] **6. Complete live-binding schema and reload behavior.** Hash parameter name, ID, type, range, impact, and default using canonical float bits; test all compatibility changes and source-only reload preservation. At successful compatible publication, invalidate renderer history and request one compositor-owned redraw; verify OnDamage/Continuous behavior and failed-reload invariants without a busy loop.

- [ ] **7. Align mask and built-in GPU numerical contracts.** Make GLES inverted/normal mask coverage match the CPU reference for the required alpha and saturated premultiplied cases. Add safe piecewise premultiplied sRGB helpers and sanitize built-in Add/Multiply/Screen/ColorMatrix/Noise outputs for finite normalized SDR values without rewriting arbitrary custom shader semantics. Add CPU/reference and real GLES numerical tests.

- [ ] **8. Update contract documentation.** Document StaticTexture as unsupported in v1, UniformOnly-only trusted parameters, normalization domains and transparent outside behavior, trusted ABI field meanings, and the finite valid premultiplied SDR responsibility for custom GLSL versus built-in sanitization. Update qualification evidence only after fresh verification.

- [ ] **9. Run fresh deterministic verification in the existing target.** Run formatting, locked check, locked clippy, focused effects/renderer/native-output suites, full locked tests, qualification dry-run, and all required focused filters. Record exact fresh counts, failures, and any environmental skips.

- [ ] **10. Perform native qualification only where permitted.** Inspect the controlling TTY/DRM state and run the documented native harness only when it can control the target. If no controlling TTY is available, record that qualification remains deferred and do not claim production hardware qualification.

- [ ] **11. Review, commit, and report evidence.** Review the diff for scope preservation and user artifacts, run final status/diff checks, commit the focused closure changes if the checkout remains a git repository, and report only the evidence-supported v1 readiness or qualification status.

