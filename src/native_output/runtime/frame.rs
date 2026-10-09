use super::*;
use crate::native_output::kms_worker::KmsCommitWorkerTransport;
use oblivion_one::effects::{
    EffectRect, EffectRegion, EffectRegistryGeneration, effect_output_influence_region,
};
use std::{
    borrow::Cow,
    collections::HashSet,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use oblivion_one::compositor::{
    AnimationTime, DecorationRenderInstance, DecorationSceneSnapshot, FullscreenRenderPlanMetrics,
    PointerWarpOrigin, PresentationFrameSnapshot, PresentationSceneSample,
    PresentedCanonicalSceneSnapshot, ResolvedEffectScene, SceneNodeId, WindowExitFrameEvidence,
};
use oblivion_one::window_lifecycle_animation::{LifecycleFrameSnapshot, LifecycleSceneSample};

use super::frame_scene_identity::{
    assert_surface_owner_alignment, assert_surface_scene_node_alignment,
    filter_surface_scene_nodes_with_owners, finalize_snapshot,
};
use super::planner::CursorServiceOpportunity;
use super::window_exit_frame::resolve_window_exit_frame_parts;

static EFFECT_OWNER_DIAGNOSTICS_ENABLED: OnceLock<bool> = OnceLock::new();
static MISSING_PRESENTATION_OWNER_DIAGNOSTICS: AtomicU64 = AtomicU64::new(0);
static MISSING_EFFECT_PROGRAM_DIAGNOSTICS: AtomicU64 = AtomicU64::new(0);

pub(super) fn initialize_effect_owner_diagnostics() {
    let _ = EFFECT_OWNER_DIAGNOSTICS_ENABLED.get_or_init(|| {
        std::env::var_os("TYPHON_EFFECT_OWNER_DIAGNOSTICS").is_some_and(|value| value == "1")
    });
}

fn effect_owner_diagnostics_enabled() -> bool {
    EFFECT_OWNER_DIAGNOSTICS_ENABLED
        .get()
        .copied()
        .unwrap_or(false)
}

fn record_effect_diagnostic(enabled: bool, occurrences: &AtomicU64) {
    if enabled {
        occurrences.fetch_add(1, Ordering::Relaxed);
    }
}

fn take_effect_diagnostic_occurrences(occurrences: &AtomicU64) -> u64 {
    occurrences.swap(0, Ordering::Relaxed)
}

pub(super) fn dump_effect_owner_diagnostics() {
    if !effect_owner_diagnostics_enabled() {
        return;
    }
    let missing_owner = take_effect_diagnostic_occurrences(&MISSING_PRESENTATION_OWNER_DIAGNOSTICS);
    let missing_program = take_effect_diagnostic_occurrences(&MISSING_EFFECT_PROGRAM_DIAGNOSTICS);
    if missing_owner > 0 {
        eprintln!(
            "native effects: effect anchors without presentation animation owners occurrences={missing_owner}; physical effect damage remains tracked"
        );
    }
    if missing_program > 0 {
        eprintln!(
            "native effects: resolved programs missing from the captured registry generation occurrences={missing_program}; output bounds used for presentation damage"
        );
    }
}

#[cfg(test)]
use super::frame_scene_identity::{
    note_identity_computation, note_snapshot_finalization, note_snapshot_owned_clone,
};

#[derive(Debug)]
pub(crate) struct ResolvedNativeFrameScene<'a> {
    pub(crate) surfaces: Cow<'a, [RenderableSurface]>,
    pub(crate) surface_scene_node_ids: Cow<'a, [SceneNodeId]>,
    pub(crate) presentation_owner_root_surface_ids: Cow<'a, [u32]>,
    pub(crate) decorations: Vec<DecorationRenderInstance>,
    pub(crate) popup_surface_ids: Cow<'a, [u32]>,
    pub(crate) external_overlay_surface_ids: Vec<u32>,
    pub(crate) render_generation: u64,
    pub(crate) visibility: FullscreenRenderPlanMetrics,
    pub(crate) snapshot: NativeSceneSnapshot,
    pub(crate) canonical_scene_evidence: Option<PresentedCanonicalSceneSnapshot>,
    pub(crate) scene_identity_signature: u64,
    pub(crate) effects: ResolvedEffectScene,
    pub(crate) effect_scene_resolution_ns: Option<u64>,
    pub(crate) effect_scene_visible_instances: usize,
    pub(crate) presentation: PresentationSceneSample,
    pub(crate) presentation_snapshot: PresentationFrameSnapshot,
    pub(crate) lifecycle: LifecycleSceneSample,
    pub(crate) lifecycle_surfaces: Vec<RenderableSurface>,
    pub(crate) lifecycle_decorations: Vec<DecorationRenderInstance>,
    pub(crate) lifecycle_snapshot: LifecycleFrameSnapshot,
    pub(crate) window_exit_evidence: Vec<WindowExitFrameEvidence>,
}

fn freeze_presentation_effect_influences(
    effects: &ResolvedEffectScene,
    registry_generation: &EffectRegistryGeneration,
    output_bounds: EffectRect,
    owner_for_surface: impl FnMut(u32) -> Option<(SceneNodeId, u32)>,
) -> Vec<crate::native_output::output::NativePresentationEffectInfluenceSnapshot> {
    freeze_presentation_effect_influences_with_diagnostics(
        effects,
        registry_generation,
        output_bounds,
        effect_owner_diagnostics_enabled(),
        &MISSING_PRESENTATION_OWNER_DIAGNOSTICS,
        &MISSING_EFFECT_PROGRAM_DIAGNOSTICS,
        owner_for_surface,
    )
}

fn freeze_presentation_effect_influences_with_diagnostics(
    effects: &ResolvedEffectScene,
    registry_generation: &EffectRegistryGeneration,
    output_bounds: EffectRect,
    diagnostics_enabled: bool,
    missing_owner_diagnostics: &AtomicU64,
    missing_program_diagnostics: &AtomicU64,
    mut owner_for_surface: impl FnMut(u32) -> Option<(SceneNodeId, u32)>,
) -> Vec<crate::native_output::output::NativePresentationEffectInfluenceSnapshot> {
    let mut influences =
        Vec::<crate::native_output::output::NativePresentationEffectInfluenceSnapshot>::new();

    for instance in &effects.instances {
        let surface_id = match instance.anchor {
            oblivion_one::compositor::EffectAnchor::BeforeSurface(surface_id)
            | oblivion_one::compositor::EffectAnchor::ReplaceSurface(surface_id)
            | oblivion_one::compositor::EffectAnchor::AfterSurface(surface_id) => surface_id,
            oblivion_one::compositor::EffectAnchor::OutputPostProcess => continue,
        };
        let Some((scene_node_id, root_surface_id)) = owner_for_surface(surface_id) else {
            // Presentation influence records exist only for window-group
            // animation transforms. Keep non-window and stale anchors out of
            // that ownership map; physical effect damage was frozen separately.
            record_effect_diagnostic(diagnostics_enabled, missing_owner_diagnostics);
            continue;
        };
        let region = match registry_generation.effect_for_program(instance.program) {
            Some(effect) => effect_output_influence_region(
                effect.program.aggregate_footprint,
                &instance.region,
                output_bounds,
            ),
            None => {
                record_effect_diagnostic(diagnostics_enabled, missing_program_diagnostics);
                EffectRegion::from_rect(output_bounds)
            }
        };

        if let Some(existing) = influences
            .iter_mut()
            .find(|influence| influence.scene_node_id == scene_node_id)
        {
            debug_assert_eq!(existing.presentation_owner_root_surface_id, root_surface_id);
            existing.region = existing.region.union(&region);
        } else {
            influences.push(
                crate::native_output::output::NativePresentationEffectInfluenceSnapshot {
                    scene_node_id,
                    presentation_owner_root_surface_id: root_surface_id,
                    region,
                },
            );
        }
    }

    influences.sort_unstable_by_key(|influence| influence.scene_node_id);
    influences
}

#[cfg(test)]
mod presentation_effect_influence_tests {
    use super::*;
    use oblivion_one::compositor::{
        EffectAnchor, EffectAnchorScope, EffectSceneOrder, ResolvedEffectInstance,
    };
    use oblivion_one::effects::{
        EffectFrameDemand, EffectInstanceId, EffectParameterBlock, EffectProgramId,
        EffectRegistryGeneration,
    };
    use std::sync::atomic::AtomicU64;

    fn instance(
        id: u64,
        program: EffectProgramId,
        anchor: EffectAnchor,
        rect: EffectRect,
    ) -> ResolvedEffectInstance {
        ResolvedEffectInstance {
            id: EffectInstanceId::new(id).expect("test instance id"),
            program,
            anchor,
            region: EffectRegion::from_rect(rect),
            target_bounds: rect,
            parameter_block: EffectParameterBlock::default(),
            coverage: None,
            signature: id,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: None,
            anchor_scope: EffectAnchorScope::Surface,
            backdrop_reuse: oblivion_one::compositor::EffectBackdropReuse::Exact,
            scene_order: EffectSceneOrder::for_anchor(anchor),
        }
    }

    #[test]
    fn owned_effects_merge_by_scene_node_and_postprocess_is_excluded() {
        let generation = EffectRegistryGeneration::with_builtin_background_blur();
        let program = oblivion_one::effects::builtin_background_blur_program_id();
        let owner = SceneNodeId::from_raw(7).expect("WindowGroup node");
        let output_bounds = EffectRect::new(0, 0, 400, 300).expect("output bounds");
        let first_rect = EffectRect::new(50, 50, 30, 30).expect("first effect");
        let second_rect = EffectRect::new(200, 100, 20, 20).expect("second effect");
        let post_rect = EffectRect::new(300, 200, 40, 30).expect("postprocess effect");
        let effects = ResolvedEffectScene::new(
            1,
            vec![
                instance(1, program, EffectAnchor::BeforeSurface(10), first_rect),
                instance(2, program, EffectAnchor::AfterSurface(11), second_rect),
                instance(3, program, EffectAnchor::OutputPostProcess, post_rect),
            ],
        );

        let mut resolved_surfaces = Vec::new();
        let influences = freeze_presentation_effect_influences(
            &effects,
            &generation,
            output_bounds,
            |surface_id| {
                resolved_surfaces.push(surface_id);
                match surface_id {
                    10 | 11 => Some((owner, 70)),
                    _ => None,
                }
            },
        );

        let footprint = generation
            .effect_for_program(program)
            .expect("registered builtin effect")
            .program
            .aggregate_footprint;
        let expected = effect_output_influence_region(
            footprint,
            &EffectRegion::from_rect(first_rect),
            output_bounds,
        )
        .union(&effect_output_influence_region(
            footprint,
            &EffectRegion::from_rect(second_rect),
            output_bounds,
        ));
        assert_eq!(influences.len(), 1);
        assert_eq!(influences[0].scene_node_id, owner);
        assert_eq!(influences[0].presentation_owner_root_surface_id, 70);
        assert_eq!(influences[0].region, expected);
        assert_eq!(resolved_surfaces, [10, 11]);
    }

    #[test]
    fn missing_program_uses_bounded_output_influence_for_its_owner() {
        let generation = EffectRegistryGeneration::empty();
        let owner = SceneNodeId::from_raw(7).expect("WindowGroup node");
        let output_bounds = EffectRect::new(0, 0, 400, 300).expect("output bounds");
        let effects = ResolvedEffectScene::new(
            1,
            vec![instance(
                1,
                EffectProgramId::new(777).expect("missing program id"),
                EffectAnchor::ReplaceSurface(10),
                EffectRect::new(50, 50, 30, 30).expect("effect"),
            )],
        );

        let influences =
            freeze_presentation_effect_influences(&effects, &generation, output_bounds, |_| {
                Some((owner, 70))
            });

        assert_eq!(influences.len(), 1);
        assert_eq!(influences[0].region, EffectRegion::from_rect(output_bounds));
        assert_eq!(influences[0].presentation_owner_root_surface_id, 70);
    }

    #[test]
    fn effect_without_presentation_owner_keeps_physical_damage() {
        let generation = EffectRegistryGeneration::with_builtin_background_blur();
        let program = oblivion_one::effects::builtin_background_blur_program_id();
        let output_bounds = EffectRect::new(0, 0, 400, 300).expect("output bounds");
        let rect = EffectRect::new(50, 50, 30, 30).expect("effect region");
        let effects = ResolvedEffectScene::new(
            1,
            vec![instance(1, program, EffectAnchor::BeforeSurface(10), rect)],
        );

        let influences =
            freeze_presentation_effect_influences(&effects, &generation, output_bounds, |_| None);
        let physical_damage = freeze_native_effect_damage(&effects, &generation, output_bounds);

        assert!(influences.is_empty());
        assert_eq!(physical_damage.instances.len(), 1);
        assert_eq!(
            physical_damage.instances[0].region,
            EffectRegion::from_rect(rect)
        );
        assert!(!physical_damage.conservative_full);
    }

    #[test]
    fn effect_diagnostic_counters_are_disabled_and_drained_once() {
        let disabled_count = AtomicU64::new(0);
        record_effect_diagnostic(false, &disabled_count);
        assert_eq!(disabled_count.load(Ordering::Relaxed), 0);

        let enabled_count = AtomicU64::new(0);
        record_effect_diagnostic(true, &enabled_count);
        record_effect_diagnostic(true, &enabled_count);
        assert_eq!(take_effect_diagnostic_occurrences(&enabled_count), 2);
        assert_eq!(take_effect_diagnostic_occurrences(&enabled_count), 0);
    }

    #[test]
    fn unchanged_missing_owner_effect_is_bounded_over_ten_thousand_frames() {
        const FRAME_COUNT: u64 = 10_000;

        let generation = EffectRegistryGeneration::with_builtin_background_blur();
        let program = oblivion_one::effects::builtin_background_blur_program_id();
        let output_bounds = EffectRect::new(0, 0, 400, 300).expect("output bounds");
        let rect = EffectRect::new(50, 50, 30, 30).expect("effect region");
        let effects = ResolvedEffectScene::new(
            1,
            vec![instance(1, program, EffectAnchor::BeforeSurface(10), rect)],
        );
        let missing_owner_count = AtomicU64::new(0);
        let missing_program_count = AtomicU64::new(0);

        for _ in 0..FRAME_COUNT {
            let influences = freeze_presentation_effect_influences_with_diagnostics(
                &effects,
                &generation,
                output_bounds,
                true,
                &missing_owner_count,
                &missing_program_count,
                |_| None,
            );
            assert!(influences.is_empty());

            let physical_damage = freeze_native_effect_damage(&effects, &generation, output_bounds);
            assert_eq!(physical_damage.instances.len(), 1);
            assert_eq!(
                physical_damage.instances[0].region,
                EffectRegion::from_rect(rect)
            );
            assert!(!physical_damage.conservative_full);
        }

        assert_eq!(missing_owner_count.load(Ordering::Relaxed), FRAME_COUNT);
        assert_eq!(missing_program_count.load(Ordering::Relaxed), 0);
        assert_eq!(
            take_effect_diagnostic_occurrences(&missing_owner_count),
            FRAME_COUNT
        );
        assert_eq!(take_effect_diagnostic_occurrences(&missing_owner_count), 0);
    }
}

impl<'a> ResolvedNativeFrameScene<'a> {
    /// A synchronous modeset candidate is a conservative transition frame.
    /// It contains no client surfaces, lifecycle sample, retained-window
    /// evidence, or presentation animation. That keeps the new-size framebuffer
    /// independent of the still-authoritative old logical output geometry and
    /// leaves every live client presentation obligation for the next ordinary
    /// frame after the KMS commit.
    pub(crate) fn for_synchronous_output_reconfiguration(server: &'a OwnCompositorServer) -> Self {
        let mut scene = Self::from_server(server);
        let output_id = scene.presentation.output_id;
        let sampled_at = scene.presentation.sampled_at;
        let sample_time_source = scene.presentation.sample_time_source;
        let presentation =
            PresentationSceneSample::empty_for_output(output_id, sampled_at, sample_time_source);
        let lifecycle = LifecycleSceneSample {
            sampled_at,
            samples: Vec::new(),
        };

        scene.surfaces = Cow::Owned(Vec::new());
        scene.surface_scene_node_ids = Cow::Owned(Vec::new());
        scene.presentation_owner_root_surface_ids = Cow::Owned(Vec::new());
        scene.decorations.clear();
        scene.popup_surface_ids = Cow::Owned(Vec::new());
        scene.external_overlay_surface_ids.clear();
        scene.visibility = FullscreenRenderPlanMetrics::default();
        scene.snapshot = NativeSceneSnapshot::default();
        scene.canonical_scene_evidence = None;
        scene.scene_identity_signature = 0;
        scene.effects = ResolvedEffectScene::default();
        scene.presentation = presentation;
        scene.presentation_snapshot = PresentationFrameSnapshot::from_sample(&scene.presentation);
        scene.lifecycle = lifecycle;
        scene.lifecycle_surfaces = Vec::new();
        scene.lifecycle_decorations = Vec::new();
        scene.lifecycle_snapshot = LifecycleFrameSnapshot::from_sample(&scene.lifecycle);
        scene.window_exit_evidence.clear();
        scene
    }

    pub(crate) fn from_server(server: &'a OwnCompositorServer) -> Self {
        let (at, source) = AnimationTime::monotonic_now().map_or(
            (
                AnimationTime::from_nanos(0),
                oblivion_one::compositor::PresentationSampleTimeSource::ZeroFallback,
            ),
            |at| {
                (
                    at,
                    oblivion_one::compositor::PresentationSampleTimeSource::MonotonicFallback,
                )
            },
        );
        Self::from_server_at_with_source(server, at, source)
    }

    #[allow(dead_code)]
    pub(crate) fn from_server_at(server: &'a OwnCompositorServer, at: AnimationTime) -> Self {
        Self::from_server_at_with_source(
            server,
            at,
            oblivion_one::compositor::PresentationSampleTimeSource::MonotonicFallback,
        )
    }

    pub(crate) fn from_server_at_with_source(
        server: &'a OwnCompositorServer,
        at: AnimationTime,
        sample_time_source: oblivion_one::compositor::PresentationSampleTimeSource,
    ) -> Self {
        let lifecycle = server.lifecycle_scene_sample_at(at);
        Self::from_server_at_with_lifecycle(server, at, sample_time_source, lifecycle)
    }

    fn from_server_at_with_lifecycle(
        server: &'a OwnCompositorServer,
        at: AnimationTime,
        sample_time_source: oblivion_one::compositor::PresentationSampleTimeSource,
        lifecycle: LifecycleSceneSample,
    ) -> Self {
        let (canonical_surfaces, canonical_scene_nodes, fullscreen_plan, visibility) =
            server.native_frame_renderable_surfaces_with_scene_nodes_and_composition_plan();
        let canonical_owner_roots = Cow::Owned(
            canonical_surfaces
                .iter()
                .map(|surface| server.presentation_owner_root_for_surface(surface.surface_id))
                .collect::<Vec<_>>(),
        );
        let lifecycle_surfaces = server.lifecycle_renderable_surfaces(&lifecycle);
        let lifecycle_decorations =
            server.lifecycle_decoration_render_instances(&lifecycle, &lifecycle_surfaces);
        let restore_suppressed_roots = lifecycle
            .samples
            .iter()
            .filter(|lamp| {
                lamp.direction
                    == oblivion_one::window_lifecycle_animation::LifecycleDirection::Restore
            })
            .map(|lamp| lamp.root_surface_id)
            .collect::<HashSet<_>>();
        let (canonical_surfaces, canonical_scene_nodes, canonical_owner_roots) =
            if restore_suppressed_roots.is_empty() {
                (
                    canonical_surfaces,
                    canonical_scene_nodes,
                    canonical_owner_roots,
                )
            } else {
                filter_surface_scene_nodes_with_owners(
                    canonical_surfaces,
                    canonical_scene_nodes,
                    canonical_owner_roots,
                    |surface| {
                        !restore_suppressed_roots.contains(
                            &server.lifecycle_root_surface_for_surface(surface.surface_id),
                        )
                    },
                )
            };
        assert_surface_scene_node_alignment(
            canonical_surfaces.as_ref(),
            canonical_scene_nodes.as_ref(),
        );
        assert_surface_owner_alignment(canonical_surfaces.as_ref(), canonical_owner_roots.as_ref());
        let exit_parts = resolve_window_exit_frame_parts(
            server,
            &canonical_surfaces,
            &canonical_scene_nodes,
            &canonical_owner_roots,
            at,
            sample_time_source,
        );
        let exits = exit_parts.exits;
        let all_canonical_ordered_surfaces = exit_parts.surfaces;
        let all_canonical_ordered_nodes = exit_parts.scene_nodes;
        let all_canonical_ordered_owners = exit_parts.owner_roots;
        let surfaces = exit_parts.presented_surfaces;
        let decorations = exit_parts.decorations;
        let targets = exit_parts.targets;
        let presentation = exit_parts.presentation;
        let canonical_surface_ids = exit_parts.canonical_surface_ids;
        let canonical_key_by_surface = exit_parts.canonical_key_by_surface;
        let presentation_snapshot = PresentationFrameSnapshot::from_sample_with_presented_windows(
            &presentation,
            server.presented_window_geometries_for_targets(&presentation, &targets),
        );
        let lifecycle_snapshot = LifecycleFrameSnapshot::from_sample(&lifecycle);
        let popup_surface_ids = Cow::Borrowed(server.popup_surface_ids());
        let external_overlay_surface_ids = server.external_overlay_surface_ids(&lifecycle);
        let render_generation = server.scene_render_generation();
        let effect_registry_generation = server.trusted_effect_registry().current();
        let effect_scene_started = crate::pointer_debug::cursor_responsiveness_trace_enabled()
            .then(std::time::Instant::now);
        let canonical_effects = server.resolved_effect_scene_for_presentation_with_lifecycle(
            &presentation,
            &fullscreen_plan,
            &lifecycle,
        );
        let effect_scene_resolution_ns = effect_scene_started
            .map(|started| started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64);
        let effect_scene_visible_instances = canonical_effects.instances.len();
        let exit_surface_ids = exits
            .iter()
            .flat_map(|exit| {
                exit.content
                    .surfaces
                    .iter()
                    .map(|surface| surface.surface_id)
            })
            .collect::<HashSet<_>>();
        let mut effect_instances = canonical_effects.instances.clone();
        effect_instances.retain(|instance| {
            let surface_id = match instance.anchor {
                oblivion_one::compositor::EffectAnchor::BeforeSurface(surface_id)
                | oblivion_one::compositor::EffectAnchor::ReplaceSurface(surface_id)
                | oblivion_one::compositor::EffectAnchor::AfterSurface(surface_id) => {
                    Some(surface_id)
                }
                oblivion_one::compositor::EffectAnchor::OutputPostProcess => None,
            };
            surface_id.is_none_or(|surface_id| !exit_surface_ids.contains(&surface_id))
        });
        effect_instances.extend(server.window_exit_effects_for_presentation(&presentation, &exits));
        let effects = ResolvedEffectScene::new(canonical_effects.generation, effect_instances);
        let mut snapshot =
            NativeSceneSnapshot::from_surfaces_with_scene_nodes_and_presentation_owners(
                surfaces.as_ref(),
                all_canonical_ordered_nodes.as_ref(),
                all_canonical_ordered_owners.as_ref(),
                decorations
                    .iter()
                    .map(DecorationRenderInstance::scene_snapshot)
                    .collect(),
                popup_surface_ids.as_ref(),
            );
        let (output_width, output_height) = server.output_dimensions();
        let physical_effect_damage = EffectRect::new(0, 0, output_width, output_height)
            .map(|output_bounds| {
                freeze_native_effect_damage(&effects, &effect_registry_generation, output_bounds)
            })
            .unwrap_or_else(|| {
                NativeEffectDamageFrameSnapshot::conservative_full(
                    effect_registry_generation.generation,
                    effects.frame_demand_snapshot().dirty_region,
                )
            });
        snapshot.physical_effect_damage = physical_effect_damage;
        if let Some(output_bounds) = EffectRect::new(0, 0, output_width, output_height) {
            snapshot.presentation_effect_influences = freeze_presentation_effect_influences(
                &effects,
                &effect_registry_generation,
                output_bounds,
                |surface_id| {
                    if let Some(exit) = exits.iter().find(|exit| {
                        exit.content
                            .surfaces
                            .iter()
                            .any(|surface| surface.surface_id == surface_id)
                    }) {
                        return Some((exit.content.scene_node_id, exit.content.root_surface_id));
                    }
                    let root_surface_id = server.presentation_owner_root_for_surface(surface_id);
                    server
                        .presentation_scene_node_id_for_root(root_surface_id)
                        .map(|scene_node_id| (scene_node_id, root_surface_id))
                },
            );
        }
        let (snapshot, scene_identity_signature) = finalize_snapshot(
            snapshot,
            &external_overlay_surface_ids,
            visibility,
            &effects,
        );
        #[cfg(test)]
        {
            note_snapshot_finalization();
            note_identity_computation();
        }
        let canonical_presented_surfaces = surfaces
            .iter()
            .filter(|surface| canonical_surface_ids.contains(&surface.surface_id))
            .cloned()
            .collect::<Vec<_>>();
        let canonical_presented_nodes = all_canonical_ordered_surfaces
            .iter()
            .zip(all_canonical_ordered_nodes.iter().copied())
            .filter_map(|(surface, node)| {
                canonical_surface_ids
                    .contains(&surface.surface_id)
                    .then_some(node)
            })
            .collect::<Vec<_>>();
        let canonical_presented_owners = all_canonical_ordered_surfaces
            .iter()
            .zip(all_canonical_ordered_owners.iter().copied())
            .filter_map(|(surface, owner)| {
                canonical_surface_ids
                    .contains(&surface.surface_id)
                    .then_some(owner)
            })
            .collect::<Vec<_>>();
        let canonical_presented_keys = canonical_presented_surfaces
            .iter()
            .map(|surface| {
                canonical_key_by_surface
                    .get(&surface.surface_id)
                    .copied()
                    .flatten()
            })
            .collect::<Vec<_>>();
        let mut canonical_snapshot = snapshot.clone();
        canonical_snapshot
            .surfaces
            .retain(|surface| canonical_surface_ids.contains(&surface.surface_id));
        let canonical_visual_root_surface_ids = canonical_snapshot
            .surfaces
            .iter()
            .map(|surface| surface.visual_root_surface_id)
            .collect::<Vec<_>>();
        let canonical_scene_evidence = server.native_output_id().and_then(|output_id| {
            PresentedCanonicalSceneSnapshot::capture(
                output_id,
                render_generation,
                canonical_effects.signature,
                &canonical_presented_surfaces,
                &canonical_presented_nodes,
                &canonical_presented_owners,
                &canonical_presented_keys,
                &canonical_visual_root_surface_ids,
            )
        });
        let window_exit_evidence = exits
            .iter()
            .map(|exit| WindowExitFrameEvidence {
                identity: exit.identity,
                payload_id: exit.payload_id,
                root_surface_id: exit.content.root_surface_id,
                scene_node_id: exit.content.scene_node_id,
            })
            .collect();
        Self {
            surfaces,
            surface_scene_node_ids: all_canonical_ordered_nodes,
            presentation_owner_root_surface_ids: all_canonical_ordered_owners,
            decorations,
            popup_surface_ids,
            external_overlay_surface_ids,
            render_generation,
            visibility,
            snapshot,
            canonical_scene_evidence,
            scene_identity_signature,
            effects,
            effect_scene_resolution_ns,
            effect_scene_visible_instances,
            presentation,
            presentation_snapshot,
            lifecycle,
            lifecycle_surfaces,
            lifecycle_decorations,
            lifecycle_snapshot,
            window_exit_evidence,
        }
    }
    pub(crate) fn into_owned(self) -> ResolvedNativeFrameScene<'static> {
        ResolvedNativeFrameScene {
            surfaces: Cow::Owned(self.surfaces.into_owned()),
            surface_scene_node_ids: Cow::Owned(self.surface_scene_node_ids.into_owned()),
            presentation_owner_root_surface_ids: Cow::Owned(
                self.presentation_owner_root_surface_ids.into_owned(),
            ),
            decorations: self.decorations,
            popup_surface_ids: Cow::Owned(self.popup_surface_ids.into_owned()),
            external_overlay_surface_ids: self.external_overlay_surface_ids,
            render_generation: self.render_generation,
            visibility: self.visibility,
            snapshot: self.snapshot,
            canonical_scene_evidence: self.canonical_scene_evidence,
            scene_identity_signature: self.scene_identity_signature,
            effects: self.effects,
            effect_scene_resolution_ns: self.effect_scene_resolution_ns,
            effect_scene_visible_instances: self.effect_scene_visible_instances,
            presentation: self.presentation,
            presentation_snapshot: self.presentation_snapshot,
            lifecycle: self.lifecycle,
            lifecycle_surfaces: self.lifecycle_surfaces,
            lifecycle_decorations: self.lifecycle_decorations,
            lifecycle_snapshot: self.lifecycle_snapshot,
            window_exit_evidence: self.window_exit_evidence,
        }
    }

    pub(crate) fn surface_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.surfaces.iter().map(|surface| surface.surface_id)
    }

    pub(crate) fn decoration_identities(&self) -> impl Iterator<Item = (WindowId, u32)> + '_ {
        self.decorations
            .iter()
            .map(DecorationRenderInstance::scene_snapshot)
            .map(|snapshot| snapshot.identity())
    }

    pub(crate) fn snapshot_ref(&self) -> &NativeSceneSnapshot {
        self.debug_assert_snapshot_consistency();
        &self.snapshot
    }

    pub(crate) fn snapshot_owned(&self) -> NativeSceneSnapshot {
        self.debug_assert_snapshot_consistency();
        #[cfg(test)]
        note_snapshot_owned_clone();
        self.snapshot.clone()
    }

    fn debug_assert_snapshot_consistency(&self) {
        assert_surface_scene_node_alignment(
            self.surfaces.as_ref(),
            self.surface_scene_node_ids.as_ref(),
        );
        debug_assert!(
            self.surface_scene_node_ids.iter().copied().eq(self
                .snapshot
                .surfaces
                .iter()
                .map(|surface| surface.scene_node_id)),
            "resolved frame scene and its snapshot diverged on SceneNode identity"
        );
        debug_assert!(
            self.surface_ids().eq(self
                .snapshot
                .surfaces
                .iter()
                .map(|surface| surface.surface_id)),
            "resolved frame scene and its snapshot diverged on surfaces"
        );
        debug_assert!(
            self.decoration_identities().eq(self
                .snapshot
                .decorations
                .iter()
                .map(DecorationSceneSnapshot::identity)),
            "resolved frame scene and its snapshot diverged on decorations"
        );
    }

    pub(crate) fn scene_identity_signature(&self) -> u64 {
        self.scene_identity_signature
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeRepaintInputs {
    // Retained for cycle diagnostics only; accepting a socket is not visual work.
    pub(crate) accepted_clients: bool,
    pub(crate) render_generation_changed: bool,
    pub(crate) pending_frame_work: bool,
    pub(crate) only_pending_surface_frame_callbacks: bool,
    pub(crate) redraw_requested: bool,
    pub(crate) cursor_work_pending: bool,
    pub(crate) effect_work_pending: bool,
    pub(crate) effect_dirty_region: EffectRegion,
    pub(crate) page_flip_pending: bool,
}

#[cfg(test)]
pub(crate) fn earliest_native_deadline(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeRepaintDecision {
    pub(crate) repaint: bool,
    pub(crate) protocol_only_present: bool,
}

pub(crate) fn native_repaint_decision(inputs: NativeRepaintInputs) -> NativeRepaintDecision {
    if inputs.page_flip_pending {
        return NativeRepaintDecision {
            repaint: false,
            protocol_only_present: false,
        };
    }

    let effect_work_pending = inputs.effect_work_pending || !inputs.effect_dirty_region.is_empty();
    let protocol_only_present = inputs.pending_frame_work
        && inputs.only_pending_surface_frame_callbacks
        && !inputs.render_generation_changed
        && !inputs.redraw_requested
        && !effect_work_pending;
    NativeRepaintDecision {
        // Accepting a socket is protocol progress, not visual work.  The
        // accepted-client bit is retained in the input for diagnostics, but
        // a client must create visible scene or protocol-owned output work
        // before the primary renderer is scheduled.
        repaint: inputs.render_generation_changed
            || inputs.redraw_requested
            || inputs.cursor_work_pending
            || effect_work_pending
            || (inputs.pending_frame_work && !protocol_only_present),
        protocol_only_present,
    }
}

pub(crate) fn normalize_refresh_hz(refresh_hz: u32) -> u32 {
    if refresh_hz == 0 {
        60
    } else {
        refresh_hz.clamp(30, 360)
    }
}

#[derive(Debug, Default)]
pub(crate) struct NativeFrameRenderer {
    pub(crate) scene_renderer: DesktopSceneRenderer,
    pub(crate) frame: Vec<u32>,
}

impl NativeFrameRenderer {
    pub(crate) fn with_cursor_image(
        cursor_image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    ) -> Self {
        Self {
            scene_renderer: DesktopSceneRenderer::with_cursor_image(cursor_image),
            frame: Vec::new(),
        }
    }

    pub(crate) fn set_cursor_image(
        &mut self,
        cursor_image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    ) {
        self.scene_renderer.set_cursor_image(cursor_image);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorRenderMode {
    Software,
    SoftwareClient,
    Hardware,
}

impl NativeCursorRenderMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Software => "software",
            Self::SoftwareClient => "software_client",
            Self::Hardware => "hardware",
        }
    }

    pub(crate) const fn is_software(self) -> bool {
        matches!(self, Self::Software | Self::SoftwareClient)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorPreference {
    Auto,
    Hardware,
    Software,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorSchedulingPolicy {
    Auto,
    Piggyback,
    Software,
}

impl NativeCursorSchedulingPolicy {
    pub(crate) fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_CURSOR_SCHEDULING") {
            Ok(value) if value == "piggyback" => Self::Piggyback,
            Ok(value) if value == "software" => Self::Software,
            Ok(value) if value == "auto" => Self::Auto,
            Ok(value) => {
                eprintln!(
                    "native cursor: unknown OBLIVION_ONE_CURSOR_SCHEDULING={value:?}; using auto"
                );
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Piggyback => "piggyback",
            Self::Software => "software",
        }
    }
}

/// Cursor output is lower-priority work than a primary scene transaction.  A
/// request opens one output opportunity for the client to respond; it does
/// not reserve the Atomic commit arbiter.  The desired epoch is replaced by
/// newer input while that opportunity is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorOutputDisposition {
    PiggybackPrimary,
    DeferForPrimary,
    SubmitPlaneDelta,
    SoftwareOverlay,
    Noop,
}

/// Snapshot of primary/KMS ownership relevant to one cursor service
/// opportunity. `primary_scene_work_pending` alone is deliberately not a
/// carrier: Auto may bypass unready scene work after its service window.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeCursorPrimaryState {
    pub(crate) primary_scene_work_pending: bool,
    pub(crate) ready_primary_carrier_attachable: bool,
    pub(crate) primary_target_owned: bool,
    pub(crate) primary_worker_queued: bool,
    pub(crate) primary_kernel_submitted: bool,
    pub(crate) independent_kms_admission: bool,
    pub(crate) physical_admission: NativeCursorPhysicalAdmission,
}

/// Physical KMS timing authority for a cursor submission. This is kept
/// separate from cursor freshness debt: a worker may accept a future target
/// now and sleep until its worker wake, while synchronous transport must wait
/// for the target's service admission point in the reactor.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorPhysicalAdmission {
    #[default]
    NotRequired,
    /// Worker admission is legal now; the worker owns the physical wait.
    WorkerQueueNow,
    /// Synchronous transport has reached the physical service point.
    PhysicalNow,
    /// Synchronous transport must wait for this exact service point.
    PhysicalAt(u64),
    /// No current physical target can legally carry the hardware cursor.
    NoValidOpportunity,
}

impl NativeCursorPhysicalAdmission {
    const fn permits_submit_at(self, now_ns: u64) -> bool {
        match self {
            Self::NotRequired | Self::WorkerQueueNow | Self::PhysicalNow => true,
            Self::PhysicalAt(deadline_ns) => now_ns >= deadline_ns,
            Self::NoValidOpportunity => false,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeCursorOutputArbitration {
    pending_since_ns: Option<u64>,
    // Immutable freshness debt for the pending epoch, first assigned from
    // its real service opportunity. Rebinding a physical target never slides
    // this deadline.
    deadline_ns: Option<u64>,
    planning_retry_ns: Option<u64>,
    retry_not_before_ns: Option<u64>,
    service_opportunity: Option<CursorServiceOpportunity>,
    desired_epoch: u64,
    primary_response_window_open: bool,
    hardware_cursor_pending: bool,
    software_overlay_pending: bool,
    pub(crate) response_windows_opened: u64,
    pub(crate) changes_coalesced: u64,
    pub(crate) plane_delta_plans: u64,
    pub(crate) plane_delta_submissions: u64,
    pub(crate) plane_delta_deferred_for_primary: u64,
    pub(crate) cursor_state_piggybacked: u64,
    pub(crate) idle_hardware_updates: u64,
    pub(crate) idle_software_updates: u64,
    pub(crate) ready_primary_piggybacks: u64,
    pub(crate) unready_primary_bypass_decisions: u64,
    pub(crate) kms_busy_retries: u64,
    pub(crate) cursor_service_target_misses: u64,
}

impl NativeCursorOutputArbitration {
    #[cfg(test)]
    pub(crate) fn request(&mut self, epoch: u64, now_ns: u64, deadline_ns: u64) {
        self.request_with_source(epoch, now_ns, Some(deadline_ns), None, false);
    }

    pub(crate) fn request_hardware(&mut self, epoch: u64, now_ns: u64, deadline_ns: u64) {
        self.request_with_source(epoch, now_ns, Some(deadline_ns), None, true);
    }

    pub(crate) fn request_hardware_for_planning_retry(
        &mut self,
        epoch: u64,
        now_ns: u64,
        retry_at_ns: u64,
    ) {
        self.request_with_source(epoch, now_ns, None, None, true);
        self.service_opportunity = None;
        self.planning_retry_ns = Some(retry_at_ns.max(now_ns.saturating_add(1)));
    }

    pub(crate) fn request_hardware_for_opportunity(
        &mut self,
        epoch: u64,
        now_ns: u64,
        opportunity: CursorServiceOpportunity,
    ) {
        self.request_with_source(
            epoch,
            now_ns,
            Some(opportunity.service_deadline_ns),
            Some(opportunity),
            true,
        );
    }

    /// Refresh physical KMS ownership for the same pending cursor work.
    /// `service_deadline_ns` admits work at its deadline; only physical
    /// target expiry, output-generation change, or primary ordering can
    /// invalidate the current opportunity.
    pub(crate) fn reconcile_pending_service_opportunity(
        &mut self,
        opportunity: CursorServiceOpportunity,
        now_ns: u64,
    ) {
        debug_assert!(self.pending_since_ns.is_some());
        self.planning_retry_ns = None;
        if self.deadline_ns.is_none() {
            self.deadline_ns = Some(opportunity.service_deadline_ns);
        }

        let replace_opportunity = self.service_opportunity.is_none_or(|current| {
            let current_claim = current.target.physical_claim();
            let new_claim = opportunity.target.physical_claim();
            let output_generation_changed =
                current_claim.clock_generation != new_claim.clock_generation;
            let new_reservation_invalidates_current =
                opportunity.ordering_after_primary.is_some_and(|primary| {
                    let primary_claim = primary.physical_claim();
                    current_claim.clock_generation == primary_claim.clock_generation
                        && current_claim.sequence <= primary_claim.sequence
                });
            let current_physical_target_expired = current.target.presentation_time.get() <= now_ns
                || current.submit_window.commit_complete_deadline_ns() <= now_ns;

            output_generation_changed
                || new_reservation_invalidates_current
                || (current.target.identity() != opportunity.target.identity()
                    && current_physical_target_expired)
        });

        if replace_opportunity {
            self.service_opportunity = Some(opportunity);
            if self.deadline_ns.is_some_and(|deadline| deadline <= now_ns) {
                self.cursor_service_target_misses =
                    self.cursor_service_target_misses.saturating_add(1);
            }
        }
    }

    fn request_with_source(
        &mut self,
        epoch: u64,
        now_ns: u64,
        deadline_ns: Option<u64>,
        opportunity: Option<CursorServiceOpportunity>,
        hardware_cursor: bool,
    ) {
        if self.pending_since_ns.is_none() {
            self.pending_since_ns = Some(now_ns);
            self.deadline_ns = opportunity
                .map(|opportunity| opportunity.service_deadline_ns)
                .or(deadline_ns);
            self.planning_retry_ns = None;
            self.retry_not_before_ns = None;
            self.service_opportunity = opportunity;
            self.primary_response_window_open = true;
            self.response_windows_opened = self.response_windows_opened.saturating_add(1);
        } else {
            if epoch != self.desired_epoch {
                self.changes_coalesced = self.changes_coalesced.saturating_add(1);
            }
            if let Some(opportunity) = opportunity {
                self.reconcile_pending_service_opportunity(opportunity, now_ns);
            }
        }
        self.desired_epoch = epoch;
        self.hardware_cursor_pending |= hardware_cursor;
    }

    pub(crate) const fn deadline_ns(&self) -> Option<u64> {
        self.deadline_ns
    }

    pub(crate) const fn service_opportunity(&self) -> Option<CursorServiceOpportunity> {
        self.service_opportunity
    }

    pub(crate) fn physical_admission_for_transport(
        &self,
        transport: KmsCommitWorkerTransport,
        now_ns: u64,
    ) -> NativeCursorPhysicalAdmission {
        if !self.pending() {
            return NativeCursorPhysicalAdmission::NotRequired;
        }
        if !self.hardware_cursor_pending {
            return NativeCursorPhysicalAdmission::NoValidOpportunity;
        }
        let Some(opportunity) = self.service_opportunity else {
            return NativeCursorPhysicalAdmission::NoValidOpportunity;
        };
        let target_expired = opportunity.target.presentation_time.get() <= now_ns
            || opportunity.submit_window.commit_complete_deadline_ns() <= now_ns;
        if target_expired {
            return NativeCursorPhysicalAdmission::NoValidOpportunity;
        }
        if transport == KmsCommitWorkerTransport::Worker {
            return NativeCursorPhysicalAdmission::WorkerQueueNow;
        }
        if opportunity.service_deadline_ns > now_ns {
            NativeCursorPhysicalAdmission::PhysicalAt(opportunity.service_deadline_ns)
        } else {
            NativeCursorPhysicalAdmission::PhysicalNow
        }
    }

    /// Return the physical-admission wake only after freshness is already
    /// mature. Worker transport has a separate timed queue and never uses
    /// this reactor deadline.
    pub(crate) fn synchronous_physical_admission_wake_deadline_ns(
        &self,
        now_ns: u64,
    ) -> Option<u64> {
        if !self.pending()
            || !self.hardware_cursor_pending
            || self.planning_retry_ns.is_some()
            || !self.due(now_ns)
        {
            return None;
        }
        let deadline_ns = self.service_opportunity?.service_deadline_ns;
        (deadline_ns > now_ns).then_some(deadline_ns)
    }

    /// Return the cursor response deadline only while it is still a future
    /// temporal boundary.  Matured debt remains pending for disposition and
    /// exact epoch settlement, but its old timestamp must not poll the owner
    /// that now controls progress.
    pub(crate) fn wake_deadline_ns(&self, now_ns: u64) -> Option<u64> {
        if self.planning_retry_ns.is_some() {
            return None;
        }
        let deadline_ns = self
            .deadline_ns
            .into_iter()
            .chain(self.retry_not_before_ns)
            .max()?;
        (deadline_ns > now_ns).then_some(deadline_ns)
    }

    pub(crate) fn planning_retry_wake_deadline_ns(&self, now_ns: u64) -> Option<u64> {
        self.planning_retry_ns
            .filter(|deadline_ns| *deadline_ns > now_ns)
    }

    pub(crate) fn has_future_wake_requirement(
        &self,
        now_ns: u64,
        transport: KmsCommitWorkerTransport,
    ) -> bool {
        self.wake_deadline_ns(now_ns).is_some()
            || self.planning_retry_wake_deadline_ns(now_ns).is_some()
            || (transport == KmsCommitWorkerTransport::Synchronous
                && self
                    .synchronous_physical_admission_wake_deadline_ns(now_ns)
                    .is_some())
    }

    pub(crate) const fn desired_epoch(&self) -> u64 {
        self.desired_epoch
    }

    pub(crate) const fn pending_since_ns(&self) -> Option<u64> {
        self.pending_since_ns
    }

    pub(crate) const fn retry_not_before_ns(&self) -> Option<u64> {
        self.retry_not_before_ns
    }

    pub(crate) const fn planning_retry_ns(&self) -> Option<u64> {
        self.planning_retry_ns
    }

    pub(crate) fn pending_age_ns(&self, now_ns: u64) -> Option<u64> {
        self.pending_since_ns
            .map(|pending_since_ns| now_ns.saturating_sub(pending_since_ns))
    }

    pub(crate) const fn pending(&self) -> bool {
        self.pending_since_ns.is_some()
    }

    pub(crate) const fn hardware_cursor_pending(&self) -> bool {
        self.hardware_cursor_pending
    }

    pub(crate) fn due(&self, now_ns: u64) -> bool {
        if let Some(planning_retry_ns) = self.planning_retry_ns {
            return now_ns >= planning_retry_ns;
        }
        // Once the original freshness deadline matures, a future replacement
        // KMS target cannot reopen the primary response window. Backpressure
        // remains an independent gate.
        self.deadline_ns.is_some_and(|deadline| now_ns >= deadline)
            && self.retry_not_before_ns.is_none_or(|retry| now_ns >= retry)
    }

    pub(crate) fn set_software_overlay_pending(&mut self, pending: bool) {
        self.software_overlay_pending = pending;
    }

    pub(crate) fn reconcile_hardware_cursor_liveness(&mut self, needed: bool) {
        if needed {
            self.hardware_cursor_pending = true;
        } else if self.hardware_cursor_pending {
            self.hardware_cursor_pending = false;
            if !self.software_overlay_pending {
                self.clear_pending();
            }
        }
    }

    pub(crate) fn note_disposition(&mut self, disposition: NativeCursorOutputDisposition) {
        match disposition {
            NativeCursorOutputDisposition::PiggybackPrimary => {
                self.cursor_state_piggybacked = self.cursor_state_piggybacked.saturating_add(1);
            }
            NativeCursorOutputDisposition::DeferForPrimary => {
                self.plane_delta_deferred_for_primary =
                    self.plane_delta_deferred_for_primary.saturating_add(1);
            }
            NativeCursorOutputDisposition::SubmitPlaneDelta => {
                self.plane_delta_plans = self.plane_delta_plans.saturating_add(1);
                self.idle_hardware_updates = self.idle_hardware_updates.saturating_add(1);
            }
            NativeCursorOutputDisposition::SoftwareOverlay => {
                self.idle_software_updates = self.idle_software_updates.saturating_add(1);
            }
            NativeCursorOutputDisposition::Noop => {}
        }
    }

    pub(crate) fn note_policy_disposition(
        &mut self,
        policy: NativeCursorSchedulingPolicy,
        primary: NativeCursorPrimaryState,
        disposition: NativeCursorOutputDisposition,
    ) {
        self.note_disposition(disposition);
        match (policy, disposition) {
            (
                NativeCursorSchedulingPolicy::Auto,
                NativeCursorOutputDisposition::PiggybackPrimary,
            ) if primary.ready_primary_carrier_attachable => {
                self.ready_primary_piggybacks = self.ready_primary_piggybacks.saturating_add(1);
            }
            (
                NativeCursorSchedulingPolicy::Auto,
                NativeCursorOutputDisposition::SubmitPlaneDelta,
            ) if primary.primary_scene_work_pending => {
                self.unready_primary_bypass_decisions =
                    self.unready_primary_bypass_decisions.saturating_add(1);
            }
            _ => {}
        }
    }

    pub(crate) fn note_kms_busy_retry(&mut self) {
        if self.pending() {
            self.kms_busy_retries = self.kms_busy_retries.saturating_add(1);
        }
    }

    pub(crate) fn note_plane_delta_submission(&mut self) {
        self.plane_delta_submissions = self.plane_delta_submissions.saturating_add(1);
    }

    pub(crate) const fn response_windows_opened(&self) -> u64 {
        self.response_windows_opened
    }

    pub(crate) const fn changes_coalesced(&self) -> u64 {
        self.changes_coalesced
    }

    pub(crate) const fn plane_delta_plans(&self) -> u64 {
        self.plane_delta_plans
    }

    pub(crate) const fn plane_delta_submissions(&self) -> u64 {
        self.plane_delta_submissions
    }

    pub(crate) const fn plane_delta_deferred_for_primary(&self) -> u64 {
        self.plane_delta_deferred_for_primary
    }

    pub(crate) const fn cursor_state_piggybacked(&self) -> u64 {
        self.cursor_state_piggybacked
    }

    pub(crate) const fn idle_hardware_updates(&self) -> u64 {
        self.idle_hardware_updates
    }

    pub(crate) const fn idle_software_updates(&self) -> u64 {
        self.idle_software_updates
    }

    #[cfg(test)]
    pub(crate) fn disposition(
        &self,
        now_ns: u64,
        primary_work_pending: bool,
        hardware_usable: bool,
    ) -> NativeCursorOutputDisposition {
        if !self.pending() {
            return NativeCursorOutputDisposition::Noop;
        }
        if primary_work_pending {
            return NativeCursorOutputDisposition::PiggybackPrimary;
        }
        if !self.due(now_ns) {
            return if self.primary_response_window_open {
                NativeCursorOutputDisposition::DeferForPrimary
            } else {
                NativeCursorOutputDisposition::Noop
            };
        }
        if hardware_usable && !self.software_overlay_pending {
            NativeCursorOutputDisposition::SubmitPlaneDelta
        } else {
            NativeCursorOutputDisposition::SoftwareOverlay
        }
    }

    pub(crate) fn consume(&mut self, epoch: u64) {
        if self.pending() && epoch == self.desired_epoch {
            self.clear_pending();
        }
    }

    pub(crate) fn consume_submitted_epoch(
        &mut self,
        submitted_epoch: u64,
        now_ns: u64,
        next_deadline_ns: u64,
    ) {
        if !self.pending() {
            return;
        }
        if submitted_epoch == self.desired_epoch {
            self.clear_pending();
            return;
        }
        // A newer epoch arrived while the transaction was owned. Preserve
        // its original freshness debt; the next runtime planning pass fills
        // in a new physical KMS target without moving this deadline.
        self.service_opportunity = None;
        self.retry_not_before_ns = None;
        self.planning_retry_ns = Some(next_deadline_ns.max(now_ns.saturating_add(1)));
    }

    pub(crate) fn set_retry_not_before(&mut self, retry_not_before_ns: u64) {
        if self.pending() {
            self.retry_not_before_ns = Some(retry_not_before_ns);
        }
    }

    pub(crate) fn clear_pending(&mut self) {
        self.pending_since_ns = None;
        self.deadline_ns = None;
        self.planning_retry_ns = None;
        self.retry_not_before_ns = None;
        self.service_opportunity = None;
        self.desired_epoch = 0;
        self.primary_response_window_open = false;
        self.hardware_cursor_pending = false;
        self.software_overlay_pending = false;
    }
}

pub(crate) fn update_cursor_output_arbitration(
    arbitration: &mut NativeCursorOutputArbitration,
    policy: NativeCursorSchedulingPolicy,
    cursor_epoch: u64,
    last_submitted_cursor_epoch: u64,
    now_ns: u64,
    frame_scheduler: &NativeFrameScheduler,
    software_overlay_pending: bool,
    hardware_cursor_work_pending: bool,
    service_opportunity: Option<CursorServiceOpportunity>,
) -> (bool, bool, bool) {
    arbitration.set_software_overlay_pending(software_overlay_pending);
    let hardware_cursor_unsatisfied =
        hardware_cursor_work_pending && cursor_epoch != last_submitted_cursor_epoch;
    let output_cursor_work_changed = hardware_cursor_unsatisfied || software_overlay_pending;
    let planning_retry_due = policy == NativeCursorSchedulingPolicy::Auto
        && arbitration
            .planning_retry_ns()
            .is_some_and(|deadline_ns| now_ns >= deadline_ns);
    if output_cursor_work_changed || planning_retry_due {
        if policy == NativeCursorSchedulingPolicy::Auto
            && (hardware_cursor_unsatisfied || planning_retry_due)
        {
            match service_opportunity {
                Some(opportunity)
                    if arbitration.pending() && arbitration.desired_epoch() == cursor_epoch =>
                {
                    // Runtime planning sees the same unsatisfied epoch on
                    // every pass. Reconcile its physical claim without
                    // treating it as new cursor work.
                    arbitration.reconcile_pending_service_opportunity(opportunity, now_ns);
                }
                Some(opportunity) => {
                    arbitration.request_hardware_for_opportunity(cursor_epoch, now_ns, opportunity);
                }
                None => {
                    arbitration.request_hardware_for_planning_retry(
                        cursor_epoch,
                        now_ns,
                        frame_scheduler.next_refresh_deadline_ns(now_ns),
                    );
                }
            }
        } else {
            let deadline_ns = service_opportunity.map_or_else(
                || match policy {
                    NativeCursorSchedulingPolicy::Auto => None,
                    NativeCursorSchedulingPolicy::Piggyback
                    | NativeCursorSchedulingPolicy::Software => {
                        Some(frame_scheduler.next_refresh_deadline_ns(now_ns))
                    }
                },
                |opportunity| Some(opportunity.service_deadline_ns),
            );
            arbitration.request_with_source(
                cursor_epoch,
                now_ns,
                deadline_ns,
                service_opportunity,
                hardware_cursor_unsatisfied,
            );
        }
    }
    let deadline_due = arbitration.due(now_ns);
    (
        output_cursor_work_changed,
        deadline_due,
        deadline_due && output_cursor_work_changed,
    )
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn plane_delta_allowed_at_deadline(
    arbitration: &mut NativeCursorOutputArbitration,
    policy: NativeCursorSchedulingPolicy,
    now_ns: u64,
    primary_work_pending: bool,
    cursor_state_changed: bool,
    hardware_usable: bool,
) -> bool {
    let disposition = cursor_output_disposition_at_deadline(
        arbitration,
        policy,
        now_ns,
        primary_work_pending,
        hardware_usable,
    );
    // Piggyback is a primary-priority policy, not a permanent prohibition on
    // idle cursor updates.  `disposition` has already selected
    // `PiggybackPrimary` whenever primary work is ready.  Once the output
    // opportunity matures with no primary work, allowing the one queued cursor
    // update prevents a permanently armed deadline in the diagnostic mode.
    cursor_state_changed && disposition == NativeCursorOutputDisposition::SubmitPlaneDelta
}

#[cfg(test)]
pub(crate) fn cursor_output_disposition_at_deadline(
    arbitration: &mut NativeCursorOutputArbitration,
    policy: NativeCursorSchedulingPolicy,
    now_ns: u64,
    primary_work_pending: bool,
    hardware_usable: bool,
) -> NativeCursorOutputDisposition {
    // This compatibility entry point models `primary_work_pending` as dirty
    // scene work. Runtime arbitration supplies the richer ownership snapshot
    // through `cursor_output_disposition_for_primary_state`.
    let primary_state = NativeCursorPrimaryState {
        primary_scene_work_pending: primary_work_pending,
        ready_primary_carrier_attachable: policy == NativeCursorSchedulingPolicy::Piggyback
            && primary_work_pending,
        independent_kms_admission: true,
        ..NativeCursorPrimaryState::default()
    };
    cursor_output_disposition_for_primary_state(
        arbitration,
        policy,
        now_ns,
        primary_state,
        hardware_usable,
    )
}

pub(crate) fn cursor_output_disposition_for_primary_state(
    arbitration: &mut NativeCursorOutputArbitration,
    policy: NativeCursorSchedulingPolicy,
    now_ns: u64,
    primary: NativeCursorPrimaryState,
    hardware_usable: bool,
) -> NativeCursorOutputDisposition {
    let disposition = if !arbitration.pending() {
        NativeCursorOutputDisposition::Noop
    } else if policy == NativeCursorSchedulingPolicy::Software {
        NativeCursorOutputDisposition::SoftwareOverlay
    } else if policy == NativeCursorSchedulingPolicy::Piggyback
        && (primary.primary_scene_work_pending
            || primary.ready_primary_carrier_attachable
            || primary.primary_target_owned
            || primary.primary_worker_queued
            || primary.primary_kernel_submitted)
    {
        NativeCursorOutputDisposition::PiggybackPrimary
    } else if primary.ready_primary_carrier_attachable && !primary.primary_kernel_submitted {
        NativeCursorOutputDisposition::PiggybackPrimary
    } else if policy == NativeCursorSchedulingPolicy::Auto
        && arbitration.planning_retry_ns.is_some()
    {
        // A planning timer authorizes opportunity recomputation only. It is
        // never authority to submit a cursor plane transaction without a
        // real KMS service opportunity.
        NativeCursorOutputDisposition::Noop
    } else if !arbitration.due(now_ns) {
        if arbitration.primary_response_window_open {
            NativeCursorOutputDisposition::DeferForPrimary
        } else {
            NativeCursorOutputDisposition::Noop
        }
    } else if policy == NativeCursorSchedulingPolicy::Auto
        && hardware_usable
        && !arbitration.software_overlay_pending
        && !primary.physical_admission.permits_submit_at(now_ns)
    {
        // Freshness remains overdue; synchronous transport retains the
        // physical target and waits on its separate admission timer.
        NativeCursorOutputDisposition::Noop
    } else if primary.primary_target_owned
        || primary.primary_worker_queued
        || primary.primary_kernel_submitted
        || !primary.independent_kms_admission
    {
        NativeCursorOutputDisposition::DeferForPrimary
    } else if hardware_usable && !arbitration.software_overlay_pending {
        NativeCursorOutputDisposition::SubmitPlaneDelta
    } else {
        NativeCursorOutputDisposition::SoftwareOverlay
    };
    arbitration.note_policy_disposition(policy, primary, disposition);
    disposition
}

impl NativeCursorPreference {
    pub(crate) fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_CURSOR") {
            Ok(value) if matches!(value.as_str(), "hardware" | "hw" | "drm") => Self::Hardware,
            Ok(value) if matches!(value.as_str(), "software" | "sw" | "cpu") => Self::Software,
            Ok(value) if value == "auto" => Self::Auto,
            Ok(value) => {
                eprintln!("native cursor: unknown OBLIVION_ONE_CURSOR={value:?}; using auto");
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Hardware => "hardware",
            Self::Software => "software",
        }
    }
}

#[derive(Debug)]
pub(crate) struct NativePointerConstraintBackend {
    pub(crate) active: Option<NativePointerConstraint>,
    pub(crate) cursor_visible: bool,
}

pub(crate) fn native_pointer_debug_log_lazy(message: impl FnOnce() -> String) {
    crate::pointer_debug::log_lazy(message);
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NativePointerConstraint {
    pub(crate) id: PointerConstraintBackendId,
    pub(crate) mode: PointerConstraintMode,
    pub(crate) anchor: CompositorOutputPosition,
    pub(crate) region: Option<OutputRegion>,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct NativePointerConstraintBackendAction {
    pub(crate) activated: Option<NativePointerConstraint>,
    pub(crate) deactivated: Option<PointerConstraintBackendId>,
    pub(crate) deactivated_mode: Option<PointerConstraintMode>,
    pub(crate) failed: Option<(PointerConstraintBackendId, &'static str)>,
    pub(crate) restore_position: Option<CompositorOutputPosition>,
    pub(crate) restore_origin: Option<PointerWarpOrigin>,
    pub(crate) cursor_position: Option<CompositorOutputPosition>,
    pub(crate) cursor_position_origin: Option<PointerWarpOrigin>,
    pub(crate) cursor_visibility_changed: Option<bool>,
    pub(crate) region_resolution_timing: Option<PointerConstraintRegionResolutionTiming>,
}

impl NativePointerConstraintBackend {
    pub(crate) fn new() -> Self {
        Self {
            active: None,
            cursor_visible: true,
        }
    }

    pub(crate) fn active_locked(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|constraint| constraint.mode == PointerConstraintMode::Locked)
    }

    pub(crate) fn active_backend_id(&self) -> Option<PointerConstraintBackendId> {
        self.active.as_ref().map(|constraint| constraint.id)
    }

    pub(crate) fn active_constraint_state(&self) -> NativePointerConstraintState {
        match self.active.as_ref() {
            Some(NativePointerConstraint {
                mode: PointerConstraintMode::Locked,
                anchor,
                ..
            }) => NativePointerConstraintState::Locked { anchor: *anchor },
            Some(NativePointerConstraint {
                mode: PointerConstraintMode::Confined,
                region: Some(region),
                ..
            }) => NativePointerConstraintState::Confined {
                region: region.clone(),
            },
            _ => NativePointerConstraintState::None,
        }
    }

    pub(crate) fn handle_request(
        &mut self,
        request: PointerConstraintBackendRequest,
        cursor_position: CompositorOutputPosition,
    ) -> NativePointerConstraintBackendAction {
        match request {
            PointerConstraintBackendRequest::ActivateLocked { id } => {
                self.activate_locked(id, cursor_position)
            }
            PointerConstraintBackendRequest::ActivateConfined {
                id,
                region,
                region_resolution_timing,
            } => self.activate_confined(id, cursor_position, region, region_resolution_timing),
            PointerConstraintBackendRequest::UpdateConfinedRegion { id, region } => {
                self.update_confined_region(id, cursor_position, region)
            }
            PointerConstraintBackendRequest::Deactivate {
                id,
                restore_position,
                restore_origin,
            } => self.deactivate(id, restore_position, restore_origin),
            PointerConstraintBackendRequest::WarpPointer { position, origin } => {
                if self.active_locked() {
                    native_pointer_debug_log_lazy(|| {
                        format!(
                            "backend warp ignored reason=active_lock position=({},{})",
                            position.x, position.y
                        )
                    });
                    return NativePointerConstraintBackendAction::default();
                }
                let constraint = self.active_constraint_state();
                let final_position = constraint.constrain_position(position);
                native_pointer_debug_log_lazy(|| {
                    format!(
                        "backend warp constraint={:?} requested=({},{}) final=({},{})",
                        constraint, position.x, position.y, final_position.x, final_position.y
                    )
                });
                NativePointerConstraintBackendAction {
                    cursor_position: Some(final_position),
                    cursor_position_origin: Some(origin),
                    ..NativePointerConstraintBackendAction::default()
                }
            }
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible } => {
                if self.cursor_visible == visible {
                    NativePointerConstraintBackendAction::default()
                } else {
                    self.cursor_visible = visible;
                    NativePointerConstraintBackendAction {
                        cursor_visibility_changed: Some(visible),
                        ..NativePointerConstraintBackendAction::default()
                    }
                }
            }
        }
    }

    pub(crate) fn handle_resolved_request(
        &mut self,
        request: PointerConstraintBackendRequest,
        cursor_position: CompositorOutputPosition,
        locked_anchor: Option<CompositorOutputPosition>,
    ) -> NativePointerConstraintBackendAction {
        match request {
            PointerConstraintBackendRequest::ActivateLocked { id } => {
                self.activate_locked(id, locked_anchor.unwrap_or(cursor_position))
            }
            request => self.handle_request(request, cursor_position),
        }
    }

    pub(crate) fn activate_locked(
        &mut self,
        id: PointerConstraintBackendId,
        anchor: CompositorOutputPosition,
    ) -> NativePointerConstraintBackendAction {
        if let Some(active) = self.active.as_ref() {
            if active.id == id {
                return NativePointerConstraintBackendAction::default();
            }
            return NativePointerConstraintBackendAction {
                failed: Some((id, "native pointer constraint already active")),
                ..NativePointerConstraintBackendAction::default()
            };
        }
        let constraint = NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Locked,
            anchor,
            region: None,
        };
        self.active = Some(constraint.clone());
        NativePointerConstraintBackendAction {
            activated: Some(constraint),
            ..NativePointerConstraintBackendAction::default()
        }
    }

    pub(crate) fn activate_confined(
        &mut self,
        id: PointerConstraintBackendId,
        anchor: CompositorOutputPosition,
        region: OutputRegion,
        region_resolution_timing: Option<PointerConstraintRegionResolutionTiming>,
    ) -> NativePointerConstraintBackendAction {
        if let Some(active) = self.active.as_ref() {
            if active.id == id {
                return NativePointerConstraintBackendAction::default();
            }
            return NativePointerConstraintBackendAction {
                failed: Some((id, "native pointer constraint already active")),
                ..NativePointerConstraintBackendAction::default()
            };
        }
        let constraint = NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Confined,
            anchor,
            region: Some(region),
        };
        self.active = Some(constraint.clone());
        NativePointerConstraintBackendAction {
            activated: Some(constraint),
            region_resolution_timing,
            ..NativePointerConstraintBackendAction::default()
        }
    }

    pub(crate) fn deactivate(
        &mut self,
        id: PointerConstraintBackendId,
        restore_position: Option<CompositorOutputPosition>,
        restore_origin: Option<PointerWarpOrigin>,
    ) -> NativePointerConstraintBackendAction {
        let Some(active) = self.active.as_ref().cloned() else {
            return NativePointerConstraintBackendAction::default();
        };
        if active.id != id {
            return NativePointerConstraintBackendAction::default();
        }
        self.active = None;
        let restore_position = (active.mode == PointerConstraintMode::Locked)
            .then_some(restore_position)
            .flatten();
        let restore_origin = restore_position
            .is_some()
            .then_some(restore_origin)
            .flatten();
        NativePointerConstraintBackendAction {
            deactivated: Some(id),
            deactivated_mode: Some(active.mode),
            restore_position,
            restore_origin,
            ..NativePointerConstraintBackendAction::default()
        }
    }

    pub(crate) fn update_confined_region(
        &mut self,
        id: PointerConstraintBackendId,
        cursor_position: CompositorOutputPosition,
        region: OutputRegion,
    ) -> NativePointerConstraintBackendAction {
        let Some(active) = self.active.as_mut() else {
            return NativePointerConstraintBackendAction::default();
        };
        if active.id != id || active.mode != PointerConstraintMode::Confined {
            return NativePointerConstraintBackendAction::default();
        }
        active.region = Some(region.clone());
        let constrained = region.closest_point(cursor_position);
        NativePointerConstraintBackendAction {
            cursor_position: (constrained != cursor_position).then_some(constrained),
            cursor_position_origin: (constrained != cursor_position)
                .then_some(PointerWarpOrigin::ConfinedRegionCorrection),
            ..NativePointerConstraintBackendAction::default()
        }
    }
}

impl Default for NativePointerConstraintBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeFrameRenderer {
    pub(crate) fn render_server_frame(
        &mut self,
        width: u32,
        height: u32,
        resolved_scene: &ResolvedNativeFrameScene<'_>,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
    ) -> NativeRenderedFrame<'_> {
        self.scene_renderer
            .set_decoration_instances(&resolved_scene.decorations);
        self.scene_renderer
            .set_popup_surface_ids(&resolved_scene.popup_surface_ids);
        self.scene_renderer.set_presentation_projection_with_clips(
            &resolved_scene.presentation.opacities,
            &resolved_scene.presentation.clips,
            resolved_scene
                .surfaces
                .iter()
                .map(|surface| surface.surface_id)
                .zip(
                    resolved_scene
                        .presentation_owner_root_surface_ids
                        .iter()
                        .copied(),
                ),
        );
        let cursor_visible = resolve_native_cursor_for_server(server, input_state).visible;
        self.render_frame(NativeFrameRequest {
            width,
            height,
            surfaces: resolved_scene.surfaces.as_ref(),
            external_overlay_surface_ids: resolved_scene.external_overlay_surface_ids.clone(),
            visual_state: input_state.desktop_visual_state(cursor_mode, cursor_visible),
            render_generation: resolved_scene.render_generation,
            client_cursor: cursor_mode
                .is_software()
                .then(|| server.client_cursor_render_state())
                .flatten(),
        })
    }

    pub(crate) fn render_frame(
        &mut self,
        request: NativeFrameRequest<'_>,
    ) -> NativeRenderedFrame<'_> {
        let NativeFrameRequest {
            width,
            height,
            surfaces,
            visual_state,
            render_generation,
            client_cursor,
            external_overlay_surface_ids,
        } = request;
        let pixel_count = width.saturating_mul(height) as usize;
        self.frame.resize(pixel_count, 0);
        self.scene_renderer
            .compose_reusing_frame(DesktopComposeRequest {
                frame: &mut self.frame,
                frame_width: width,
                frame_height: height,
                output_scale: 1.0,
                surfaces,
                external_overlay_surface_ids,
                content_generation: native_scene_content_generation(render_generation),
                visual_state,
                client_cursor,
            });
        NativeRenderedFrame {
            pixels: &self.frame,
            scene_rebuild_kind: self.scene_renderer.last_rebuild_kind(),
            frame_copy_kind: self.scene_renderer.last_frame_copy_kind(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn egl_scene_draw_request<'a>(
        &mut self,
        width: u32,
        height: u32,
        resolved_scene: &'a ResolvedNativeFrameScene<'_>,
        server: &'a OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        current_damage: Option<OutputDamage>,
    ) -> EglSceneDrawRequest<'a> {
        // The resolved scene and this authority lookup are borrowed from the
        // same immutable compositor state on the native compositor thread.
        // Keeping the lookup at this synchronous request boundary prevents a
        // deferred renderer from pairing frozen surfaces with newer journal
        // metadata; such a renderer would need an explicit snapshot identity.
        let cursor_visible = resolve_native_cursor_for_server(server, input_state).visible;
        let surface_resource_sync_states = server.surface_resource_sync_states(
            resolved_scene
                .surfaces
                .iter()
                .map(|surface| surface.surface_id)
                .chain(
                    cursor_mode
                        .is_software()
                        .then(|| server.client_cursor_render_state())
                        .flatten()
                        .map(|cursor| cursor.surface.surface_id),
                ),
        );
        let lifecycle_surface_resource_sync_states = server.surface_resource_sync_states(
            resolved_scene
                .lifecycle_surfaces
                .iter()
                .map(|surface| surface.surface_id),
        );
        let mut surface_resource_sync_states = surface_resource_sync_states;
        surface_resource_sync_states.extend(lifecycle_surface_resource_sync_states);
        EglSceneDrawRequest {
            width,
            height,
            surfaces: resolved_scene.surfaces.as_ref(),
            external_overlay_surface_ids: &resolved_scene.external_overlay_surface_ids,
            popup_surface_ids: &resolved_scene.popup_surface_ids,
            content_generation: native_scene_content_generation(resolved_scene.render_generation),
            frame_id: None,
            render_generation: Some(resolved_scene.render_generation),
            scene_generation: resolved_scene.effects.generation,
            scene_signature: resolved_scene.scene_identity_signature(),
            visual_state: input_state.desktop_visual_state(cursor_mode, cursor_visible),
            output_scale: 1.0,
            decoration_instances: &resolved_scene.decorations,
            presentation_visual_signature: resolved_scene
                .presentation
                .presentation_visual_signature(),
            presentation_opacities: &resolved_scene.presentation.opacities,
            presentation_clips: &resolved_scene.presentation.clips,
            presentation_owner_root_surface_ids: resolved_scene
                .presentation_owner_root_surface_ids
                .as_ref(),
            client_cursor: cursor_mode
                .is_software()
                .then(|| server.client_cursor_render_state())
                .flatten(),
            current_damage,
            effects: &resolved_scene.effects,
            surface_resource_sync_states,
            lifecycle: &resolved_scene.lifecycle,
            lifecycle_surfaces: &resolved_scene.lifecycle_surfaces,
            lifecycle_decorations: &resolved_scene.lifecycle_decorations,
        }
    }
}

pub(crate) struct NativeRenderedFrame<'a> {
    pub(crate) pixels: &'a [u32],
    pub(crate) scene_rebuild_kind: DesktopSceneRebuildKind,
    pub(crate) frame_copy_kind: DesktopFrameCopyKind,
}

pub(crate) struct NativeFrameRequest<'a> {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) surfaces: &'a [RenderableSurface],
    pub(crate) external_overlay_surface_ids: Vec<u32>,
    pub(crate) visual_state: DesktopVisualState,
    pub(crate) render_generation: u64,
    pub(crate) client_cursor: Option<oblivion_one::compositor::ClientCursorRenderState<'a>>,
}

pub(crate) const fn native_scene_content_generation(render_generation: u64) -> u64 {
    render_generation
}
