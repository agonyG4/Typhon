use super::*;
use crate::compositor::decoration::types::DecorationHit;

#[derive(Debug, Clone)]
pub(in crate::compositor) enum PointerSceneHit {
    Client {
        target: PointerTarget,
    },
    Decoration {
        window_id: WindowId,
        root_surface_id: u32,
        hit: DecorationHit,
    },
    None,
}

#[derive(Debug, Clone)]
pub(in crate::compositor) struct PointerSceneHitCache {
    x: f64,
    y: f64,
    #[allow(dead_code)]
    scene_render_generation: u64,
    pointer_hit_generation: u64,
    hit: PointerSceneHit,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct PointerInputMetrics {
    pub(in crate::compositor) pointer_scene_hit_calls: u64,
    pub(in crate::compositor) pointer_scene_hit_cache_hits: u64,
    pub(in crate::compositor) pointer_scene_hit_cache_misses: u64,
    pub(in crate::compositor) full_scene_hit_scans: u64,
    pub(in crate::compositor) owner_locality_fast_hits: u64,
    pub(in crate::compositor) pointer_hit_generation_invalidations: u64,
    pub(in crate::compositor) pointer_scene_hit_groups_inspected: u64,
    pub(in crate::compositor) pointer_scene_hit_surfaces_inspected: u64,
    pub(in crate::compositor) pointer_scene_hit_origin_cache_clones: u64,
    pub(in crate::compositor) pointer_scene_hit_root_linear_searches: u64,
    pub(in crate::compositor) global_origin_cache_recomputes: u64,
    pub(in crate::compositor) active_scene_index_hits: u64,
    pub(in crate::compositor) grabbed_target_active_scene_hits: u64,
    pub(in crate::compositor) grabbed_target_global_fallbacks: u64,
    pub(in crate::compositor) raw_pointer_motion_samples: u64,
    pub(in crate::compositor) interaction_pointer_resource_iterations: u64,
    pub(in crate::compositor) interaction_pointer_temporary_vectors: u64,
    pub(in crate::compositor) interaction_pointer_local_updates: u64,
    pub(in crate::compositor) interaction_generic_hover_hit_tests_avoided: u64,
    pub(in crate::compositor) pointer_focus_transitions: u64,
    pub(in crate::compositor) pointer_scene_hit_cpu_nanos: u64,
    pub(in crate::compositor) desktop_focus_pipeline_invocations: u64,
    pub(in crate::compositor) desktop_focus_same_window_noops: u64,
    pub(in crate::compositor) keyboard_focus_reconciliations: u64,
    pub(in crate::compositor) pointer_constraint_reconciliations: u64,
}

#[cfg(test)]
impl PointerSceneHitCache {
    pub(in crate::compositor) fn new_for_test(
        x: f64,
        y: f64,
        scene_render_generation: u64,
        pointer_hit_generation: u64,
        hit: PointerSceneHit,
    ) -> Self {
        Self {
            x,
            y,
            scene_render_generation,
            pointer_hit_generation,
            hit,
        }
    }

    pub(in crate::compositor) fn pointer_hit_generation(&self) -> u64 {
        self.pointer_hit_generation
    }
}

impl CompositorState {
    pub(in crate::compositor) fn root_surface_hit_at(
        &mut self,
        x: f64,
        y: f64,
    ) -> Option<RootSurfaceHit> {
        self.refresh_surface_origin_cache();
        let surfaces = self.active_scene_surfaces();
        let origins = self.active_scene_surface_origins();
        for (index, renderable) in surfaces.iter().enumerate().rev() {
            let Some(origin) = origins.get(index).copied() else {
                continue;
            };

            let root_surface_id = self.root_surface_id_for_surface(renderable.surface_id);
            if self.window_id_for_surface(root_surface_id).is_none() {
                continue;
            }
            let Some(window_id) = self.window_id_for_surface(root_surface_id) else {
                continue;
            };
            let Some(root_index) = surfaces
                .iter()
                .position(|surface| surface.surface_id == root_surface_id)
            else {
                continue;
            };
            let Some(root_origin) = origins.get(root_index).copied() else {
                continue;
            };
            let root_surface = &surfaces[root_index];
            let local_x = x - f64::from(root_origin.0);
            let local_y = y - f64::from(root_origin.1);
            if window_frame_action_for_local_point(
                local_x,
                local_y,
                root_surface.width,
                root_surface.height,
            )
            .is_some()
            {
                return Some(RootSurfaceHit {
                    window_id,
                    root_surface_id,
                    local_x,
                    local_y,
                    width: root_surface.width,
                    height: root_surface.height,
                });
            }

            if let Some((surface_x, surface_y)) =
                render::surface_local_point_at_origin(renderable, origin, x, y)
                && self.surface_accepts_input_at(renderable, surface_x, surface_y)
            {
                return None;
            }
        }

        None
    }

    pub(in crate::compositor) fn root_surface_id_for_surface(&self, surface_id: u32) -> u32 {
        root_surface_id_for_surface_in_placements(&self.surface_placements, surface_id)
    }

    pub(in crate::compositor) fn root_window_local_point_at(
        &mut self,
        root_surface_id: u32,
        x: f64,
        y: f64,
    ) -> Option<(f64, f64, u32, u32)> {
        self.refresh_surface_origin_cache();
        let surfaces = self.active_scene_surfaces();
        let origins = self.active_scene_surface_origins();
        let root_index = surfaces
            .iter()
            .position(|surface| surface.surface_id == root_surface_id)?;
        let root_origin = origins.get(root_index).copied()?;
        let geometry = self.current_root_window_geometry(root_surface_id)?;
        let window_geometry = self
            .surface_window_geometries
            .get(&root_surface_id)
            .copied();
        let local_x = x
            - f64::from(root_origin.0)
            - f64::from(
                window_geometry
                    .map(|geometry| geometry.x)
                    .unwrap_or_default(),
            );
        let local_y = y
            - f64::from(root_origin.1)
            - f64::from(
                window_geometry
                    .map(|geometry| geometry.y)
                    .unwrap_or_default(),
            );
        Some((local_x, local_y, geometry.width, geometry.height))
    }

    pub(in crate::compositor) fn pointer_target_at(
        &mut self,
        x: f64,
        y: f64,
    ) -> Option<PointerTarget> {
        let hit = self.pointer_scene_hit_at(x, y);
        self.pointer_target_from_scene_hit(&hit, x, y)
    }

    pub(in crate::compositor) fn pointer_target_from_scene_hit(
        &self,
        hit: &PointerSceneHit,
        x: f64,
        y: f64,
    ) -> Option<PointerTarget> {
        match hit {
            PointerSceneHit::Client { target } => Some(target.clone()),
            PointerSceneHit::Decoration { .. } | PointerSceneHit::None => {
                if self.active_scene_surfaces().is_empty() {
                    self.focused_surface.clone().map(|surface| PointerTarget {
                        surface,
                        surface_x: x,
                        surface_y: y,
                    })
                } else {
                    None
                }
            }
        }
    }

    pub(in crate::compositor) fn pointer_scene_hit_at(
        &mut self,
        x: f64,
        y: f64,
    ) -> PointerSceneHit {
        let instrumentation_enabled = self.pointer_hit_instrumentation_enabled;
        let started_at = instrumentation_enabled.then(Instant::now);
        if instrumentation_enabled {
            self.pointer_hit_metrics.pointer_scene_hit_calls += 1;
        }
        if let Some(cache) = self.pointer_scene_hit_cache.as_ref()
            && cache.pointer_hit_generation == self.pointer_hit_generation
            && cache.x == x
            && cache.y == y
        {
            if instrumentation_enabled {
                self.pointer_hit_metrics.pointer_scene_hit_cache_hits += 1;
                self.pointer_hit_metrics.pointer_scene_hit_cpu_nanos += started_at
                    .expect("instrumented hit-test has a start time")
                    .elapsed()
                    .as_nanos()
                    .min(u128::from(u64::MAX))
                    as u64;
            }
            return cache.hit.clone();
        }

        if let Some(hit) = self.pointer_scene_hit_locality_at(x, y) {
            if instrumentation_enabled {
                self.pointer_hit_metrics.owner_locality_fast_hits = self
                    .pointer_hit_metrics
                    .owner_locality_fast_hits
                    .saturating_add(1);
                self.pointer_hit_metrics.pointer_scene_hit_cpu_nanos += started_at
                    .expect("instrumented hit-test has a start time")
                    .elapsed()
                    .as_nanos()
                    .min(u128::from(u64::MAX))
                    as u64;
            }
            self.pointer_scene_hit_cache = Some(PointerSceneHitCache {
                x,
                y,
                scene_render_generation: self.scene_render_generation,
                pointer_hit_generation: self.pointer_hit_generation,
                hit: hit.clone(),
            });
            return hit;
        }

        if instrumentation_enabled {
            self.pointer_hit_metrics.pointer_scene_hit_cache_misses += 1;
        }
        self.pointer_hit_metrics.full_scene_hit_scans = self
            .pointer_hit_metrics
            .full_scene_hit_scans
            .saturating_add(1);
        self.refresh_visual_stack_groups_cache();
        let (hit, groups_inspected, surfaces_inspected) =
            self.pointer_scene_hit_uncached(x, y, instrumentation_enabled);
        if instrumentation_enabled {
            self.pointer_hit_metrics.pointer_scene_hit_groups_inspected += groups_inspected;
            self.pointer_hit_metrics
                .pointer_scene_hit_surfaces_inspected += surfaces_inspected;
            self.pointer_hit_metrics.pointer_scene_hit_cpu_nanos += started_at
                .expect("instrumented hit-test has a start time")
                .elapsed()
                .as_nanos()
                .min(u128::from(u64::MAX))
                as u64;
        }
        self.pointer_scene_hit_cache = Some(PointerSceneHitCache {
            x,
            y,
            scene_render_generation: self.scene_render_generation,
            pointer_hit_generation: self.pointer_hit_generation,
            hit: hit.clone(),
        });
        hit
    }

    fn pointer_scene_hit_uncached(
        &self,
        x: f64,
        y: f64,
        collect_metrics: bool,
    ) -> (PointerSceneHit, u64, u64) {
        let surfaces = self.active_scene_surfaces();
        let origins = self.active_scene_surface_origins();
        let mut groups_inspected = 0;
        let mut surfaces_inspected = 0;
        for group in self.visual_stack_groups_cache.iter().rev() {
            if collect_metrics {
                groups_inspected += 1;
            }
            let root_index = group.root_surface_index();
            let Some(root_surface) = surfaces.get(root_index) else {
                continue;
            };
            if root_surface.surface_id != group.root_surface_id() {
                continue;
            }
            let Some(root_origin) = origins.get(root_index).copied() else {
                continue;
            };
            if !group.is_popup()
                && let Some(hit) =
                    self.decoration_hit_for_root_at(group.root_surface_id(), root_origin, x, y)
                && (!self.root_surface_accepts_input_at(root_index, root_origin, x, y)
                    || (matches!(hit, DecorationHit::Resize(_))
                        && self
                            .window_id_for_surface(group.root_surface_id())
                            .and_then(|window_id| self.window(window_id))
                            .and_then(|window| window.management)
                            .is_some_and(|management| {
                                management.layout() == crate::wm::LayoutMembership::Tiled
                                    && management.chrome_policy()
                                        == crate::wm::WindowChromePolicy::Minimal
                            })))
            {
                let Some(window_id) = self.window_id_for_surface(group.root_surface_id()) else {
                    continue;
                };
                return (
                    PointerSceneHit::Decoration {
                        window_id,
                        root_surface_id: group.root_surface_id(),
                        hit,
                    },
                    groups_inspected,
                    surfaces_inspected,
                );
            }
            for &index in group.surface_indices().iter().rev() {
                if collect_metrics {
                    surfaces_inspected += 1;
                }
                let Some(renderable) = surfaces.get(index) else {
                    continue;
                };
                let Some(origin) = origins.get(index).copied() else {
                    continue;
                };
                let Some((surface_x, surface_y)) =
                    render::surface_local_point_at_origin(renderable, origin, x, y)
                else {
                    continue;
                };
                if !self.surface_accepts_input_at(renderable, surface_x, surface_y) {
                    continue;
                }
                let Some(surface) = self.surface_resource_by_id(renderable.surface_id) else {
                    continue;
                };

                return (
                    PointerSceneHit::Client {
                        target: PointerTarget {
                            surface,
                            surface_x,
                            surface_y,
                        },
                    },
                    groups_inspected,
                    surfaces_inspected,
                );
            }
        }
        (PointerSceneHit::None, groups_inspected, surfaces_inspected)
    }

    fn pointer_scene_hit_locality_at(&mut self, x: f64, y: f64) -> Option<PointerSceneHit> {
        let cache = self.pointer_scene_hit_cache.as_ref()?;
        if cache.pointer_hit_generation != self.pointer_hit_generation {
            return None;
        }
        match &cache.hit {
            PointerSceneHit::Client { target } => {
                let surface_id = compositor_surface_id(&target.surface);
                let root_surface_id = self.root_surface_id_for_surface(surface_id);
                if !self.pointer_scene_owner_is_frontmost(
                    root_surface_id,
                    Some(surface_id),
                    None,
                    x,
                    y,
                ) {
                    return None;
                }
                let index = self.active_scene_surface_index(surface_id)?;
                self.pointer_hit_metrics.active_scene_index_hits = self
                    .pointer_hit_metrics
                    .active_scene_index_hits
                    .saturating_add(1);
                let renderable = self.active_scene_surfaces().get(index)?;
                let origin = self.active_scene_surface_origins().get(index).copied()?;
                let (surface_x, surface_y) =
                    render::surface_local_point_at_origin(renderable, origin, x, y)?;
                if !self.surface_accepts_input_at(renderable, surface_x, surface_y) {
                    return None;
                }
                let surface = self.surface_resource_by_id(surface_id)?;
                Some(PointerSceneHit::Client {
                    target: PointerTarget {
                        surface,
                        surface_x,
                        surface_y,
                    },
                })
            }
            PointerSceneHit::Decoration {
                window_id,
                root_surface_id,
                hit,
            } => {
                if !self.pointer_scene_owner_is_frontmost(*root_surface_id, None, Some(*hit), x, y)
                {
                    return None;
                }
                let index = self.active_scene_surface_index(*root_surface_id)?;
                self.pointer_hit_metrics.active_scene_index_hits = self
                    .pointer_hit_metrics
                    .active_scene_index_hits
                    .saturating_add(1);
                let origin = self.active_scene_surface_origins().get(index).copied()?;
                (self.decoration_hit_for_root_at(*root_surface_id, origin, x, y) == Some(*hit))
                    .then_some(PointerSceneHit::Decoration {
                        window_id: *window_id,
                        root_surface_id: *root_surface_id,
                        hit: *hit,
                    })
            }
            PointerSceneHit::None => None,
        }
    }

    fn pointer_scene_owner_is_frontmost(
        &self,
        owner_root_surface_id: u32,
        owner_surface_id: Option<u32>,
        owner_decoration: Option<DecorationHit>,
        x: f64,
        y: f64,
    ) -> bool {
        for group in self.visual_stack_groups_cache.iter().rev() {
            let Some(root_index) = self.active_scene_surface_index(group.root_surface_id()) else {
                continue;
            };
            let Some(root_origin) = self.active_scene_surface_origins().get(root_index).copied()
            else {
                continue;
            };
            let decoration_hit = (!group.is_popup())
                .then(|| {
                    self.decoration_hit_for_root_at(group.root_surface_id(), root_origin, x, y)
                })
                .flatten()
                .filter(|hit| {
                    !self.root_surface_accepts_input_at(root_index, root_origin, x, y)
                        || (matches!(hit, DecorationHit::Resize(_))
                            && self
                                .window_id_for_surface(group.root_surface_id())
                                .and_then(|window_id| self.window(window_id))
                                .and_then(|window| window.management)
                                .is_some_and(|management| {
                                    management.layout() == crate::wm::LayoutMembership::Tiled
                                        && management.chrome_policy()
                                            == crate::wm::WindowChromePolicy::Minimal
                                }))
                });
            let hit = decoration_hit
                .map(|hit| (group.root_surface_id(), Some(hit)))
                .or_else(|| {
                    group
                        .surface_indices()
                        .iter()
                        .rev()
                        .filter_map(|index| {
                            let renderable = self.active_scene_surfaces().get(*index)?;
                            let origin =
                                self.active_scene_surface_origins().get(*index).copied()?;
                            let (surface_x, surface_y) =
                                render::surface_local_point_at_origin(renderable, origin, x, y)?;
                            self.surface_accepts_input_at(renderable, surface_x, surface_y)
                                .then_some((renderable.surface_id, None))
                        })
                        .find(|(surface_id, _)| self.surface_resource_by_id(*surface_id).is_some())
                });
            let Some((surface_id, decoration)) = hit else {
                continue;
            };
            if group.root_surface_id() != owner_root_surface_id {
                return false;
            }
            return match (owner_surface_id, owner_decoration, decoration) {
                (Some(owner_surface_id), None, None) => surface_id == owner_surface_id,
                (None, Some(owner_decoration), Some(decoration)) => {
                    surface_id == owner_root_surface_id && decoration == owner_decoration
                }
                _ => false,
            };
        }
        false
    }

    fn root_surface_accepts_input_at(
        &self,
        root_index: usize,
        root_origin: (i32, i32),
        x: f64,
        y: f64,
    ) -> bool {
        let Some(root_surface) = self.active_scene_surfaces().get(root_index) else {
            return false;
        };
        let Some((surface_x, surface_y)) =
            render::surface_local_point_at_origin(root_surface, root_origin, x, y)
        else {
            return false;
        };
        self.surface_accepts_input_at(root_surface, surface_x, surface_y)
    }

    fn refresh_visual_stack_groups_cache(&mut self) {
        if self.visual_stack_groups_cache_generation == Some(self.pointer_hit_generation) {
            return;
        }
        self.visual_stack_groups_cache = render::visual_stack_groups(
            self.active_scene_surfaces(),
            self.active_scene_popup_surface_ids(),
        );
        self.visual_stack_groups_cache_generation = Some(self.pointer_hit_generation);
    }

    fn pointer_target_at_visual_root_window(
        &mut self,
        root_surface_id: u32,
        x: f64,
        y: f64,
    ) -> Option<PointerTarget> {
        self.refresh_surface_origin_cache();
        let surfaces = self.active_scene_surfaces();
        let origins = self.active_scene_surface_origins();
        let root_index = surfaces
            .iter()
            .position(|surface| surface.surface_id == root_surface_id)?;
        let origin = origins.get(root_index).copied()?;
        let geometry = self.current_visual_root_window_geometry(root_surface_id)?;
        let surface_x = x - f64::from(origin.0);
        let surface_y = y - f64::from(origin.1);
        if surface_x < 0.0
            || surface_y < 0.0
            || surface_x >= f64::from(geometry.width)
            || surface_y >= f64::from(geometry.height)
        {
            return None;
        }
        let surface = self.surface_resource_by_id(root_surface_id)?;
        Some(PointerTarget {
            surface,
            surface_x,
            surface_y,
        })
    }

    pub(in crate::compositor) fn pointer_target_for_surface_at_output(
        &mut self,
        surface: &wl_surface::WlSurface,
        x: f64,
        y: f64,
    ) -> Option<PointerTarget> {
        let surface_id = compositor_surface_id(surface);
        self.refresh_surface_origin_cache();
        let surfaces = self.active_scene_surfaces();
        let origins = self.active_scene_surface_origins();
        let index = surfaces
            .iter()
            .position(|renderable| renderable.surface_id == surface_id)?;
        let renderable = &surfaces[index];
        let origin = origins.get(index).copied()?;
        let (surface_x, surface_y) =
            render::surface_local_point_at_origin(renderable, origin, x, y)?;
        Some(PointerTarget {
            surface: surface.clone(),
            surface_x,
            surface_y,
        })
    }

    pub(in crate::compositor) fn surface_accepts_input_at(
        &self,
        surface: &RenderableSurface,
        surface_x: f64,
        surface_y: f64,
    ) -> bool {
        self.surface_resource_by_id(surface.surface_id)
            .and_then(|resource| {
                resource.data::<SurfaceData>().map(|data| {
                    data.input_region_contains(surface_x, surface_y, surface.width, surface.height)
                })
            })
            .unwrap_or(true)
    }

    pub(in crate::compositor) fn refresh_pointer_focus_at_last_position(&mut self) {
        self.refresh_pointer_focus_at_last_position_for_visual_root(None);
    }

    pub(in crate::compositor) fn refresh_pointer_focus_at_last_position_for_visual_root(
        &mut self,
        visual_root_surface_id: Option<u32>,
    ) {
        if self.workspace_scene_transition_active {
            return;
        }
        if self.defer_pointer_focus_refresh() {
            return;
        }
        if self.active_locked_pointer_binding().is_some() {
            if let Some(active) = self.active_locked_pointer_binding() {
                self.pin_locked_pointer_focus(&active);
            }
            return;
        }

        let scene_hit = self.pointer_scene_hit_at(self.last_pointer_x, self.last_pointer_y);
        if !self.pointer_scene_hit_allowed_by_popup_grab(&scene_hit) {
            self.clear_pointer_focus();
            pointer_debug_log("post-unlock focus target=blocked");
            return;
        }
        let target = match scene_hit {
            PointerSceneHit::Client { target } => Some(target),
            PointerSceneHit::Decoration { .. } => {
                self.focus_desktop_window_at_pointer_scene_hit(&scene_hit);
                self.clear_pointer_focus();
                pointer_debug_log("post-unlock focus target=decoration");
                return;
            }
            PointerSceneHit::None => visual_root_surface_id.and_then(|root_surface_id| {
                self.pointer_target_at_visual_root_window(
                    root_surface_id,
                    self.last_pointer_x,
                    self.last_pointer_y,
                )
            }),
        };
        let Some(target) = target else {
            self.clear_pointer_focus();
            pointer_debug_log("post-unlock focus target=none");
            return;
        };

        self.focus_desktop_window_at_pointer_target(&target);

        pointer_debug_log(format!(
            "post-unlock focus target={} x={} y={}",
            compositor_surface_id(&target.surface),
            target.surface_x,
            target.surface_y
        ));
        self.ensure_pointer_focus(&target.surface);
        self.send_pointer_enter_if_needed(&target);
    }

    pub(in crate::compositor) fn commit_pointer_crossing_at_last_position(&mut self) {
        if let Some(active) = self.active_locked_pointer_binding() {
            self.pin_locked_pointer_focus(&active);
            return;
        }
        if let Some(active) = self.active_confined_pointer_binding() {
            self.pin_confined_pointer_focus(&active);
            return;
        }
        if let Some(grabbed_surface) = self.implicit_pointer_grab_surface("surface-destroyed") {
            self.pointer_surface = Some(grabbed_surface);
            return;
        }

        let scene_hit = self.pointer_scene_hit_at(self.last_pointer_x, self.last_pointer_y);
        if !self.pointer_scene_hit_allowed_by_popup_grab(&scene_hit) {
            self.clear_pointer_focus();
            return;
        }
        if matches!(scene_hit, PointerSceneHit::Decoration { .. }) {
            self.focus_desktop_window_at_pointer_scene_hit(&scene_hit);
        }
        let target = match scene_hit {
            PointerSceneHit::Client { target } if target.surface.is_alive() => Some(target),
            PointerSceneHit::Client { .. }
            | PointerSceneHit::Decoration { .. }
            | PointerSceneHit::None => None,
        };
        if let Some(target) = target.as_ref() {
            self.focus_desktop_window_at_pointer_target(target);
        }
        let target_surface = target.as_ref().map(|target| target.surface.clone());
        let same_target = self
            .pointer_surface
            .as_ref()
            .zip(target_surface.as_ref())
            .is_some_and(|(previous, target)| same_surface_resource(previous, target));
        if same_target {
            if let Some(target) = target.as_ref() {
                let mut frame_pointers = Vec::new();
                self.queue_pointer_enter_events(target, &mut frame_pointers);
                for pointer in frame_pointers {
                    send_pointer_frame_if_supported(&pointer);
                }
            }
            return;
        }

        let previous = self.clear_pointer_focus_state();
        self.pointer_surface = target_surface;
        let mut frame_pointers = Vec::new();
        for (pointer, surface) in previous {
            if !surface.is_alive() {
                continue;
            }
            let serial = self.next_configure_serial();
            let _ = pointer.send_event(wl_pointer::Event::Leave { serial, surface });
            push_pointer_frame_once(&mut frame_pointers, pointer);
        }
        if let Some(target) = target.as_ref() {
            self.queue_pointer_enter_events(target, &mut frame_pointers);
        }
        for pointer in frame_pointers {
            send_pointer_frame_if_supported(&pointer);
        }
    }

    pub(in crate::compositor) fn refresh_pointer_focus_after_implicit_grab(
        &mut self,
        old_surface_id: Option<u32>,
    ) {
        let terminal_visual_root_surface_id = if self.window_interaction_terminal_refresh_pending {
            self.window_interaction_terminal_refresh_pending = false;
            self.window_interaction_release_metrics
                .window_interaction_post_terminal_pointer_refreshes = self
                .window_interaction_release_metrics
                .window_interaction_post_terminal_pointer_refreshes
                .saturating_add(1);
            self.window_interaction_terminal_refresh_root_surface_id
                .take()
        } else {
            None
        };
        if self.active_locked_pointer_binding().is_some() {
            self.refresh_pointer_focus_at_last_position_for_visual_root(
                terminal_visual_root_surface_id,
            );
            return;
        }

        let scene_hit = self.pointer_scene_hit_at(self.last_pointer_x, self.last_pointer_y);
        if !self.pointer_scene_hit_allowed_by_popup_grab(&scene_hit) {
            self.clear_pointer_focus();
            return;
        }
        if matches!(scene_hit, PointerSceneHit::Decoration { .. }) {
            self.focus_desktop_window_at_pointer_scene_hit(&scene_hit);
            self.clear_pointer_focus();
            return;
        }
        let target = match scene_hit {
            PointerSceneHit::Client { target } => Some(target),
            PointerSceneHit::None => terminal_visual_root_surface_id.and_then(|root_surface_id| {
                self.pointer_target_at_visual_root_window(
                    root_surface_id,
                    self.last_pointer_x,
                    self.last_pointer_y,
                )
            }),
            PointerSceneHit::Decoration { .. } => None,
        };
        let new_surface_id = target
            .as_ref()
            .map(|target| compositor_surface_id(&target.surface));
        pointer_debug_log(format!(
            "post-grab focus surface={} -> {}",
            old_surface_id
                .map(|surface_id| surface_id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            new_surface_id
                .map(|surface_id| surface_id.to_string())
                .unwrap_or_else(|| "none".to_string())
        ));
        let Some(target) = target else {
            self.clear_pointer_focus();
            return;
        };
        if !self.pointer_target_allowed_by_popup_grab(&target) {
            self.clear_pointer_focus();
            return;
        }
        self.focus_desktop_window_at_pointer_target(&target);
        self.ensure_pointer_focus(&target.surface);
        self.send_pointer_enter_if_needed(&target);
    }

    pub(in crate::compositor) fn restore_locked_pointer_position(
        &mut self,
        surface: &wl_surface::WlSurface,
        cursor_position_hint: Option<(f64, f64)>,
    ) -> Option<OutputPosition> {
        if let Some((surface_x, surface_y)) = cursor_position_hint {
            if !surface_x.is_finite() || !surface_y.is_finite() {
                pointer_debug_log(format!(
                    "pointer.unlock restore_source=committed_hint ignored reason=non_finite hint=({},{})",
                    surface_x, surface_y
                ));
            } else if let Some((output_x, output_y)) =
                self.output_position_for_valid_cursor_hint(surface, surface_x, surface_y)
            {
                self.last_pointer_x = output_x;
                self.last_pointer_y = output_y;
                pointer_debug_log(format!(
                    "pointer.unlock restore_source=committed_hint hint=({surface_x},{surface_y}) restore_output=({output_x},{output_y})"
                ));
                return Some(OutputPosition {
                    x: output_x,
                    y: output_y,
                });
            } else {
                pointer_debug_log(format!(
                    "pointer.unlock restore_source=committed_hint ignored reason=unresolved hint=({surface_x},{surface_y})"
                ));
            }
        }

        let fallback_position = self
            .active_locked_pointer_routing
            .as_ref()
            .filter(|active| same_surface_resource(&active.surface, surface))
            .map(|active| active.activation_anchor);
        let Some(position) = fallback_position else {
            pointer_debug_log("pointer.unlock restore_source=none restore_output=unchanged");
            return None;
        };
        self.last_pointer_x = position.x;
        self.last_pointer_y = position.y;
        pointer_debug_log(format!(
            "pointer.unlock restore_source=activation_anchor restore_output=({},{})",
            position.x, position.y
        ));
        Some(position)
    }

    pub(in crate::compositor) fn output_position_for_valid_cursor_hint(
        &mut self,
        surface: &wl_surface::WlSurface,
        surface_x: f64,
        surface_y: f64,
    ) -> Option<(f64, f64)> {
        let surface_id = compositor_surface_id(surface);
        self.refresh_surface_origin_cache();
        let index = self
            .renderable_surfaces
            .iter()
            .position(|renderable| renderable.surface_id == surface_id)?;
        let renderable = &self.renderable_surfaces[index];
        if surface_x < 0.0
            || surface_y < 0.0
            || surface_x >= f64::from(renderable.width)
            || surface_y >= f64::from(renderable.height)
        {
            pointer_debug_log(format!(
                "pointer.unlock restore_source=committed_hint ignored reason=out_of_bounds hint=({},{}) size={}x{}",
                surface_x, surface_y, renderable.width, renderable.height
            ));
            return None;
        }
        let origin = self.surface_origin_cache.get(index).copied()?;
        Some((
            f64::from(origin.0) + surface_x,
            f64::from(origin.1) + surface_y,
        ))
    }

    pub(in crate::compositor) fn surface_resource_by_id(
        &self,
        surface_id: u32,
    ) -> Option<wl_surface::WlSurface> {
        self.surface_resources.get(&surface_id).cloned()
    }

    pub(in crate::compositor) fn ensure_pointer_focus(&mut self, surface: &wl_surface::WlSurface) {
        if let Some(active) = self.active_locked_pointer_binding()
            && !same_surface_resource(&active.surface, surface)
        {
            pointer_debug_log(format!(
                "pointer focus change suppressed by locked route id={} locked_surface={} requested={}",
                active.constraint_id,
                compositor_surface_id(&active.surface),
                compositor_surface_id(surface)
            ));
            self.pin_locked_pointer_focus(&active);
            return;
        }
        if let Some(active) = self.active_confined_pointer_binding()
            && !same_surface_resource(&active.surface, surface)
        {
            self.pin_confined_pointer_focus(&active);
            return;
        }
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, surface))
        {
            return;
        }

        self.pointer_hit_metrics.pointer_focus_transitions = self
            .pointer_hit_metrics
            .pointer_focus_transitions
            .saturating_add(1);
        if self.pointer_surface.is_some() {
            self.clear_pointer_focus();
        }
        self.pointer_surface = Some(surface.clone());
    }

    pub(in crate::compositor) fn pointer_resource_entered_surface(
        &self,
        pointer: &wl_pointer::WlPointer,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.pointer_entered_surfaces
            .iter()
            .any(|(resource, entered_surface)| {
                same_wayland_resource(resource, pointer)
                    && same_surface_resource(entered_surface, surface)
            })
    }

    pub(in crate::compositor) fn pointer_has_current_enter_serial(
        &self,
        pointer: &wl_pointer::WlPointer,
        serial: u32,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.pointer_enter_serials.iter().any(|entry| {
            same_wayland_resource(&entry.pointer, pointer)
                && same_surface_resource(&entry.surface, surface)
                && entry.serial == serial
        })
    }

    pub(in crate::compositor) fn pointer_has_current_enter_serial_for_client(
        &self,
        pointer: &wl_pointer::WlPointer,
        serial: u32,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        resource_belongs_to_surface_client(pointer, surface)
            && self.validate_set_cursor_serial(serial, surface)
    }

    pub(in crate::compositor) fn warp_pointer_protocol_request(
        &mut self,
        surface: wl_surface::WlSurface,
        pointer: wl_pointer::WlPointer,
        surface_x: f64,
        surface_y: f64,
        serial: u32,
    ) {
        let reject = |reason: &str| {
            pointer_debug_log(format!(
                "pointer_warp rejected pointer={} surface={} serial={} local=({},{}) reason={}",
                pointer.id().protocol_id(),
                compositor_surface_id(&surface),
                serial,
                surface_x,
                surface_y,
                reason
            ));
        };
        if !pointer.is_alive() || !surface.is_alive() {
            reject("dead_resource");
            return;
        }
        if !surface_x.is_finite() || !surface_y.is_finite() {
            reject("non_finite");
            return;
        }
        if !resource_belongs_to_surface_client(&pointer, &surface) {
            reject("wrong_client_pointer");
            return;
        }
        if !self
            .pointer_resources
            .iter()
            .any(|resource| same_wayland_resource(resource, &pointer))
        {
            reject("unknown_pointer");
            return;
        }
        let focused_surface = self
            .implicit_pointer_grab
            .as_ref()
            .map(|grab| grab.surface.clone())
            .or_else(|| self.pointer_surface.clone());
        let Some(focused_surface) = focused_surface else {
            reject("no_pointer_focus");
            return;
        };
        if !same_surface_resource(&focused_surface, &surface) {
            reject("surface_not_focused");
            return;
        }
        if !self.pointer_has_current_enter_serial_for_client(&pointer, serial, &surface) {
            reject("invalid_serial");
            return;
        }
        let Some(position) =
            self.valid_cursor_hint_output_position(&surface, Some((surface_x, surface_y)))
        else {
            reject("out_of_surface");
            return;
        };
        pointer_debug_log(format!(
            "pointer_warp accepted pointer={} serial={} local=({},{}) output=({},{}) matches_pending_unlock={}",
            pointer.id().protocol_id(),
            serial,
            surface_x,
            surface_y,
            position.x,
            position.y,
            self.pending_locked_pointer_reveal_matches(&pointer, &surface)
        ));
        let matches_pending_unlock = self.pending_locked_pointer_reveal_matches(&pointer, &surface);
        self.apply_pointer_warp(position, true);
        if matches_pending_unlock {
            if let Some(pending) = self.pending_locked_pointer_reveal.as_mut() {
                pending.fallback_position = Some(position);
            }
            self.finalize_pending_locked_pointer_reveal("matching_client_warp");
        }
    }

    pub(in crate::compositor) fn remember_pointer_enter_serial(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        surface: &wl_surface::WlSurface,
        serial: u32,
    ) {
        self.pointer_enter_serials
            .retain(|entry| !same_wayland_resource(&entry.pointer, pointer));
        self.pointer_enter_serials.push(PointerEnterSerial {
            pointer: pointer.clone(),
            surface: surface.clone(),
            serial,
        });
    }

    pub(in crate::compositor) fn forget_pointer_enter_serial(
        &mut self,
        pointer: &wl_pointer::WlPointer,
    ) {
        self.pointer_enter_serials
            .retain(|entry| !same_wayland_resource(&entry.pointer, pointer));
    }

    pub(in crate::compositor) fn synchronize_pointer_resource_focus(
        &mut self,
        pointer: &wl_pointer::WlPointer,
    ) -> bool {
        let Some(focused_surface) = self.pointer_surface.clone() else {
            return false;
        };
        if !pointer.is_alive() || !resource_belongs_to_surface_client(pointer, &focused_surface) {
            return false;
        }
        if self.pointer_resource_entered_surface(pointer, &focused_surface) {
            return true;
        }
        let Some(target) = self.pointer_target_at(self.last_pointer_x, self.last_pointer_y) else {
            return false;
        };
        if !same_surface_resource(&target.surface, &focused_surface) {
            return false;
        }
        self.send_pointer_enter_to_resource(pointer, &target);
        true
    }

    pub(in crate::compositor) fn send_pointer_enter_to_resource(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        target: &PointerTarget,
    ) {
        if let Some(index) = self
            .pointer_entered_surfaces
            .iter()
            .position(|(resource, _)| same_wayland_resource(resource, pointer))
        {
            if same_surface_resource(&self.pointer_entered_surfaces[index].1, &target.surface) {
                return;
            }

            let (_, previous_surface) = self.pointer_entered_surfaces.remove(index);
            self.forget_pointer_enter_serial(pointer);
            if resource_belongs_to_surface_client(pointer, &previous_surface) {
                let serial = self.next_configure_serial();
                let _ = pointer.send_event(wl_pointer::Event::Leave {
                    serial,
                    surface: previous_surface,
                });
                send_pointer_frame_if_supported(pointer);
            }
        }

        let serial = self.next_configure_serial();
        let _ = pointer.send_event(wl_pointer::Event::Enter {
            serial,
            surface: target.surface.clone(),
            surface_x: target.surface_x,
            surface_y: target.surface_y,
        });
        pointer_debug_log(format!(
            "wl_pointer {} synchronized enter for surface {}",
            pointer.id().protocol_id(),
            compositor_surface_id(&target.surface)
        ));
        self.remember_input_serial(
            serial,
            target.surface.clone(),
            InputSerialKind::PointerEnter,
        );
        self.remember_pointer_enter_serial(pointer, &target.surface, serial);
        send_pointer_frame_if_supported(pointer);
        self.pointer_entered_surfaces
            .push((pointer.clone(), target.surface.clone()));
    }

    pub(in crate::compositor) fn send_pointer_enter_if_needed(&mut self, target: &PointerTarget) {
        self.pointer_resources.retain(Resource::is_alive);
        let pointers = self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &target.surface))
            .cloned()
            .collect::<Vec<_>>();

        for pointer in pointers {
            self.send_pointer_enter_to_resource(&pointer, target);
        }
        let surface_id = compositor_surface_id(&target.surface);
        let constraint_ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for constraint_id in constraint_ids {
            self.maybe_request_pointer_constraint_activation(constraint_id);
        }
    }

    fn queue_pointer_enter_events(
        &mut self,
        target: &PointerTarget,
        frame_pointers: &mut Vec<wl_pointer::WlPointer>,
    ) {
        self.pointer_resources.retain(Resource::is_alive);
        let pointers = self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &target.surface))
            .cloned()
            .collect::<Vec<_>>();
        for pointer in pointers {
            if self.pointer_resource_entered_surface(&pointer, &target.surface) {
                continue;
            }
            let serial = self.next_configure_serial();
            let _ = pointer.send_event(wl_pointer::Event::Enter {
                serial,
                surface: target.surface.clone(),
                surface_x: target.surface_x,
                surface_y: target.surface_y,
            });
            self.remember_input_serial(
                serial,
                target.surface.clone(),
                InputSerialKind::PointerEnter,
            );
            self.remember_pointer_enter_serial(&pointer, &target.surface, serial);
            self.pointer_entered_surfaces
                .push((pointer.clone(), target.surface.clone()));
            push_pointer_frame_once(frame_pointers, pointer);
        }
        let surface_id = compositor_surface_id(&target.surface);
        let constraint_ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for constraint_id in constraint_ids {
            self.maybe_request_pointer_constraint_activation(constraint_id);
        }
    }

    fn clear_pointer_focus_state(&mut self) -> Vec<(wl_pointer::WlPointer, wl_surface::WlSurface)> {
        if let Some(active) = self.active_locked_pointer_binding() {
            self.pin_locked_pointer_focus(&active);
            return Vec::new();
        }
        if let Some(active) = self.active_confined_pointer_binding() {
            self.pin_confined_pointer_focus(&active);
            return Vec::new();
        }
        if self.pointer_surface.is_none()
            && self.pointer_entered_surfaces.is_empty()
            && self.focused_client_cursor.is_none()
            && self.cursor_visibility.client_hidden_pointer.is_none()
            && self.cursor_visibility.client_cursor_pointer.is_none()
        {
            return Vec::new();
        }
        if let Some(surface_id) = self.pointer_surface.as_ref().map(compositor_surface_id) {
            self.deactivate_pointer_constraints_for_surface_focus_loss(surface_id, true);
        }
        let cleared_client_cursor = self.focused_client_cursor.take().is_some();
        self.cursor_visibility.client_hidden_pointer = None;
        self.cursor_visibility.client_cursor_pointer = None;
        if cleared_client_cursor {
            self.advance_render_generation(RenderGenerationCause::CursorState);
        }
        self.sync_cursor_visibility_request();
        self.pointer_surface = None;
        self.pointer_resources.retain(Resource::is_alive);
        let pointers = self.pointer_resources.clone();
        let mut leaves = Vec::new();
        for pointer in pointers {
            let Some(index) = self
                .pointer_entered_surfaces
                .iter()
                .position(|(resource, _)| same_wayland_resource(resource, &pointer))
            else {
                continue;
            };
            let (_, surface) = self.pointer_entered_surfaces.remove(index);
            self.forget_pointer_enter_serial(&pointer);
            if resource_belongs_to_surface_client(&pointer, &surface) {
                leaves.push((pointer, surface));
            }
        }
        leaves
    }

    pub(in crate::compositor) fn clear_pointer_focus(&mut self) {
        let leaves = self.clear_pointer_focus_state();
        for (pointer, surface) in leaves {
            let serial = self.next_configure_serial();
            let _ = pointer.send_event(wl_pointer::Event::Leave { serial, surface });
            send_pointer_frame_if_supported(&pointer);
        }
    }
}

fn push_pointer_frame_once(
    frame_pointers: &mut Vec<wl_pointer::WlPointer>,
    pointer: wl_pointer::WlPointer,
) {
    if !frame_pointers
        .iter()
        .any(|existing| same_wayland_resource(existing, &pointer))
    {
        frame_pointers.push(pointer);
    }
}
