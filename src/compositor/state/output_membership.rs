use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

static OUTPUT_LIFECYCLE_DIAGNOSTIC_COUNT: AtomicUsize = AtomicUsize::new(0);
const OUTPUT_LIFECYCLE_DIAGNOSTIC_LIMIT: usize = 256;

fn output_lifecycle_trace_enabled(setting: Option<&str>) -> bool {
    setting == Some("1")
}

#[derive(Debug, Clone)]
pub(in crate::compositor) struct OutputBinding {
    pub(in crate::compositor) output_id: OutputId,
    pub(in crate::compositor) client_id: ClientId,
    pub(in crate::compositor) resource: wl_output::WlOutput,
}

#[derive(Clone, Copy)]
enum SurfaceOutputEvent {
    Enter,
    Leave,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compositor) fn output_lifecycle_trace(
    event: &str,
    transition: &str,
    surface_id: Option<&ObjectId>,
    binding: &OutputBinding,
    binding_alive: bool,
    surface_alive: Option<bool>,
    current: bool,
    outcome: &str,
    error: Option<&str>,
) {
    if !output_lifecycle_trace_enabled(
        std::env::var("TYPHON_OUTPUT_LIFECYCLE_TRACE")
            .ok()
            .as_deref(),
    ) {
        return;
    }
    if OUTPUT_LIFECYCLE_DIAGNOSTIC_COUNT.fetch_add(1, Ordering::Relaxed)
        >= OUTPUT_LIFECYCLE_DIAGNOSTIC_LIMIT
    {
        return;
    }
    eprintln!(
        "typhon output_lifecycle event={event} transition={transition} surface={surface_id:?} binding={:?} protocol_id={} output_id={:?} client={:?} binding_alive={binding_alive} surface_alive={surface_alive:?} current={current} outcome={outcome} error={error:?}",
        binding.resource.id(),
        binding.resource.id().protocol_id(),
        binding.output_id,
        binding.client_id,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::OutputId;

    #[test]
    fn membership_keeps_distinct_logical_output_ids_separate() {
        let first = OutputId::from_raw(1).expect("nonzero output id");
        let second = OutputId::from_raw(2).expect("nonzero output id");
        let mut membership = SurfaceOutputMembership::default();

        membership.physical_outputs.insert(first);

        assert!(membership.physical_outputs.contains(&first));
        assert!(!membership.physical_outputs.contains(&second));
    }

    #[test]
    fn compositor_allocates_one_real_native_output_id() {
        let state = CompositorState::new(None);

        assert_eq!(state.native_output_id().map(OutputId::get), Some(1));
    }

    #[test]
    fn output_lifecycle_trace_requires_exact_explicit_opt_in() {
        assert!(output_lifecycle_trace_enabled(Some("1")));
        assert!(!output_lifecycle_trace_enabled(None));
        assert!(!output_lifecycle_trace_enabled(Some("0")));
        assert!(!output_lifecycle_trace_enabled(Some("true")));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct SurfaceBufferPreference {
    pub(in crate::compositor) scale: i32,
    pub(in crate::compositor) transform: wl_output::Transform,
}

#[derive(Debug, Default, Clone)]
pub(in crate::compositor) struct SurfaceOutputMembership {
    pub(in crate::compositor) physical_outputs: HashSet<OutputId>,
    pub(in crate::compositor) entered_resources: HashSet<ObjectId>,
    pub(in crate::compositor) last_preference: Option<SurfaceBufferPreference>,
}

impl CompositorState {
    pub(in crate::compositor) fn register_output_resource(&mut self, output: wl_output::WlOutput) {
        let Some(output_id) = self
            .native_output_id()
            .filter(|output_id| self.logical_output_ids.contains(output_id))
        else {
            return;
        };
        let Some(client_id) = output.client().map(|client| client.id()) else {
            return;
        };
        let object_id = output.id();
        if self
            .output_resources
            .iter()
            .any(|binding| binding.resource.id() == object_id)
        {
            return;
        }
        send_output_description(
            &output,
            self.output_size,
            self.output_scale,
            self.output_refresh,
        );
        let binding = OutputBinding {
            output_id,
            client_id,
            resource: output,
        };
        self.output_resources.push(binding.clone());
        self.publish_workspace_output_enter(&binding);
        self.reconcile_all_surface_output_memberships();
        debug_assert!(self.check_surface_output_membership_invariants());
        debug_assert!(self.check_output_binding_invariants());
    }

    pub(in crate::compositor) fn forget_output_binding(
        &mut self,
        output: &wl_output::WlOutput,
        transition: &str,
    ) {
        let object_id = output.id();
        let forgotten = self
            .output_resources
            .iter()
            .find(|binding| binding.resource.id() == object_id)
            .cloned();
        self.output_resources
            .retain(|binding| binding.resource.id() != object_id);
        for membership in self.surface_output_memberships.values_mut() {
            membership.entered_resources.remove(&object_id);
        }
        self.workspace_protocol.forget_output_binding(&object_id);
        if let Some(binding) = forgotten {
            output_lifecycle_trace(
                "binding_destroyed",
                transition,
                None,
                &binding,
                binding.resource.is_alive(),
                None,
                false,
                "bookkeeping_removed",
                None,
            );
        }
        self.scrub_empty_surface_output_memberships();
    }

    pub(in crate::compositor) fn forget_output_bindings_for_client(
        &mut self,
        client_id: &ClientId,
    ) {
        let outputs = self
            .output_resources
            .iter()
            .filter(|binding| binding.client_id == *client_id)
            .map(|binding| binding.resource.clone())
            .collect::<Vec<_>>();
        for output in outputs {
            self.forget_output_binding(&output, "client_teardown");
        }
        debug_assert!(self.check_surface_output_membership_invariants());
        debug_assert!(self.check_output_binding_invariants());
    }

    // This retires Typhon's logical and binding state but leaves the advertised wl_output global
    // in place. The runtime has no hotplug feed today; before DRM hotplug calls this, the global
    // needs first-class lifetime ownership so withdrawal can also remove it from the registry.
    #[allow(dead_code)]
    pub(in crate::compositor) fn withdraw_output(&mut self, output_id: OutputId) {
        if !self.logical_output_ids.contains(&output_id) {
            return;
        }
        let bindings = self
            .output_resources
            .iter()
            .filter(|binding| binding.output_id == output_id)
            .cloned()
            .collect::<Vec<_>>();
        self.fail_astrea_screen_captures_for_output(output_id, "output_gone");
        for binding in &bindings {
            output_lifecycle_trace(
                "logical_output.withdraw",
                "logical_output_withdrawal",
                None,
                binding,
                binding.resource.is_alive(),
                None,
                true,
                "started",
                None,
            );
            let binding_id = binding.resource.id();
            let surfaces = self
                .surface_resources
                .values()
                .filter(|surface| {
                    surface
                        .client()
                        .is_some_and(|client| client.id() == binding.client_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            for surface in surfaces {
                let surface_id = compositor_surface_id(&surface);
                let was_entered = self
                    .surface_output_memberships
                    .get_mut(&surface_id)
                    .is_some_and(|membership| membership.entered_resources.remove(&binding_id));
                if was_entered {
                    self.send_surface_output_event(
                        &surface,
                        binding,
                        SurfaceOutputEvent::Leave,
                        "logical_output_withdrawal",
                    );
                }
            }
            self.publish_workspace_output_leave(binding);
        }
        for membership in self.surface_output_memberships.values_mut() {
            membership.physical_outputs.remove(&output_id);
            for binding in &bindings {
                membership.entered_resources.remove(&binding.resource.id());
            }
        }
        for binding in &bindings {
            self.forget_output_binding(&binding.resource, "logical_output_withdrawal");
        }
        self.logical_output_ids.remove(&output_id);
        self.scrub_empty_surface_output_memberships();
        debug_assert!(self.check_surface_output_membership_invariants());
        debug_assert!(self.check_output_binding_invariants());
    }

    pub(in crate::compositor) fn send_output_mode_to_bound_outputs(&self) {
        for binding in &self.output_resources {
            send_output_mode(&binding.resource, self.output_size, self.output_refresh);
            send_output_done_if_supported(&binding.resource);
        }
    }

    pub(in crate::compositor) fn send_output_scale_to_bound_outputs(&self) {
        for binding in &self.output_resources {
            send_output_scale(&binding.resource, self.output_scale);
            send_output_done_if_supported(&binding.resource);
        }
    }

    fn send_surface_output_event(
        &mut self,
        surface: &wl_surface::WlSurface,
        binding: &OutputBinding,
        event: SurfaceOutputEvent,
        transition: &str,
    ) -> bool {
        let surface_object_id = surface.id();
        let binding_object_id = binding.resource.id();
        let current = self.logical_output_ids.contains(&binding.output_id)
            && self
                .output_resources
                .iter()
                .any(|candidate| candidate.resource.id() == binding_object_id);
        let surface_alive = surface.is_alive();
        let binding_alive = binding.resource.is_alive();
        if !surface_alive || !binding_alive || !current {
            output_lifecycle_trace(
                match event {
                    SurfaceOutputEvent::Enter => "wl_surface.enter",
                    SurfaceOutputEvent::Leave => "wl_surface.leave",
                },
                transition,
                Some(&surface_object_id),
                binding,
                binding_alive,
                Some(surface_alive),
                current,
                "not_attempted",
                Some("surface_or_binding_not_alive_or_current"),
            );
            return false;
        }
        let result = match event {
            SurfaceOutputEvent::Enter => surface.send_event(wl_surface::Event::Enter {
                output: binding.resource.clone(),
            }),
            SurfaceOutputEvent::Leave => surface.send_event(wl_surface::Event::Leave {
                output: binding.resource.clone(),
            }),
        };
        let (outcome, error) = match result {
            Ok(()) => {
                match event {
                    SurfaceOutputEvent::Enter => {
                        self.compliance_metrics.surface_enter_events = self
                            .compliance_metrics
                            .surface_enter_events
                            .saturating_add(1);
                    }
                    SurfaceOutputEvent::Leave => {
                        self.compliance_metrics.surface_leave_events = self
                            .compliance_metrics
                            .surface_leave_events
                            .saturating_add(1);
                    }
                }
                ("queued", None)
            }
            Err(error) => ("rejected", Some(format!("{error:?}"))),
        };
        output_lifecycle_trace(
            match event {
                SurfaceOutputEvent::Enter => "wl_surface.enter",
                SurfaceOutputEvent::Leave => "wl_surface.leave",
            },
            transition,
            Some(&surface_object_id),
            binding,
            binding_alive,
            Some(surface_alive),
            current,
            outcome,
            error.as_deref(),
        );
        error.is_none()
    }

    pub(in crate::compositor) fn register_fractional_scale_resource(
        &mut self,
        surface: &wl_surface::WlSurface,
        fractional_scale: wp_fractional_scale_v1::WpFractionalScaleV1,
    ) {
        let surface_id = compositor_surface_id(surface);
        fractional_scale.preferred_scale(self.output_scale.preferred_scale());
        self.fractional_scale_resources
            .entry(surface_id)
            .or_default()
            .push(fractional_scale);
    }

    pub(in crate::compositor) fn unregister_fractional_scale_resources_for_surface(
        &mut self,
        surface_id: u32,
    ) {
        self.fractional_scale_resources.remove(&surface_id);
    }

    pub(in crate::compositor) fn unregister_fractional_scale_resource(
        &mut self,
        surface_id: u32,
        resource_id: u32,
    ) {
        if let Some(resources) = self.fractional_scale_resources.get_mut(&surface_id) {
            resources.retain(|resource| resource.id().protocol_id() != resource_id);
            if resources.is_empty() {
                self.fractional_scale_resources.remove(&surface_id);
            }
        }
    }

    pub(in crate::compositor) fn send_fractional_scale_to_bound_surfaces(&self) {
        for fractional_scales in self.fractional_scale_resources.values() {
            for fractional_scale in fractional_scales {
                fractional_scale.preferred_scale(self.output_scale.preferred_scale());
            }
        }
    }

    pub(in crate::compositor) fn reconcile_surface_output_membership(
        &mut self,
        surface: &wl_surface::WlSurface,
    ) {
        self.compliance_metrics.membership_surfaces_inspected = self
            .compliance_metrics
            .membership_surfaces_inspected
            .saturating_add(1);
        let surface_id = compositor_surface_id(surface);
        let Some(native_output_id) = self.ensure_native_output_id() else {
            return;
        };
        let overlaps = self.logical_output_ids.contains(&native_output_id)
            && self.surface_overlaps_native_output(surface_id);
        let was_overlapping = self
            .surface_output_memberships
            .entry(surface_id)
            .or_default()
            .physical_outputs
            .contains(&native_output_id);
        let mut membership_changed = was_overlapping != overlaps;

        {
            let membership = self
                .surface_output_memberships
                .entry(surface_id)
                .or_default();
            if overlaps {
                membership.physical_outputs.insert(native_output_id);
            } else {
                membership.physical_outputs.remove(&native_output_id);
            }
        }

        let output_bindings = self
            .output_resources
            .iter()
            .filter(|binding| {
                self.logical_output_ids.contains(&binding.output_id)
                    && output_binding_belongs_to_surface_client(binding, surface)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !overlaps {
            for binding in output_bindings {
                let binding_id = binding.resource.id();
                let was_entered = self
                    .surface_output_memberships
                    .get_mut(&surface_id)
                    .is_some_and(|membership| membership.entered_resources.remove(&binding_id));
                if was_entered {
                    membership_changed = true;
                    self.send_surface_output_event(
                        surface,
                        &binding,
                        SurfaceOutputEvent::Leave,
                        "surface_reconciliation",
                    );
                }
            }
            if !membership_changed {
                self.compliance_metrics.membership_noops =
                    self.compliance_metrics.membership_noops.saturating_add(1);
            }
            self.scrub_empty_surface_output_memberships();
            debug_assert!(self.check_surface_output_membership_invariants());
            return;
        }

        for binding in output_bindings {
            let binding_id = binding.resource.id();
            let already_entered = self
                .surface_output_memberships
                .get(&surface_id)
                .is_some_and(|membership| membership.entered_resources.contains(&binding_id));
            if !already_entered
                && self.send_surface_output_event(
                    surface,
                    &binding,
                    SurfaceOutputEvent::Enter,
                    "surface_reconciliation",
                )
            {
                self.surface_output_memberships
                    .entry(surface_id)
                    .or_default()
                    .entered_resources
                    .insert(binding_id);
                membership_changed = true;
            }
        }
        if !membership_changed {
            self.compliance_metrics.membership_noops =
                self.compliance_metrics.membership_noops.saturating_add(1);
        }
        self.send_preferred_buffer_preferences(surface.clone());
        debug_assert!(self.check_surface_output_membership_invariants());
    }

    pub(in crate::compositor) fn reconcile_all_surface_output_memberships(&mut self) {
        self.compliance_metrics.broad_membership_reconciliations = self
            .compliance_metrics
            .broad_membership_reconciliations
            .saturating_add(1);
        let surfaces = self.surface_resources.values().cloned().collect::<Vec<_>>();
        for surface in surfaces {
            self.reconcile_surface_output_membership(&surface);
        }
        self.reconcile_idle_inhibition();
    }

    pub(in crate::compositor) fn reconcile_surface_tree_output_memberships(
        &mut self,
        root_surface_id: u32,
    ) {
        self.compliance_metrics
            .affected_root_membership_reconciliations = self
            .compliance_metrics
            .affected_root_membership_reconciliations
            .saturating_add(1);
        let surface_ids = self
            .renderable_surfaces
            .iter()
            .filter(|surface| {
                self.root_surface_id_for_surface(surface.surface_id) == root_surface_id
            })
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        for surface_id in surface_ids {
            let Some(surface) = self.surface_resource_by_id(surface_id) else {
                continue;
            };
            self.reconcile_surface_output_membership(&surface);
        }
    }

    pub(in crate::compositor) fn scrub_surface_output_membership(&mut self, surface_id: u32) {
        self.surface_output_memberships.remove(&surface_id);
    }

    pub(in crate::compositor) fn check_surface_output_membership_invariants(&self) -> bool {
        self.surface_output_memberships
            .iter()
            .all(|(surface_id, membership)| {
                let Some(surface) = self.surface_resources.get(surface_id) else {
                    return false;
                };
                membership
                    .physical_outputs
                    .iter()
                    .all(|output_id| self.logical_output_ids.contains(output_id))
                    && membership.entered_resources.iter().all(|object_id| {
                        self.output_resources.iter().any(|output| {
                            output.resource.id() == *object_id
                                && self.logical_output_ids.contains(&output.output_id)
                                && output.resource.is_alive()
                                && output_binding_belongs_to_surface_client(output, surface)
                        })
                    })
            })
    }

    pub(in crate::compositor) fn check_output_binding_invariants(&self) -> bool {
        #[allow(clippy::mutable_key_type)]
        let mut object_ids = HashSet::new();
        self.output_resources.iter().all(|binding| {
            binding.resource.is_alive()
                && self.logical_output_ids.contains(&binding.output_id)
                && binding
                    .resource
                    .client()
                    .is_some_and(|client| client.id() == binding.client_id)
                && object_ids.insert(binding.resource.id())
        })
    }

    fn scrub_empty_surface_output_memberships(&mut self) {
        self.surface_output_memberships.retain(|_, membership| {
            !membership.physical_outputs.is_empty()
                || !membership.entered_resources.is_empty()
                || membership.last_preference.is_some()
        });
    }

    fn surface_overlaps_native_output(&mut self, surface_id: u32) -> bool {
        if !self.surface_is_effectively_mapped_for_output(surface_id) {
            return false;
        }
        self.refresh_surface_origin_cache();
        let Some(index) = self
            .renderable_surfaces
            .iter()
            .position(|surface| surface.surface_id == surface_id)
        else {
            return false;
        };
        let Some((x, y)) = self.surface_origin_cache.get(index).copied() else {
            return false;
        };
        let surface = &self.renderable_surfaces[index];
        let output_width = i64::from(self.output_size.width);
        let output_height = i64::from(self.output_size.height);
        let left = i64::from(x);
        let top = i64::from(y);
        let right = left.saturating_add(i64::from(surface.width));
        let bottom = top.saturating_add(i64::from(surface.height));
        left < output_width && top < output_height && right > 0 && bottom > 0
    }

    fn surface_is_effectively_mapped_for_output(&self, surface_id: u32) -> bool {
        let mut current = Some(surface_id);
        let mut visited = HashSet::new();
        while let Some(id) = current {
            if !visited.insert(id)
                || !self
                    .renderable_surfaces
                    .iter()
                    .any(|surface| surface.surface_id == id)
            {
                return false;
            }
            current = self
                .renderable_surfaces
                .iter()
                .find(|surface| surface.surface_id == id)
                .and_then(|surface| surface.placement.parent_surface_id);
        }
        true
    }

    fn send_preferred_buffer_preferences(&mut self, surface: wl_surface::WlSurface) {
        // The Core defaults are scale 1 and normal transform. Announce a
        // scale only when a non-default single-output preference is selected
        // or when that preference changes back.
        if surface.version() < 6 {
            return;
        }
        let preferred = SurfaceBufferPreference {
            scale: self.output_scale.wl_output_scale(),
            transform: self
                .preferred_output_transform
                .unwrap_or(wl_output::Transform::Normal),
        };
        let surface_id = compositor_surface_id(&surface);
        let previous = self
            .surface_output_memberships
            .entry(surface_id)
            .or_default()
            .last_preference
            .replace(preferred);
        if previous == Some(preferred) {
            return;
        }
        if previous.map_or(preferred.scale != 1, |old| old.scale != preferred.scale) {
            let _ = surface.send_event(wl_surface::Event::PreferredBufferScale {
                factor: preferred.scale,
            });
            self.compliance_metrics.preferred_scale_events = self
                .compliance_metrics
                .preferred_scale_events
                .saturating_add(1);
        }
        if previous.map_or(preferred.transform != wl_output::Transform::Normal, |old| {
            old.transform != preferred.transform
        }) {
            let _ = surface.send_event(wl_surface::Event::PreferredBufferTransform {
                transform: WEnum::Value(preferred.transform),
            });
            self.compliance_metrics.preferred_transform_events = self
                .compliance_metrics
                .preferred_transform_events
                .saturating_add(1);
        }
    }
}

fn output_binding_belongs_to_surface_client(
    binding: &OutputBinding,
    surface: &wl_surface::WlSurface,
) -> bool {
    surface
        .client()
        .is_some_and(|client| client.id() == binding.client_id)
}
