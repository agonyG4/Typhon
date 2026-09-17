# Opaque RGB Direct Scanout and Doctor Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support explicit opaque XRGB8888 and XBGR8888 Direct Scanout formats while adding bounded one-shot diagnostics for the remaining visible overlay and effect composition blockers.

**Architecture:** Centralize format opacity in `DrmFormat::is_opaque_rgb8888()`, use semantic `PresentationCoverageOpacity::OpaqueRgb8888`, and preserve exact `(fourcc, modifier)` values through scene candidates, physical capability checks, framebuffer import, validation, and KMS transactions. Build layer-shell and effect detail strings only from the doctor dispatch using authoritative compositor state, with explicit bounds and truncation flags.

**Tech Stack:** Rust 2024, Cargo, Wayland compositor state, DRM/KMS Atomic, EGL/GBM, existing unit/integration tests, `astreactl doctor` control output.

## Global Constraints

- Compile and run Cargo commands in `/home/agony/GitHub/Typhon` so artifacts stay in the current folder.
- Do not use subagents.
- Follow red → verify red → minimal green → verify green for every behavior change.
- Do not weaken overlay, effect, layer-shell visibility, or composition rules.
- Do not rewrite `XBGR8888` as `XRGB8888`.
- Do not add ARGB opaque-region scanout, plane allocation, shell namespace exceptions, per-frame diagnostics, or changes to compositor render-target negotiation.
- Use the exact primary-plane capability pair as the physical authority before import.
- Keep doctor-only metadata bounded and omit shader source.

---

### Task 1: Establish baseline and add red format/opacity/scene tests

**Files:**
- Modify: `src/render_backend/buffer.rs` tests
- Modify: `src/compositor/presentation_coverage.rs` tests
- Modify: `src/compositor/state/presentation_coverage.rs` only as needed for test access
- Modify: `src/compositor/direct_scanout.rs` tests and rejection naming assertions
- Modify: `src/compositor/state/direct_scanout.rs` scene qualification tests
- Modify: `src/compositor/state/fullscreen.rs` opacity test coverage if the existing test seam permits it
- Test: existing focused Rust test modules in the files above

**Interfaces:**
- Consumes: existing `DrmFormat`, `PresentationCoverageOpacity`, `DirectScanoutSceneRejection`, and compositor test fixtures.
- Produces: failing tests that require distinct XBGR parsing, semantic opaque RGB coverage, XBGR scene candidacy, and non-opaque ARGB/unknown rejection.

- [ ] **Step 1: Capture the starting evidence in the current checkout.**

Run:

```bash
rtk git rev-parse HEAD
rtk git status --short --branch
./bin/check-source-layout
```

Record the SHA, existing dirty paths, and source-layout result for the completion report. Do not stage or alter the unrelated dirty paths.

- [ ] **Step 2: Add a red `DrmFormat` contract test.**

Add a test in `src/render_backend/buffer.rs` that uses `u32::from_le_bytes(*b"XR24")` and `u32::from_le_bytes(*b"XB24")`, asserts the two enum variants differ, asserts both round-trip their exact FOURCC, and asserts `Xrgb8888`/`Xbgr8888` are opaque while `Argb8888`/`Other(…)` are not.

- [ ] **Step 3: Add red XBGR presentation-coverage and scene-candidate tests.**

Extend the existing compositor fixtures so a full-output one-plane DMA-BUF with `DrmFormat::Xbgr8888`, identity viewport/transform, and no blockers is passed through coverage and scene analysis. Assert semantic opaque coverage and a candidate. Add an ARGB case that remains unknown/rejected. Update the expected rejection variant to the format-neutral name before implementation so the new assertions fail for the missing enum/helper and old XRGB-only match.

- [ ] **Step 4: Run only the new and directly affected tests and verify they fail for the intended reasons.**

Run:

```bash
rtk cargo test --locked render_backend::buffer::tests compositor::presentation_coverage compositor::state::direct_scanout compositor::tests::direct_scanout
```

Expected: compilation/test failure caused by missing `Xbgr8888`, missing `is_opaque_rgb8888`, semantic opacity variant, and format-neutral rejection—not by malformed fixtures. Fix only test setup errors until the feature assertions fail correctly.

- [ ] **Step 5: Commit the red tests.**

```bash
git add src/render_backend/buffer.rs src/compositor/presentation_coverage.rs src/compositor/state/presentation_coverage.rs src/compositor/direct_scanout.rs src/compositor/state/direct_scanout.rs src/compositor/state/fullscreen.rs
git commit -m "test: cover opaque XBGR direct scanout qualification"
```

### Task 2: Implement the centralized opaque RGB model and scene qualification

**Files:**
- Modify: `src/render_backend/buffer.rs`
- Modify: `src/compositor/presentation_coverage.rs`
- Modify: `src/compositor/state/presentation_coverage.rs`
- Modify: `src/compositor/direct_scanout.rs`
- Modify: `src/compositor/state/direct_scanout.rs`
- Modify: `src/compositor/state/fullscreen.rs`
- Test: focused tests from Task 1 and existing alpha/coverage/fullscreen tests

**Interfaces:**
- Consumes: red tests from Task 1 and existing exact-format data flow.
- Produces: `DrmFormat::Xbgr8888`, `DrmFormat::XBGR8888_FOURCC`, `DrmFormat::is_opaque_rgb8888()`, `PresentationCoverageOpacity::OpaqueRgb8888`, and `DirectScanoutSceneRejection::FormatNotProvenOpaque` with string `format_not_proven_opaque`.

- [ ] **Step 1: Add the minimum `DrmFormat` implementation.**

Add the explicit enum variant and associated constant, update only `from_fourcc()` and `as_fourcc()`, and implement:

```rust
pub const fn is_opaque_rgb8888(self) -> bool {
    matches!(self, Self::Xrgb8888 | Self::Xbgr8888)
}
```

Use the helper in `CommittedSurfaceBuffer::alpha_capability()` so XRGB and XBGR are both opaque while ARGB and `Other` remain conservative.

- [ ] **Step 2: Make presentation coverage semantic and conservative.**

Rename `OpaqueXrgb8888` to `OpaqueRgb8888`, update `as_str()` to `opaque_rgb8888`, and make `is_proven_opaque()` match only that semantic variant. In `CompositorState::presentation_coverage_opacity()`, retain every existing geometry/source/size/clip/placement/render-target condition and replace only the XRGB equality with `buffer.format().is_opaque_rgb8888()`.

- [ ] **Step 3: Generalize scene and fullscreen opacity checks.**

Replace the XRGB-specific scene rejection with `FormatNotProvenOpaque` and use `buffer.format().is_opaque_rgb8888()` in `src/compositor/state/direct_scanout.rs` and `src/compositor/state/fullscreen.rs`. Do not change source selection, owner/source identity, effect blockers, overlay blockers, or viewport rules.

- [ ] **Step 4: Run the focused tests and verify green.**

Run the Task 1 command again. Expected: all new format, coverage, ARGB-conservative, and scene-candidate assertions pass, with existing directly affected tests passing.

- [ ] **Step 5: Commit the semantic model.**

```bash
git add src/render_backend/buffer.rs src/compositor/presentation_coverage.rs src/compositor/state/presentation_coverage.rs src/compositor/direct_scanout.rs src/compositor/state/direct_scanout.rs src/compositor/state/fullscreen.rs
git commit -m "feat: generalize opaque RGB8888 scanout qualification"
```

### Task 3: Add red native validation and exact framebuffer identity tests

**Files:**
- Modify: `src/native_output/scanout/direct.rs` tests
- Modify: `src/native_output/tests/scanout.rs` if shared AddFB2 test helpers need an XBGR case
- Modify: `src/native_output/scanout/direct_validation.rs` tests for format-bearing validation keys
- Test: `src/native_output/scanout/direct.rs`, `src/native_output/scanout/direct_validation.rs`, and native scanout test targets

**Interfaces:**
- Consumes: semantic opaque format helper and existing `DmabufBufferHandle`/`DirectFramebufferIo` test doubles.
- Produces: failing tests for valid one-plane XBGR validation, invalid stride/offset/modifier/plane count rejection, and exact XBGR FOURCC in imported framebuffer metadata and validation identity.

- [ ] **Step 1: Add red direct-import tests.**

Add a test helper that constructs a valid one-plane 4-byte XBGR DMA-BUF and tests `validate_direct_dma_buf()` succeeds. Add separate tests that mutate one condition at a time: zero/too-small stride, nonzero offset, `DrmModifier::INVALID`, and non-single-plane layout. Assert the errors remain rejected and contain the format-neutral structural message where applicable.

- [ ] **Step 2: Add a red exact-FOURCC import test.**

Use the existing fake framebuffer IO and import an XBGR buffer. Assert `ImportedDirectFramebuffer.format == DrmFormat::XBGR8888_FOURCC` and, where the fake IO records the descriptor, assert `ExplicitFramebufferDescriptor::format()` is the same XBGR FOURCC. The test must fail before the validator accepts XBGR.

- [ ] **Step 3: Run the native tests and verify the failures are feature failures.**

Run:

```bash
rtk cargo test --locked native_output::scanout::direct native_output::scanout::direct_validation native_output::tests::scanout
```

Expected: XBGR validation and exact identity tests fail because the importer still accepts only XRGB; structural negative tests continue to fail only if their fixtures are invalidly constructed.

- [ ] **Step 4: Commit the native red tests.**

```bash
git add src/native_output/scanout/direct.rs src/native_output/scanout/direct_validation.rs src/native_output/tests/scanout.rs
git commit -m "test: require exact XBGR native scanout identity"
```

### Task 4: Generalize native validation without changing physical authority

**Files:**
- Modify: `src/native_output/scanout/direct.rs`
- Modify: `src/native_output/scanout/direct_validation.rs` only if test fixture identity needs adjustment
- Test: native tests from Task 3 plus existing import/cleanup tests

**Interfaces:**
- Consumes: `DrmFormat::is_opaque_rgb8888()` and exact-format red tests.
- Produces: native validation accepting XRGB/XBGR only, preserving all existing structural checks and forwarding the exact FOURCC.

- [ ] **Step 1: Replace the native XRGB-only format check.**

In `validate_direct_dma_buf()`, reject only when `!buffer.format().is_opaque_rgb8888()`. Use format-neutral errors such as `opaque RGB8888 direct scanout must have one plane`, `opaque RGB8888 stride overflow`, and `impossible opaque RGB8888 dma-buf layout`. Keep one-plane, nonzero stride, offset zero, minimum width×4 stride, valid modifier, and contiguous-index checks unchanged.

- [ ] **Step 2: Confirm the existing import path remains exact.**

Keep `DmabufImageKey::from_handle()`, `ExplicitFramebufferDescriptor::new()`, and `ImportedDirectFramebuffer::format` fed by `buffer.format().as_fourcc()`. Do not add any conversion or fallback to XRGB.

- [ ] **Step 3: Run the native tests and verify green.**

Run the Task 3 command. Expected: valid XRGB and XBGR imports pass, ARGB/unknown formats and all structural invalid cases reject, and AddFB2 cleanup/error diagnostics remain green.

- [ ] **Step 4: Commit the native implementation.**

```bash
git add src/native_output/scanout/direct.rs src/native_output/scanout/direct_validation.rs
git commit -m "feat: accept explicit opaque RGB8888 direct imports"
```

### Task 5: Add red and green Atomic capability-discovery tests

**Files:**
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/tests/scanout.rs` only if the extracted helper is intentionally tested from the native test module
- Test: Atomic capability-discovery unit tests and existing format-negotiation tests

**Interfaces:**
- Consumes: `DrmFormat::from_fourcc()`, `is_opaque_rgb8888()`, `EglGlesDmabufFeedback::supports()`, `GbmAllocationProbe`, and `DrmFormatModifierPair`.
- Produces: a bounded helper that returns exact primary-plane scanout capability pairs after opaque-format, modifier, renderer-feedback, and GBM probe checks.

- [ ] **Step 1: Add red helper-level capability tests.**

Extract the bootstrap filter into a testable helper with inputs `&[DrmFormatModifierPair]`, renderer feedback, and a mutable `GbmAllocationProbe`. Add tests proving: supported XRGB and XBGR exact pairs are returned; ARGB, invalid modifier, missing EGL feedback, and GBM-rejected pairs are omitted; and returned `format` values equal the original FOURCC without normalization.

- [ ] **Step 2: Run the helper tests and verify they fail before implementation.**

Run:

```bash
rtk cargo test --locked native_output::scanout::atomic_egl_gbm
```

Expected: helper symbol/behavior failures, not fixture errors.

- [ ] **Step 3: Implement the exact-format Atomic filter.**

Use `let drm_format = DrmFormat::from_fourcc(format.fourcc);`, require `drm_format.is_opaque_rgb8888()`, reject `DRM_FORMAT_MOD_INVALID`, require `renderer_dmabuf_feedback.supports(drm_format, modifier)`, require the existing GBM probe, and push the original `format.fourcc` plus modifier. Replace the inline loop with the helper.

- [ ] **Step 4: Run capability tests and verify green.**

Run the Task 5 test command and the existing `native_output::tests::scanout` format-negotiation tests. Confirm compositor render-target `preference_key()` remains unchanged.

- [ ] **Step 5: Commit capability discovery.**

```bash
git add src/native_output/scanout/atomic_egl_gbm.rs
git commit -m "feat: discover exact opaque RGB scanout capabilities"
```

### Task 6: Add red doctor diagnostics tests and state adapters

**Files:**
- Modify: `src/compositor/layer_shell.rs`
- Modify: `src/compositor/effects.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs` tests
- Test: layer-shell/effect state tests and doctor formatting tests

**Interfaces:**
- Consumes: authoritative `LayerSurfaceRole`, active renderable surfaces/targets, `ResolvedEffectScene`, and existing doctor scene formatting.
- Produces: doctor-only server adapters returning bounded `visible_above` detail entries and bounded effect detail entries, each with explicit counts/truncation information.

- [ ] **Step 1: Add red pure formatting tests for layer-shell details.**

Add a test fixture/formatter assertion requiring a layer entry to contain root, namespace, committed layer, mapped state, configured geometry, surface count/IDs, output intersection, and buffer source/format when available. Add a truncation assertion for more than the chosen root/detail bound.

- [ ] **Step 2: Add red pure formatting tests for effect details.**

Build a resolved effect fixture and assert the detail contains visible instance count, instance ID, program identity/name, anchor, output-space region, target surface where available, `requires_composition:true`, and an explicit truncation marker when over the bound. Do not include shader source.

- [ ] **Step 3: Run the diagnostics tests and verify red.**

Run:

```bash
rtk cargo test --locked compositor::layer_shell compositor::effects native_output::runtime::cycle_dispatch
```

Expected: missing diagnostic adapters/fields or missing formatted metadata.

- [ ] **Step 4: Implement the layer-shell doctor adapter.**

In the compositor state, compute active render-space targets only when the adapter is called, select only roots already classified as visible layer-shell content above the scanout source, and format authoritative role fields plus bounded active surface metadata. Do not alter `analyze_presentation_coverage()` or the layer-shell blocker classification. Expose root/detail truncation explicitly.

- [ ] **Step 5: Implement the effect doctor adapter.**

Resolve the existing effect scene only from the doctor adapter, map each program ID to its registered name when available (using a stable built-in/background-blur name), format anchor and output-space region, include target surface IDs for surface anchors, and cap entries with an explicit truncation flag. Do not trace frames or include shader text.

- [ ] **Step 6: Expose adapters through `OwnCompositorServer` and verify green.**

Add narrow public server methods used only by the native doctor dispatch. Run the Task 6 test command and confirm ordinary scene/render paths do not call the new adapters.

- [ ] **Step 7: Commit the state adapters and their tests.**

```bash
git add src/compositor/layer_shell.rs src/compositor/effects.rs src/compositor/server.rs src/native_output/runtime/cycle_dispatch.rs
git commit -m "feat: expose bounded scanout blocker diagnostics"
```

### Task 7: Integrate exact scanout format and detailed doctor output

**Files:**
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/compositor/tests/direct_scanout.rs` and `src/compositor/state/desktop_window_tests.rs` for renamed blocker assertions
- Test: doctor output tests, compositor scene tests, and native direct-scanout integration tests

**Interfaces:**
- Consumes: semantic coverage, exact candidate buffer format, `NativeScanoutBackend::dmabuf_scanout_capabilities()`, and the two server diagnostic adapters.
- Produces: one-shot output with `scanout_format={fourcc,name,proven_opaque,primary_plane_supported}`, detailed `visible_above`, detailed `effects`, and unchanged blocker semantics.

- [ ] **Step 1: Add red doctor output assertions.**

Extend `DirectScanoutDoctorScene` fixtures and formatting tests to require an XBGR block with `fourcc:0x34324258`, `name:XBGR8888`, `proven_opaque:true`, and exact capability result; detailed layer/effect entries; and explicit truncation fields. Assert `format_not_opaque_xrgb8888` is absent while `overlay_visible` and `effect_requires_composition` remain present when supplied.

- [ ] **Step 2: Run doctor tests and verify red.**

Run:

```bash
rtk cargo test --locked native_output::runtime::cycle_dispatch compositor::tests::direct_scanout compositor::state::desktop_window_tests
```

Expected: missing doctor fields/arguments or old format output.

- [ ] **Step 3: Add exact scanout-format derivation.**

Derive the source format from the coverage-selected physical source, not the logical owner. Use `analysis.coverage.opacity.is_proven_opaque()` for the proof and query the native scanout capability authority with the exact source FOURCC and first-plane modifier. Report unknown support only when the native capability set is unavailable; never substitute XRGB.

- [ ] **Step 4: Wire detailed visible-content and effect entries into the doctor snapshot.**

Keep summary entries for non-layer content, replace layer-shell summary entries with the detailed state adapter output, and include effect count/details/truncation. Bound all lists and preserve the existing one-shot dispatch location; no frame-loop calls or allocations are added to normal presentation.

- [ ] **Step 5: Run focused doctor/scene/native tests and verify green.**

Run the Task 7 command plus:

```bash
rtk cargo test --locked native_output::scanout::direct native_output::scanout::atomic_egl_gbm compositor::tests::protocol_buffers
```

Verify exact XBGR capability rejection occurs before import and exact XBGR acceptance proceeds to the existing import/TEST_ONLY stages.

- [ ] **Step 6: Commit the integrated diagnostics.**

```bash
git add src/native_output/runtime/cycle_dispatch.rs src/compositor/tests/direct_scanout.rs src/compositor/state/desktop_window_tests.rs
git commit -m "feat: diagnose opaque RGB scanout blockers"
```

### Task 8: Full deterministic verification, source-layout comparison, and hardware retest

**Files:**
- Modify: none unless verification exposes a directly caused issue; any fix must add a red test first and be committed separately
- Test: repository-wide locked checks and real Cyberpunk session

**Interfaces:**
- Consumes: all committed implementation tasks and the recorded starting baseline.
- Produces: verified Git SHA, test/check results, source-layout baseline/final, diff hygiene, and hardware blocker evidence.

- [ ] **Step 1: Run the required deterministic checks from the repository folder.**

Run each command freshly and retain exit status/output:

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
./bin/check-source-layout
rtk git diff --check
```

If a global command fails due to pre-existing dirty work, isolate the exact failure and run changed-file-focused checks; report the distinction without claiming global success.

- [ ] **Step 2: Audit exact-format flow and diff scope.**

Run:

```bash
rtk rg -n -g '*.rs' 'Xbgr8888|XBGR8888_FOURCC|FormatNotProvenOpaque|OpaqueRgb8888|primary_plane_format_modifier_unsupported|format\(\)\.as_fourcc\(\)' src
rtk git diff --stat HEAD~8..HEAD
rtk git status --short --branch
```

Confirm no new XRGB substitution, no change to `format_negotiation.rs` preferences, no namespace allowlist, and no frame-loop diagnostic call.

- [ ] **Step 3: Retest real Cyberpunk with the experimental setting.**

Run the existing approved launch/session procedure with:

```bash
OBLIVION_ONE_DIRECT_SCANOUT=experimental-auto
```

Use only one `astreactl doctor` snapshot. Confirm the previous `format_not_opaque_xrgb8888` blocker is absent, inspect `scanout_format` exact FOURCC/capability status, and correlate detailed layer roots with effect target/anchor metadata. Preserve composition if visible overlay/effect content remains genuinely contributing. Do not claim Direct Scanout is fixed unless physical qualification and eventual presentation counters prove it.

- [ ] **Step 4: Commit any final verification-only documentation if needed and record ending SHA.**

Run:

```bash
rtk git rev-parse HEAD
rtk git status --short --branch
```

The final report must include starting/ending SHA, red failure reasons, exact format and opacity changes, exact physical flow/capability behavior, unchanged compositor negotiation, doctor output, focused/full results, source-layout baseline/final, diff check, remaining hardware blockers, and whether Cyberpunk was retested.
