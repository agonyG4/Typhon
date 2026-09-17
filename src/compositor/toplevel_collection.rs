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
        let kind = self.astrea_toplevel_kind_if_eligible(window_id)?;
        let window = self.desktop_windows.get(&window_id)?;
        Some({
            let mut states = AstreaToplevelStates::default();
            if self.focused_window_id == Some(window_id) {
                states = states.union(AstreaToplevelStates::ACTIVE);
            }
            if window.state.is_minimized() {
                states = states.union(AstreaToplevelStates::MINIMIZED);
            }
            match window.state.mode() {
                super::window_state::ToplevelMode::Normal => {}
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

    pub(in crate::compositor) fn astrea_toplevel_kind_if_eligible(
        &self,
        window_id: WindowId,
    ) -> Option<AstreaToplevelKind> {
        let window = self.desktop_windows.get(&window_id)?;
        match window.backend {
            WindowBackend::Xdg(handle) => {
                let lifecycle = self.xdg_surface_lifecycle(handle.root_surface_id())?;
                (self
                    .toplevel_surfaces
                    .contains_key(&handle.root_surface_id())
                    && lifecycle.currently_mapped)
                    .then_some(AstreaToplevelKind::XdgToplevel)
            }
            WindowBackend::X11(_) => {
                let kind = match window.x11_role {
                    Some(X11DesktopRole::Toplevel) => AstreaToplevelKind::X11Toplevel,
                    Some(X11DesktopRole::Dialog) => AstreaToplevelKind::X11Dialog,
                    _ => return None,
                };
                (window.kind == DesktopWindowKind::Managed).then_some(kind)
            }
        }
    }
}
