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
pub(crate) const MAX_ASTREA_TOPLEVEL_HANDLES_PER_CLIENT: usize = 4096;
pub(crate) const MAX_ASTREA_TOPLEVEL_HANDLES_GLOBAL: usize = 16_384;
pub(crate) const MAX_ASTREA_RETIRED_HANDLES_PER_CLIENT: usize = 8192;
pub(crate) const MAX_ASTREA_RETIRED_HANDLES_TOTAL: usize = 32_768;
pub(crate) const MAX_ASTREA_TOPLEVEL_UPDATES_PER_CYCLE: usize = 256;
pub(crate) const MAX_ASTREA_ELIGIBLE_WINDOWS: usize = 65_536;
const MAX_ASTREA_TOPLEVEL_DIRTY_WINDOWS: usize = 8192;

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
    pub(crate) eligible_ids: BTreeSet<WindowId>,
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
    pub(crate) budget_exhausted: bool,
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
    pub(crate) handle_limit_rejections: u64,
    pub(crate) retired_resource_limit_rejections: u64,
    pub(crate) retired_handles_destroyed: u64,
    pub(crate) manager_publication_failures: u64,
    pub(crate) active_handles: u64,
    pub(crate) retired_handles: u64,
    pub(crate) initial_enumeration_rollbacks: u64,
    pub(crate) handles_closed_by_manager_destruction: u64,
    pub(crate) handles_closed_by_window_destruction: u64,
    pub(crate) client_disconnect_cleanups: u64,
    pub(crate) dirty_windows_queued: u64,
    pub(crate) dirty_updates_coalesced: u64,
    pub(crate) incremental_updates_published: u64,
    pub(crate) full_reconciliations: u64,
    pub(crate) full_reconciliation_corrections: u64,
    pub(crate) publication_budget_exhaustions: u64,
}

#[derive(Debug, Clone)]
pub(in crate::compositor) struct AstreaToplevelResourceData {
    pub(in crate::compositor) manager_id: ObjectId,
    pub(in crate::compositor) window_id: WindowId,
    pub(in crate::compositor) client_id: ClientId,
}

#[derive(Debug, Clone)]
struct AstreaToplevelHandleBinding {
    resource: astrea_toplevel_v1::AstreaToplevelV1,
    snapshot: AstreaToplevelSnapshot,
    lifecycle: ToplevelHandleLifecycle,
}

#[derive(Debug, Clone)]
struct RetiredAstreaToplevelHandle {
    resource: astrea_toplevel_v1::AstreaToplevelV1,
    client_id: ClientId,
    manager_id: ObjectId,
    window_id: WindowId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToplevelHandleLifecycle {
    Live,
    Closed,
}

#[derive(Debug, Clone)]
struct AstreaToplevelManagerBinding {
    resource: astrea_toplevel_manager_v1::AstreaToplevelManagerV1,
    client: Client,
    client_id: ClientId,
    handles: HashMap<ObjectId, AstreaToplevelHandleBinding>,
    active_handles: BTreeMap<WindowId, ObjectId>,
    suppressed: BTreeSet<WindowId>,
    last_total: u32,
    last_truncated: bool,
}

#[derive(Debug)]
pub(in crate::compositor) struct AstreaToplevelPublisher {
    pub(in crate::compositor) revision: u64,
    pub(in crate::compositor) canonical: BTreeMap<WindowId, AstreaToplevelSnapshot>,
    canonical_eligible_ids: BTreeSet<WindowId>,
    pub(in crate::compositor) canonical_total: u32,
    managers: HashMap<ObjectId, AstreaToplevelManagerBinding>,
    retired_handles: HashMap<ObjectId, RetiredAstreaToplevelHandle>,
    dirty_windows: BTreeSet<WindowId>,
    removed_windows: BTreeSet<WindowId>,
    structure_dirty: bool,
    initial_reconciliation_pending: bool,
    pending_collection: Option<AstreaToplevelCollection>,
    pending_ids: BTreeSet<WindowId>,
    pending_manager_failures: Vec<(Client, astrea_toplevel_manager_v1::AstreaToplevelManagerV1)>,
    pub(in crate::compositor) metrics: AstreaToplevelMetrics,
}

struct ToplevelPublicationBatch<'a> {
    display: &'a DisplayHandle,
    old_canonical: &'a BTreeMap<WindowId, AstreaToplevelSnapshot>,
    target: &'a AstreaToplevelCollection,
    process_ids: &'a [WindowId],
    revision: u64,
    final_batch: bool,
}

impl Default for AstreaToplevelPublisher {
    fn default() -> Self {
        Self {
            revision: 0,
            canonical: BTreeMap::new(),
            canonical_eligible_ids: BTreeSet::new(),
            canonical_total: 0,
            managers: HashMap::new(),
            retired_handles: HashMap::new(),
            dirty_windows: BTreeSet::new(),
            removed_windows: BTreeSet::new(),
            structure_dirty: false,
            initial_reconciliation_pending: true,
            pending_collection: None,
            pending_ids: BTreeSet::new(),
            pending_manager_failures: Vec::new(),
            metrics: AstreaToplevelMetrics::default(),
        }
    }
}

impl AstreaToplevelPublisher {
    fn refresh_resource_metrics(&mut self) {
        self.metrics.active_handles = self.active_handle_count() as u64;
        self.metrics.retired_handles = self.retired_handle_count() as u64;
    }

    pub(in crate::compositor) fn manager_count(&self) -> usize {
        self.managers.values().count()
    }

    pub(in crate::compositor) fn manager_count_for_client(&self, client_id: &ClientId) -> usize {
        self.managers
            .values()
            .filter(|binding| binding.client_id == *client_id)
            .count()
    }

    pub(in crate::compositor) fn handle_count_for_client(&self, client_id: &ClientId) -> usize {
        let active = self
            .managers
            .values()
            .filter(|binding| binding.client_id == *client_id)
            .map(|binding| binding.handles.len())
            .sum::<usize>();
        active
            + self
                .retired_handles
                .values()
                .filter(|handle| handle.client_id == *client_id)
                .count()
    }

    pub(in crate::compositor) fn handle_count(&self) -> usize {
        self.managers
            .values()
            .map(|binding| binding.handles.len())
            .sum::<usize>()
            .saturating_add(self.retired_handle_count())
    }

    pub(in crate::compositor) fn active_handle_count(&self) -> usize {
        self.managers
            .values()
            .map(|binding| binding.handles.len())
            .sum()
    }

    fn active_handle_count_for_client(&self, client_id: &ClientId) -> usize {
        self.managers
            .values()
            .filter(|binding| binding.client_id == *client_id)
            .map(|binding| binding.handles.len())
            .sum()
    }

    pub(in crate::compositor) fn retired_handle_count(&self) -> usize {
        self.retired_handles.len()
    }

    pub(in crate::compositor) fn mark_window_dirty(&mut self, window_id: WindowId) {
        if self.dirty_windows.len() >= MAX_ASTREA_TOPLEVEL_DIRTY_WINDOWS
            && !self.dirty_windows.contains(&window_id)
        {
            self.structure_dirty = true;
            self.metrics.publication_budget_exhaustions = self
                .metrics
                .publication_budget_exhaustions
                .saturating_add(1);
            return;
        }
        if self.dirty_windows.insert(window_id) {
            self.metrics.dirty_windows_queued = self.metrics.dirty_windows_queued.saturating_add(1);
        } else {
            self.metrics.dirty_updates_coalesced =
                self.metrics.dirty_updates_coalesced.saturating_add(1);
        }
    }

    pub(in crate::compositor) fn mark_structure_dirty(&mut self) {
        self.structure_dirty = true;
    }

    pub(in crate::compositor) fn mark_window_removed(&mut self, window_id: WindowId) {
        self.removed_windows.insert(window_id);
        self.mark_window_dirty(window_id);
        self.mark_structure_dirty();
    }

    pub(in crate::compositor) fn needs_full_reconciliation(&self) -> bool {
        self.initial_reconciliation_pending || self.structure_dirty
    }

    pub(in crate::compositor) fn dirty_window_ids(&self) -> Vec<WindowId> {
        self.dirty_windows
            .iter()
            .chain(self.removed_windows.iter())
            .copied()
            .collect()
    }

    pub(in crate::compositor) fn can_allocate_manager(
        &mut self,
        client_id: &ClientId,
        handle_count: usize,
    ) -> bool {
        self.prune_dead_resources();
        self.manager_count() < MAX_ASTREA_TOPLEVEL_MANAGERS
            && self.manager_count_for_client(client_id) < MAX_ASTREA_TOPLEVEL_MANAGERS_PER_CLIENT
            && self
                .active_handle_count_for_client(client_id)
                .saturating_add(handle_count)
                <= MAX_ASTREA_TOPLEVEL_HANDLES_PER_CLIENT
            && self.active_handle_count().saturating_add(handle_count)
                <= MAX_ASTREA_TOPLEVEL_HANDLES_GLOBAL
            && self
                .handle_count_for_client(client_id)
                .saturating_add(handle_count)
                <= MAX_ASTREA_RETIRED_HANDLES_PER_CLIENT
            && self.handle_count().saturating_add(handle_count) <= MAX_ASTREA_RETIRED_HANDLES_TOTAL
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
        let binding = AstreaToplevelManagerBinding {
            resource: manager.clone(),
            client,
            client_id,
            handles: HashMap::new(),
            active_handles: BTreeMap::new(),
            suppressed: BTreeSet::new(),
            last_total: collection.total,
            last_truncated: collection.total as usize > MAX_ASTREA_TOPLEVELS_PER_MANAGER,
        };
        let revision = self.revision;
        if self.managers.contains_key(&manager_id) {
            return Err(());
        }
        self.managers.insert(manager_id.clone(), binding);
        if self
            .create_initial_handles(display, &manager_id, collection, revision)
            .is_err()
        {
            self.metrics.initial_enumeration_rollbacks =
                self.metrics.initial_enumeration_rollbacks.saturating_add(1);
            self.fail_manager(&manager_id);
            return Err(());
        }
        let initial_done = self
            .managers
            .get(&manager_id)
            .ok_or(())
            .and_then(|binding| {
                send_manager_done(
                    &binding.resource,
                    revision,
                    collection.total,
                    binding.last_truncated,
                )
                .map_err(|_| ())
            });
        if initial_done.is_err() {
            self.fail_manager(&manager_id);
            return Err(());
        }
        self.metrics.managers_accepted = self.metrics.managers_accepted.saturating_add(1);
        if collection.total as usize > MAX_ASTREA_TOPLEVELS_PER_MANAGER {
            self.metrics.truncated_manager_snapshots =
                self.metrics.truncated_manager_snapshots.saturating_add(1);
        }
        self.refresh_resource_metrics();
        Ok(())
    }

    fn create_initial_handles(
        &mut self,
        display: &DisplayHandle,
        manager_id: &ObjectId,
        collection: &AstreaToplevelCollection,
        revision: u64,
    ) -> Result<(), ()> {
        for snapshot in collection.snapshots.values() {
            let mut binding = self.managers.remove(manager_id).ok_or(())?;
            let result = self.create_handle(display, &mut binding, snapshot.clone(), revision);
            self.managers.insert(manager_id.clone(), binding);
            if result.is_err() {
                self.metrics.resource_creation_failures =
                    self.metrics.resource_creation_failures.saturating_add(1);
                return Err(());
            }
        }
        Ok(())
    }

    fn create_handle(
        &mut self,
        display: &DisplayHandle,
        binding: &mut AstreaToplevelManagerBinding,
        snapshot: AstreaToplevelSnapshot,
        revision: u64,
    ) -> Result<(), ()> {
        if !self.can_allocate_handle(binding) {
            if self.handle_count_for_client(&binding.client_id)
                >= MAX_ASTREA_RETIRED_HANDLES_PER_CLIENT
                || self.handle_count() >= MAX_ASTREA_RETIRED_HANDLES_TOTAL
            {
                self.metrics.retired_resource_limit_rejections = self
                    .metrics
                    .retired_resource_limit_rejections
                    .saturating_add(1);
            }
            self.metrics.handle_limit_rejections =
                self.metrics.handle_limit_rejections.saturating_add(1);
            return Err(());
        }
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
                    client_id: binding.client_id.clone(),
                },
            )
            .map_err(|_| ())?;
        let resource_id = resource.id();
        binding.handles.insert(
            resource_id.clone(),
            AstreaToplevelHandleBinding {
                resource: resource.clone(),
                snapshot: snapshot.clone(),
                lifecycle: ToplevelHandleLifecycle::Live,
            },
        );
        binding.active_handles.insert(snapshot.id, resource_id);
        if binding
            .resource
            .send_event(astrea_toplevel_manager_v1::Event::Toplevel {
                id: resource.clone(),
            })
            .is_err()
        {
            let _ = self.close_active_handle(
                binding,
                snapshot.id,
                HandleCloseReason::PublicationFailure,
            );
            return Err(());
        }
        if send_initial_handle(&resource, &snapshot, revision).is_err() {
            let _ = self.close_active_handle(
                binding,
                snapshot.id,
                HandleCloseReason::PublicationFailure,
            );
            return Err(());
        }
        self.metrics.handles_created = self.metrics.handles_created.saturating_add(1);
        Ok(())
    }

    fn can_allocate_handle(&self, binding: &AstreaToplevelManagerBinding) -> bool {
        let binding_is_tracked = self.managers.contains_key(&binding.resource.id());
        let own_active_handles = if !binding_is_tracked {
            binding.handles.len()
        } else {
            0
        };
        let active_for_client = self
            .active_handle_count_for_client(&binding.client_id)
            .saturating_add(own_active_handles);
        let active_total = self
            .active_handle_count()
            .saturating_add(own_active_handles);
        let tracked_for_client = self
            .handle_count_for_client(&binding.client_id)
            .saturating_add(own_active_handles);
        let tracked_total = self.handle_count().saturating_add(own_active_handles);
        binding.handles.len() < MAX_ASTREA_TOPLEVELS_PER_MANAGER
            && active_for_client.saturating_add(1) <= MAX_ASTREA_TOPLEVEL_HANDLES_PER_CLIENT
            && active_total.saturating_add(1) <= MAX_ASTREA_TOPLEVEL_HANDLES_GLOBAL
            && tracked_for_client.saturating_add(1) <= MAX_ASTREA_RETIRED_HANDLES_PER_CLIENT
            && tracked_total.saturating_add(1) <= MAX_ASTREA_RETIRED_HANDLES_TOTAL
    }

    pub(in crate::compositor) fn reconcile(
        &mut self,
        display: &DisplayHandle,
        collection: Option<AstreaToplevelCollection>,
        dirty_snapshots: BTreeMap<WindowId, Option<AstreaToplevelSnapshot>>,
    ) -> AstreaToplevelPublicationSummary {
        self.prune_dead_resources();
        self.refresh_resource_metrics();

        if self.initial_reconciliation_pending || self.structure_dirty {
            let Some(collection) = collection else {
                return self.noop_summary();
            };
            self.metrics.full_reconciliations = self.metrics.full_reconciliations.saturating_add(1);
            if self.manager_count() == 0 {
                self.canonical = collection.snapshots;
                self.canonical_eligible_ids = collection.eligible_ids;
                self.canonical_total = collection.total;
                self.initial_reconciliation_pending = false;
                self.structure_dirty = false;
                self.dirty_windows.clear();
                self.removed_windows.clear();
                self.pending_collection = None;
                self.pending_ids.clear();
                return self.noop_summary();
            }
            self.pending_collection = Some(collection);
            self.pending_ids = self.diff_ids(
                &self
                    .pending_collection
                    .as_ref()
                    .expect("pending collection was just installed")
                    .clone(),
            );
            self.initial_reconciliation_pending = false;
            self.structure_dirty = false;
            self.dirty_windows.clear();
            self.removed_windows.clear();
        } else if let Some(pending) = self.pending_collection.as_mut() {
            for (window_id, snapshot) in dirty_snapshots {
                match snapshot {
                    Some(snapshot) => {
                        pending.eligible_ids.insert(window_id);
                        pending.snapshots.insert(window_id, snapshot);
                        if pending.snapshots.len() > MAX_ASTREA_TOPLEVELS_PER_MANAGER
                            && let Some(largest) = pending.snapshots.keys().next_back().copied()
                        {
                            pending.snapshots.remove(&largest);
                        }
                    }
                    None => {
                        pending.eligible_ids.remove(&window_id);
                        pending.snapshots.remove(&window_id);
                    }
                }
            }
            pending.total = pending.eligible_ids.len().min(u32::MAX as usize) as u32;
            let target = pending.clone();
            self.pending_ids = self.diff_ids(&target);
            self.dirty_windows.clear();
            self.removed_windows.clear();
        } else if !dirty_snapshots.is_empty() {
            let mut pending = AstreaToplevelCollection {
                snapshots: self.canonical.clone(),
                eligible_ids: self.canonical_eligible_ids.clone(),
                total: self.canonical_total,
            };
            for (window_id, snapshot) in dirty_snapshots {
                match snapshot {
                    Some(snapshot) => {
                        pending.eligible_ids.insert(window_id);
                        pending.snapshots.insert(window_id, snapshot);
                        if pending.snapshots.len() > MAX_ASTREA_TOPLEVELS_PER_MANAGER
                            && let Some(largest) = pending.snapshots.keys().next_back().copied()
                        {
                            pending.snapshots.remove(&largest);
                        }
                    }
                    None => {
                        pending.eligible_ids.remove(&window_id);
                        pending.snapshots.remove(&window_id);
                    }
                }
            }
            pending.total = pending.eligible_ids.len().min(u32::MAX as usize) as u32;
            self.pending_ids = self.diff_ids(&pending);
            self.pending_collection = Some(pending);
            self.dirty_windows.clear();
            self.removed_windows.clear();
        }

        let Some(target) = self.pending_collection.clone() else {
            return self.noop_summary();
        };
        let old_canonical = self.canonical.clone();
        let target_changed = old_canonical != target.snapshots
            || self.canonical_eligible_ids != target.eligible_ids
            || self.canonical_total != target.total;
        if !target_changed {
            self.pending_collection = None;
            self.pending_ids.clear();
            return self.noop_summary();
        }

        let process_ids = self.next_publication_ids();
        let final_batch = process_ids.len() >= self.pending_ids.len();
        let budget_exhausted = !final_batch;
        if process_ids.is_empty() && !target_changed {
            return self.noop_summary();
        }

        self.revision = self.revision.wrapping_add(1);
        let revision = self.revision;
        let mut summary = AstreaToplevelPublicationSummary {
            revision,
            manager_count: self.manager_count(),
            changed: true,
            ..AstreaToplevelPublicationSummary::default()
        };
        let batch = ToplevelPublicationBatch {
            display,
            old_canonical: &old_canonical,
            target: &target,
            process_ids: &process_ids,
            revision,
            final_batch,
        };
        self.publish_delta(batch, &mut summary);

        for window_id in process_ids {
            if let Some(snapshot) = target.snapshots.get(&window_id) {
                self.canonical.insert(window_id, snapshot.clone());
            } else {
                self.canonical.remove(&window_id);
            }
            self.pending_ids.remove(&window_id);
        }
        if self.pending_ids.is_empty() {
            self.canonical_eligible_ids = target.eligible_ids.clone();
            self.canonical_total = target.total;
            self.pending_collection = None;
        }
        if budget_exhausted {
            self.metrics.publication_budget_exhaustions = self
                .metrics
                .publication_budget_exhaustions
                .saturating_add(1);
            summary.budget_exhausted = true;
        }
        self.metrics.batches_published = self.metrics.batches_published.saturating_add(1);
        self.refresh_resource_metrics();
        summary
    }

    fn noop_summary(&mut self) -> AstreaToplevelPublicationSummary {
        self.metrics.noop_reconciliations = self.metrics.noop_reconciliations.saturating_add(1);
        AstreaToplevelPublicationSummary {
            revision: self.revision,
            manager_count: self.manager_count(),
            ..AstreaToplevelPublicationSummary::default()
        }
    }

    fn diff_ids(&self, target: &AstreaToplevelCollection) -> BTreeSet<WindowId> {
        self.canonical
            .keys()
            .chain(target.snapshots.keys())
            .chain(self.canonical_eligible_ids.iter())
            .chain(target.eligible_ids.iter())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|window_id| {
                self.canonical.get(window_id) != target.snapshots.get(window_id)
                    || self.canonical_eligible_ids.contains(window_id)
                        != target.eligible_ids.contains(window_id)
            })
            .collect()
    }

    fn next_publication_ids(&self) -> Vec<WindowId> {
        let mut ids = self
            .pending_ids
            .iter()
            .copied()
            .filter(|window_id| {
                !self
                    .pending_collection
                    .as_ref()
                    .is_some_and(|target| target.snapshots.contains_key(window_id))
            })
            .take(MAX_ASTREA_TOPLEVEL_UPDATES_PER_CYCLE)
            .collect::<Vec<_>>();
        if ids.len() < MAX_ASTREA_TOPLEVEL_UPDATES_PER_CYCLE {
            let remaining = MAX_ASTREA_TOPLEVEL_UPDATES_PER_CYCLE.saturating_sub(ids.len());
            let extra = self
                .pending_ids
                .iter()
                .copied()
                .filter(|window_id| !ids.contains(window_id))
                .take(remaining)
                .collect::<Vec<_>>();
            ids.extend(extra);
        }
        ids
    }

    fn publish_delta(
        &mut self,
        batch: ToplevelPublicationBatch<'_>,
        summary: &mut AstreaToplevelPublicationSummary,
    ) {
        let new_truncated = batch.target.total as usize > MAX_ASTREA_TOPLEVELS_PER_MANAGER;
        let prefix_changed = batch.old_canonical.keys().ne(batch.target.snapshots.keys());
        let manager_ids = self.managers.keys().cloned().collect::<Vec<_>>();
        for manager_id in manager_ids {
            let Some(mut binding) = self.managers.remove(&manager_id) else {
                continue;
            };
            let mut manager_added = 0usize;
            let mut manager_updated = 0usize;
            let mut manager_closed = 0usize;
            let mut failed = false;
            for window_id in batch.process_ids {
                let Some(snapshot) = batch.target.snapshots.get(window_id) else {
                    let reason = if batch.target.eligible_ids.contains(window_id) {
                        HandleCloseReason::PrefixEviction
                    } else {
                        HandleCloseReason::WindowDestruction
                    };
                    match self.close_active_handle(&mut binding, *window_id, reason) {
                        Ok(true) => {
                            manager_closed = manager_closed.saturating_add(1);
                            summary.closed = summary.closed.saturating_add(1);
                        }
                        Ok(false) => {}
                        Err(_) => {
                            failed = true;
                            break;
                        }
                    }
                    if !batch.target.eligible_ids.contains(window_id) {
                        binding.suppressed.remove(window_id);
                    }
                    continue;
                };
                if binding.suppressed.contains(window_id) {
                    continue;
                }
                if let Some(resource_id) = binding.active_handles.get(window_id).cloned()
                    && let Some(handle) = binding.handles.get_mut(&resource_id)
                {
                    if batch.old_canonical.get(window_id) != Some(snapshot) {
                        if send_changed_handle(
                            &handle.resource,
                            &handle.snapshot,
                            snapshot,
                            batch.revision,
                        )
                        .is_err()
                        {
                            failed = true;
                            break;
                        }
                        handle.snapshot = snapshot.clone();
                        manager_updated = manager_updated.saturating_add(1);
                        summary.updated = summary.updated.saturating_add(1);
                        self.metrics.windows_updated =
                            self.metrics.windows_updated.saturating_add(1);
                        self.metrics.incremental_updates_published =
                            self.metrics.incremental_updates_published.saturating_add(1);
                    }
                } else if self
                    .create_handle(
                        batch.display,
                        &mut binding,
                        snapshot.clone(),
                        batch.revision,
                    )
                    .is_ok()
                {
                    manager_added = manager_added.saturating_add(1);
                    summary.added = summary.added.saturating_add(1);
                } else {
                    self.metrics.resource_creation_failures =
                        self.metrics.resource_creation_failures.saturating_add(1);
                    failed = true;
                    break;
                }
            }
            if failed {
                self.fail_binding(binding);
                continue;
            }
            let manager_changed = manager_added != 0
                || manager_updated != 0
                || manager_closed != 0
                || (batch.final_batch
                    && (binding.last_total != batch.target.total
                        || binding.last_truncated != new_truncated
                        || prefix_changed));
            if manager_changed && batch.final_batch {
                if send_manager_done(
                    &binding.resource,
                    batch.revision,
                    batch.target.total,
                    new_truncated,
                )
                .is_err()
                {
                    self.fail_binding(binding);
                    continue;
                }
                binding.last_total = batch.target.total;
                binding.last_truncated = new_truncated;
            }
            if new_truncated {
                summary.truncated_manager_count = summary.truncated_manager_count.saturating_add(1);
            }
            self.managers.insert(manager_id, binding);
        }
    }

    fn fail_manager(&mut self, manager_id: &ObjectId) {
        let Some(binding) = self.managers.remove(manager_id) else {
            return;
        };
        self.fail_binding(binding);
    }

    pub(in crate::compositor) fn fail_all_managers(&mut self) {
        let manager_ids = self.managers.keys().cloned().collect::<Vec<_>>();
        for manager_id in manager_ids {
            self.fail_manager(&manager_id);
        }
    }

    fn fail_binding(&mut self, mut binding: AstreaToplevelManagerBinding) {
        let manager_id = binding.resource.id();
        let window_ids = binding.active_handles.keys().copied().collect::<Vec<_>>();
        for window_id in window_ids {
            let _ = self.close_active_handle(
                &mut binding,
                window_id,
                HandleCloseReason::PublicationFailure,
            );
        }
        self.metrics.manager_publication_failures =
            self.metrics.manager_publication_failures.saturating_add(1);
        if binding.resource.is_alive()
            && self.pending_manager_failures.len() < MAX_ASTREA_TOPLEVEL_MANAGERS
        {
            self.pending_manager_failures
                .push((binding.client, binding.resource));
        }
        debug_assert!(
            !self.managers.contains_key(&manager_id),
            "failed manager must be removed from active publication"
        );
    }

    pub(in crate::compositor) fn take_manager_failures(
        &mut self,
    ) -> Vec<(Client, astrea_toplevel_manager_v1::AstreaToplevelManagerV1)> {
        std::mem::take(&mut self.pending_manager_failures)
    }

    pub(in crate::compositor) fn remove_manager(
        &mut self,
        client_id: &ClientId,
        manager_id: &ObjectId,
    ) {
        let Some(mut binding) = self.managers.remove(manager_id) else {
            return;
        };
        if binding.client_id != *client_id {
            self.managers.insert(manager_id.clone(), binding);
            return;
        }
        let window_ids = binding.active_handles.keys().copied().collect::<Vec<_>>();
        for window_id in window_ids {
            if self
                .close_active_handle(
                    &mut binding,
                    window_id,
                    HandleCloseReason::ManagerDestruction,
                )
                .is_ok_and(|closed| closed)
            {
                self.metrics.handles_closed_by_manager_destruction = self
                    .metrics
                    .handles_closed_by_manager_destruction
                    .saturating_add(1);
            }
        }
        self.metrics.dead_resources_pruned = self.metrics.dead_resources_pruned.saturating_add(1);
        self.refresh_resource_metrics();
    }

    pub(in crate::compositor) fn remove_handle(
        &mut self,
        client_id: &ClientId,
        manager_id: &ObjectId,
        window_id: WindowId,
        resource_id: &ObjectId,
    ) {
        if self.retired_handles.get(resource_id).is_some_and(|handle| {
            handle.client_id == *client_id
                && handle.manager_id == *manager_id
                && handle.window_id == window_id
        }) {
            self.retired_handles.remove(resource_id);
            self.metrics.retired_handles_destroyed =
                self.metrics.retired_handles_destroyed.saturating_add(1);
            self.refresh_resource_metrics();
            return;
        }
        let Some(binding) = self.managers.get_mut(manager_id) else {
            return;
        };
        if binding.client_id != *client_id {
            return;
        }
        let Some(handle) = binding.handles.get(resource_id) else {
            return;
        };
        if handle.snapshot.id != window_id {
            return;
        }
        binding.handles.remove(resource_id);
        if binding.active_handles.remove(&window_id).is_some() {
            binding.suppressed.insert(window_id);
        }
        self.refresh_resource_metrics();
    }

    pub(in crate::compositor) fn remove_client(&mut self, client_id: &ClientId) {
        let before_managers = self.managers.len();
        self.managers
            .retain(|_, binding| binding.client_id != *client_id);
        self.retired_handles
            .retain(|_, handle| handle.client_id != *client_id);
        self.pending_manager_failures
            .retain(|(client, _)| client.id() != *client_id);
        self.refresh_resource_metrics();
        let removed = before_managers.saturating_sub(self.managers.len());
        if removed != 0 {
            self.metrics.client_disconnect_cleanups = self
                .metrics
                .client_disconnect_cleanups
                .saturating_add(removed as u64);
        }
    }

    fn prune_dead_resources(&mut self) {
        let before_retired = self.retired_handles.len();
        self.retired_handles
            .retain(|_, handle| handle.resource.is_alive());
        self.metrics.retired_handles_destroyed = self
            .metrics
            .retired_handles_destroyed
            .saturating_add((before_retired - self.retired_handles.len()) as u64);
        let manager_ids = self.managers.keys().cloned().collect::<Vec<_>>();
        for manager_id in manager_ids {
            let Some(mut binding) = self.managers.remove(&manager_id) else {
                continue;
            };
            if !binding.resource.is_alive() {
                let window_ids = binding.active_handles.keys().copied().collect::<Vec<_>>();
                for window_id in window_ids {
                    let _ = self.close_active_handle(
                        &mut binding,
                        window_id,
                        HandleCloseReason::ManagerDestruction,
                    );
                }
                continue;
            }
            let before = binding.handles.len();
            binding.handles.retain(|resource_id, handle| {
                let alive = handle.resource.is_alive();
                if !alive {
                    binding
                        .active_handles
                        .retain(|_, active_id| active_id != resource_id);
                }
                alive
            });
            self.metrics.dead_resources_pruned = self
                .metrics
                .dead_resources_pruned
                .saturating_add((before - binding.handles.len()) as u64);
            if binding.resource.is_alive() {
                self.managers.insert(manager_id, binding);
            }
        }
        self.refresh_resource_metrics();
    }

    fn close_active_handle(
        &mut self,
        binding: &mut AstreaToplevelManagerBinding,
        window_id: WindowId,
        reason: HandleCloseReason,
    ) -> Result<bool, wayland_server::backend::InvalidId> {
        let Some(resource_id) = binding.active_handles.remove(&window_id) else {
            return Ok(false);
        };
        let Some(mut handle) = binding.handles.remove(&resource_id) else {
            return Ok(false);
        };
        if handle.lifecycle == ToplevelHandleLifecycle::Closed {
            return Ok(false);
        }
        let send_result = handle
            .resource
            .send_event(astrea_toplevel_v1::Event::Closed);
        handle.lifecycle = ToplevelHandleLifecycle::Closed;
        self.metrics.handles_closed = self.metrics.handles_closed.saturating_add(1);
        if reason == HandleCloseReason::WindowDestruction {
            self.metrics.handles_closed_by_window_destruction = self
                .metrics
                .handles_closed_by_window_destruction
                .saturating_add(1);
        }
        let resource_id = handle.resource.id();
        self.retired_handles.insert(
            resource_id.clone(),
            RetiredAstreaToplevelHandle {
                resource: handle.resource,
                client_id: binding.client_id.clone(),
                manager_id: binding.resource.id(),
                window_id,
            },
        );
        self.refresh_resource_metrics();
        send_result.map(|()| true)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandleCloseReason {
    ManagerDestruction,
    WindowDestruction,
    PrefixEviction,
    PublicationFailure,
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
) -> Result<(), wayland_server::backend::InvalidId> {
    if old.app_id != new.app_id {
        resource.send_event(astrea_toplevel_v1::Event::AppId {
            app_id: new.app_id.clone(),
        })?;
    }
    if old.title != new.title {
        resource.send_event(astrea_toplevel_v1::Event::Title {
            title: new.title.clone(),
        })?;
    }
    if old.pid != new.pid {
        resource.send_event(astrea_toplevel_v1::Event::Pid { pid: new.pid })?;
    }
    if old.kind != new.kind {
        resource.send_event(astrea_toplevel_v1::Event::Kind {
            kind: WEnum::Value(new.kind.wire()),
        })?;
    }
    if old.states != new.states {
        resource.send_event(astrea_toplevel_v1::Event::State {
            state: WEnum::Value(new.states.wire()),
        })?;
    }
    if old.focus_serial != new.focus_serial {
        let (serial_hi, serial_lo) = split_u64(new.focus_serial);
        resource.send_event(astrea_toplevel_v1::Event::FocusSerial {
            serial_hi,
            serial_lo,
        })?;
    }
    let (revision_hi, revision_lo) = split_u64(revision);
    resource.send_event(astrea_toplevel_v1::Event::Done {
        revision_hi,
        revision_lo,
    })
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
    pub(in crate::compositor) fn collect_astrea_toplevels(
        &self,
    ) -> Result<AstreaToplevelCollection, ()> {
        let mut snapshots = BTreeMap::new();
        let mut eligible_ids = BTreeSet::new();
        let mut total = 0u32;
        for window_id in self.desktop_windows.keys().copied() {
            let Some(snapshot) = self.astrea_toplevel_snapshot(window_id) else {
                continue;
            };
            if eligible_ids.len() >= MAX_ASTREA_ELIGIBLE_WINDOWS {
                return Err(());
            }
            eligible_ids.insert(window_id);
            total = total.saturating_add(1);
            snapshots.insert(window_id, snapshot);
            if snapshots.len() > MAX_ASTREA_TOPLEVELS_PER_MANAGER {
                let largest = snapshots.keys().next_back().copied();
                if let Some(largest) = largest {
                    snapshots.remove(&largest);
                }
            }
        }
        Ok(AstreaToplevelCollection {
            snapshots,
            eligible_ids,
            total,
        })
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
            collection.eligible_ids.insert(id(value as u64));
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
        assert_eq!(
            collection.eligible_ids.len(),
            MAX_ASTREA_TOPLEVELS_PER_MANAGER + 2
        );
        assert_eq!(collection.snapshots.keys().next().unwrap().get(), 1);
        assert_eq!(
            collection.snapshots.keys().next_back().unwrap().get(),
            MAX_ASTREA_TOPLEVELS_PER_MANAGER as u64
        );
    }

    #[test]
    fn dirty_windows_are_coalesced_and_bounded() {
        let mut publisher = AstreaToplevelPublisher::default();
        publisher.mark_window_dirty(id(1));
        publisher.mark_window_dirty(id(1));
        publisher.mark_window_dirty(id(2));

        assert_eq!(publisher.dirty_window_ids(), vec![id(1), id(2)]);
        assert_eq!(publisher.metrics.dirty_windows_queued, 2);
        assert_eq!(publisher.metrics.dirty_updates_coalesced, 1);
    }

    #[test]
    fn first_reconciliation_is_the_only_unprompted_full_scan() {
        let mut publisher = AstreaToplevelPublisher::default();
        assert!(publisher.needs_full_reconciliation());
        publisher.initial_reconciliation_pending = false;
        assert!(!publisher.needs_full_reconciliation());
        publisher.mark_window_dirty(id(1));
        assert!(!publisher.needs_full_reconciliation());
    }
}
