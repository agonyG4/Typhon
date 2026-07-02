use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum SurfaceRole {
    Unassigned,
    XdgToplevel,
    XdgPopup,
    LayerSurface,
    Subsurface { parent_id: u32 },
    Cursor,
}

impl SurfaceRole {
    pub(in crate::compositor) const fn label(self) -> &'static str {
        match self {
            Self::Unassigned => "unassigned",
            Self::XdgToplevel => "xdg_toplevel",
            Self::XdgPopup => "xdg_popup",
            Self::LayerSurface => "layer_surface",
            Self::Subsurface { .. } => "subsurface",
            Self::Cursor => "cursor",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum SurfaceRoleError {
    AlreadyAssigned {
        current: SurfaceRole,
        requested: SurfaceRole,
    },
    MissingSurface,
    MissingParent,
    Cycle,
}

impl SurfaceRoleError {
    pub(in crate::compositor) fn message(self) -> String {
        match self {
            Self::AlreadyAssigned { current, requested } => format!(
                "wl_surface already has role {} and cannot become {}",
                current.label(),
                requested.label()
            ),
            Self::MissingSurface => "wl_surface is not known to this compositor".to_string(),
            Self::MissingParent => "parent wl_surface is not known to this compositor".to_string(),
            Self::Cycle => "subsurface parent relationship would create a cycle".to_string(),
        }
    }
}

impl CompositorState {
    pub(in crate::compositor) fn surface_role(&self, surface_id: u32) -> SurfaceRole {
        self.surface_roles
            .get(&surface_id)
            .copied()
            .unwrap_or(SurfaceRole::Unassigned)
    }

    pub(in crate::compositor) fn assign_surface_role(
        &mut self,
        surface_id: u32,
        requested: SurfaceRole,
    ) -> Result<(), SurfaceRoleError> {
        if !self.surface_resources.contains_key(&surface_id) {
            return Err(SurfaceRoleError::MissingSurface);
        }
        let current = self.surface_role(surface_id);
        if current == requested && matches!(requested, SurfaceRole::Cursor) {
            return Ok(());
        }
        if current != SurfaceRole::Unassigned {
            return Err(SurfaceRoleError::AlreadyAssigned { current, requested });
        }
        if let SurfaceRole::Subsurface { parent_id } = requested {
            if !self.surface_resources.contains_key(&parent_id) {
                return Err(SurfaceRoleError::MissingParent);
            }
            if self.subsurface_role_would_cycle(surface_id, parent_id) {
                return Err(SurfaceRoleError::Cycle);
            }
        }

        self.surface_roles.insert(surface_id, requested);
        if surface_tree_debug_enabled() {
            eprintln!(
                "oblivion-one compositor: surface_role surface={surface_id} old={} new={} parent={:?}",
                current.label(),
                requested.label(),
                match requested {
                    SurfaceRole::Subsurface { parent_id } => Some(parent_id),
                    _ => None,
                }
            );
        }
        Ok(())
    }

    pub(in crate::compositor) fn clear_surface_role(&mut self, surface_id: u32) {
        self.surface_roles.remove(&surface_id);
    }

    pub(in crate::compositor) fn clear_surface_role_if(
        &mut self,
        surface_id: u32,
        expected: SurfaceRole,
    ) {
        if self.surface_role(surface_id) == expected {
            self.clear_surface_role(surface_id);
        }
    }

    fn subsurface_role_would_cycle(&self, surface_id: u32, mut parent_id: u32) -> bool {
        let mut visited = HashSet::new();
        while visited.insert(parent_id) {
            if parent_id == surface_id {
                return true;
            }
            parent_id = match self.surface_role(parent_id) {
                SurfaceRole::Subsurface { parent_id } => parent_id,
                _ => match self.subsurface_transactions.parent(parent_id) {
                    Some(parent_id) => parent_id,
                    None => return false,
                },
            };
        }
        true
    }
}

pub(in crate::compositor) fn surface_tree_debug_enabled() -> bool {
    std::env::var_os("OBLIVION_ONE_DEBUG_SURFACE_TREE").is_some_and(|value| value != "0")
}
