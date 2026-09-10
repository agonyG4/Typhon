# Typhon Blur Policy Semantics Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Repair the existing Typhon blur policy’s persistent schema, canonical assignment semantics, opaque-region optimization, runtime status, reload behavior, and real direct-scanout integration.

**Architecture:** Keep `BlurPolicyConfig` as the bounded persistent input and compile its named rules once into `CompiledBlurRules`. Keep `BlurPolicySnapshot` as runtime status only. Make `ResolvedBlurAssignment` the one compositor decision carrying source, scope, surface ID, and bounded region; feed it directly into `resolved_effect_scene()` and status calculation.

**Tech Stack:** Rust 1.70+, serde/serde_json, regex, Wayland server test fixtures, existing Effects v1 and direct-scanout integration helpers.

## Global Constraints

- Preserve the existing Effects v1 renderer, `EffectSceneOrder`, trusted `Surface`/`VisualGroup` semantics, client exact-region deduplication, renderer capability separation, transactional reloads, and existing control commands.
- Wayland applications support `auto`, `rules_only`, and `disabled`; XWayland supports `rules_only` and `disabled`; layer default supports `client_only` and `disabled`.
- Persistent window/layer rules use `name`, `match`, and `blur`; the flattened `app_id`/`action` form is not accepted at the JSON boundary.
- Configuration file limit is 256 KiB; each rule list is limited to 128 entries; rule names are limited to 128 bytes; regex patterns are limited to 256 bytes.
- Compile in the existing Typhon checkout and reuse the existing `target/` directory; do not create an alternate build tree.

---

### Task 1: Separate the persistent config model from runtime status

**Files:**
- Modify: `src/blur_policy/model.rs`
- Modify: `src/blur_policy/config.rs`
- Modify: `src/blur_policy/mod.rs`
- Test: `src/blur_policy/model.rs`
- Test: `src/blur_policy/config.rs`

**Interfaces:**
- Produces `BlurPolicyConfig`, `BlurPolicySnapshot`, `BlurWaylandMode`/`BlurApplicationMode`, `BlurXwaylandMode`, `BlurLayerMode`, `BlurWindowRule`, `BlurLayerRule`, and their bounded serde behavior.
- `config::load()` and `load_from_path()` return `BlurPolicyConfig`, never `BlurPolicySnapshot`.

- [ ] **Step 1: Write failing tests for the public schema and runtime-field rejection.** Add JSON fixtures with nested `match` and `blur`, an `xwayland: auto` fixture, a `layers.default: auto` fixture, and a `renderer_supported` fixture. Assert only the approved config parses, invalid modes fail, and runtime-only fields fail.

- [ ] **Step 2: Run the focused model/config tests and confirm the expected failures.**

Run:

```bash
rtk cargo test --locked blur_policy::model blur_policy::config
```

Expected: the new schema tests fail against the current flattened model and runtime-bearing snapshot.

- [ ] **Step 3: Implement the two model layers.** Make the config rule structs use `name`, `#[serde(rename = "match")] matcher`, and `blur`; add separate mode enums so invalid v1 combinations have no serde representation; move renderer/generation/status fields into the runtime snapshot; add bounded status count structures and source enum.

- [ ] **Step 4: Add loader bounds and config conversion behavior.** Set `MAX_CONFIG_BYTES` to 256 KiB and `MAX_RULES` to 128, keep missing files on built-in defaults, reject unknown fields through `deny_unknown_fields`, and return `(PathBuf, BlurPolicyConfig)` from `load()`.

- [ ] **Step 5: Run the focused tests and verify they pass.**

Run:

```bash
rtk cargo test --locked blur_policy::model blur_policy::config
```

Expected: schema, mode, bounds, and runtime-field rejection tests pass.

- [ ] **Step 6: Commit the model/config boundary.**

```bash
rtk git add src/blur_policy/model.rs src/blur_policy/config.rs src/blur_policy/mod.rs
rtk git commit -m "fix(blur): separate persistent config from runtime status"
```

### Task 2: Compile bounded named rules and preserve matching semantics

**Files:**
- Modify: `src/blur_policy/rules.rs`
- Modify: `src/blur_policy/model.rs` only if rule conversion needs a private helper
- Test: `src/blur_policy/rules.rs`

**Interfaces:**
- `CompiledBlurRules::compile(&[BlurWindowRule], &[BlurLayerRule])` compiles regexes only during load/reload.
- `last_window_action()` and `last_layer_action()` continue to implement all-fields-AND and last-matching-rule-wins.

- [ ] **Step 1: Write failing tests for names and regex bounds.** Cover duplicate names across both rule lists, empty names, 129-byte names, 257-byte patterns, nested matcher matching, and last matching rule behavior.

- [ ] **Step 2: Run the rules tests and confirm the new validation failures.**

```bash
rtk cargo test --locked blur_policy::rules
```

- [ ] **Step 3: Implement bounded name validation and nested matcher compilation.** Store names only in the compiled rule diagnostics data, reject duplicates/empty/overlong names, set each regex limit to 256 bytes, and keep the compiled hot-path matcher free of unbounded allocations per frame.

- [ ] **Step 4: Run the rules tests and verify all matching cases pass.**

```bash
rtk cargo test --locked blur_policy::rules
```

- [ ] **Step 5: Commit the rule compiler.**

```bash
rtk git add src/blur_policy/rules.rs src/blur_policy/model.rs
rtk git commit -m "fix(blur): validate stable named rule schema"
```

### Task 3: Add bounded `EffectRegion` subtraction and canonical assignment decisions

**Files:**
- Modify: `src/effects/damage.rs`
- Modify: `src/compositor/blur_assignment.rs`
- Modify: `src/blur_policy/model.rs`
- Test: `src/effects/damage.rs`
- Test: `src/compositor/blur_assignment.rs`

**Interfaces:**
- `EffectRegion::subtract(&self, excluded: &EffectRegion) -> EffectRegion` preserves bounded deterministic rectangle algebra.
- `BlurAssignmentSource` identifies `Client`, `WaylandAuto`, `WindowRule`, or `LayerRule`.
- `ResolvedBlurAssignment` contains `source`, `anchor_scope`, `target_surface_id`, and `region` and contains no rule strings.
- Resolver methods retain testable decision helpers while exposing a compact source/scope decision to the compositor.

- [ ] **Step 1: Write failing subtraction tests.** Add a central-hole test, a multi-rectangle deterministic subtraction test, a full exclusion test, and a no-exclusion test using `EffectRegion::contains_point()` and exact rectangle ordering.

- [ ] **Step 2: Run the damage tests and confirm the API/test failures.**

```bash
rtk cargo test --locked effects::damage
```

- [ ] **Step 3: Implement `EffectRegion::subtract()`.** Use the existing four-piece rectangle algebra, honor empty/conservative regions, and route every piece through the existing bounded `push()` behavior.

- [ ] **Step 4: Write failing assignment tests for source and scope.** Cover client exact → `Client`/`Surface`, Wayland auto → `WaylandAuto`/`VisualGroup`, window enable rule → `WindowRule`/`VisualGroup`, and layer enable rule → `LayerRule`/`Surface`.

- [ ] **Step 5: Run the assignment tests and verify the new source/scope assertions fail before implementation.**

```bash
rtk cargo test --locked compositor::blur_assignment
```

- [ ] **Step 6: Implement the canonical decision path.** Keep disable > client > enable > auto precedence, suppress automatic assignment for `Full`, allow only approved auto modes, and make explicit client requests bypass opaque subtraction.

- [ ] **Step 7: Run the damage and assignment tests together.**

```bash
rtk cargo test --locked effects::damage compositor::blur_assignment
```

- [ ] **Step 8: Commit the canonical decision/algebra changes.**

```bash
rtk git add src/effects/damage.rs src/compositor/blur_assignment.rs src/blur_policy/model.rs
rtk git commit -m "fix(blur): preserve assignment source and subtract opaque regions"
```

### Task 4: Feed canonical assignments into the resolved scene

**Files:**
- Modify: `src/compositor/effects.rs`
- Modify: `src/compositor/blur_assignment.rs`
- Test: `src/compositor/tests/background_effect.rs`
- Test: `src/compositor/effects.rs`

**Interfaces:**
- `CompositorState::blur_assignment_for_surface()` returns the canonical resolved assignment with output-space region.
- `CompositorState::resolved_effect_scene()` emits synthesized desktop effects at `VisualGroup` scope, synthesized layer-rule effects at `Surface` scope, and client exact effects at `Surface` scope.

- [ ] **Step 1: Add failing integration regressions for opaque metadata and visual-group scope.** Use a real surface tree with below-root, root, and above-root subsurfaces; add full and partial opaque declarations; assert the synthesized region excludes opaque rectangles and the desktop effect has `VisualGroup` scope.

- [ ] **Step 2: Run the background-effect tests and capture the expected failures.**

```bash
rtk cargo test --locked compositor::tests::background_effect
```

- [ ] **Step 3: Replace the enum-only scene branch with the canonical assignment.** Build the output candidate, translate/clip `SurfaceOpaqueRegion::Partial` rectangles, subtract them only for synthesized assignments, preserve the explicit client region, skip empty synthetic regions, and copy the assignment scope into `ResolvedEffectInstance`.

- [ ] **Step 4: Update scene ordering only at the assignment boundary.** Keep `EffectSceneOrder` and the existing visual-group calculation; use `anchor_scope` to place visual-group synthesized background effects before the whole group.

- [ ] **Step 5: Run focused background-effect and effects tests.**

```bash
rtk cargo test --locked compositor::tests::background_effect
rtk cargo test --locked compositor::effects
```

- [ ] **Step 6: Commit resolved-scene integration.**

```bash
rtk git add src/compositor/effects.rs src/compositor/blur_assignment.rs src/compositor/tests/background_effect.rs
rtk git commit -m "fix(blur): resolve synthesized effects at policy scope"
```

### Task 5: Runtime status, startup diagnostics, reload preservation, and real scanout regression

**Files:**
- Modify: `src/compositor/blur_assignment.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/astreactl/client.rs`
- Modify: `src/astreactl/output.rs`
- Modify: `src/compositor/tests/direct_scanout.rs` or `src/compositor/tests/background_effect.rs`
- Test: `src/compositor/blur_assignment.rs`
- Test: `src/compositor/tests/direct_scanout.rs`

**Interfaces:**
- `OwnCompositorServer::blur_policy_snapshot()` returns runtime status.
- `OwnCompositorServer::reload_blur_policy()` atomically installs a valid `BlurPolicyConfig` and returns an error without replacing the prior generation on failure.
- `blur.status` serializes only bounded runtime status fields.

- [ ] **Step 1: Write failing status/reload tests.** Assert status contains modes, capability, generation, counts by source, path, and last reload error without rule arrays; assert invalid reload preserves the previous config/generation and records a bounded error.

- [ ] **Step 2: Run the focused status/reload tests and confirm failures.**

```bash
rtk cargo test --locked blur_policy compositor::blur_assignment
```

- [ ] **Step 3: Implement status assembly.** Calculate active assignment totals from the same canonical resolved-assignment path used by the scene, map source counts to fixed bounded fields, and bound diagnostic/path strings.

- [ ] **Step 4: Implement startup warning and transactional reload.** Replace `let _ = state.reload_blur_policy_from_disk();` with explicit warning handling; set the configured path and error on failure while retaining built-in/previous valid config; clear the error only after a valid replacement.

- [ ] **Step 5: Update control client decoding and terminal formatting.** Decode the runtime status type and print its bounded fields without assuming persistent rule arrays.

- [ ] **Step 6: Add the real resolved-scene direct-scanout regression.** Through the existing fullscreen Wayland helper, assert automatic fullscreen blur produces no effect with the default `auto_fullscreen=false`, an explicit enable rule creates a real resolved effect and `EffectRequiresComposition`, and removing/reloading the rule clears the effect and recovers scanout eligibility.

- [ ] **Step 7: Run direct-scanout and status tests.**

```bash
rtk cargo test --locked compositor::tests::direct_scanout
rtk cargo test --locked compositor::tests::background_effect
```

- [ ] **Step 8: Commit runtime/status integration.**

```bash
rtk git add src/compositor/blur_assignment.rs src/compositor/server.rs src/native_output/runtime/cycle_dispatch.rs src/astreactl/client.rs src/astreactl/output.rs src/compositor/tests/direct_scanout.rs src/compositor/tests/background_effect.rs
rtk git commit -m "fix(blur): expose runtime status and preserve reload failures"
```

### Task 6: Full Typhon verification and qualification

**Files:**
- Modify: only files required by verified failures; do not alter unrelated work.

- [ ] **Step 1: Run formatting, build, lint, focused suites, full tests, and qualification using the existing `target/`.**

```bash
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked effects
rtk cargo test --locked egl_renderer
rtk cargo test --locked native_output
rtk cargo test --locked
rtk bin/qualify-presentation --dry-run
```

- [ ] **Step 2: Run exact focused filters for policy precedence, persistent validation, rule matching, alpha capability, opaque subtraction, VisualGroup/Surface scope, mode enforcement, reload, and real scanout transition.** Record fresh pass/fail output.

- [ ] **Step 3: Inspect the final diff, run `rtk git diff --check`, and commit any verification-only correction separately.**

