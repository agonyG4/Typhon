use super::*;

impl CompositorState {
    pub(in crate::compositor) fn scene_node_id_for_surface(
        &self,
        surface_id: u32,
    ) -> Option<SceneNodeId> {
        self.scene_registry
            .node_for_source(SceneSource::Surface(surface_id))
    }

    #[allow(dead_code)]
    pub(in crate::compositor) fn scene_node_metadata_for_surface(
        &self,
        surface_id: u32,
    ) -> Option<&SceneNodeMetadata> {
        let node = self.scene_node_id_for_surface(surface_id)?;
        self.scene_registry.metadata(node)
    }

    pub(in crate::compositor) fn scene_node_id_for_server_decoration(
        &self,
        window_id: WindowId,
    ) -> Option<SceneNodeId> {
        self.scene_registry
            .node_for_source(SceneSource::ServerDecoration(window_id))
    }

    pub(in crate::compositor) fn ensure_surface_scene_node(
        &mut self,
        surface_id: u32,
    ) -> SceneNodeId {
        if let Some(node) = self.scene_node_id_for_surface(surface_id) {
            return node;
        }
        self.scene_registry
            .register(
                SceneSource::Surface(surface_id),
                SceneOwner::Surface(surface_id),
                SceneRole::UnassignedSurface,
                SceneDomainAssignment::Inherit {
                    fallback: SceneDomain::Content,
                },
            )
            .unwrap_or_else(|error| panic!("failed to register surface scene node: {error:?}"))
    }

    pub(in crate::compositor) fn remove_surface_scene_node(&mut self, surface_id: u32) {
        let Some(node) = self.scene_node_id_for_surface(surface_id) else {
            return;
        };
        let _ = self.scene_registry.remove(node);
    }

    pub(in crate::compositor) fn sync_scene_surface_metadata(&mut self, surface_id: u32) {
        let Some(node) = self.scene_node_id_for_surface(surface_id) else {
            return;
        };
        let lifecycle = self
            .surface_role_lifecycles
            .get(&surface_id)
            .copied()
            .unwrap_or_default();
        let role = lifecycle
            .permanent
            .map(scene_role_for_permanent)
            .unwrap_or(SceneRole::UnassignedSurface);
        let domain = scene_domain_for_surface(self, surface_id, lifecycle.permanent);
        self.scene_registry
            .update_metadata(node, role, domain)
            .unwrap_or_else(|error| panic!("surface scene metadata disappeared: {error:?}"));
        let parent = scene_parent_for_surface(self, surface_id, lifecycle.live_instance);
        self.scene_registry
            .set_visual_parent(node, parent)
            .unwrap_or_else(|error| panic!("invalid surface scene parent: {error:?}"));
    }

    pub(in crate::compositor) fn ensure_window_scene_nodes(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
    ) {
        let group = self
            .ensure_window_scene_node(SceneSource::WindowGroup(window_id), SceneRole::WindowGroup);
        let decoration = self.ensure_window_scene_node(
            SceneSource::ServerDecoration(window_id),
            SceneRole::ServerDecoration,
        );
        self.scene_registry
            .set_visual_parent(decoration, Some(group))
            .unwrap_or_else(|error| panic!("invalid decoration scene parent: {error:?}"));
        if let Some(surface_node) = self.scene_node_id_for_surface(root_surface_id) {
            self.scene_registry
                .set_visual_parent(surface_node, Some(group))
                .unwrap_or_else(|error| panic!("invalid window root scene parent: {error:?}"));
        }
    }

    pub(in crate::compositor) fn remove_window_scene_nodes(&mut self, window_id: WindowId) {
        if let Some(group) = self
            .scene_registry
            .node_for_source(SceneSource::WindowGroup(window_id))
        {
            let _ = self.scene_registry.remove(group);
        }
        if let Some(decoration) = self
            .scene_registry
            .node_for_source(SceneSource::ServerDecoration(window_id))
        {
            let _ = self.scene_registry.remove(decoration);
        }
    }

    pub(in crate::compositor) fn attach_surface_to_window_scene(
        &mut self,
        surface_id: u32,
        window_id: WindowId,
    ) {
        let Some(surface_node) = self.scene_node_id_for_surface(surface_id) else {
            return;
        };
        let Some(group) = self
            .scene_registry
            .node_for_source(SceneSource::WindowGroup(window_id))
        else {
            return;
        };
        self.scene_registry
            .set_visual_parent(surface_node, Some(group))
            .unwrap_or_else(|error| panic!("invalid window surface scene parent: {error:?}"));
    }

    pub(in crate::compositor) fn detach_surface_from_window_scene(&mut self, surface_id: u32) {
        let Some(surface_node) = self.scene_node_id_for_surface(surface_id) else {
            return;
        };
        self.scene_registry
            .set_visual_parent(surface_node, None)
            .unwrap_or_else(|error| panic!("invalid detached surface scene node: {error:?}"));
    }

    fn ensure_window_scene_node(&mut self, source: SceneSource, role: SceneRole) -> SceneNodeId {
        if let Some(node) = self.scene_registry.node_for_source(source) {
            return node;
        }
        let window_id = match source {
            SceneSource::WindowGroup(window_id) | SceneSource::ServerDecoration(window_id) => {
                window_id
            }
            SceneSource::Surface(_) => unreachable!(),
        };
        self.scene_registry
            .register(
                source,
                SceneOwner::Window(window_id),
                role,
                match role {
                    SceneRole::ServerDecoration => {
                        SceneDomainAssignment::Explicit(SceneDomain::Chrome)
                    }
                    SceneRole::WindowGroup => SceneDomainAssignment::Explicit(SceneDomain::Content),
                    _ => unreachable!(),
                },
            )
            .unwrap_or_else(|error| panic!("failed to register window scene node: {error:?}"))
    }
}

fn scene_role_for_permanent(permanent: PermanentSurfaceRole) -> SceneRole {
    match permanent {
        PermanentSurfaceRole::XdgToplevel | PermanentSurfaceRole::Xwayland => {
            SceneRole::ClientSurface
        }
        PermanentSurfaceRole::XdgPopup => SceneRole::PopupSurface,
        PermanentSurfaceRole::LayerSurface => SceneRole::LayerSurface,
        PermanentSurfaceRole::Subsurface => SceneRole::Subsurface,
        PermanentSurfaceRole::Cursor => SceneRole::CursorSurface,
        PermanentSurfaceRole::DragIcon => SceneRole::DragIcon,
    }
}

fn scene_domain_for_surface(
    state: &CompositorState,
    surface_id: u32,
    permanent: Option<PermanentSurfaceRole>,
) -> SceneDomainAssignment {
    match permanent {
        Some(PermanentSurfaceRole::Cursor | PermanentSurfaceRole::DragIcon) => {
            SceneDomainAssignment::Explicit(SceneDomain::Input)
        }
        Some(PermanentSurfaceRole::LayerSurface) => {
            let domain = state
                .layer_surfaces
                .get(&surface_id)
                .map(|role| scene_domain_for_layer(role.committed.layer))
                .unwrap_or(SceneDomain::Chrome);
            SceneDomainAssignment::Explicit(domain)
        }
        Some(PermanentSurfaceRole::XdgToplevel | PermanentSurfaceRole::Xwayland)
        | Some(PermanentSurfaceRole::XdgPopup | PermanentSurfaceRole::Subsurface)
        | None => SceneDomainAssignment::Inherit {
            fallback: SceneDomain::Content,
        },
    }
}

fn scene_domain_for_layer(layer: crate::compositor::layer_shell::Layer) -> SceneDomain {
    match layer {
        crate::compositor::layer_shell::Layer::Background => SceneDomain::Desktop,
        crate::compositor::layer_shell::Layer::Bottom
        | crate::compositor::layer_shell::Layer::Top
        | crate::compositor::layer_shell::Layer::Overlay => SceneDomain::Chrome,
    }
}

fn scene_parent_for_surface(
    state: &CompositorState,
    surface_id: u32,
    live: Option<LiveRoleInstance>,
) -> Option<SceneNodeId> {
    match live {
        Some(LiveRoleInstance::Subsurface { parent_id }) => {
            state.scene_node_id_for_surface(parent_id)
        }
        Some(LiveRoleInstance::XdgPopup) => state
            .popup_surfaces
            .get(&surface_id)
            .and_then(|popup| popup.parent_surface_id)
            .and_then(|parent_id| state.scene_node_id_for_surface(parent_id)),
        Some(LiveRoleInstance::XdgToplevel | LiveRoleInstance::Xwayland) => state
            .window_id_for_surface(surface_id)
            .and_then(|window_id| {
                state
                    .scene_registry
                    .node_for_source(SceneSource::WindowGroup(window_id))
            }),
        Some(LiveRoleInstance::LayerSurface)
        | Some(LiveRoleInstance::Cursor)
        | Some(LiveRoleInstance::DragIcon)
        | None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_semantic_domains_are_metadata_only_and_conservative() {
        use crate::compositor::layer_shell::Layer;

        assert_eq!(
            scene_domain_for_layer(Layer::Background),
            SceneDomain::Desktop
        );
        assert_eq!(scene_domain_for_layer(Layer::Bottom), SceneDomain::Chrome);
        assert_eq!(scene_domain_for_layer(Layer::Top), SceneDomain::Chrome);
        assert_eq!(scene_domain_for_layer(Layer::Overlay), SceneDomain::Chrome);
    }
}
