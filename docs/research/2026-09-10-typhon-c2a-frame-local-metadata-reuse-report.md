# Typhon C2a — Frame-Local Metadata Reuse and Graph Planning Qualification

Date: 2026-09-10

Scope: C2a from the 2026-09-07 performance/resource audit. This report covers
same-resolved-frame metadata reuse, exact graph peak-live analysis, and bounded
measurement of the remaining scene/effect metadata work. It does not introduce
a persistent scene cache or effect topology cache.

## Result

C2a is complete. The implementation has two focused production changes:

1. `ResolvedNativeFrameScene` now owns one complete immutable
   `NativeSceneSnapshot` and one cached exact-frame identity signature.
2. Render-graph peak-live analysis now uses inclusive lifetime intervals and an
   event sweep with O(passes + textures) work.

The remaining repeated effect compiler work is instrumented. The measurements
support a separate C2b investigation into immutable validated-program lowering
metadata, but do not justify a cross-frame scene cache or full effect-frame
topology cache yet.

## 1. Audit mechanism and source evidence

The audit finding was “reduce graph and scene metadata reconstruction,” marked
“Measurement required.” Source inspection confirmed two different mechanisms:

- Same-frame duplication was provably redundant. The old
  `ResolvedNativeFrameScene::snapshot()` cloned the stored snapshot, copied
  popup and overlay IDs, recomputed effect damage and effect identity, and
  recomputed visibility metadata. `scene_identity_signature()` called that
  method and then scanned the newly finalized snapshot again.
- Cross-frame scene and effect reconstruction was not automatically cacheable.
  Presentation samples can change transformed bounds and effect regions while
  canonical scene generation remains stable. `compile_instance()` also rebuilt
  per-instance node/output maps from immutable validated program data.

The relevant source paths remain the authority: `frame.rs`, `scene_history.rs`,
`presentation_worker.rs`, `atomic_egl_gbm.rs`, `damage.rs`, and
`render_graph.rs`. The presentation-animation design requirement is preserved:
instrument first, and defer stable-topology caching until measurements justify a
complete key.

## 2. Resolved-frame snapshot flow

### Before

The resolved frame first built a base snapshot, then resolved effects. Each
consumer could call the generic `snapshot()` method. In the atomic path, the
sequence was effectively:

```text
snapshot()                  -> clone/finalize metadata
scene_identity_signature()  -> snapshot() again, then full identity scan
```

Damage and scene-history paths also obtained an owned snapshot through the same
reconstruction path.

The RED regression captured this directly: the pre-change test observed two
snapshot finalizations where one exact resolved-frame finalization was expected
(`left: 2`, `right: 1`).

### After

`ResolvedNativeFrameScene::from_server_at()` now:

1. resolves surfaces, presentation, decorations, and effects;
2. builds the base `NativeSceneSnapshot` once;
3. installs popup IDs, external overlay IDs, visibility signature, effect
   damage, and effect identity signature once;
4. computes the exact scene identity once; and
5. publishes the complete immutable snapshot and cached signature together.

The borrow API is `snapshot_ref()`. The explicit ownership API is
`snapshot_owned()`. The generic reconstruction-shaped `snapshot()` API is gone.

`scene_identity_signature()` is now an O(1) read of the cached field. Its
semantics remain:

```text
NativeSceneSnapshot::identity_signature()
    XOR effects.signature
    then multiply by the existing FNV constant
```

The visibility mixing logic was extracted into a pure helper without changing
its fields or arithmetic.

## 3. Snapshot counts and ownership proof

The test-only local work counter reports, for one resolved frame:

| Work | Before | After |
|---|---:|---:|
| Exact snapshot finalization during resolution | incomplete base plus consumer finalization | 1 |
| Additional finalization from repeated snapshot/identity reads | 2 in the RED atomic-shaped sequence | 0 |
| Full identity computations after construction | once per identity call | 0; one at construction |
| Current snapshot clone for damage comparison | 1 | 0 |
| Exact owned history/ready snapshot clone | 1 reconstruction-shaped copy | 1 deliberate `snapshot_owned()` copy |
| Atomic ready/transaction snapshot clone | 1 reconstruction-shaped copy plus identity reconstruction | 1 deliberate `snapshot_owned()` copy |

The after-path regression performs repeated borrowed snapshot and signature reads,
then explicitly requests one owned snapshot. It observes:

```text
construction: finalizations=1, identity_computations=1
repeated reads: finalizations=0, owned_clones=0, identity_computations=0
explicit ownership boundary: owned_clones=1
```

The damage path passes `resolved_scene.snapshot_ref()` into
`native_output_damage_for_scene_snapshots()`. The previous presented snapshot is
still an independently owned frozen history value.

Remaining production copies are intentional:

- `NativeFrameSceneSnapshot::from_resolved_frame_scene()` performs one
  `snapshot_owned()` clone to freeze exact ready/history ownership.
- The atomic rendered-frame path performs one `snapshot_owned()` clone at its
  ready/transaction ownership boundary.
- `NativeFrameRenderer::render_server_frame()` still copies the external overlay
  ID vector into an owned request. This is a small request-ownership copy, not a
  `NativeSceneSnapshot` reconstruction.

No remaining hot-path `NativeSceneSnapshot` clone is unexplained. SHM snapshots
remain compositor-owned Arc/COW values; C2a does not deep-copy pixel payloads.

## 4. Regression coverage

The native regressions cover:

- finalized dynamic fields: popup IDs, external overlays, visibility,
  effect-damage union, and effect identity;
- borrowed snapshot equality with the explicit owned API;
- exact cached identity arithmetic;
- repeated reads with zero additional finalization, identity scans, or clones;
- scene-history ownership through `snapshot_owned()`;
- presentation animation samples with equal canonical render generation but
  different transformed bounds and different exact-frame identity;
- existing damage, fullscreen, scene-history, and presentation tests.

The animation regression samples two timestamps in one transition and proves:

```text
same render_generation
different snapshot bounds
different scene identity
one finalization and one identity computation per sample
```

The frozen history path remains value-owned. Resolving a later scene cannot
mutate an earlier ready/submitted/presented snapshot. Scene-history capacity and
promotion/discard semantics were not changed.

## 5. Peak-live analysis

### Before

After graph fusion, the old diagnostic calculation scanned every texture for
every pass:

```text
for each pass P:
    scan each texture T
```

This was O(P × T). It also made the diagnostic work scale with the product of
the two graph dimensions even though each texture already carried
`first_use`/`last_use`.

### After

The new helper runs after `fuse_compatible_local_stages()`. Each intermediate
texture contributes one inclusive interval `[first_use, last_use]`:

```text
delta[first_index] += 1
delta[last_index + 1] -= 1
```

The sweep performs one step per pass. `GraphPassId(1)` is converted to pass
index zero. A one-pass lifetime therefore contributes for exactly one pass.

Malformed one-sided or reversed metadata is debug-asserted and skipped rather
than causing a production diagnostic panic. Stable pass-ID gaps left by fusion
are conservatively clamped to the compact post-fusion pass range, preserving
the old per-pass definition.

Complexity is:

```text
O(T) interval insertions + O(P) sweep steps
O(P) auxiliary memory
```

`RenderGraphCompileStats` and all of its meanings are unchanged.

The brute-force reference remains test-only. Deterministic tests cover one/one,
overlap, disjoint, nested, same first/last use, unused, non-intermediate, and
many-interval cases. The fused-stage test compares the sweep to the brute-force
definition after fusion. The synthetic linear-work test uses P=256 and T=512
and observes exactly 256 sweep steps and 512 interval insertions.

## 6. Metadata measurements

All measurements below are deterministic test work units, not fabricated heap
bytes or wall-clock claims. Counters are test-only and thread-local.

### Native snapshot work

The native snapshot workload reports one build and one visual-group build for
each fixture. Surface projection visits scale with the input surface count:

| Workload | Snapshot builds | Surface projection visits | Visual-group builds |
|---|---:|---:|---:|
| 1 ordinary surface, no effects | 1 | 1 | 1 |
| 16 ordinary surfaces | 1 | 16 | 1 |
| 128 ordinary surfaces | 1 | 128 | 1 |
| 32-surface deep subsurface tree with popup membership | 1 | 32 | 1 |

This confirms real per-frame projection/group reconstruction. It does not prove
that the pointer hit-test cache or another visual-stack cache has compatible
invalidations; no such reuse was added.

### Effect graph work

The counters include graph compiles, instance compiles, node visits, per-instance
program lookup-map builds, output-map builds, interval insertions, and sweep
steps.

| Workload | Graph compiles | Instance compiles | Node visits | Lookup maps | Output maps | Passes | Textures | Intermediate | Peak live |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 blur | 1 | 1 | 2 | 1 | 1 | 6 | 6 | 4 | 2 |
| 8 blur instances | 1 | 8 | 16 | 8 | 8 | 48 | 41 | 32 | 2 |
| 32 blur instances | 1 | 32 | 64 | 32 | 32 | 192 | 161 | 128 | 2 |

The new peak-live work for those rows is respectively 4/6, 32/48, and
128/192 interval insertions/sweep steps, rather than a P×T candidate scan.

### Uniform-only parameter animation

The program, anchor, region, target bounds, ordering, and registry are held
constant while one uniform value changes. Both samples repeat exactly:

```text
graph_compiles=1
instance_compiles=1
node_visits=2
program_lookup_map_builds=1
output_map_builds=1
interval_insertions=4
sweep_steps=6
```

Passes, textures, and peak-live statistics are identical. In this deterministic
fixture, 1/1 changed samples have structurally reusable topology, but this is
not a production hit-rate estimate.

### Effect geometry animation

The same blur program is compiled once for a 320×180 region and once for a
640×240 region. The compiled texture plans differ, while both samples still
perform one graph compile, one instance compile, one node traversal, one
program lookup-map build, and one output-map build. This is the required
counterexample to treating all effect frames as one reusable topology.

No elapsed-time or allocator-byte measurements were performed. Native-output
hardware timing was unavailable in this environment, and the existing perf
logger was not extended with always-on clocks. No global allocator replacement
was used, and no `size_of` estimate was presented as heap traffic.

## 7. C2b decision gate

Ranked recommendation:

1. **C2b justified: stable validated-program metadata reuse — investigate
   first.** The repeated per-instance lookup/output maps and node traversal are
   directly observed, including 32 lookup-map builds and 32 output-map builds in
   the 32-instance fixture. A later task can pre-resolve immutable topological
   node/input indices during validation or registry publication. This is a
   bounded design and does not require a cross-frame cache key.
2. **Native scene topology projection cache — not justified yet.** Snapshot
   projection is measurable and linear, but C2a has not separated stable surface
   topology from presentation-transformed bounds, decoration geometry, content
   generations, damage evidence, or effect transition metadata. More timing and
   invalidation evidence is required.
3. **Full effect-frame topology cache — not justified yet.** Uniform-only values
   preserve topology in the fixture, while changing geometry changes texture
   plans. `ResolvedEffectScene.signature` also includes dynamic parameter values,
   so it is not a topology key. A future key must include registry generation,
   output bounds, ordered instance identity/program/anchor/scope/group/order,
   target geometry and effect regions wherever they affect capture/layout/fusion.
   Future non-`UniformOnly` parameter impacts must either enter that structural
   key or bypass the cache.

The measured fraction of structurally reusable topology is therefore “not yet
known” for real workloads; only the bounded uniform-only fixture is 1/1. The
measurements do not establish that key construction and graph materialization
would cost less than compilation. No persistent graph or scene cache was
introduced.

## 8. Optional cleanup and invariants

The retained debug consistency checks compare iterators directly and do not
allocate temporary ID vectors. No broad micro-allocation sweep was undertaken.

Unchanged authorities and policies:

- native damage authority and effect transition damage;
- ready/submitted/presented scene-history ownership and capacity;
- B1 SHM Arc/COW ownership;
- B2 effect dependency closure, execution demand, and pruning;
- C1 consumer-driven resource realization and synchronization;
- effect resource/shader behavior;
- Direct Scanout eligibility, identity, release ownership, and KMS worker;
- presentation sampling, retargeting, settlement, acknowledgement, and input
  inverse mapping.

## 9. Verification

Focused tests passed in the clean C2a state:

- resolved-frame scene tests: 6 passed;
- scene-history tests: 10 passed;
- native output tests: 59 passed;
- fullscreen frame-scene tests: 4 passed;
- render-graph tests: 25 passed;
- changing-geometry qualification: 1 passed.

The clean-state checks also passed:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
```

The full locked test run on the clean C2a state completed with 2304 passed,
2 ignored, and 3 unrelated failures:

- two compositor maximize/tiled interaction expectations in
  `src/compositor/tests/input_output/window_interaction.rs`;
- one KMS out-fence expectation in `src/native/kms/tests.rs`.

These failures are outside the C2a implementation. The shared checkout now has
additional concurrent blur-policy edits that do not compile independently
(`WindowBackend` import, an invalid `?`, a shadowed resolver binding, and a
non-exhaustive `WEnum` match); those edits were preserved and not included in
C2a verification or commits.

`./bin/check-source-layout` remains failing on pre-existing oversized modules,
including compositor test/state modules, native-output runtime modules,
`src/native_output/output/damage.rs`, `src/native_output/scanout/atomic_egl_gbm.rs`,
`src/native_output/tests/output.rs`, and `src/effects/render_graph.rs`. No
unrelated module refactor was attempted. `git diff --check` was clean for the
C2a changes.

## 10. Commits and files

C2a commits:

- `e08d5ad` — design specification;
- `44f84bf` — implementation plan;
- `cbc7edf` — `perf(native): reuse resolved frame metadata`;
- `b4c9278` — `perf(effects): linearize graph lifetime analysis`;
- `a8cea89` — deterministic changing-geometry qualification.

Changed C2a files:

- `src/native_output/runtime/frame.rs`
- `src/native_output/runtime/scene_history.rs`
- `src/native_output/runtime/presentation_worker.rs`
- `src/native_output/scanout/atomic_egl_gbm.rs`
- `src/native_output/output/damage.rs`
- `src/native_output/tests/fullscreen_frame_scene.rs`
- `src/native_output/tests/output.rs`
- `src/effects/render_graph.rs`
- the C2a design, plan, and this report under `docs/`

Concurrent user changes in the working tree were not staged or included.

## 11. Required yes/no answers

1. Can one `ResolvedNativeFrameScene` rebuild/finalize its snapshot multiple
   times merely because consumers need damage, identity, and history metadata?
   **No.**
2. Can `scene_identity_signature()` require another snapshot clone or complete
   surface/decorations scan after exact identity was computed?
   **No.**
3. Can native damage comparison require an owned clone of the current exact
   snapshot?
   **No.**
4. Can ready/submitted/presented snapshots refer back to mutable current-scene
   state?
   **No.**
5. Can two presentation-animation samples share one frame-local snapshot merely
   because canonical generation is unchanged?
   **No.**
6. Does `peak_live_intermediates` still require every pass to scan every
   texture?
   **No.**
7. Does the new calculation preserve inclusive first/last semantics after
   fusion?
   **Yes.**
8. Was a persistent graph/scene cache introduced without measured
   justification?
   **No.**
9. Do C1, B2, presentation damage, Direct Scanout, and exact scene-history
   ownership remain unchanged?
   **Yes.**
