use super::*;
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::server::zwp_keyboard_shortcuts_inhibitor_v1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::compositor) struct ShortcutInhibitorId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::compositor) struct LogicalSeatId(u8);

impl LogicalSeatId {
    pub(in crate::compositor) const PRIMARY: Self = Self(0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum ShortcutInhibitionEventKind {
    Active,
    Inactive,
}

#[derive(Debug, Clone)]
pub(in crate::compositor) struct ShortcutInhibitionEvent {
    pub(in crate::compositor) resource:
        zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
    pub(in crate::compositor) kind: ShortcutInhibitionEventKind,
}

#[derive(Debug, Clone)]
struct ShortcutInhibitor {
    id: ShortcutInhibitorId,
    surface: wl_surface::WlSurface,
    seat: LogicalSeatId,
    client_id: ClientId,
    resource: zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
    policy_enabled: bool,
    effective: bool,
}

#[derive(Debug, Default)]
pub(in crate::compositor) struct ShortcutInhibitionRegistry {
    next_id: u64,
    effective_generation: u64,
    effective_count: usize,
    inhibitors: HashMap<ShortcutInhibitorId, ShortcutInhibitor>,
    metrics: KeyboardShortcutInhibitionMetrics,
}

impl ShortcutInhibitionRegistry {
    pub(in crate::compositor) fn contains_pair(
        &self,
        surface: &wl_surface::WlSurface,
        seat: LogicalSeatId,
    ) -> bool {
        self.inhibitors.values().any(|inhibitor| {
            inhibitor.seat == seat && same_surface_resource(&inhibitor.surface, surface)
        })
    }

    pub(in crate::compositor) fn insert(
        &mut self,
        surface: wl_surface::WlSurface,
        seat: LogicalSeatId,
        client_id: ClientId,
        resource: zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
    ) -> ShortcutInhibitorId {
        let id = self.allocate_id();
        self.inhibitors.insert(
            id,
            ShortcutInhibitor {
                id,
                surface,
                seat,
                client_id,
                resource,
                policy_enabled: true,
                effective: false,
            },
        );
        self.metrics.created = self.metrics.created.saturating_add(1);
        id
    }

    pub(in crate::compositor) fn note_duplicate(&mut self) {
        self.metrics.duplicate_requests = self.metrics.duplicate_requests.saturating_add(1);
    }

    pub(in crate::compositor) fn remove(&mut self, id: ShortcutInhibitorId) -> bool {
        let Some(inhibitor) = self.inhibitors.remove(&id) else {
            return false;
        };
        self.metrics.destroyed = self.metrics.destroyed.saturating_add(1);
        if inhibitor.effective {
            self.effective_count = self.effective_count.saturating_sub(1);
            self.bump_generation();
        }
        true
    }

    pub(in crate::compositor) fn remove_resource(
        &mut self,
        resource: &zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
    ) -> bool {
        let id = self
            .inhibitors
            .values()
            .find(|inhibitor| same_wayland_resource(&inhibitor.resource, resource))
            .map(|inhibitor| inhibitor.id);
        id.is_some_and(|id| self.remove(id))
    }

    pub(in crate::compositor) fn remove_surface(&mut self, surface_id: u32) -> usize {
        self.remove_where(|inhibitor| compositor_surface_id(&inhibitor.surface) == surface_id)
    }

    pub(in crate::compositor) fn remove_client(&mut self, client_id: &ClientId) -> usize {
        self.remove_where(|inhibitor| inhibitor.client_id == *client_id)
    }

    fn remove_where(&mut self, mut predicate: impl FnMut(&ShortcutInhibitor) -> bool) -> usize {
        let ids = self
            .inhibitors
            .values()
            .filter(|inhibitor| predicate(inhibitor))
            .map(|inhibitor| inhibitor.id)
            .collect::<Vec<_>>();
        let mut removed = 0;
        for id in ids {
            if self.remove(id) {
                removed += 1;
            }
        }
        removed
    }

    pub(in crate::compositor) fn set_policy_enabled(
        &mut self,
        id: ShortcutInhibitorId,
        enabled: bool,
        is_relevant: impl Fn(&wl_surface::WlSurface) -> bool,
    ) -> Vec<ShortcutInhibitionEvent> {
        let Some(inhibitor) = self.inhibitors.get_mut(&id) else {
            return Vec::new();
        };
        if inhibitor.policy_enabled == enabled {
            return Vec::new();
        }
        inhibitor.policy_enabled = enabled;
        if enabled {
            self.metrics.policy_reactivations = self.metrics.policy_reactivations.saturating_add(1);
        } else {
            self.metrics.policy_deactivations = self.metrics.policy_deactivations.saturating_add(1);
        }
        self.refresh(is_relevant, Some(id))
    }

    pub(in crate::compositor) fn refresh(
        &mut self,
        mut is_relevant: impl FnMut(&wl_surface::WlSurface) -> bool,
        policy_changed: Option<ShortcutInhibitorId>,
    ) -> Vec<ShortcutInhibitionEvent> {
        let stale_ids = self
            .inhibitors
            .values()
            .filter(|inhibitor| !inhibitor.surface.is_alive() || !inhibitor.resource.is_alive())
            .map(|inhibitor| inhibitor.id)
            .collect::<Vec<_>>();
        if !stale_ids.is_empty() {
            self.metrics.stale_cleanup = self
                .metrics
                .stale_cleanup
                .saturating_add(stale_ids.len() as u64);
            for id in stale_ids {
                self.remove(id);
            }
        }

        let mut events = Vec::new();
        let ids = self.inhibitors.keys().copied().collect::<Vec<_>>();
        for id in ids {
            let Some((relevant, effective, was_effective, resource)) =
                self.inhibitors.get_mut(&id).map(|inhibitor| {
                    let relevant = is_relevant(&inhibitor.surface);
                    let effective = relevant && inhibitor.policy_enabled;
                    let was_effective = inhibitor.effective;
                    inhibitor.effective = effective;
                    (
                        relevant,
                        effective,
                        was_effective,
                        inhibitor.resource.clone(),
                    )
                })
            else {
                continue;
            };
            if was_effective == effective {
                continue;
            }
            if effective {
                self.effective_count = self.effective_count.saturating_add(1);
            } else {
                self.effective_count = self.effective_count.saturating_sub(1);
            }
            self.bump_generation();
            if effective {
                self.metrics.effective_activations =
                    self.metrics.effective_activations.saturating_add(1);
                events.push(ShortcutInhibitionEvent {
                    resource,
                    kind: ShortcutInhibitionEventKind::Active,
                });
            } else if !relevant {
                self.metrics.relevance_deactivations =
                    self.metrics.relevance_deactivations.saturating_add(1);
            } else if policy_changed == Some(id) {
                events.push(ShortcutInhibitionEvent {
                    resource,
                    kind: ShortcutInhibitionEventKind::Inactive,
                });
            }
        }
        events
    }

    pub(in crate::compositor) fn snapshot(&self) -> KeyboardShortcutInhibitionSnapshot {
        KeyboardShortcutInhibitionSnapshot::new(
            self.has_effective_inhibitor(),
            self.effective_generation,
        )
    }

    pub(in crate::compositor) fn has_effective_inhibitor(&self) -> bool {
        self.effective_count > 0
    }

    pub(in crate::compositor) const fn metrics(&self) -> KeyboardShortcutInhibitionMetrics {
        self.metrics
    }

    fn allocate_id(&mut self) -> ShortcutInhibitorId {
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            if self.next_id == 0 {
                continue;
            }
            let id = ShortcutInhibitorId(self.next_id);
            if !self.inhibitors.contains_key(&id) {
                return id;
            }
        }
    }

    fn bump_generation(&mut self) {
        self.effective_generation = advance_nonzero_serial(self.effective_generation);
    }
}

impl CompositorState {
    pub(in crate::compositor) fn refresh_keyboard_shortcut_inhibition(&mut self) {
        let focused_surface = self.keyboard_surface.clone();
        let mapped_surface_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .chain(
                self.xdg_surface_lifecycles
                    .iter()
                    .filter_map(|(id, lifecycle)| lifecycle.currently_mapped.then_some(*id)),
            )
            .chain(
                self.layer_surfaces
                    .iter()
                    .filter_map(|(id, role)| role.mapped.then_some(*id)),
            )
            .collect::<HashSet<_>>();
        let events = self.shortcut_inhibition.refresh(
            |surface| {
                focused_surface
                    .as_ref()
                    .is_some_and(|focused| same_surface_resource(focused, surface))
                    && mapped_surface_ids.contains(&compositor_surface_id(surface))
            },
            None,
        );
        self.send_keyboard_shortcut_inhibition_events(events);
    }

    pub(in crate::compositor) fn register_keyboard_shortcut_inhibitor(
        &mut self,
        surface: wl_surface::WlSurface,
        seat: LogicalSeatId,
        client_id: ClientId,
        resource: zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
    ) -> ShortcutInhibitorId {
        let id = self
            .shortcut_inhibition
            .insert(surface, seat, client_id, resource);
        self.refresh_keyboard_shortcut_inhibition();
        id
    }

    pub(in crate::compositor) fn remove_keyboard_shortcut_inhibitor_resource(
        &mut self,
        resource: &zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
    ) {
        self.shortcut_inhibition.remove_resource(resource);
    }

    pub(in crate::compositor) fn remove_keyboard_shortcut_inhibitors_for_surface(
        &mut self,
        surface_id: u32,
    ) {
        self.shortcut_inhibition.remove_surface(surface_id);
    }

    pub(in crate::compositor) fn remove_keyboard_shortcut_inhibitors_for_client(
        &mut self,
        client_id: &ClientId,
    ) {
        self.shortcut_inhibition.remove_client(client_id);
    }

    #[allow(dead_code)]
    pub(in crate::compositor) fn set_shortcut_inhibitor_policy_enabled(
        &mut self,
        id: ShortcutInhibitorId,
        enabled: bool,
    ) {
        let focused_surface = self.keyboard_surface.clone();
        let mapped_surface_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .chain(
                self.xdg_surface_lifecycles
                    .iter()
                    .filter_map(|(id, lifecycle)| lifecycle.currently_mapped.then_some(*id)),
            )
            .chain(
                self.layer_surfaces
                    .iter()
                    .filter_map(|(id, role)| role.mapped.then_some(*id)),
            )
            .collect::<HashSet<_>>();
        let events = self
            .shortcut_inhibition
            .set_policy_enabled(id, enabled, |surface| {
                focused_surface
                    .as_ref()
                    .is_some_and(|focused| same_surface_resource(focused, surface))
                    && mapped_surface_ids.contains(&compositor_surface_id(surface))
            });
        self.send_keyboard_shortcut_inhibition_events(events);
    }

    #[cfg(test)]
    pub(in crate::compositor) fn set_shortcut_inhibitor_policy_enabled_for_surface(
        &mut self,
        surface_id: u32,
        enabled: bool,
    ) -> bool {
        let Some(id) = self
            .shortcut_inhibition
            .inhibitors
            .values()
            .find(|inhibitor| compositor_surface_id(&inhibitor.surface) == surface_id)
            .map(|inhibitor| inhibitor.id)
        else {
            return false;
        };
        self.set_shortcut_inhibitor_policy_enabled(id, enabled);
        true
    }

    pub(in crate::compositor) fn keyboard_shortcut_inhibition_snapshot(
        &self,
    ) -> KeyboardShortcutInhibitionSnapshot {
        self.shortcut_inhibition.snapshot()
    }

    pub(in crate::compositor) fn keyboard_shortcut_inhibition_metrics(
        &self,
    ) -> KeyboardShortcutInhibitionMetrics {
        self.shortcut_inhibition.metrics()
    }

    fn send_keyboard_shortcut_inhibition_events(&self, events: Vec<ShortcutInhibitionEvent>) {
        for event in events {
            let result = match event.kind {
                ShortcutInhibitionEventKind::Active => event
                    .resource
                    .send_event(zwp_keyboard_shortcuts_inhibitor_v1::Event::Active),
                ShortcutInhibitionEventKind::Inactive => event
                    .resource
                    .send_event(zwp_keyboard_shortcuts_inhibitor_v1::Event::Inactive),
            };
            let _ = result;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_seat_is_stable_across_resources() {
        assert_eq!(LogicalSeatId::PRIMARY, LogicalSeatId::PRIMARY);
    }
}
