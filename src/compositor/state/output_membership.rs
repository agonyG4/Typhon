use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::compositor) struct PhysicalOutputId(u8);

const NATIVE_PHYSICAL_OUTPUT: PhysicalOutputId = PhysicalOutputId(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct SurfaceBufferPreference {
    pub(in crate::compositor) scale: i32,
    pub(in crate::compositor) transform: wl_output::Transform,
}

#[derive(Debug, Default, Clone)]
pub(in crate::compositor) struct SurfaceOutputMembership {
    pub(in crate::compositor) physical_outputs: HashSet<PhysicalOutputId>,
    pub(in crate::compositor) entered_resources: HashSet<u32>,
    pub(in crate::compositor) last_preference: Option<SurfaceBufferPreference>,
}

impl CompositorState {
    pub(in crate::compositor) fn register_output_resource(&mut self, output: wl_output::WlOutput) {
        if self
            .output_resources
            .iter()
            .any(|resource| same_wayland_resource(resource, &output))
        {
            return;
        }
        send_output_description(
            &output,
            self.output_size,
            self.output_scale,
            self.output_refresh,
        );
        self.output_resources.push(output);
        self.reconcile_all_surface_output_memberships();
    }

    pub(in crate::compositor) fn unregister_output_resource(
        &mut self,
        output: &wl_output::WlOutput,
    ) {
        let output_id = output.id().protocol_id();
        let affected_surfaces = self
            .surface_output_memberships
            .iter()
            .filter(|(_, membership)| membership.entered_resources.contains(&output_id))
            .map(|(surface_id, _)| *surface_id)
            .collect::<Vec<_>>();
        for surface_id in affected_surfaces {
            if let Some(surface) = self.surface_resource_by_id(surface_id)
                && surface.is_alive()
            {
                let _ = surface.send_event(wl_surface::Event::Leave {
                    output: output.clone(),
                });
                self.compliance_metrics.surface_leave_events = self
                    .compliance_metrics
                    .surface_leave_events
                    .saturating_add(1);
            }
        }
        self.output_resources
            .retain(|resource| !same_wayland_resource(resource, output));
        for membership in self.surface_output_memberships.values_mut() {
            membership.entered_resources.remove(&output_id);
        }
        self.scrub_empty_surface_output_memberships();
    }

    pub(in crate::compositor) fn send_output_mode_to_bound_outputs(&self) {
        for output in &self.output_resources {
            send_output_mode(output, self.output_size, self.output_refresh);
            send_output_done_if_supported(output);
        }
    }

    pub(in crate::compositor) fn send_output_scale_to_bound_outputs(&self) {
        for output in &self.output_resources {
            send_output_scale(output, self.output_scale);
            send_output_done_if_supported(output);
        }
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
        let surface_id = compositor_surface_id(surface);
        let overlaps = self.surface_overlaps_native_output(surface_id);
        let membership = self
            .surface_output_memberships
            .entry(surface_id)
            .or_default();

        if overlaps {
            membership.physical_outputs.insert(NATIVE_PHYSICAL_OUTPUT);
        } else {
            membership.physical_outputs.remove(&NATIVE_PHYSICAL_OUTPUT);
        }

        let output_resources = self
            .output_resources
            .iter()
            .filter(|output| resource_belongs_to_surface_client(*output, surface))
            .cloned()
            .collect::<Vec<_>>();
        if !overlaps {
            for output in output_resources {
                let output_id = output.id().protocol_id();
                if membership.entered_resources.remove(&output_id) && surface.is_alive() {
                    let _ = surface.send_event(wl_surface::Event::Leave { output });
                    self.compliance_metrics.surface_leave_events = self
                        .compliance_metrics
                        .surface_leave_events
                        .saturating_add(1);
                }
            }
            self.scrub_empty_surface_output_memberships();
            return;
        }

        for output in output_resources {
            let output_id = output.id().protocol_id();
            if membership.entered_resources.insert(output_id) && surface.is_alive() {
                let _ = surface.send_event(wl_surface::Event::Enter {
                    output: output.clone(),
                });
                self.compliance_metrics.surface_enter_events = self
                    .compliance_metrics
                    .surface_enter_events
                    .saturating_add(1);
            }
        }
        self.send_preferred_buffer_preferences(surface.clone());
    }

    pub(in crate::compositor) fn reconcile_all_surface_output_memberships(&mut self) {
        let surfaces = self.surface_resources.values().cloned().collect::<Vec<_>>();
        for surface in surfaces {
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
                membership.entered_resources.iter().all(|output_id| {
                    self.output_resources.iter().any(|output| {
                        output.id().protocol_id() == *output_id
                            && output.is_alive()
                            && resource_belongs_to_surface_client(output, surface)
                    })
                })
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
