# KMS-Preferred DMA-BUF Client Allocation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prefer exact active-primary-plane-compatible DMA-BUF modifiers per FOURCC for client allocation while preserving Typhon's dynamic Direct Scanout feedback and physical qualification architecture.

**Architecture:** Add a pure, vendor-independent KMS-presentable discovery and per-FOURCC selection module beside the existing device-identity compatibility policy. Atomic EGL/GBM derives both the existing narrow Direct Scanout set and the broader KMS-presentable set from immutable raw EGL feedback, then stores client feedback built from the selected renderer tranche. Compositor state receives a neutral bounded policy summary and reports fallback data from each binding's actual `last_snapshot`.

**Tech Stack:** Rust 2024, Cargo, Wayland linux-dmabuf v4, EGL DMA-BUF modifier queries, KMS `DrmFormatModifierPair`, existing Rust unit/integration tests, `rtk cargo` wrappers.

## Global Constraints

- Do not hardcode any NVIDIA DRM modifier or special-case NVIDIA values, Wine, Vulkan, Cyberpunk, or a FOURCC.
- Keep `DirectScanoutFeedbackCapabilities` narrow and separate from KMS-presentable client formats.
- Do not mutate raw EGL renderer feedback before either capability discovery.
- Preserve default renderer-only feedback, dynamic per-surface hint activation/clearing, scanout tranche ordering, target-device handling, same-device normalization, import, atomic TEST_ONLY, and v4 bind behavior.
- `OBLIVION_ONE_DMABUF_KMS_PREFERRED=off` must preserve the current client renderer set; `auto` is NVIDIA-only when meaningful; `force` is vendor-independent; unknown values safely use `off` with a bounded diagnostic.
- Use the repository-local Cargo target directory; do not redirect compilation artifacts to `/tmp` or another filesystem.
- Preserve unrelated pre-existing worktree changes and commit only files belonging to this feature.

## File Map

- Create `src/native_output/scanout/kms_preferred.rs`: policy parsing, KMS-presentable discovery, deterministic per-FOURCC selection, and focused policy/discovery tests.
- Modify `src/native_output/scanout/mod.rs`: register/re-export the new policy module and expose the backend state accessor.
- Modify `src/native_output/scanout/atomic_egl_gbm.rs`: derive the broader KMS-presentable set, resolve policy after raw feedback discovery, build client feedback from the selected renderer set, and retain Direct Scanout discovery unchanged.
- Modify `src/native_output/scanout/feedback.rs`: pass the neutral policy summary into compositor feedback state.
- Modify `src/native_output/scanout/feedback_policy.rs`: leave the NVIDIA device-identity policy independent; add regression assertions that the new policy has no coupling to it only where appropriate.
- Modify `src/compositor/dmabuf.rs`: define the neutral KMS-preferred summary, add actual-snapshot fallback views, and extend doctor state.
- Modify `src/compositor/state/dmabuf_feedback.rs`: retain/store the summary, derive fallback diagnostics from live snapshots, and test consistency/dead/inert filtering.
- Modify `src/compositor/mod.rs` and `src/compositor/server.rs`: store/re-export the new state and add a setter overload that preserves existing callers.
- Modify `src/render_backend/egl_gles.rs`: add/extend focused feedback tests proving selected tranche formats are also the effective table formats; do not add policy logic here.
- Modify `src/compositor/tests/protocol_buffers.rs`: verify selected fallback modifiers, table contents, default renderer-only semantics, and hinted scanout tranche behavior.
- Modify `src/native_output/runtime/bootstrap.rs`: print bounded KMS-preferred startup diagnostics and retain the existing compatibility diagnostic independently.
- Modify `src/native_output/runtime/cycle_dispatch.rs`: include policy counts and actual fallback snapshot fields in the one-shot direct-scanout doctor output.
- Modify `bin/start-oblivion-one` and `tests/start_launcher.rs`: document and test the new environment escape hatch.
- Modify `docs/superpowers/specs/2026-09-18-kms-preferred-dmabuf-client-allocation-design.md`: already committed as the approved design; do not rewrite during implementation.

---

### Task 1: Add the pure KMS-presentable selector and policy

**Files:**
- Create: `src/native_output/scanout/kms_preferred.rs`
- Modify: `src/native_output/scanout/mod.rs`

**Interfaces:**
- `discover_kms_presentable_client_formats(plane_formats: &[DrmFormatModifierPair], renderer_formats: &[EglGlesDmabufFormat]) -> Vec<DrmFormatModifierPair>` returns the exact pair intersection, sorted/deduplicated, without an opaque-RGB filter.
- `KmsPreferredDmabufPolicy::{from_env, parse, is_known_value, as_str}` parses `OBLIVION_ONE_DMABUF_KMS_PREFERRED` with default `auto` and unknown-value fallback `off`.
- `resolve_kms_preferred_dmabuf_policy(requested, egl_vendor, raw_renderer_formats, kms_presentable_formats) -> KmsPreferredDmabufSelection` returns selected renderer formats plus requested/effective/reason/count diagnostics.
- `KmsPreferredDmabufSelection::summary() -> DmabufKmsPreferredState` converts the selection to the compositor-neutral diagnostic state.

- [ ] **Step 1: Write failing discovery tests** for exact intersection, renderer-only exclusion, KMS-only exclusion, ARGB preservation, exact FOURCC/modifier identity, and deterministic deduplication.
- [ ] **Step 2: Run the focused test target and verify the expected compile/test failure** because the module and functions do not exist yet.

Run: `rtk cargo test --lib native_output::scanout::kms_preferred`

Expected: FAIL because the new module/functions are absent.

- [ ] **Step 3: Implement exact KMS-presentable discovery** by comparing `(format.as_fourcc(), modifier.0)` pairs and sorting/deduplicating the returned `DrmFormatModifierPair` values.
- [ ] **Step 4: Add failing selector tests** for same-FOURCC safe replacement, multiple safe modifiers, no-intersection preservation, FOURCC isolation, no cross-FOURCC substitution, vendor-independent algorithm behavior, and deterministic output.
- [ ] **Step 5: Run the focused tests and verify the selector failures** are caused by the missing selection behavior.
- [ ] **Step 6: Implement the selector and policy resolution** using normalized `(fourcc, modifier)` sets. For each renderer FOURCC, keep only the intersection when non-empty; otherwise retain every renderer modifier for that FOURCC. Mark effective only when the selected set differs and the requested mode permits activation.
- [ ] **Step 7: Add policy tests** for off, auto/NVIDIA, auto/non-NVIDIA, auto/no-change, force/any-vendor, unknown fallback, default parsing, and stable raw/KMS/advertised/removed counts.
- [ ] **Step 8: Run the focused module tests and verify they pass** before integrating the selector.
- [ ] **Step 9: Run `rtk cargo fmt -- --check`** for the new module and fix only formatting issues in feature files.
- [ ] **Step 10: Commit the pure policy module** with `rtk git add src/native_output/scanout/kms_preferred.rs src/native_output/scanout/mod.rs && rtk git commit -m "feat: add KMS-preferred DMA-BUF selector"`.

---

### Task 2: Integrate selection into Atomic EGL/GBM without widening Direct Scanout

**Files:**
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/scanout/mod.rs`
- Modify: `src/native_output/scanout/feedback.rs`
- Test: `src/native_output/scanout/atomic_egl_gbm.rs`

**Interfaces:**
- `AtomicEglGbmScanout` stores the client-facing `EglGlesDmabufFeedback` and `DmabufKmsPreferredState` independently from `DirectScanoutFeedbackCapabilities`.
- `NativeScanoutBackend::dmabuf_kms_preferred_state() -> DmabufKmsPreferredState` returns the Atomic policy state and an off/default state for backends without this path.
- `apply_native_scanout_feedback` passes the stored summary to the compositor while retaining the existing target-device override argument.

- [ ] **Step 1: Write failing integration/unit tests** proving an ARGB KMS-presentable pair survives broader discovery while `discover_direct_scanout_capabilities` still excludes ARGB, and proving the Atomic construction policy can select safe renderer pairs without changing the direct capability result.
- [ ] **Step 2: Run the focused Atomic tests and verify they fail** because the new discovery/selection result is not wired into construction.
- [ ] **Step 3: Keep raw feedback immutable and add the integration sequence** immediately after the raw EGL query: existing direct capability discovery, broader KMS-presentable discovery, policy resolution, then `with_scanout_tranche` using the selected renderer formats.
- [ ] **Step 4: Add the Atomic/backend state accessor** and initialize non-Atomic backends with a safe off summary; do not alter their existing renderer feedback.
- [ ] **Step 5: Update the feedback application path** to send the summary alongside the existing scanout capability and target-device data.
- [ ] **Step 6: Run focused Atomic and scanout tests** and verify all existing Direct Scanout separation tests remain green.
- [ ] **Step 7: Commit the integration** with `rtk git add src/native_output/scanout/atomic_egl_gbm.rs src/native_output/scanout/mod.rs src/native_output/scanout/feedback.rs && rtk git commit -m "feat: apply KMS-preferred renderer feedback"`.

---

### Task 3: Carry policy state and actual fallback snapshots through compositor feedback

**Files:**
- Modify: `src/compositor/dmabuf.rs`
- Modify: `src/compositor/state/dmabuf_feedback.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/render_backend/egl_gles.rs`

**Interfaces:**
- `DmabufKmsPreferredState` is a neutral, copyable, bounded summary with requested/effective mode and raw/KMS-presentable/advertised/removed renderer pair counts.
- `DmabufFeedbackSnapshot::fallback_pair_count()` and `fallback_modifiers_for_fourcc(fourcc)` read only non-scanout formats from the stored snapshot.
- `DmabufFeedbackDoctorState` exposes policy fields plus optional fallback pair/modifier observations.
- Existing feedback setters retain their signatures and default to an off summary; a new overload accepts the summary for the native path.

- [ ] **Step 1: Write failing snapshot/doctor tests** for fallback count/modifiers from actual snapshots, identical snapshot deduplication, inconsistent snapshot withholding, inert/other/dead resource filtering, and bounded fallback output.
- [ ] **Step 2: Run the focused compositor tests and verify the expected failures** because fallback views and fields are absent.
- [ ] **Step 3: Implement snapshot fallback helpers** by unioning only non-scanout tranche pairs, sorting/deduplicating modifiers, and leaving scanout helpers unchanged.
- [ ] **Step 4: Extend summary and doctor state** with fallback observations and the neutral KMS policy summary; preserve `last_snapshot` as the source of truth.
- [ ] **Step 5: Add the server/state setter overload** so existing tests and CPU paths use the off default while Atomic feedback supplies the real summary.
- [ ] **Step 6: Add the focused `EglGlesDmabufFeedback` test** that a selected renderer set is both the fallback tranche and the effective format table, with no removed modifier in either view.
- [ ] **Step 7: Run focused compositor/render-backend tests and verify they pass**.
- [ ] **Step 8: Commit the compositor feedback-state work** with `rtk git add src/compositor/dmabuf.rs src/compositor/state/dmabuf_feedback.rs src/compositor/mod.rs src/compositor/server.rs src/render_backend/egl_gles.rs && rtk git commit -m "feat: observe KMS-preferred feedback snapshots"`.

---

### Task 4: Add protocol regressions for default, hinted surface, and Direct Scanout separation

**Files:**
- Modify: `src/compositor/tests/protocol_buffers.rs`
- Modify: `src/native_output/scanout/feedback_policy.rs`

- [ ] **Step 1: Add a synthetic safe/unsafe XBGR protocol fixture** that builds selected feedback from a renderer pair set and a KMS-presentable safe pair.
- [ ] **Step 2: Run the focused protocol test and verify it fails** because the fixture cannot yet observe the selected format-table/tranche behavior.
- [ ] **Step 3: Assert the default tranche is renderer-only**, its effective table contains the safe pair but not the unsafe pair, and the selected fallback is preserved when no surface hint is active.
- [ ] **Step 4: Activate the existing surface hint and assert scanout tranche ordering/flag/target plus the same selected renderer fallback**, with no unsafe modifier reintroduced.
- [ ] **Step 5: Add/retain the ARGB separation assertion** showing broader KMS-presentable discovery does not add ARGB to `DirectScanoutFeedbackCapabilities`.
- [ ] **Step 6: Run `rtk cargo test --lib compositor::tests::protocol_buffers` and the existing `feedback_policy` tests**.
- [ ] **Step 7: Commit the protocol regressions** with `rtk git add src/compositor/tests/protocol_buffers.rs src/native_output/scanout/feedback_policy.rs && rtk git commit -m "test: cover KMS-preferred client feedback"`.

---

### Task 5: Add startup and one-shot doctor observability

**Files:**
- Modify: `src/native_output/runtime/bootstrap.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/native_output/runtime/metrics.rs` only if the existing diagnostic plumbing requires a field carrier; do not add per-frame logging.

- [ ] **Step 1: Add failing formatting/doctor tests** for requested/effective policy fields, raw/KMS/advertised/removed counts, bounded fallback source format, and deterministic output.
- [ ] **Step 2: Run the focused cycle-dispatch tests and verify failures** because the new fields are not formatted.
- [ ] **Step 3: Print one bounded startup diagnostic** for KMS-preferred policy state alongside, but independently from, the existing NVIDIA device-identity compatibility diagnostic.
- [ ] **Step 4: Extend the one-shot direct-scanout doctor detail** with policy fields and fallback snapshot fields using the existing `MAX_DOCTOR_MODIFIERS` convention.
- [ ] **Step 5: Add tests for repeated identical snapshots and inconsistent snapshots** to prove no arbitrary snapshot is selected.
- [ ] **Step 6: Run focused cycle-dispatch/bootstrap tests and verify they pass**.
- [ ] **Step 7: Commit diagnostics** with `rtk git add src/native_output/runtime/bootstrap.rs src/native_output/runtime/cycle_dispatch.rs src/native_output/runtime/metrics.rs && rtk git commit -m "feat: report KMS-preferred DMA-BUF diagnostics"`.

---

### Task 6: Document the environment policy and run full validation

**Files:**
- Modify: `bin/start-oblivion-one`
- Modify: `tests/start_launcher.rs`

- [ ] **Step 1: Update launcher help** to document `OBLIVION_ONE_DMABUF_KMS_PREFERRED` as `auto` default, `off` escape hatch, and `force` test mode.
- [ ] **Step 2: Add the launcher documentation regression test** and run it to verify the expected failure before the help text change.
- [ ] **Step 3: Implement the help text and verify the launcher test passes**.
- [ ] **Step 4: Run `rtk cargo fmt -- --check`**.
- [ ] **Step 5: Run `rtk cargo check`** using the repository-local target directory.
- [ ] **Step 6: Run `rtk cargo test`** and record exact pass/fail counts.
- [ ] **Step 7: Run `rtk cargo clippy --all-targets --all-features -- -D warnings`** and record exact results.
- [ ] **Step 8: Run `rtk run -c "git diff --check"` and inspect the feature-only diff/status**, confirming unrelated dirty files were not staged.
- [ ] **Step 9: Commit launcher/documentation changes** with `rtk git add bin/start-oblivion-one tests/start_launcher.rs && rtk git commit -m "docs: expose KMS-preferred DMA-BUF policy"`.
- [ ] **Step 10: Re-run the full validation commands after the final commit** and report exact command output, counts, residual unrelated failures, and that no hardware success is claimed without the RTX 3060 Ti acceptance sequence.

## Hardware Follow-up

After software validation, run on the RTX 3060 Ti with effects/blur disabled:

```bash
OBLIVION_ONE_DMABUF_KMS_PREFERRED=auto <existing native Vulkan VK_KHR_wayland_surface probe>
OBLIVION_ONE_DMABUF_KMS_PREFERRED=auto <existing Cyberpunk native Wine-Wayland launch>
OBLIVION_ONE_DMABUF_KMS_PREFERRED=off <existing native Vulkan VK_KHR_wayland_surface probe>
```

Compare one-shot doctor output and client observations for effective policy, fallback XBGR modifiers, primary-plane support, import attempts/failures, TEST_ONLY attempts/rejections, real submits, submissions, and presentations. Accept any active-primary-plane-supported modifier; do not require a specific modifier or bypass validation.
