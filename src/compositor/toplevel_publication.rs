use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::astrea_toplevel_management::server::{astrea_toplevel_manager_v1, astrea_toplevel_v1};
use crate::control_snapshots::truncate_utf8;

use super::{CompositorState, DesktopWindowKind, WindowBackend, WindowId, X11DesktopRole};
use wayland_server::{
    Client, DisplayHandle, Resource, WEnum,
    backend::{ClientId, ObjectId},
};

pub(crate) const MAX_ASTREA_TOPLEVEL_TITLE_BYTES: usize = 1024;
pub(crate) const MAX_ASTREA_TOPLEVEL_APP_ID_BYTES: usize = 256;
pub(crate) const MAX_ASTREA_TOPLEVEL_MANAGERS: usize = 32;
pub(crate) const MAX_ASTREA_TOPLEVEL_MANAGERS_PER_CLIENT: usize = 4;
pub(crate) const MAX_ASTREA_TOPLEVELS_PER_MANAGER: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AstreaToplevelKind {
    XdgToplevel,
    X11Toplevel,
    X11Dialog,
}

impl AstreaToplevelKind {
    const fn wire(self) -> astrea_toplevel_v1::Kind {
        match self {
            Self::XdgToplevel => astrea_toplevel_v1::Kind::XdgToplevel,
            Self::X11Toplevel => astrea_toplevel_v1::Kind::X11Toplevel,
            Self::X11Dialog => astrea_toplevel_v1::Kind::X11Dialog,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AstreaToplevelStates(u32);

impl AstreaToplevelStates {
    pub(crate) const ACTIVE: Self = Self(1);
    pub(crate) const MINIMIZED: Self = Self(2);
    pub(crate) const MAXIMIZED: Self = Self(4);
    pub(crate) const FULLSCREEN: Self = Self(8);

    pub(crate) const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    const fn wire(self) -> astrea_toplevel_v1::State {
        astrea_toplevel_v1::State::from_bits_truncate(self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AstreaToplevelSnapshot {
    pub(crate) id: WindowId,
    pub(crate) app_id: String,
    pub(crate) title: String,
    pub(crate) pid: u32,
    pub(crate) kind: AstreaToplevelKind,
    pub(crate) states: AstreaToplevelStates,
    pub(crate) focus_serial: u64,
}

impl AstreaToplevelSnapshot {
    pub(crate) fn bounded(
        id: WindowId,
        app_id: Option<&str>,
        title: Option<&str>,
        pid: Option<u32>,
        kind: AstreaToplevelKind,
        states: AstreaToplevelStates,
        focus_serial: u64,
    ) -> Self {
        Self {
            id,
            app_id: app_id
                .map(|value| truncate_utf8(value, MAX_ASTREA_TOPLEVEL_APP_ID_BYTES))
                .unwrap_or_default(),
            title: truncate_utf8(title.unwrap_or_default(), MAX_ASTREA_TOPLEVEL_TITLE_BYTES),
            pid: pid.unwrap_or_default(),
            kind,
            states,
            focus_serial,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AstreaToplevelCollection {
    pub(crate) snapshots: BTreeMap<WindowId, AstreaToplevelSnapshot>,
    pub(crate) total: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AstreaToplevelPublicationSummary {
    pub(crate) revision: u64,
    pub(crate) added: usize,
    pub(crate) updated: usize,
    pub(crate) closed: usize,
    pub(crate) manager_count: usize,
    pub(crate) truncated_manager_count: usize,
    pub(crate) changed: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AstreaToplevelMetrics {
    pub(crate) managers_accepted: u64,
    pub(crate) unauthorized_manager_binds: u64,
    pub(crate) manager_limit_rejections: u64,
    pub(crate) handles_created: u64,
    pub(crate) handles_closed: u64,
    pub(crate) batches_published: u64,
    pub(crate) windows_updated: u64,
    pub(crate) noop_reconciliations: u64,
    pub(crate) resource_creation_failures: u64,
    pub(crate) dead_resources_pruned: u64,
    pub(crate) truncated_manager_snapshots: u64,
}

#[derive(Debug, Clone)]
pub(in crate::compositor) struct AstreaToplevelResourceData {
    pub(in crate::compositor) manager_id: ObjectId,
    pub(in crate::compositor) window_id: WindowId,
}

#[derive(Debug, Clone)]
struct AstreaToplevelHandleBinding {
    resource: astrea_toplevel_v1::AstreaToplevelV1,
    snapshot: AstreaToplevelSnapshot,
}

#[derive(Debug, Clone)]
struct AstreaToplevelManagerBinding {
    resource: astrea_toplevel_manager_v1::AstreaToplevelManagerV1,
    client: Client,
    client_id: ClientId,
    handles: BTreeMap<WindowId, AstreaToplevelHandleBinding>,
    suppressed: BTreeSet<WindowId>,
    last_total: u32,
    last_truncated: bool,
}

#[derive(Debug, Default)]
pub(in crate::compositor) struct AstreaToplevelPublisher {
    pub(in crate::compositor) revision: u64,
    pub(in crate::compositor) canonical: BTreeMap<WindowId, AstreaToplevelSnapshot>,
    pub(in crate::compositor) canonical_total: u32,
    managers: HashMap<ObjectId, AstreaToplevelManagerBinding>,
    pub(in crate::compositor) metrics: AstreaToplevelMetrics,
}

impl AstreaToplevelPublisher {
    pub(in crate::compositor) fn manager_count(&self) -> usize {
        self.managers.len()
    }

    pub(in crate::compositor) fn manager_count_for_client(&self, client_id: &ClientId) -> usize {
        self.managers
            .values()
            .filter(|binding| binding.client_id == *client_id)
            .count()
    }

    pub(in crate::compositor) fn bind_manager(
        &mut self,
        display: &DisplayHandle,
        manager: astrea_toplevel_manager_v1::AstreaToplevelManagerV1,
        client: Client,
        collection: &AstreaToplevelCollection,
    ) -> Result<(), ()> {
        let manager_id = manager.id();
        let client_id = client.id();
        let mut binding = AstreaToplevelManagerBinding {
            resource: manager.clone(),
            client,
            client_id,
            handles: BTreeMap::new(),
            suppressed: BTreeSet::new(),
            last_total: collection.total,
            last_truncated: collection.total as usize > MAX_ASTREA_TOPLEVELS_PER_MANAGER,
        };
        let truncated = binding.last_truncated;
        let revision = self.revision;
        for snapshot in collection.snapshots.values() {
            if self
                .create_handle(display, &mut binding, snapshot.clone(), revision)
                .is_err()
            {
                self.metrics.resource_creation_failures =
                    self.metrics.resource_creation_failures.saturating_add(1);
                return Err(());
            }
        }
        if self.managers.insert(manager_id, binding).is_some() {
            return Err(());
        }
        self.metrics.managers_accepted = self.metrics.managers_accepted.saturating_add(1);
        self.metrics.handles_created = self
            .metrics
            .handles_created
            .saturating_add(collection.snapshots.len() as u64);
        if truncated {
            self.metrics.truncated_manager_snapshots =
                self.metrics.truncated_manager_snapshots.saturating_add(1);
        }
        Ok(())
    }

    pub(in crate::compositor) fn send_initial_done(
        &self,
        manager_id: &ObjectId,
        collection: &AstreaToplevelCollection,
    ) {
        let Some(binding) = self.managers.get(manager_id) else {
            return;
        };
        let _ = send_manager_done(
            &binding.resource,
            self.revision,
            collection.total,
            binding.last_truncated,
        );
    }

    fn create_handle(
        &mut self,
        display: &DisplayHandle,
        binding: &mut AstreaToplevelManagerBinding,
        snapshot: AstreaToplevelSnapshot,
        revision: u64,
    ) -> Result<(), ()> {
        let resource = binding
            .client
            .create_resource::<
                astrea_toplevel_v1::AstreaToplevelV1,
                AstreaToplevelResourceData,
                CompositorState,
            >(
                display,
                1,
                AstreaToplevelResourceData {
                    manager_id: binding.resource.id(),
                    window_id: snapshot.id,
                },
            )
            .map_err(|_| ())?;
        binding
            .resource
            .send_event(astrea_toplevel_manager_v1::Event::Toplevel {
                id: resource.clone(),
            })
            .map_err(|_| ())?;
        send_initial_handle(&resource, &snapshot, revision).map_err(|_| ())?;
        binding.handles.insert(
            snapshot.id,
            AstreaToplevelHandleBinding { resource, snapshot },
        );
        Ok(())
    }

    pub(in crate::compositor) fn reconcile(
        &mut self,
        display: &DisplayHandle,
        collection: AstreaToplevelCollection,
    ) -> AstreaToplevelPublicationSummary {
        self.prune_dead_resources();
        let canonical_changed =
            self.canonical != collection.snapshots || self.canonical_total != collection.total;
        let new_truncated = collection.total as usize > MAX_ASTREA_TOPLEVELS_PER_MANAGER;
        if !canonical_changed {
            self.metrics.noop_reconciliations = self.metrics.noop_reconciliations.saturating_add(1);
            return AstreaToplevelPublicationSummary {
                revision: self.revision,
                manager_count: self.managers.len(),
                ..AstreaToplevelPublicationSummary::default()
            };
        }

        let old_canonical = std::mem::take(&mut self.canonical);
        self.revision = self.revision.wrapping_add(1);
        let revision = self.revision;
        let mut summary = AstreaToplevelPublicationSummary {
            revision,
            manager_count: self.managers.len(),
            changed: true,
            ..AstreaToplevelPublicationSummary::default()
        };

        let manager_ids = self.managers.keys().cloned().collect::<Vec<_>>();
        for manager_id in manager_ids {
            let Some(mut binding) = self.managers.remove(&manager_id) else {
                continue;
            };
            let mut manager_added = 0usize;
            let mut manager_updated = 0usize;
            let mut manager_closed = 0usize;
            let old_ids = binding.handles.keys().copied().collect::<Vec<_>>();
            for window_id in old_ids {
                if !collection.snapshots.contains_key(&window_id) {
                    if let Some(handle) = binding.handles.remove(&window_id) {
                        let _ = handle
                            .resource
                            .send_event(astrea_toplevel_v1::Event::Closed);
                        summary.closed = summary.closed.saturating_add(1);
                        manager_closed = manager_closed.saturating_add(1);
                        self.metrics.handles_closed = self.metrics.handles_closed.saturating_add(1);
                    }
                    binding.suppressed.remove(&window_id);
                }
            }
            binding
                .suppressed
                .retain(|window_id| collection.snapshots.contains_key(window_id));

            for (window_id, snapshot) in &collection.snapshots {
                if binding.suppressed.contains(window_id) {
                    continue;
                }
                if let Some(handle) = binding.handles.get_mut(window_id) {
                    if old_canonical.get(window_id) != Some(snapshot) {
                        send_changed_handle(&handle.resource, &handle.snapshot, snapshot, revision);
                        handle.snapshot = snapshot.clone();
                        summary.updated = summary.updated.saturating_add(1);
                        manager_updated = manager_updated.saturating_add(1);
                        self.metrics.windows_updated =
                            self.metrics.windows_updated.saturating_add(1);
                    }
                } else if self
                    .create_handle(display, &mut binding, snapshot.clone(), revision)
                    .is_ok()
                {
                    summary.added = summary.added.saturating_add(1);
                    manager_added = manager_added.saturating_add(1);
                    self.metrics.handles_created = self.metrics.handles_created.saturating_add(1);
                } else {
                    self.metrics.resource_creation_failures =
                        self.metrics.resource_creation_failures.saturating_add(1);
                }
            }

            let prefix_changed = old_canonical.keys().ne(collection.snapshots.keys());
            let total_changed = binding.last_total != collection.total;
            let truncated_changed = binding.last_truncated != new_truncated;
            let manager_changed = total_changed || truncated_changed || prefix_changed;
            if manager_changed || manager_added != 0 || manager_updated != 0 || manager_closed != 0
            {
                let _ =
                    send_manager_done(&binding.resource, revision, collection.total, new_truncated);
            }
            binding.last_total = collection.total;
            binding.last_truncated = new_truncated;
            if new_truncated {
                summary.truncated_manager_count = summary.truncated_manager_count.saturating_add(1);
            }
            self.managers.insert(manager_id, binding);
        }

        self.canonical = collection.snapshots;
        self.canonical_total = collection.total;
        self.metrics.batches_published = self.metrics.batches_published.saturating_add(1);
        summary
    }

    pub(in crate::compositor) fn remove_manager(&mut self, manager_id: &ObjectId) {
        if self.managers.remove(manager_id).is_some() {
            self.metrics.dead_resources_pruned =
                self.metrics.dead_resources_pruned.saturating_add(1);
        }
    }

    pub(in crate::compositor) fn remove_handle(
        &mut self,
        manager_id: &ObjectId,
        window_id: WindowId,
        resource_id: &ObjectId,
    ) {
        let Some(binding) = self.managers.get_mut(manager_id) else {
            return;
        };
        if binding
            .handles
            .get(&window_id)
            .is_some_and(|handle| handle.resource.id() == *resource_id)
        {
            binding.handles.remove(&window_id);
            binding.suppressed.insert(window_id);
        }
    }

    pub(in crate::compositor) fn remove_client(&mut self, client_id: &ClientId) {
        self.managers
            .retain(|_, binding| binding.client_id != *client_id);
    }

    fn prune_dead_resources(&mut self) {
        let before = self.managers.len();
        self.managers
            .retain(|_, binding| binding.resource.is_alive());
        self.metrics.dead_resources_pruned = self
            .metrics
            .dead_resources_pruned
            .saturating_add((before - self.managers.len()) as u64);
        for binding in self.managers.values_mut() {
            let before = binding.handles.len();
            binding
                .handles
                .retain(|_, handle| handle.resource.is_alive());
            self.metrics.dead_resources_pruned = self
                .metrics
                .dead_resources_pruned
                .saturating_add((before - binding.handles.len()) as u64);
        }
    }
}

fn send_initial_handle(
    resource: &astrea_toplevel_v1::AstreaToplevelV1,
    snapshot: &AstreaToplevelSnapshot,
    revision: u64,
) -> Result<(), wayland_server::backend::InvalidId> {
    resource.send_event(astrea_toplevel_v1::Event::Identifier {
        identifier: snapshot.id.get().to_string(),
    })?;
    resource.send_event(astrea_toplevel_v1::Event::AppId {
        app_id: snapshot.app_id.clone(),
    })?;
    resource.send_event(astrea_toplevel_v1::Event::Title {
        title: snapshot.title.clone(),
    })?;
    resource.send_event(astrea_toplevel_v1::Event::Pid { pid: snapshot.pid })?;
    resource.send_event(astrea_toplevel_v1::Event::Kind {
        kind: WEnum::Value(snapshot.kind.wire()),
    })?;
    resource.send_event(astrea_toplevel_v1::Event::State {
        state: WEnum::Value(snapshot.states.wire()),
    })?;
    let (serial_hi, serial_lo) = split_u64(snapshot.focus_serial);
    resource.send_event(astrea_toplevel_v1::Event::FocusSerial {
        serial_hi,
        serial_lo,
    })?;
    let (revision_hi, revision_lo) = split_u64(revision);
    resource.send_event(astrea_toplevel_v1::Event::Done {
        revision_hi,
        revision_lo,
    })?;
    Ok(())
}

fn send_changed_handle(
    resource: &astrea_toplevel_v1::AstreaToplevelV1,
    old: &AstreaToplevelSnapshot,
    new: &AstreaToplevelSnapshot,
    revision: u64,
) {
    if old.app_id != new.app_id {
        let _ = resource.send_event(astrea_toplevel_v1::Event::AppId {
            app_id: new.app_id.clone(),
        });
    }
    if old.title != new.title {
        let _ = resource.send_event(astrea_toplevel_v1::Event::Title {
            title: new.title.clone(),
        });
    }
    if old.pid != new.pid {
        let _ = resource.send_event(astrea_toplevel_v1::Event::Pid { pid: new.pid });
    }
    if old.kind != new.kind {
        let _ = resource.send_event(astrea_toplevel_v1::Event::Kind {
            kind: WEnum::Value(new.kind.wire()),
        });
    }
    if old.states != new.states {
        let _ = resource.send_event(astrea_toplevel_v1::Event::State {
            state: WEnum::Value(new.states.wire()),
        });
    }
    if old.focus_serial != new.focus_serial {
        let (serial_hi, serial_lo) = split_u64(new.focus_serial);
        let _ = resource.send_event(astrea_toplevel_v1::Event::FocusSerial {
            serial_hi,
            serial_lo,
        });
    }
    let (revision_hi, revision_lo) = split_u64(revision);
    let _ = resource.send_event(astrea_toplevel_v1::Event::Done {
        revision_hi,
        revision_lo,
    });
}

fn send_manager_done(
    resource: &astrea_toplevel_manager_v1::AstreaToplevelManagerV1,
    revision: u64,
    total: u32,
    truncated: bool,
) -> Result<(), wayland_server::backend::InvalidId> {
    let (revision_hi, revision_lo) = split_u64(revision);
    let flags = truncated.then_some(astrea_toplevel_manager_v1::DoneFlags::Truncated);
    resource.send_event(astrea_toplevel_manager_v1::Event::Done {
        revision_hi,
        revision_lo,
        total,
        flags: WEnum::Value(flags.unwrap_or_else(astrea_toplevel_manager_v1::DoneFlags::empty)),
    })
}

pub(crate) const fn split_u64(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

#[allow(dead_code)]
pub(crate) const fn join_u64(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

impl CompositorState {
    pub(in crate::compositor) fn collect_astrea_toplevels(&self) -> AstreaToplevelCollection {
        let mut snapshots = BTreeMap::new();
        let mut total = 0u32;
        for window_id in self.desktop_windows.keys().copied() {
            let Some(snapshot) = self.astrea_toplevel_snapshot(window_id) else {
                continue;
            };
            total = total.saturating_add(1);
            snapshots.insert(window_id, snapshot);
            if snapshots.len() > MAX_ASTREA_TOPLEVELS_PER_MANAGER {
                let largest = snapshots.keys().next_back().copied();
                if let Some(largest) = largest {
                    snapshots.remove(&largest);
                }
            }
        }
        AstreaToplevelCollection { snapshots, total }
    }

    pub(in crate::compositor) fn astrea_toplevel_snapshot(
        &self,
        window_id: WindowId,
    ) -> Option<AstreaToplevelSnapshot> {
        let window = self.desktop_windows.get(&window_id)?;
        let (kind, eligible) = match window.backend {
            WindowBackend::Xdg(handle) => {
                let lifecycle = self.xdg_surface_lifecycle(handle.root_surface_id())?;
                (
                    AstreaToplevelKind::XdgToplevel,
                    self.toplevel_surfaces
                        .contains_key(&handle.root_surface_id())
                        && lifecycle.currently_mapped,
                )
            }
            WindowBackend::X11(_) => (
                match window.x11_role {
                    Some(X11DesktopRole::Toplevel) => AstreaToplevelKind::X11Toplevel,
                    Some(X11DesktopRole::Dialog) => AstreaToplevelKind::X11Dialog,
                    _ => return None,
                },
                window.kind == DesktopWindowKind::Managed
                    && window
                        .x11_surface_id
                        .and_then(|surface_id| self.surface_resource_by_id(surface_id))
                        .is_some(),
            ),
        };
        eligible.then(|| {
            let mut states = AstreaToplevelStates::default();
            if self.focused_window_id == Some(window_id) {
                states = states.union(AstreaToplevelStates::ACTIVE);
            }
            if window.state.is_minimized() {
                states = states.union(AstreaToplevelStates::MINIMIZED);
            }
            match window.state.mode() {
                super::window_state::ToplevelMode::Floating => {}
                super::window_state::ToplevelMode::Maximized => {
                    states = states.union(AstreaToplevelStates::MAXIMIZED);
                }
                super::window_state::ToplevelMode::Fullscreen => {
                    states = states.union(AstreaToplevelStates::FULLSCREEN);
                }
            }
            AstreaToplevelSnapshot::bounded(
                window.id,
                window.metadata.app_id.as_deref(),
                window.metadata.title.as_deref(),
                window.metadata.pid,
                kind,
                states,
                window.last_focus_serial,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;

    fn id(value: u64) -> WindowId {
        WindowId::new(NonZeroU64::new(value).expect("nonzero test id"))
    }

    #[test]
    fn revision_split_join_round_trips_boundaries() {
        for value in [0, 1, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX] {
            let (high, low) = split_u64(value);
            assert_eq!(join_u64(high, low), value);
        }
    }

    #[test]
    fn snapshot_strings_are_bounded_at_utf8_boundaries() {
        let snapshot = AstreaToplevelSnapshot::bounded(
            id(1),
            Some(&"é".repeat(MAX_ASTREA_TOPLEVEL_APP_ID_BYTES)),
            Some(&"😀".repeat(MAX_ASTREA_TOPLEVEL_TITLE_BYTES)),
            None,
            AstreaToplevelKind::XdgToplevel,
            AstreaToplevelStates::default(),
            0,
        );
        assert!(snapshot.app_id.len() <= MAX_ASTREA_TOPLEVEL_APP_ID_BYTES);
        assert!(snapshot.title.len() <= MAX_ASTREA_TOPLEVEL_TITLE_BYTES);
        assert!(snapshot.app_id.is_char_boundary(snapshot.app_id.len()));
        assert!(snapshot.title.is_char_boundary(snapshot.title.len()));
    }

    #[test]
    fn collection_keeps_the_lowest_bounded_prefix() {
        let mut collection = AstreaToplevelCollection::default();
        for value in (1..=MAX_ASTREA_TOPLEVELS_PER_MANAGER + 2).rev() {
            collection.total = collection.total.saturating_add(1);
            collection.snapshots.insert(
                id(value as u64),
                AstreaToplevelSnapshot::bounded(
                    id(value as u64),
                    None,
                    None,
                    None,
                    AstreaToplevelKind::XdgToplevel,
                    AstreaToplevelStates::default(),
                    0,
                ),
            );
            if collection.snapshots.len() > MAX_ASTREA_TOPLEVELS_PER_MANAGER {
                let largest = collection.snapshots.keys().next_back().copied().unwrap();
                collection.snapshots.remove(&largest);
            }
        }
        assert_eq!(
            collection.total,
            (MAX_ASTREA_TOPLEVELS_PER_MANAGER + 2) as u32
        );
        assert_eq!(collection.snapshots.len(), MAX_ASTREA_TOPLEVELS_PER_MANAGER);
        assert_eq!(collection.snapshots.keys().next().unwrap().get(), 1);
        assert_eq!(
            collection.snapshots.keys().next_back().unwrap().get(),
            MAX_ASTREA_TOPLEVELS_PER_MANAGER as u64
        );
    }
}
