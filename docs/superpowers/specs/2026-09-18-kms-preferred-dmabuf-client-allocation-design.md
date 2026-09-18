# KMS-Preferred DMA-BUF Client Allocation Design

## Goal

Make initial client DMA-BUF allocation prefer modifiers that the active KMS primary plane can present, without hardcoding vendor-specific modifiers or changing Typhon's dynamic per-surface Direct Scanout feedback and physical qualification paths.

## Root cause

The Atomic EGL/GBM backend currently queries renderer/import-compatible DMA-BUF pairs, derives the narrow opaque-RGB Direct Scanout capability intersection, and then exposes the raw renderer pairs as the renderer tranche. On NVIDIA, the renderer set can contain a modifier that the active primary plane rejects even when a same-FourCC primary-plane-compatible alternative exists. The failure therefore happens before import and atomic TEST_ONLY.

## Boundaries

Two independent sets will remain explicit:

- `KMS-presentable client pairs`: exact primary-plane format/modifier pairs intersected with renderer/import-compatible pairs, without the opaque-RGB filter. This set only drives allocation preference.
- `DirectScanoutFeedbackCapabilities`: the existing exact primary-plane intersection with the current opaque-RGB and invalid-modifier restrictions. This remains the authority for Direct Scanout eligibility.

The raw EGL renderer feedback is not mutated. Both sets are derived from it before a client-facing feedback object is built.

## Policy

`OBLIVION_ONE_DMABUF_KMS_PREFERRED=auto|off|force` is parsed separately from `OBLIVION_ONE_NVIDIA_EGL_WAYLAND2_COMPAT`.

- `off` returns the current renderer set unchanged.
- `auto` applies the generic selection algorithm only for an NVIDIA EGL vendor and only when at least one same-FourCC renderer modifier is removed/replaced by an exact KMS-presentable alternative.
- `force` applies the same generic algorithm for any vendor, with the same no-op behavior when no pair changes.
- Unknown values safely resolve to `off` and produce one bounded startup diagnostic.

The pure selector operates independently for each FOURCC. If a FOURCC has any renderer/KMS modifier intersection, only that intersection is advertised for the FOURCC. If it has no intersection, all renderer modifiers for that FOURCC remain available. Results are sorted and deduplicated by `(fourcc, modifier)`.

## Integration

Atomic EGL/GBM will perform the following sequence:

1. Query raw EGL renderer feedback.
2. Derive existing Direct Scanout capabilities from primary-plane pairs and raw feedback.
3. Derive KMS-presentable client pairs from primary-plane pairs and raw feedback without narrowing formats.
4. Resolve the new policy using the EGL vendor and whether the selector changes the renderer set.
5. Build `EglGlesDmabufFeedback::with_scanout_tranche` with the existing Direct Scanout tranche and the selected renderer tranche.

Because `with_scanout_tranche` derives its table from the supplied tranches, removed renderer modifiers are absent from the effective client format table as well as the fallback tranche.

Default and surface feedback without an active hint remain renderer-only. An active hint retains the existing preferred scanout tranche, target-device handling, same-device normalization, dynamic activation/clearing, and Direct Scanout capability filtering; its fallback tranche is the selected renderer set.

## Observability

The compositor will retain a bounded policy summary containing requested/effective mode and raw, KMS-presentable, advertised, and removed renderer pair counts. Doctor output will report those fields. Live binding diagnostics will continue to read `last_snapshot`, ignore non-surface/inert/dead resources, deduplicate equal snapshots, and withhold effective values for inconsistent snapshots. The snapshot summary will additionally expose fallback-tranche pair count and source-FourCC modifiers, using the existing bounded modifier convention.

## Tests

Tests will cover exact KMS-presentable discovery, ARGB preservation, deterministic deduplication, per-FOURCC selection and fallback, all policy modes and safe parsing, Direct Scanout separation, effective format-table contents, default/surface hint tranche behavior, and actual live snapshot diagnostics including inconsistency and bounded output.

## Non-goals

No NVIDIA modifier constants, application/client special cases, modifier conversion, forced KMS import, weakened TEST_ONLY validation, multi-output union, or changes to the linux-dmabuf v4 bind behavior will be introduced.
