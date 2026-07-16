use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Layer {
    Background,
    Bottom,
    Top,
    Overlay,
}

impl Layer {
    pub(super) const fn scene_rank(self) -> u8 {
        match self {
            Self::Background => 0,
            Self::Bottom => 1,
            Self::Top => 3,
            Self::Overlay => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyboardInteractivity {
    None,
    Exclusive,
    OnDemand,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct LayerAnchors {
    pub(super) top: bool,
    pub(super) bottom: bool,
    pub(super) left: bool,
    pub(super) right: bool,
}

impl LayerAnchors {
    const fn horizontal_opposite(self) -> bool {
        self.left && self.right
    }

    const fn vertical_opposite(self) -> bool {
        self.top && self.bottom
    }

    const fn exclusive_edge(self) -> Option<ExclusiveEdge> {
        match (
            self.top,
            self.bottom,
            self.left,
            self.right,
            self.horizontal_opposite(),
            self.vertical_opposite(),
        ) {
            (true, false, false, false, _, _) | (true, false, true, true, _, _) => {
                Some(ExclusiveEdge::Top)
            }
            (false, true, false, false, _, _) | (false, true, true, true, _, _) => {
                Some(ExclusiveEdge::Bottom)
            }
            (false, false, true, false, _, _) | (true, true, true, false, _, _) => {
                Some(ExclusiveEdge::Left)
            }
            (false, false, false, true, _, _) | (true, true, false, true, _, _) => {
                Some(ExclusiveEdge::Right)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExclusiveEdge {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct LayerMargins {
    pub(super) top: i32,
    pub(super) right: i32,
    pub(super) bottom: i32,
    pub(super) left: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct LayerRequestedSize {
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LayerSurfaceCommitState {
    pub(super) layer: Layer,
    pub(super) anchors: LayerAnchors,
    pub(super) size: LayerRequestedSize,
    pub(super) margins: LayerMargins,
    pub(super) exclusive_zone: i32,
    pub(super) keyboard_interactivity: KeyboardInteractivity,
}

impl LayerSurfaceCommitState {
    fn exclusive_edge(self) -> Option<ExclusiveEdge> {
        (self.exclusive_zone > 0)
            .then(|| self.anchors.exclusive_edge())
            .flatten()
    }

    fn reservation_extent(self, edge: ExclusiveEdge) -> u32 {
        let margin = match edge {
            ExclusiveEdge::Top => self.margins.top,
            ExclusiveEdge::Bottom => self.margins.bottom,
            ExclusiveEdge::Left => self.margins.left,
            ExclusiveEdge::Right => self.margins.right,
        };
        u32::try_from(self.exclusive_zone)
            .unwrap_or_default()
            .saturating_add(u32::try_from(margin.max(0)).unwrap_or_default())
    }
}

impl Default for LayerSurfaceCommitState {
    fn default() -> Self {
        Self {
            layer: Layer::Background,
            anchors: LayerAnchors::default(),
            size: LayerRequestedSize::default(),
            margins: LayerMargins::default(),
            exclusive_zone: 0,
            keyboard_interactivity: KeyboardInteractivity::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LayerGeometry {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl LayerGeometry {
    const fn size(self) -> (u32, u32) {
        (self.width, self.height)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PendingLayerConfigure {
    pub(super) serial: u32,
    pub(super) committed_state: LayerSurfaceCommitState,
    pub(super) geometry: LayerGeometry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LayerLayoutRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl LayerLayoutRect {
    const fn from_output(output_size: OutputSize) -> Self {
        Self {
            x: 0,
            y: 0,
            width: output_size.width,
            height: output_size.height,
        }
    }

    fn reserve(&mut self, edge: ExclusiveEdge, amount: u32) {
        match edge {
            ExclusiveEdge::Top => {
                let applied = amount.min(self.height);
                self.y = self.y.saturating_add(applied as i32);
                self.height = self.height.saturating_sub(applied);
            }
            ExclusiveEdge::Bottom => {
                self.height = self.height.saturating_sub(amount.min(self.height));
            }
            ExclusiveEdge::Left => {
                let applied = amount.min(self.width);
                self.x = self.x.saturating_add(applied as i32);
                self.width = self.width.saturating_sub(applied);
            }
            ExclusiveEdge::Right => {
                self.width = self.width.saturating_sub(amount.min(self.width));
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct LayerSurfaceRole {
    pub(super) surface: wl_surface::WlSurface,
    pub(super) resource: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    pub(super) output_id: Option<u32>,
    pub(super) namespace: String,
    pub(super) pending: LayerSurfaceCommitState,
    pub(super) committed: LayerSurfaceCommitState,
    pub(super) initial_configure_sent: bool,
    pub(super) pending_configures: VecDeque<PendingLayerConfigure>,
    pub(super) acked_configure: Option<PendingLayerConfigure>,
    pub(super) last_configure_size: Option<(u32, u32)>,
    pub(super) mapped: bool,
    pub(super) geometry: Option<LayerGeometry>,
    pub(super) version: u32,
    pub(super) order: u64,
}

impl LayerSurfaceRole {
    pub(super) fn new(
        surface: wl_surface::WlSurface,
        resource: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        output_id: Option<u32>,
        namespace: String,
        layer: Layer,
        version: u32,
        order: u64,
    ) -> Self {
        let state = LayerSurfaceCommitState {
            layer,
            ..LayerSurfaceCommitState::default()
        };
        Self {
            surface,
            resource,
            output_id,
            namespace,
            pending: state,
            committed: state,
            initial_configure_sent: false,
            pending_configures: VecDeque::new(),
            acked_configure: None,
            last_configure_size: None,
            mapped: false,
            geometry: None,
            version,
            order,
        }
    }
}

impl CompositorState {
    pub(in crate::compositor) fn register_layer_surface(
        &mut self,
        surface: wl_surface::WlSurface,
        resource: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        output: Option<wl_output::WlOutput>,
        namespace: String,
        layer: Layer,
    ) {
        let surface_id = compositor_surface_id(&surface);
        self.layer_surface_order = self.layer_surface_order.saturating_add(1);
        let output_id = output.as_ref().map(Resource::id).map(|id| id.protocol_id());
        let role = LayerSurfaceRole::new(
            surface,
            resource.clone(),
            output_id,
            namespace,
            layer,
            resource.version(),
            self.layer_surface_order,
        );
        self.layer_surfaces.insert(surface_id, role);
        self.store_surface_placement(surface_id, SurfacePlacement::root());
        layer_shell_debug_log(|| format!("create surface={surface_id}"));
    }

    pub(in crate::compositor) fn apply_layer_surface_commit(&mut self, surface_id: u32) -> bool {
        if !self.layer_surfaces.contains_key(&surface_id) {
            return true;
        }
        let previous_usable = self.reserved_usable_geometry();
        let committed_change = match self.commit_pending_layer_surface_state(surface_id) {
            Some(committed_change) => committed_change,
            None => {
                return false;
            }
        };
        if committed_change {
            self.arrange_layer_surfaces_and_reconfigure_stateful_windows_from(previous_usable);
            self.reorder_renderable_surfaces_by_committed_stack();
        }
        if self
            .layer_surfaces
            .get(&surface_id)
            .is_some_and(|role| !role.initial_configure_sent)
        {
            self.configure_layer_surface(surface_id);
        }
        true
    }

    pub(in crate::compositor) fn layer_surface_can_publish_buffer(
        &mut self,
        surface_id: u32,
        pending_surface_size: Option<BufferSize>,
    ) -> bool {
        if !self.layer_surfaces.contains_key(&surface_id) {
            return true;
        }
        let previous_usable = self.reserved_usable_geometry();
        if self
            .commit_pending_layer_surface_state(surface_id)
            .is_none()
        {
            return false;
        }
        if self
            .layer_surfaces
            .get(&surface_id)
            .is_some_and(|role| role.mapped)
        {
            self.arrange_layer_surfaces_and_reconfigure_stateful_windows_from(previous_usable);
            self.reorder_renderable_surfaces_by_committed_stack();
        }
        let Some(role) = self.layer_surfaces.get(&surface_id) else {
            return false;
        };
        let requires_initial_ack = !role.mapped && role.acked_configure.is_none();
        if !role.initial_configure_sent || requires_initial_ack {
            let resource = role.resource.clone();
            self.note_protocol_error_metric();
            resource.post_error(
                zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                "layer surface buffer committed before configure was acknowledged".to_string(),
            );
            layer_shell_debug_log(|| {
                format!("commit surface={surface_id} rejected=buffer-before-configure-ack")
            });
            return false;
        }
        if role.mapped
            && role.acked_configure.is_none()
            && !role.pending_configures.is_empty()
            && let Some(pending) = pending_surface_size
        {
            let commits_unacked_configure_size = role.pending_configures.iter().any(|configure| {
                let size = configure.geometry.size();
                pending.width == size.0 && pending.height == size.1
            });
            if commits_unacked_configure_size {
                let resource = role.resource.clone();
                self.note_protocol_error_metric();
                resource.post_error(
                    zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                    "layer surface buffer committed before configure was acknowledged".to_string(),
                );
                layer_shell_debug_log(|| {
                    format!("commit surface={surface_id} rejected=buffer-before-configure-ack")
                });
                return false;
            }
        }
        if let Some(role) = self.layer_surfaces.get_mut(&surface_id) {
            role.acked_configure = None;
        }
        true
    }

    pub(in crate::compositor) fn note_layer_surface_buffer_published(&mut self, surface_id: u32) {
        if !self.layer_surfaces.contains_key(&surface_id) {
            return;
        }
        let previous_usable = self.reserved_usable_geometry();
        let should_focus = self.layer_surfaces.get(&surface_id).is_some_and(|role| {
            role.committed.keyboard_interactivity == KeyboardInteractivity::Exclusive
        });
        self.layer_surface_order = self.layer_surface_order.saturating_add(1);
        let activation_order = self.layer_surface_order;
        if let Some(role) = self.layer_surfaces.get_mut(&surface_id) {
            role.mapped = true;
            role.order = activation_order;
        }
        self.arrange_layer_surfaces_and_reconfigure_stateful_windows_from(previous_usable);
        if should_focus {
            self.recompute_layer_keyboard_focus();
        }
        self.reorder_renderable_surfaces_by_committed_stack();
        layer_shell_debug_log(|| format!("map surface={surface_id}"));
    }

    pub(in crate::compositor) fn note_layer_surface_unmapped(&mut self, surface_id: u32) {
        let previous_usable = self.reserved_usable_geometry();
        let Some(role) = self.layer_surfaces.get_mut(&surface_id) else {
            return;
        };
        let was_mapped = role.mapped;
        role.mapped = false;
        role.acked_configure = None;
        role.pending_configures.clear();
        role.last_configure_size = None;
        role.initial_configure_sent = false;
        if was_mapped {
            self.unregister_layer_surface_popups(surface_id);
            self.arrange_layer_surfaces_and_reconfigure_stateful_windows_from(previous_usable);
            self.reorder_renderable_surfaces_by_committed_stack();
        }
        self.recompute_layer_keyboard_focus();
        layer_shell_debug_log(|| format!("unmap surface={surface_id}"));
    }

    pub(in crate::compositor) fn associate_layer_surface_popup(
        &mut self,
        surface_id: u32,
        popup_surface_id: u32,
    ) -> Result<(), &'static str> {
        if !self
            .layer_surfaces
            .get(&surface_id)
            .is_some_and(|role| role.mapped && role.initial_configure_sent)
        {
            return Err("layer parent is not mapped");
        }
        let Some(popup_role) = self.popup_surfaces.get_mut(&popup_surface_id) else {
            return Err("popup is not registered");
        };
        if popup_role.parent_surface_id.is_some() {
            return Err("popup already has a parent");
        }
        popup_role.parent_surface_id = Some(surface_id);
        self.relink_popup_node(
            popup_surface_id,
            PopupOwner::LayerSurface(surface_id),
            surface_id,
        );
        self.set_surface_placement(
            popup_surface_id,
            SurfacePlacement::subsurface(surface_id, 0, 0),
        );
        self.reorder_renderable_surfaces_by_committed_stack();
        Ok(())
    }

    fn unregister_layer_surface_popups(&mut self, surface_id: u32) {
        self.dismiss_popup_children_for_parent(surface_id);
    }

    pub(in crate::compositor) fn destroy_layer_surface_role(&mut self, surface_id: u32) {
        self.teardown_layer_surface(surface_id);
        layer_shell_debug_log(|| format!("destroy surface={surface_id}"));
    }

    pub(in crate::compositor) fn teardown_layer_surface(&mut self, surface_id: u32) {
        if !self.layer_surfaces.contains_key(&surface_id) {
            return;
        }
        let previous_usable = self.reserved_usable_geometry();
        self.unregister_layer_surface_popups(surface_id);
        if let Some(role) = self.layer_surfaces.get_mut(&surface_id) {
            role.mapped = false;
            role.acked_configure = None;
            role.pending_configures.clear();
            role.last_configure_size = None;
            role.initial_configure_sent = false;
        }
        self.layer_surfaces.remove(&surface_id);
        self.deactivate_role_instance_if(surface_id, SurfaceRole::LayerSurface);
        self.unmap_surface_content(surface_id);
        self.arrange_layer_surfaces_and_reconfigure_stateful_windows_from(previous_usable);
        self.reorder_renderable_surfaces_by_committed_stack();
        self.recompute_layer_keyboard_focus();
    }

    pub(in crate::compositor) fn ack_layer_surface_configure(
        &mut self,
        surface_id: u32,
        serial: u32,
    ) {
        let Some(role) = self.layer_surfaces.get_mut(&surface_id) else {
            return;
        };
        let Some(position) = role
            .pending_configures
            .iter()
            .position(|configure| configure.serial == serial)
        else {
            layer_shell_debug_log(|| format!("ack surface={surface_id} serial={serial} unknown"));
            return;
        };
        let configure = {
            let mut acknowledged = role.pending_configures.drain(..=position);
            let Some(configure) = acknowledged.next_back() else {
                return;
            };
            configure
        };
        role.acked_configure = Some(configure);
        role.committed = configure.committed_state;
        role.geometry = Some(configure.geometry);
        layer_shell_debug_log(|| format!("ack surface={surface_id} serial={serial}"));
    }

    pub(in crate::compositor) fn set_layer_surface_pending_layer(
        &mut self,
        surface_id: u32,
        layer: Layer,
    ) {
        if let Some(role) = self.layer_surfaces.get_mut(&surface_id) {
            role.pending.layer = layer;
        }
    }

    pub(in crate::compositor) fn set_layer_surface_pending_size(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
    ) {
        if let Some(role) = self.layer_surfaces.get_mut(&surface_id) {
            role.pending.size = LayerRequestedSize { width, height };
        }
    }

    pub(in crate::compositor) fn set_layer_surface_pending_anchor(
        &mut self,
        surface_id: u32,
        anchors: LayerAnchors,
    ) {
        if let Some(role) = self.layer_surfaces.get_mut(&surface_id) {
            role.pending.anchors = anchors;
        }
    }

    pub(in crate::compositor) fn set_layer_surface_pending_margins(
        &mut self,
        surface_id: u32,
        margins: LayerMargins,
    ) {
        if let Some(role) = self.layer_surfaces.get_mut(&surface_id) {
            role.pending.margins = margins;
        }
    }

    pub(in crate::compositor) fn set_layer_surface_pending_exclusive_zone(
        &mut self,
        surface_id: u32,
        exclusive_zone: i32,
    ) {
        if let Some(role) = self.layer_surfaces.get_mut(&surface_id) {
            role.pending.exclusive_zone = exclusive_zone;
        }
    }

    pub(in crate::compositor) fn set_layer_surface_pending_keyboard_interactivity(
        &mut self,
        surface_id: u32,
        keyboard_interactivity: KeyboardInteractivity,
    ) {
        if let Some(role) = self.layer_surfaces.get_mut(&surface_id) {
            role.pending.keyboard_interactivity = keyboard_interactivity;
        }
    }

    fn commit_pending_layer_surface_state(&mut self, surface_id: u32) -> Option<bool> {
        let Some(previous) = self
            .layer_surfaces
            .get(&surface_id)
            .map(|role| role.committed)
        else {
            return Some(false);
        };
        let pending = self.layer_surfaces[&surface_id].pending;
        if let Err(message) = validate_layer_surface_size(pending) {
            let resource = self.layer_surfaces[&surface_id].resource.clone();
            self.note_protocol_error_metric();
            resource.post_error(zwlr_layer_surface_v1::Error::InvalidSize, message);
            return None;
        }
        let mut rerun_focus = false;
        let mapped_changed = previous != pending;
        if mapped_changed {
            self.layer_surface_order = self.layer_surface_order.saturating_add(1);
        }
        let order = self.layer_surface_order;
        let Some(role) = self.layer_surfaces.get_mut(&surface_id) else {
            return Some(false);
        };
        role.committed = pending;
        if role.mapped && mapped_changed {
            role.order = order;
            rerun_focus = previous.layer != pending.layer
                || previous.keyboard_interactivity != pending.keyboard_interactivity;
        }
        let committed_change = role.mapped && mapped_changed;
        if rerun_focus {
            self.recompute_layer_keyboard_focus();
        }
        Some(committed_change)
    }

    fn configure_layer_surface(&mut self, surface_id: u32) {
        let Some(geometry) = self.arranged_layer_geometry_for_surface(surface_id) else {
            return;
        };
        self.send_layer_surface_configure(surface_id, geometry);
    }

    fn send_layer_surface_configure(&mut self, surface_id: u32, geometry: LayerGeometry) {
        let serial = self.next_configure_serial();
        let Some(role) = self.layer_surfaces.get_mut(&surface_id) else {
            return;
        };
        let configure = PendingLayerConfigure {
            serial,
            committed_state: role.committed,
            geometry,
        };
        role.initial_configure_sent = true;
        role.geometry = Some(geometry);
        role.last_configure_size = Some(geometry.size());
        role.pending_configures.push_back(configure);
        role.resource
            .configure(serial, geometry.width, geometry.height);
        layer_shell_debug_log(|| {
            format!(
                "configure surface={surface_id} wl_surface={} namespace={} layer={:?} output={:?} version={} serial={serial} size={}x{}",
                role.surface.id().protocol_id(),
                role.namespace,
                role.committed.layer,
                role.output_id,
                role.version,
                geometry.width,
                geometry.height
            )
        });
    }

    pub(in crate::compositor) fn reconfigure_layer_surfaces_for_output_change(&mut self) {
        self.arrange_layer_surfaces();
    }

    pub(in crate::compositor) fn usable_output_geometry(&self) -> OutputRect {
        let usable = self.reserved_usable_geometry();
        OutputRect {
            x: f64::from(usable.x),
            y: f64::from(usable.y),
            width: f64::from(usable.width),
            height: f64::from(usable.height),
        }
    }

    fn reserved_usable_geometry(&self) -> LayerLayoutRect {
        let mut usable = LayerLayoutRect::from_output(self.output_size);
        let mut roles = self.layer_surfaces.values().collect::<Vec<_>>();
        roles.sort_by_key(|role| (role.committed.layer, role.order));
        for role in roles {
            if !role.mapped || role.committed.exclusive_zone <= 0 {
                continue;
            }
            if let Some(edge) = role.committed.exclusive_edge() {
                usable.reserve(edge, role.committed.reservation_extent(edge));
            }
        }
        usable
    }

    fn arranged_layer_geometry_for_surface(&self, target_surface_id: u32) -> Option<LayerGeometry> {
        self.arranged_layer_geometries(Some(target_surface_id))
            .remove(&target_surface_id)
    }

    fn arranged_layer_geometries(
        &self,
        include_unmapped_surface_id: Option<u32>,
    ) -> HashMap<u32, LayerGeometry> {
        let full = LayerLayoutRect::from_output(self.output_size);
        let mut usable = full;
        let is_active = |surface_id: u32, role: &LayerSurfaceRole| {
            role.mapped
                || role.initial_configure_sent
                || include_unmapped_surface_id == Some(surface_id)
        };

        let mut exclusive = self
            .layer_surfaces
            .iter()
            .filter_map(|(surface_id, role)| {
                if !role.mapped {
                    return None;
                }
                role.committed
                    .exclusive_edge()
                    .map(|edge| (*surface_id, role.committed, edge, role.order))
            })
            .collect::<Vec<_>>();
        exclusive.sort_by_key(|(_, state, _, order)| (state.layer, *order));

        let mut geometries = HashMap::new();
        for (surface_id, state, edge, _) in exclusive {
            let geometry = self.calculate_layer_geometry_in_rect(state, usable);
            geometries.insert(surface_id, geometry);
            usable.reserve(edge, state.reservation_extent(edge));
        }

        let mut remaining =
            self.layer_surfaces
                .iter()
                .filter_map(|(surface_id, role)| {
                    (is_active(*surface_id, role) && !geometries.contains_key(surface_id))
                        .then_some((*surface_id, role.committed, role.order))
                })
                .collect::<Vec<_>>();
        remaining.sort_by_key(|(_, state, order)| (state.layer, *order));
        for (surface_id, state, _) in remaining {
            let rect = if state.exclusive_zone == -1 {
                full
            } else {
                usable
            };
            geometries.insert(
                surface_id,
                self.calculate_layer_geometry_in_rect(state, rect),
            );
        }

        geometries
    }

    fn arrange_layer_surfaces(&mut self) {
        let mut geometries = self
            .arranged_layer_geometries(None)
            .into_iter()
            .collect::<Vec<_>>();
        geometries.sort_by_key(|(surface_id, _)| *surface_id);
        for (surface_id, geometry) in geometries {
            if let Some(role) = self.layer_surfaces.get_mut(&surface_id) {
                role.geometry = Some(geometry);
            }
            self.set_surface_placement(
                surface_id,
                SurfacePlacement::absolute_root_at(geometry.x, geometry.y),
            );
            if self.layer_surface_needs_size_configure(surface_id, geometry) {
                self.send_layer_surface_configure(surface_id, geometry);
            }
            layer_shell_debug_log(|| {
                format!(
                    "arrange surface={surface_id} geometry={},{} {}x{}",
                    geometry.x, geometry.y, geometry.width, geometry.height
                )
            });
        }
        let usable = self.reserved_usable_geometry();
        layer_shell_debug_log(|| {
            format!(
                "usable geometry={},{} {}x{}",
                usable.x, usable.y, usable.width, usable.height
            )
        });
    }

    fn arrange_layer_surfaces_and_reconfigure_stateful_windows_from(
        &mut self,
        previous_usable: LayerLayoutRect,
    ) {
        self.arrange_layer_surfaces();
        if self.reserved_usable_geometry() != previous_usable {
            self.reconfigure_stateful_windows_for_output_size();
        }
    }

    fn layer_surface_needs_size_configure(&self, surface_id: u32, geometry: LayerGeometry) -> bool {
        let Some(role) = self.layer_surfaces.get(&surface_id) else {
            return false;
        };
        if !role.mapped && !role.initial_configure_sent {
            return false;
        }
        role.pending_configures
            .back()
            .map(|configure| configure.geometry.size())
            .or(role.last_configure_size)
            .is_some_and(|size| size != geometry.size())
    }

    fn calculate_layer_geometry_in_rect(
        &self,
        state: LayerSurfaceCommitState,
        rect: LayerLayoutRect,
    ) -> LayerGeometry {
        let output_width = i64::from(rect.width);
        let output_height = i64::from(rect.height);
        let margins = state.margins;
        let horizontal_margins = i64::from(margins.left).saturating_add(i64::from(margins.right));
        let vertical_margins = i64::from(margins.top).saturating_add(i64::from(margins.bottom));
        let available_width = output_width.saturating_sub(horizontal_margins).max(1);
        let available_height = output_height.saturating_sub(vertical_margins).max(1);

        let stretch_width = state.size.width == 0 && state.anchors.horizontal_opposite();
        let stretch_height = state.size.height == 0 && state.anchors.vertical_opposite();
        let width = if stretch_width {
            available_width
        } else {
            i64::from(state.size.width)
        }
        .clamp(1, output_width.max(1));
        let height = if stretch_height {
            available_height
        } else {
            i64::from(state.size.height)
        }
        .clamp(1, output_height.max(1));

        let x = if stretch_width {
            i64::from(rect.x).saturating_add(i64::from(margins.left))
        } else if state.anchors.horizontal_opposite() {
            i64::from(rect.x).saturating_add(output_width.saturating_sub(width) / 2)
        } else if state.anchors.left {
            i64::from(rect.x).saturating_add(i64::from(margins.left))
        } else if state.anchors.right {
            i64::from(rect.x)
                .saturating_add(output_width)
                .saturating_sub(width)
                .saturating_sub(i64::from(margins.right))
        } else {
            i64::from(rect.x).saturating_add(output_width.saturating_sub(width) / 2)
        };
        let y = if stretch_height {
            i64::from(rect.y).saturating_add(i64::from(margins.top))
        } else if state.anchors.vertical_opposite() {
            i64::from(rect.y).saturating_add(output_height.saturating_sub(height) / 2)
        } else if state.anchors.top {
            i64::from(rect.y).saturating_add(i64::from(margins.top))
        } else if state.anchors.bottom {
            i64::from(rect.y)
                .saturating_add(output_height)
                .saturating_sub(height)
                .saturating_sub(i64::from(margins.bottom))
        } else {
            i64::from(rect.y).saturating_add(output_height.saturating_sub(height) / 2)
        };

        LayerGeometry {
            x: x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            y: y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            width: width as u32,
            height: height as u32,
        }
    }

    pub(in crate::compositor) fn recompute_layer_keyboard_focus(&mut self) {
        let winner = self.active_exclusive_layer_surface_id();
        if let Some(winner) = winner {
            let Some(surface) = self.surface_resource_by_id(winner) else {
                return;
            };
            if !self.focused_surface_is_layer_surface() {
                self.last_application_keyboard_focus = self.focused_surface.clone();
            }
            self.exclusive_keyboard_layer_surface = Some(winner);
            self.focused_surface = Some(surface.clone());
            self.ensure_keyboard_focus(&surface);
            layer_shell_debug_log(|| format!("focus_take surface={winner}"));
            return;
        }

        if self.exclusive_keyboard_layer_surface.take().is_some() {
            let previous = self
                .last_application_keyboard_focus
                .clone()
                .filter(Resource::is_alive)
                .filter(|surface| {
                    let root = self.root_surface_id_for_surface(compositor_surface_id(surface));
                    !self.layer_surfaces.contains_key(&root)
                        && self.toplevel_surfaces.contains_key(&root)
                });
            if let Some(previous) = previous {
                self.focused_surface = Some(previous.clone());
                self.ensure_keyboard_focus(&previous);
            } else {
                self.focused_surface = None;
                self.focused_window_id = None;
                self.clear_keyboard_focus();
                let _ = self.focus_topmost_renderable_toplevel();
            }
            layer_shell_debug_log(|| "focus_restore".to_string());
        }
    }

    pub(in crate::compositor) fn active_exclusive_layer_surface_id(&self) -> Option<u32> {
        self.layer_surfaces
            .iter()
            .filter(|(_, role)| {
                role.mapped
                    && role.committed.keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && role.surface.is_alive()
            })
            .max_by_key(|(_, role)| (role.committed.layer.scene_rank(), role.order))
            .map(|(surface_id, _)| *surface_id)
    }

    fn focused_surface_is_layer_surface(&self) -> bool {
        self.focused_surface.as_ref().is_some_and(|focused| {
            let root_id = self.root_surface_id_for_surface(compositor_surface_id(focused));
            self.layer_surfaces.contains_key(&root_id)
        })
    }

    pub(in crate::compositor) fn activate_ondemand_layer_surface(
        &mut self,
        root_surface_id: u32,
    ) -> bool {
        if self.active_exclusive_layer_surface_id().is_some() {
            return self.layer_surfaces.contains_key(&root_surface_id);
        }
        let Some(role) = self.layer_surfaces.get(&root_surface_id) else {
            return false;
        };
        if !role.mapped || role.committed.keyboard_interactivity != KeyboardInteractivity::OnDemand
        {
            return true;
        }
        let surface = role.surface.clone();
        self.last_application_keyboard_focus = self
            .focused_surface
            .clone()
            .filter(|focused| !same_surface_resource(focused, &surface));
        self.focused_surface = Some(surface.clone());
        self.ensure_keyboard_focus(&surface);
        true
    }

    pub(in crate::compositor) fn activate_layer_surface_from_activation(
        &mut self,
        root_surface_id: u32,
    ) -> bool {
        let Some(role) = self.layer_surfaces.get(&root_surface_id) else {
            return false;
        };
        if !role.mapped || role.committed.keyboard_interactivity != KeyboardInteractivity::OnDemand
        {
            return false;
        }
        self.activate_ondemand_layer_surface(root_surface_id)
    }

    pub(in crate::compositor) fn surface_scene_rank(&self, surface_id: u32) -> u8 {
        let root_id = self.root_surface_id_for_surface(surface_id);
        if let Some(layer) = self
            .layer_surfaces
            .get(&root_id)
            .map(|role| role.committed.layer.scene_rank())
        {
            return layer;
        }
        2
    }

    pub(in crate::compositor) fn surface_scene_order(&self, surface_id: u32) -> u64 {
        let root_id = self.root_surface_id_for_surface(surface_id);
        self.layer_surfaces
            .get(&root_id)
            .map(|role| role.order)
            .unwrap_or(0)
    }

    pub(in crate::compositor) fn external_overlay_surface_ids(&self) -> Vec<u32> {
        self.renderable_surfaces
            .iter()
            .filter_map(|surface| {
                let root_id = self.root_surface_id_for_surface(surface.surface_id);
                self.layer_surfaces
                    .get(&root_id)
                    .is_some_and(|role| role.committed.layer == Layer::Overlay)
                    .then_some(surface.surface_id)
            })
            .collect()
    }
}

pub(super) fn layer_from_protocol(
    layer: WEnum<zwlr_layer_shell_v1::Layer>,
) -> Result<Layer, zwlr_layer_shell_v1::Error> {
    match layer {
        WEnum::Value(zwlr_layer_shell_v1::Layer::Background) => Ok(Layer::Background),
        WEnum::Value(zwlr_layer_shell_v1::Layer::Bottom) => Ok(Layer::Bottom),
        WEnum::Value(zwlr_layer_shell_v1::Layer::Top) => Ok(Layer::Top),
        WEnum::Value(zwlr_layer_shell_v1::Layer::Overlay) => Ok(Layer::Overlay),
        WEnum::Value(_) | WEnum::Unknown(_) => Err(zwlr_layer_shell_v1::Error::InvalidLayer),
    }
}

pub(super) fn anchors_from_protocol(
    anchors: WEnum<zwlr_layer_surface_v1::Anchor>,
) -> Option<LayerAnchors> {
    let WEnum::Value(anchors) = anchors else {
        return None;
    };
    Some(LayerAnchors {
        top: anchors.contains(zwlr_layer_surface_v1::Anchor::Top),
        bottom: anchors.contains(zwlr_layer_surface_v1::Anchor::Bottom),
        left: anchors.contains(zwlr_layer_surface_v1::Anchor::Left),
        right: anchors.contains(zwlr_layer_surface_v1::Anchor::Right),
    })
}

pub(super) fn keyboard_interactivity_from_protocol(
    keyboard_interactivity: WEnum<zwlr_layer_surface_v1::KeyboardInteractivity>,
) -> Result<KeyboardInteractivity, zwlr_layer_surface_v1::Error> {
    match keyboard_interactivity {
        WEnum::Value(zwlr_layer_surface_v1::KeyboardInteractivity::None) => {
            Ok(KeyboardInteractivity::None)
        }
        WEnum::Value(zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive) => {
            Ok(KeyboardInteractivity::Exclusive)
        }
        WEnum::Value(zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand) => {
            Ok(KeyboardInteractivity::OnDemand)
        }
        WEnum::Value(_) | WEnum::Unknown(_) => {
            Err(zwlr_layer_surface_v1::Error::InvalidKeyboardInteractivity)
        }
    }
}

fn validate_layer_surface_size(state: LayerSurfaceCommitState) -> Result<(), String> {
    if state.size.width == 0 && !(state.anchors.left && state.anchors.right) {
        return Err("width 0 requires left and right anchors".to_string());
    }
    if state.size.height == 0 && !(state.anchors.top && state.anchors.bottom) {
        return Err("height 0 requires top and bottom anchors".to_string());
    }
    Ok(())
}

pub(super) fn layer_shell_debug_log(message: impl FnOnce() -> String) {
    if std::env::var_os("OBLIVION_ONE_LAYER_SHELL_DEBUG").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        eprintln!("oblivion-one layer-shell: {}", message());
    }
}
