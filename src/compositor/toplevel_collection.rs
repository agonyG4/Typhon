use super::toplevel_publication::{
    AstreaToplevelCollection, AstreaToplevelKind, AstreaToplevelSnapshot, AstreaToplevelStates,
    MAX_ASTREA_ELIGIBLE_WINDOWS, MAX_ASTREA_TOPLEVELS_PER_MANAGER,
};
use super::*;
use std::collections::{BTreeMap, BTreeSet};

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
            if snapshots.len() > MAX_ASTREA_TOPLEVELS_PER_MANAGER
                && let Some(largest) = snapshots.keys().next_back().copied()
            {
                snapshots.remove(&largest);
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
