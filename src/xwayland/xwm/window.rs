use std::collections::HashMap;

use crate::compositor::{DesktopWindowKind, WindowConstraints, WindowMetadata};

use super::{
    AssociatedSurface, X11Geometry, X11PublishedState, X11WindowHandle, X11WindowSnapshot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11WindowLifecycle {
    Observed,
    Auxiliary,
    MapRequested,
    PropertiesPending,
    MapCommanded,
    MappedAwaitingAssociation,
    AssociatedAwaitingBuffer,
    Renderable,
    Withdrawn,
    Destroyed,
}

#[derive(Debug, Clone)]
pub(crate) struct X11WindowRecord {
    pub(crate) lifecycle: X11WindowLifecycle,
    pub(crate) snapshot: Option<X11WindowSnapshot>,
    pub(crate) kind: DesktopWindowKind,
    pub(crate) geometry: X11Geometry,
    pub(crate) map_requested: bool,
    pub(crate) map_authorized: bool,
    pub(crate) mapped_notified: bool,
    pub(crate) association: Option<AssociatedSurface>,
    pub(crate) buffer_ready: bool,
    pub(crate) map_operation_pending: bool,
    pub(crate) properties: X11PropertySnapshot,
    pub(crate) staging_properties: X11PropertySnapshot,
    pub(crate) properties_ready: bool,
    pub(crate) resolved_properties: u16,
    pub(crate) pending_properties: u16,
    pub(crate) dirty_properties: u16,
    pub(crate) refresh_properties: u16,
    pub(crate) refresh_all: bool,
    pub(crate) property_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct X11PropertySnapshot {
    pub(crate) title: Option<String>,
    pub(crate) app_id: Option<String>,
    pub(crate) pid: Option<u32>,
    pub(crate) window_type: Option<X11WindowType>,
    pub(crate) accepts_input: Option<bool>,
    pub(crate) constraints: WindowConstraints,
    pub(crate) state: X11PublishedState,
    pub(crate) transient_for: Option<X11WindowHandle>,
    pub(crate) supports_delete: bool,
    pub(crate) supports_take_focus: bool,
    pub(crate) sync_counter: Option<u64>,
    pub(crate) net_wm_name: Option<String>,
    pub(crate) wm_name: Option<String>,
    pub(crate) window_role: Option<String>,
    pub(crate) startup_id: Option<String>,
    pub(crate) user_time: Option<u32>,
    pub(crate) client_leader: Option<X11WindowHandle>,
    pub(crate) user_time_window: Option<X11WindowHandle>,
    pub(crate) urgency: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11WindowType {
    Normal,
    Dialog,
    Utility,
    Menu,
    PopupMenu,
    DropdownMenu,
    Tooltip,
    Notification,
    Other(u32),
}

#[derive(Debug, Default)]
pub(crate) struct X11WindowRegistry {
    records: HashMap<X11WindowHandle, X11WindowRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuxiliaryReconciliation {
    Unchanged,
    WithdrawDesktop,
    ReadinessRestored,
}

impl X11WindowRegistry {
    #[cfg(test)]
    pub(crate) fn insert_observed(&mut self, handle: X11WindowHandle) -> bool {
        if self.records.contains_key(&handle) {
            return false;
        }
        self.records.insert(
            handle,
            X11WindowRecord {
                lifecycle: X11WindowLifecycle::Observed,
                snapshot: None,
                kind: DesktopWindowKind::Managed,
                geometry: X11Geometry::default(),
                map_requested: false,
                map_authorized: false,
                mapped_notified: false,
                association: None,
                buffer_ready: false,
                map_operation_pending: false,
                properties: X11PropertySnapshot::default(),
                staging_properties: X11PropertySnapshot::default(),
                properties_ready: true,
                resolved_properties: u16::MAX,
                pending_properties: 0,
                dirty_properties: 0,
                refresh_properties: 0,
                refresh_all: false,
                property_epoch: 0,
            },
        );
        true
    }

    pub(crate) fn insert_observed_with_kind(
        &mut self,
        handle: X11WindowHandle,
        kind: DesktopWindowKind,
        geometry: X11Geometry,
    ) -> bool {
        if self.records.contains_key(&handle) {
            return false;
        }
        self.records.insert(
            handle,
            X11WindowRecord {
                lifecycle: X11WindowLifecycle::Observed,
                snapshot: None,
                kind,
                geometry,
                map_requested: false,
                map_authorized: false,
                mapped_notified: false,
                association: None,
                buffer_ready: false,
                map_operation_pending: false,
                properties: X11PropertySnapshot::default(),
                staging_properties: X11PropertySnapshot::default(),
                properties_ready: false,
                resolved_properties: 0,
                pending_properties: 0,
                dirty_properties: 0,
                refresh_properties: 0,
                refresh_all: false,
                property_epoch: 0,
            },
        );
        true
    }

    pub(crate) fn insert_snapshot(&mut self, snapshot: X11WindowSnapshot) -> bool {
        if self.records.contains_key(&snapshot.handle) {
            return false;
        }
        let kind = snapshot.kind;
        let geometry = snapshot.geometry;
        let properties = X11PropertySnapshot {
            title: snapshot.metadata.title.clone(),
            app_id: snapshot.metadata.app_id.clone(),
            pid: snapshot.metadata.pid,
            constraints: snapshot.constraints,
            state: snapshot.state,
            transient_for: snapshot.transient_for,
            supports_delete: snapshot.supports_delete,
            supports_take_focus: snapshot.supports_take_focus,
            accepts_input: snapshot.accepts_input,
            window_role: snapshot.window_role.clone(),
            startup_id: snapshot.startup_id.clone(),
            user_time: snapshot.user_time,
            urgency: snapshot.urgency,
            sync_counter: snapshot.sync_counter,
            window_type: snapshot.window_type,
            ..X11PropertySnapshot::default()
        };
        self.records.insert(
            snapshot.handle,
            X11WindowRecord {
                lifecycle: X11WindowLifecycle::Renderable,
                snapshot: Some(snapshot.clone()),
                kind,
                geometry,
                map_requested: true,
                map_authorized: true,
                mapped_notified: true,
                association: None,
                buffer_ready: true,
                map_operation_pending: false,
                properties: properties.clone(),
                staging_properties: properties,
                properties_ready: true,
                resolved_properties: u16::MAX,
                pending_properties: 0,
                dirty_properties: 0,
                refresh_properties: 0,
                refresh_all: false,
                property_epoch: 0,
            },
        );
        true
    }

    pub(crate) fn get(&self, handle: X11WindowHandle) -> Option<&X11WindowRecord> {
        self.records.get(&handle)
    }

    pub(crate) fn snapshots(&self) -> impl Iterator<Item = (X11WindowHandle, &X11WindowSnapshot)> {
        self.records.iter().filter_map(|(handle, record)| {
            record.snapshot.as_ref().map(|snapshot| (*handle, snapshot))
        })
    }

    pub(crate) fn get_mut(&mut self, handle: X11WindowHandle) -> Option<&mut X11WindowRecord> {
        self.records.get_mut(&handle)
    }

    pub(crate) fn remove(&mut self, handle: X11WindowHandle) -> Option<X11WindowRecord> {
        self.records.remove(&handle)
    }

    pub(crate) fn clear_generation(&mut self, generation: super::XwaylandGeneration) {
        self.records
            .retain(|handle, _| handle.generation() != generation);
    }

    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn contains(&self, handle: X11WindowHandle) -> bool {
        self.records.contains_key(&handle)
    }

    #[cfg(test)]
    pub(crate) fn lifecycle(&self, handle: X11WindowHandle) -> Option<X11WindowLifecycle> {
        self.records.get(&handle).map(|record| record.lifecycle)
    }

    pub(crate) fn mark_map_requested(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<(), &'static str> {
        let record = self.record_mut(handle)?;
        if matches!(record.lifecycle, X11WindowLifecycle::Destroyed) {
            return Err("window was destroyed");
        }
        if record.map_requested && !matches!(record.lifecycle, X11WindowLifecycle::Withdrawn) {
            return Ok(());
        }
        record.map_requested = true;
        record.map_authorized = false;
        record.mapped_notified = false;
        record.snapshot = None;
        record.properties_ready = false;
        self.update_pending_lifecycle(handle)
    }

    pub(crate) fn mark_map_authorized(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<bool, &'static str> {
        let record = self.record_mut(handle)?;
        if record.map_authorized {
            return Ok(false);
        }
        record.map_authorized = true;
        self.update_pending_lifecycle(handle)?;
        Ok(true)
    }

    pub(crate) fn mark_associated(
        &mut self,
        handle: X11WindowHandle,
        association: AssociatedSurface,
    ) -> Result<(), &'static str> {
        let record = self.record_mut(handle)?;
        if record.association.is_some() {
            return Err("window is already associated");
        }
        if record.snapshot.is_some() {
            return Err("window is already ready");
        }
        record.association = Some(association);
        self.update_pending_lifecycle(handle)
    }

    pub(crate) fn clear_association(
        &mut self,
        handle: X11WindowHandle,
        surface_id: u32,
    ) -> Result<bool, &'static str> {
        let record = self.record_mut(handle)?;
        if record
            .association
            .is_none_or(|association| association.surface_id != surface_id)
        {
            return Ok(false);
        }
        record.association = None;
        record.buffer_ready = false;
        let was_ready = record.snapshot.take().is_some();
        self.update_pending_lifecycle(handle)?;
        Ok(was_ready)
    }

    pub(crate) fn mark_buffer_ready(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<(), &'static str> {
        let record = self.record_mut(handle)?;
        if matches!(
            record.lifecycle,
            X11WindowLifecycle::Withdrawn | X11WindowLifecycle::Destroyed
        ) {
            return Err("window is no longer mappable");
        }
        record.buffer_ready = true;
        self.update_pending_lifecycle(handle)
    }

    pub(crate) fn try_ready(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<Option<X11WindowSnapshot>, &'static str> {
        let record = self.record_mut(handle)?;
        if record.snapshot.is_some()
            || !record.map_requested
            || !record.buffer_ready
            || record.association.is_none()
            || !record.mapped_notified
            || matches!(
                record.lifecycle,
                X11WindowLifecycle::Withdrawn | X11WindowLifecycle::Destroyed
            )
        {
            return Ok(None);
        }
        let association = record.association.expect("association checked");
        if is_auxiliary_client_leader(handle, record) {
            record.lifecycle = X11WindowLifecycle::Auxiliary;
            return Ok(None);
        }
        let snapshot = X11WindowSnapshot {
            handle,
            surface_id: association.surface_id,
            kind: record.kind,
            window_type: record.properties.window_type,
            override_redirect: record.kind == DesktopWindowKind::OverrideRedirect,
            geometry: record.geometry,
            metadata: WindowMetadata {
                app_id: record.properties.app_id.clone(),
                title: record.properties.title.clone(),
                pid: record.properties.pid,
            },
            constraints: record.properties.constraints,
            state: record.properties.state,
            transient_for: record.properties.transient_for,
            supports_delete: record.properties.supports_delete,
            supports_take_focus: record.properties.supports_take_focus,
            accepts_input: record.properties.accepts_input,
            window_role: record.properties.window_role.clone(),
            startup_id: record.properties.startup_id.clone(),
            user_time: record.properties.user_time,
            urgency: record.properties.urgency,
            sync_counter: record.properties.sync_counter,
        };
        record.lifecycle = X11WindowLifecycle::Renderable;
        record.snapshot = Some(snapshot.clone());
        Ok(Some(snapshot))
    }

    pub(crate) fn reconcile_auxiliary(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<AuxiliaryReconciliation, &'static str> {
        let record = self.record_mut(handle)?;
        if is_auxiliary_client_leader(handle, record) {
            let transition = if record.snapshot.take().is_some() {
                AuxiliaryReconciliation::WithdrawDesktop
            } else {
                AuxiliaryReconciliation::Unchanged
            };
            record.lifecycle = X11WindowLifecycle::Auxiliary;
            return Ok(transition);
        }
        if record.lifecycle != X11WindowLifecycle::Auxiliary {
            return Ok(AuxiliaryReconciliation::Unchanged);
        }
        record.lifecycle = X11WindowLifecycle::Observed;
        self.update_pending_lifecycle(handle)?;
        Ok(AuxiliaryReconciliation::ReadinessRestored)
    }

    pub(crate) fn mark_map_commanded(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<(), &'static str> {
        let record = self.record_mut(handle)?;
        if !record.map_requested {
            return Err("window was not requested for mapping");
        }
        record.map_operation_pending = true;
        record.map_authorized = true;
        if record.snapshot.is_none() {
            record.lifecycle = X11WindowLifecycle::MapCommanded;
        }
        Ok(())
    }

    pub(crate) fn adopt_mapped(&mut self, handle: X11WindowHandle) -> Result<(), &'static str> {
        let record = self.record_mut(handle)?;
        record.map_requested = true;
        record.map_authorized = true;
        record.mapped_notified = true;
        record.map_operation_pending = false;
        record.lifecycle = if record.association.is_some() {
            X11WindowLifecycle::AssociatedAwaitingBuffer
        } else {
            X11WindowLifecycle::MappedAwaitingAssociation
        };
        Ok(())
    }

    pub(crate) fn map_command_is_new(&self, handle: X11WindowHandle) -> Result<bool, &'static str> {
        let record = self.records.get(&handle).ok_or("unknown X11 window")?;
        Ok(!record.map_operation_pending && !record.mapped_notified && record.snapshot.is_none())
    }

    pub(crate) fn confirm_map_notify(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<bool, &'static str> {
        let record = self.record_mut(handle)?;
        if !record.map_operation_pending {
            return Ok(false);
        }
        record.map_operation_pending = false;
        record.map_authorized = true;
        record.mapped_notified = true;
        record.lifecycle = if record.snapshot.is_some() {
            X11WindowLifecycle::Renderable
        } else if record.association.is_some() {
            X11WindowLifecycle::AssociatedAwaitingBuffer
        } else {
            X11WindowLifecycle::MappedAwaitingAssociation
        };
        Ok(true)
    }

    pub(crate) fn confirm_external_map_notify(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<bool, &'static str> {
        let record = self.record_mut(handle)?;
        record.map_requested = true;
        record.map_operation_pending = false;
        record.map_authorized = true;
        record.mapped_notified = true;
        record.lifecycle = if record.snapshot.is_some() {
            X11WindowLifecycle::Renderable
        } else if record.association.is_some() {
            X11WindowLifecycle::AssociatedAwaitingBuffer
        } else {
            X11WindowLifecycle::MappedAwaitingAssociation
        };
        Ok(true)
    }

    pub(crate) fn mark_unmapped(&mut self, handle: X11WindowHandle) -> Result<bool, &'static str> {
        let record = self.record_mut(handle)?;
        let was_mapped = matches!(record.lifecycle, X11WindowLifecycle::Renderable);
        record.lifecycle = X11WindowLifecycle::Withdrawn;
        record.map_requested = false;
        record.map_authorized = false;
        record.mapped_notified = false;
        record.map_operation_pending = false;
        record.buffer_ready = false;
        record.snapshot = None;
        record.properties_ready = false;
        Ok(was_mapped)
    }

    pub(crate) fn destroy(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<Option<X11WindowRecord>, &'static str> {
        Ok(self.records.remove(&handle).map(|mut record| {
            record.lifecycle = X11WindowLifecycle::Destroyed;
            record
        }))
    }

    fn record_mut(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<&mut X11WindowRecord, &'static str> {
        self.records.get_mut(&handle).ok_or("unknown X11 window")
    }

    fn update_pending_lifecycle(&mut self, handle: X11WindowHandle) -> Result<(), &'static str> {
        let record = self.record_mut(handle)?;
        if record.snapshot.is_some() {
            record.lifecycle = X11WindowLifecycle::Renderable;
            return Ok(());
        }
        record.lifecycle = if record.map_operation_pending {
            X11WindowLifecycle::MapCommanded
        } else if record.map_requested && !record.properties_ready {
            X11WindowLifecycle::PropertiesPending
        } else if record.map_requested && !record.mapped_notified {
            X11WindowLifecycle::MapRequested
        } else if record.map_requested && record.association.is_none() {
            X11WindowLifecycle::MappedAwaitingAssociation
        } else if record.map_requested && !record.buffer_ready {
            X11WindowLifecycle::AssociatedAwaitingBuffer
        } else {
            X11WindowLifecycle::Observed
        };
        Ok(())
    }
}

fn is_auxiliary_client_leader(handle: X11WindowHandle, record: &X11WindowRecord) -> bool {
    record.kind == DesktopWindowKind::Managed
        && record.geometry.width <= 16
        && record.geometry.height <= 16
        && record.properties.client_leader == Some(handle)
        && record.properties.window_type.is_none()
        && record.properties.accepts_input.is_none()
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use crate::compositor::DesktopWindowKind;
    use crate::xwayland::XwaylandGeneration;

    use super::*;
    use crate::xwayland::xwm::AssociatedSurface;

    fn generation(value: u64) -> XwaylandGeneration {
        XwaylandGeneration::new(NonZeroU64::new(value).expect("nonzero"))
    }

    fn handle(generation: XwaylandGeneration, xid: u32) -> X11WindowHandle {
        X11WindowHandle::new(generation, xid)
    }

    fn associated(
        generation: XwaylandGeneration,
        serial: u64,
        surface_id: u32,
    ) -> AssociatedSurface {
        AssociatedSurface {
            generation,
            serial: NonZeroU64::new(serial).expect("nonzero"),
            surface_id,
        }
    }

    fn complete_properties(registry: &mut X11WindowRegistry, window: X11WindowHandle) {
        registry
            .get_mut(window)
            .expect("known window")
            .properties_ready = true;
    }

    #[test]
    fn map_before_association_waits() {
        let generation = generation(1);
        let window = handle(generation, 10);
        let mut registry = X11WindowRegistry::default();

        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry.mark_map_requested(window).expect("known window");

        assert!(registry.try_ready(window).expect("known window").is_none());
        assert_eq!(
            registry.lifecycle(window),
            Some(X11WindowLifecycle::PropertiesPending)
        );
    }

    #[test]
    fn association_before_map_request_waits() {
        let generation = generation(1);
        let window = handle(generation, 11);
        let mut registry = X11WindowRegistry::default();

        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry
            .mark_associated(window, associated(generation, 7, 42))
            .expect("known window");

        assert!(registry.try_ready(window).expect("known window").is_none());
        assert_eq!(
            registry.lifecycle(window),
            Some(X11WindowLifecycle::Observed)
        );
    }

    #[test]
    fn buffer_before_first_map_request_completes_mapping_gate() {
        let generation = generation(1);
        let window = handle(generation, 26);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry
            .mark_associated(window, associated(generation, 9, 45))
            .expect("association");
        registry.mark_buffer_ready(window).expect("retained buffer");

        registry
            .mark_map_requested(window)
            .expect("first map request");
        complete_properties(&mut registry, window);
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");

        let snapshot = registry
            .try_ready(window)
            .expect("known window")
            .expect("retained buffer completes first map");
        assert_eq!(snapshot.surface_id, 45);
    }

    #[test]
    fn first_buffer_completes_mapping_gate() {
        let generation = generation(1);
        let window = handle(generation, 12);
        let mut registry = X11WindowRegistry::default();

        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry.mark_map_requested(window).expect("known window");
        complete_properties(&mut registry, window);
        registry
            .mark_associated(window, associated(generation, 8, 43))
            .expect("known window");
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");
        assert!(registry.try_ready(window).expect("known window").is_none());

        registry.mark_buffer_ready(window).expect("known window");
        let snapshot = registry
            .try_ready(window)
            .expect("known window")
            .expect("mapping gate");
        assert_eq!(snapshot.surface_id, 43);
        assert_eq!(snapshot.handle, window);
        assert_eq!(
            registry.lifecycle(window),
            Some(X11WindowLifecycle::Renderable)
        );
        assert!(registry.try_ready(window).expect("known window").is_none());
    }

    #[test]
    fn map_command_then_notify_preserves_ready_snapshot() {
        let generation = generation(1);
        let window = handle(generation, 16);
        let mut registry = X11WindowRegistry::default();

        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry.mark_map_requested(window).expect("known window");
        complete_properties(&mut registry, window);
        registry
            .mark_associated(window, associated(generation, 1, 46))
            .expect("association");
        registry.mark_buffer_ready(window).expect("buffer");
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");
        let expected = registry
            .try_ready(window)
            .expect("known window")
            .expect("mapping gate");

        registry
            .mark_map_commanded(window)
            .expect("ready window can be mapped");
        assert_eq!(
            registry.lifecycle(window),
            Some(X11WindowLifecycle::Renderable)
        );
        assert!(registry.confirm_map_notify(window).expect("map notify"));
        assert_eq!(
            registry.lifecycle(window),
            Some(X11WindowLifecycle::Renderable)
        );
        assert_eq!(
            registry
                .get(window)
                .and_then(|record| record.snapshot.as_ref()),
            Some(&expected)
        );
        assert!(
            registry
                .get(window)
                .is_some_and(|record| record.buffer_ready)
        );
        assert!(
            !registry
                .confirm_map_notify(window)
                .expect("duplicate map notify")
        );
        assert!(registry.try_ready(window).expect("known window").is_none());
    }

    #[test]
    fn managed_window_receives_exactly_one_map_command() {
        let generation = generation(1);
        let window = handle(generation, 20);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry.mark_map_requested(window).expect("map request");
        assert!(registry.map_command_is_new(window).expect("known window"));
        registry.mark_map_commanded(window).expect("map command");
        assert!(!registry.map_command_is_new(window).expect("known window"));
    }

    #[test]
    fn window_ready_does_not_remap() {
        let generation = generation(1);
        let window = handle(generation, 21);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry.mark_map_requested(window).expect("map request");
        complete_properties(&mut registry, window);
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");
        assert!(!registry.map_command_is_new(window).expect("known window"));
    }

    #[test]
    fn duplicate_map_request_is_idempotent() {
        let generation = generation(1);
        let window = handle(generation, 22);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry
            .mark_map_requested(window)
            .expect("first map request");
        registry
            .mark_map_requested(window)
            .expect("duplicate map request");
        assert!(registry.map_command_is_new(window).expect("known window"));
    }

    #[test]
    fn map_notify_clears_pending_map_state() {
        let generation = generation(1);
        let window = handle(generation, 23);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry.mark_map_requested(window).expect("map request");
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");
        assert!(
            !registry
                .get(window)
                .expect("known window")
                .map_operation_pending
        );
    }

    #[test]
    fn unmap_remap_creates_one_new_map_operation() {
        let generation = generation(1);
        let window = handle(generation, 24);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry
            .mark_map_requested(window)
            .expect("first map request");
        registry
            .mark_map_commanded(window)
            .expect("first map command");
        registry
            .confirm_map_notify(window)
            .expect("first map notify");
        registry.mark_unmapped(window).expect("unmap");
        registry.mark_map_requested(window).expect("remap request");
        assert!(registry.map_command_is_new(window).expect("known window"));
        registry.mark_map_commanded(window).expect("remap command");
        assert!(!registry.map_command_is_new(window).expect("known window"));
    }

    #[test]
    fn override_redirect_never_receives_managed_map_command() {
        let generation = generation(1);
        let window = handle(generation, 25);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::OverrideRedirect,
            X11Geometry::default(),
        );
        registry
            .confirm_external_map_notify(window)
            .expect("external map notify");
        assert!(
            !registry
                .get(window)
                .expect("known window")
                .map_operation_pending
        );
    }

    #[test]
    fn existing_override_redirect_window_is_adopted_correctly() {
        let generation = generation(21);
        let window = handle(generation, 210);
        let geometry = X11Geometry {
            x: 12,
            y: 14,
            width: 320,
            height: 180,
        };
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(window, DesktopWindowKind::OverrideRedirect, geometry);
        registry.adopt_mapped(window).expect("adopt mapped popup");
        let record = registry.get(window).expect("adopted popup");
        assert_eq!(record.kind, DesktopWindowKind::OverrideRedirect);
        assert_eq!(record.geometry, geometry);
        assert!(record.map_requested);
        assert!(record.map_authorized);
        assert!(record.mapped_notified);
        assert!(!record.map_operation_pending);
    }

    #[test]
    fn existing_window_uses_real_attributes_and_geometry() {
        let generation = generation(22);
        let window = handle(generation, 220);
        let geometry = X11Geometry {
            x: -7,
            y: 9,
            width: 1024,
            height: 768,
        };
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(window, DesktopWindowKind::Managed, geometry);
        let record = registry.get(window).expect("adopted window");
        assert_eq!(record.geometry, geometry);
        assert_eq!(record.lifecycle, X11WindowLifecycle::Observed);
    }

    #[test]
    fn managed_unmap_requires_a_fresh_buffer_before_remap() {
        let generation = generation(1);
        let window = handle(generation, 19);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry.mark_map_requested(window).expect("known window");
        complete_properties(&mut registry, window);
        registry
            .mark_associated(window, associated(generation, 1, 44))
            .expect("association");
        registry.mark_buffer_ready(window).expect("buffer");
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");
        assert!(registry.try_ready(window).unwrap().is_some());
        registry.mark_unmapped(window).expect("unmap");
        registry.mark_map_requested(window).expect("remap");
        complete_properties(&mut registry, window);
        registry.mark_map_commanded(window).expect("remap command");
        registry.confirm_map_notify(window).expect("remap notify");
        assert!(registry.try_ready(window).unwrap().is_none());
        registry.mark_buffer_ready(window).expect("fresh buffer");
        assert!(registry.try_ready(window).unwrap().is_some());
    }

    #[test]
    fn unmap_before_ready_never_creates_desktop_window() {
        let generation = generation(1);
        let window = handle(generation, 13);
        let mut registry = X11WindowRegistry::default();

        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        assert!(!registry.mark_unmapped(window).expect("known window"));
        assert_eq!(
            registry.lifecycle(window),
            Some(X11WindowLifecycle::Withdrawn)
        );
        assert!(registry.try_ready(window).expect("known window").is_none());
    }

    #[test]
    fn destroy_after_ready_emits_one_destroy_event() {
        let generation = generation(1);
        let window = handle(generation, 14);
        let mut registry = X11WindowRegistry::default();

        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry::default(),
        );
        registry.mark_map_requested(window).expect("known window");
        complete_properties(&mut registry, window);
        registry
            .mark_associated(window, associated(generation, 9, 44))
            .expect("known window");
        registry.mark_buffer_ready(window).expect("known window");
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");
        assert!(registry.try_ready(window).expect("known window").is_some());

        assert!(registry.destroy(window).expect("known window").is_some());
        assert!(registry.destroy(window).expect("unknown window").is_none());
    }

    #[test]
    fn override_redirect_maps_without_normal_focus() {
        let generation = generation(1);
        let window = handle(generation, 15);
        let mut registry = X11WindowRegistry::default();

        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::OverrideRedirect,
            X11Geometry::default(),
        );
        registry
            .confirm_external_map_notify(window)
            .expect("map notify");
        complete_properties(&mut registry, window);
        registry
            .mark_associated(window, associated(generation, 10, 45))
            .expect("known window");
        registry.mark_buffer_ready(window).expect("known window");
        let snapshot = registry
            .try_ready(window)
            .expect("known window")
            .expect("mapping gate");

        assert_eq!(snapshot.kind, DesktopWindowKind::OverrideRedirect);
        assert!(!snapshot.state.activated);
    }

    #[test]
    fn client_leader_support_window_is_not_desktop_ready() {
        let generation = generation(1);
        let window = handle(generation, 27);
        let user_time_window = handle(generation, 28);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry {
                x: 10,
                y: 10,
                width: 10,
                height: 10,
            },
        );
        registry.mark_map_requested(window).expect("map request");
        complete_properties(&mut registry, window);
        {
            let properties = &mut registry.get_mut(window).expect("window").properties;
            properties.client_leader = Some(window);
            properties.user_time_window = Some(user_time_window);
        }
        registry
            .mark_associated(window, associated(generation, 11, 46))
            .expect("association");
        registry.mark_buffer_ready(window).expect("buffer");
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");

        assert_eq!(registry.try_ready(window).expect("known window"), None);
        assert_eq!(
            registry.get(window).expect("support window").lifecycle,
            X11WindowLifecycle::Auxiliary
        );
    }

    #[test]
    fn tiny_typed_input_window_remains_desktop_ready() {
        let generation = generation(1);
        let window = handle(generation, 29);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry {
                width: 10,
                height: 10,
                ..X11Geometry::default()
            },
        );
        registry.mark_map_requested(window).expect("map request");
        complete_properties(&mut registry, window);
        {
            let properties = &mut registry.get_mut(window).expect("window").properties;
            properties.client_leader = Some(window);
            properties.user_time_window = Some(handle(generation, 30));
            properties.window_type = Some(X11WindowType::Normal);
            properties.accepts_input = Some(true);
        }
        registry
            .mark_associated(window, associated(generation, 12, 47))
            .expect("association");
        registry.mark_buffer_ready(window).expect("buffer");
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");

        assert!(registry.try_ready(window).expect("known window").is_some());
    }

    #[test]
    fn late_auxiliary_identity_withdraws_ready_snapshot() {
        let generation = generation(1);
        let window = handle(generation, 31);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry {
                x: 10,
                y: 10,
                width: 10,
                height: 10,
            },
        );
        registry.mark_map_requested(window).expect("map request");
        complete_properties(&mut registry, window);
        registry
            .mark_associated(window, associated(generation, 13, 48))
            .expect("association");
        registry.mark_buffer_ready(window).expect("buffer");
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");
        assert!(registry.try_ready(window).expect("known window").is_some());

        registry
            .get_mut(window)
            .expect("window")
            .properties
            .client_leader = Some(window);

        assert_eq!(
            registry.reconcile_auxiliary(window).expect("known window"),
            AuxiliaryReconciliation::WithdrawDesktop,
        );
        let record = registry.get(window).expect("window");
        assert!(record.snapshot.is_none());
        assert_eq!(record.lifecycle, X11WindowLifecycle::Auxiliary);
    }

    #[test]
    fn self_client_leader_without_user_time_is_auxiliary() {
        let generation = generation(1);
        let window = handle(generation, 32);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry {
                width: 10,
                height: 10,
                ..X11Geometry::default()
            },
        );
        registry.mark_map_requested(window).expect("map request");
        complete_properties(&mut registry, window);
        registry
            .get_mut(window)
            .expect("window")
            .properties
            .client_leader = Some(window);
        registry
            .mark_associated(window, associated(generation, 14, 49))
            .expect("association");
        registry.mark_buffer_ready(window).expect("buffer");
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");

        assert!(registry.try_ready(window).expect("known window").is_none());
        assert_eq!(
            registry.get(window).expect("window").lifecycle,
            X11WindowLifecycle::Auxiliary,
        );
    }

    #[test]
    fn late_auxiliary_can_be_readmitted_as_typed_input_window() {
        let generation = generation(1);
        let window = handle(generation, 33);
        let mut registry = X11WindowRegistry::default();
        registry.insert_observed_with_kind(
            window,
            DesktopWindowKind::Managed,
            X11Geometry {
                width: 10,
                height: 10,
                ..X11Geometry::default()
            },
        );
        registry.mark_map_requested(window).expect("map request");
        complete_properties(&mut registry, window);
        registry
            .mark_associated(window, associated(generation, 15, 50))
            .expect("association");
        registry.mark_buffer_ready(window).expect("buffer");
        registry.mark_map_commanded(window).expect("map command");
        registry.confirm_map_notify(window).expect("map notify");
        assert!(registry.try_ready(window).expect("known window").is_some());
        registry
            .get_mut(window)
            .expect("window")
            .properties
            .client_leader = Some(window);
        assert_eq!(
            registry.reconcile_auxiliary(window).expect("known window"),
            AuxiliaryReconciliation::WithdrawDesktop,
        );

        {
            let properties = &mut registry.get_mut(window).expect("window").properties;
            properties.window_type = Some(X11WindowType::Normal);
            properties.accepts_input = Some(true);
        }
        assert_eq!(
            registry.reconcile_auxiliary(window).expect("known window"),
            AuxiliaryReconciliation::ReadinessRestored,
        );
        assert!(registry.try_ready(window).expect("known window").is_some());
    }
}
