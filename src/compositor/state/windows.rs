use super::*;

impl CompositorState {
    pub(in crate::compositor) fn register_toplevel_surface(
        &mut self,
        surface: wl_surface::WlSurface,
        xdg_surface: xdg_surface::XdgSurface,
        toplevel: xdg_toplevel::XdgToplevel,
    ) {
        let surface_id = compositor_surface_id(&surface);
        if self.is_cursor_surface(surface_id) {
            pointer_debug_log(format!(
                "cursor surface role isolation surface={} rejected=xdg-toplevel",
                surface_id
            ));
            return;
        }
        self.configured_xdg_surfaces.remove(&surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
        self.toplevel_surfaces.insert(
            surface_id,
            ToplevelSurface {
                app_id: None,
                xdg_surface,
                toplevel,
                window: WindowState::default(),
                constraints: Default::default(),
            },
        );
        self.set_surface_placement(surface_id, SurfacePlacement::root());
        self.focus_surface(surface);
    }

    pub(in crate::compositor) fn register_popup_surface(
        &mut self,
        surface: wl_surface::WlSurface,
        parent: Option<wl_surface::WlSurface>,
        xdg_surface: xdg_surface::XdgSurface,
        popup: xdg_popup::XdgPopup,
        positioner: XdgPositionerState,
    ) {
        let surface_id = compositor_surface_id(&surface);
        if self.is_cursor_surface(surface_id) {
            pointer_debug_log(format!(
                "cursor surface role isolation surface={} rejected=xdg-popup",
                surface_id
            ));
            return;
        }
        self.configured_xdg_surfaces.remove(&surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
        let parent_owner = parent
            .as_ref()
            .map(compositor_surface_id)
            .map(|parent_id| self.popup_owner_for_parent(parent_id))
            .unwrap_or(PopupOwner::Toplevel(surface_id));
        let owner_root_id = match parent_owner {
            PopupOwner::Popup(parent_id) => self
                .popup_nodes
                .get(&parent_id)
                .map(|node| node.owner_root_id)
                .unwrap_or(parent_id),
            PopupOwner::LayerSurface(parent_id) | PopupOwner::Toplevel(parent_id) => parent_id,
        };
        self.popup_surfaces.insert(
            surface_id,
            PopupSurface {
                parent_surface_id: parent.as_ref().map(compositor_surface_id),
                xdg_surface,
                popup,
                positioner,
            },
        );
        self.attach_popup_node(
            surface_id,
            PopupNode {
                owner_root_id,
                parent: parent_owner,
                children: Vec::new(),
                lifecycle: PopupLifecycle::Alive,
                mapped: false,
                configured: false,
                popup_done_sent: false,
                grab_generation: None,
            },
        );
        popup_debug_log(|| {
            format!(
                "popup_create popup={surface_id} owner_root={owner_root_id} parent={parent_owner:?}"
            )
        });
        self.note_xdg_popup_created();
    }

    pub(in crate::compositor) fn unregister_toplevel_surface(&mut self, surface_id: u32) {
        self.unmap_xdg_role_surfaces(surface_id);
        self.toplevel_surfaces.remove(&surface_id);
        self.surface_placements.remove(&surface_id);
        self.configured_xdg_surfaces.remove(&surface_id);
        self.xdg_configure_serials.remove(&surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
    }

    pub(in crate::compositor) fn unregister_xdg_surface_role(&mut self, surface_id: u32) {
        self.destroy_popup_children_for_parent(surface_id);

        self.unregister_toplevel_surface(surface_id);
        self.unregister_popup_surface(surface_id);
        self.surface_window_geometries.remove(&surface_id);
        self.pending_surface_window_geometries.remove(&surface_id);
        self.configured_xdg_surfaces.remove(&surface_id);
        self.surface_placements.remove(&surface_id);
        self.clear_popup_grab_for_surface_ids(&[surface_id]);
        self.popup_grab_stack.retain(|id| *id != surface_id);
        self.recent_input_serials
            .retain(|input| compositor_surface_id(&input.surface) != surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
    }

    pub(in crate::compositor) fn grab_popup_surface(
        &mut self,
        surface: &wl_surface::WlSurface,
        seat: &wl_seat::WlSeat,
        serial: u32,
    ) -> bool {
        let surface_id = compositor_surface_id(surface);
        if !self.popup_node_is_alive(surface_id)
            || !resource_belongs_to_surface_client(seat, surface)
            || !self.has_recent_input_serial_for_surface(serial, surface)
        {
            self.dismiss_popup_surface(surface_id);
            return false;
        }

        let Some(owner_client) = surface.client().map(|client| client.id()) else {
            self.dismiss_popup_surface(surface_id);
            return false;
        };
        let Some(owner_root_id) = self
            .popup_nodes
            .get(&surface_id)
            .map(|node| node.owner_root_id)
        else {
            self.dismiss_popup_surface(surface_id);
            return false;
        };
        let tree_root_popup_id = self.popup_tree_root(surface_id);
        if let Some(active) = &self.popup_grab
            && (active.owner_client != owner_client || active.owner_root_id != owner_root_id)
        {
            self.dismiss_popup_surface(active.tree_root_popup_id);
        }
        self.next_popup_grab_generation = self.next_popup_grab_generation.saturating_add(1);
        let generation = self.next_popup_grab_generation;
        if let Some(node) = self.popup_nodes.get_mut(&surface_id) {
            node.grab_generation = Some(generation);
        }
        self.popup_grab = Some(PopupGrab {
            owner_client,
            owner_root_id,
            tree_root_popup_id,
            focused_popup_id: surface_id,
            serial,
            generation,
        });
        self.popup_grab_stack.retain(|id| *id != surface_id);
        self.popup_grab_stack.push(surface_id);
        self.focus_surface(surface.clone());
        popup_debug_log(|| {
            format!(
                "popup_grab popup={surface_id} owner_root={owner_root_id} generation={generation} serial={serial}"
            )
        });
        true
    }

    pub(in crate::compositor) fn unregister_popup_surface(&mut self, surface_id: u32) {
        self.destroy_popup_role(surface_id);
    }

    fn destroy_popup_role(&mut self, surface_id: u32) {
        let children = self
            .popup_nodes
            .get(&surface_id)
            .map(|node| node.children.clone())
            .unwrap_or_default();
        for child_surface_id in children {
            self.destroy_popup_role(child_surface_id);
        }
        let parent_surface_id = self
            .popup_surfaces
            .get(&surface_id)
            .and_then(|popup| popup.parent_surface_id);
        self.unmap_xdg_role_surfaces(surface_id);
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| compositor_surface_id(surface) == surface_id)
        {
            self.clear_pointer_focus();
        }
        self.clear_popup_grab_for_surface_ids(&[surface_id]);
        self.popup_grab_stack.retain(|id| *id != surface_id);
        self.recent_input_serials
            .retain(|input| compositor_surface_id(&input.surface) != surface_id);
        self.popup_surfaces.remove(&surface_id);
        self.detach_popup_node(surface_id, PopupLifecycle::Destroyed);
        self.surface_placements.remove(&surface_id);
        self.configured_xdg_surfaces.remove(&surface_id);
        self.surface_window_geometries.remove(&surface_id);
        self.pending_surface_window_geometries.remove(&surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
        popup_debug_log(|| format!("popup_destroy_role popup={surface_id}"));
        if self
            .focused_surface
            .as_ref()
            .is_some_and(|surface| compositor_surface_id(surface) == surface_id)
        {
            if let Some(parent_surface) =
                parent_surface_id.and_then(|parent_id| self.surface_resource_by_id(parent_id))
            {
                self.focus_surface(parent_surface);
            } else {
                self.focused_surface = None;
                if self
                    .keyboard_surface
                    .as_ref()
                    .is_some_and(|surface| compositor_surface_id(surface) == surface_id)
                {
                    self.clear_keyboard_focus();
                }
                let _ = self.focus_topmost_renderable_toplevel();
            }
        }
    }

    pub(in crate::compositor) fn dismiss_popup_surface(&mut self, surface_id: u32) -> bool {
        let Some(node) = self.popup_nodes.get(&surface_id).cloned() else {
            return false;
        };
        if node.lifecycle == PopupLifecycle::Destroyed {
            return false;
        }
        let child_popup_ids = node.children.clone();
        for child_surface_id in child_popup_ids {
            self.dismiss_popup_surface(child_surface_id);
        }

        if let Some(popup_node) = self.popup_nodes.get_mut(&surface_id) {
            popup_node.lifecycle = PopupLifecycle::Inert;
            popup_node.mapped = false;
        }
        if !node.popup_done_sent {
            if let Some(popup_surface) = self.popup_surfaces.get(&surface_id) {
                let _ = popup_surface.popup.send_event(xdg_popup::Event::PopupDone);
            }
            if let Some(popup_node) = self.popup_nodes.get_mut(&surface_id) {
                popup_node.popup_done_sent = true;
            }
        }
        self.unmap_xdg_role_surfaces(surface_id);
        self.clear_popup_grab_for_surface_ids(&[surface_id]);
        popup_debug_log(|| {
            format!(
                "popup_dismiss popup={surface_id} owner_root={} mapped=false inert=true",
                node.owner_root_id
            )
        });
        true
    }

    pub(in crate::compositor) fn popup_grab_to_dismiss_for_pointer_target(
        &self,
        target: Option<&PointerTarget>,
    ) -> Option<u32> {
        let popup_surface_id = self.topmost_popup_grab_surface_id()?;
        if let Some(target) = target {
            let target_surface_id = compositor_surface_id(&target.surface);
            if self.surface_is_descendant_of(target_surface_id, popup_surface_id) {
                return None;
            }
        }

        Some(popup_surface_id)
    }

    pub(in crate::compositor) fn pointer_target_allowed_by_popup_grab(
        &self,
        target: &PointerTarget,
    ) -> bool {
        let Some(popup_surface_id) = self.topmost_popup_grab_surface_id() else {
            return true;
        };
        let target_surface_id = compositor_surface_id(&target.surface);
        self.surface_is_descendant_of(target_surface_id, popup_surface_id)
    }

    pub(in crate::compositor) fn topmost_popup_grab_surface_id(&self) -> Option<u32> {
        self.popup_grab
            .as_ref()
            .map(|grab| grab.focused_popup_id)
            .filter(|surface_id| self.popup_node_is_alive(*surface_id))
            .or_else(|| {
                self.popup_grab_stack
                    .iter()
                    .rev()
                    .copied()
                    .find(|surface_id| self.popup_node_is_alive(*surface_id))
            })
    }

    pub(in crate::compositor) fn surface_is_descendant_of(
        &self,
        surface_id: u32,
        ancestor_surface_id: u32,
    ) -> bool {
        let mut current = surface_id;
        for _ in 0..self.surface_placements.len().saturating_add(1) {
            if current == ancestor_surface_id {
                return true;
            }
            let Some(parent_surface_id) = self
                .surface_placements
                .get(&current)
                .copied()
                .and_then(|placement| placement.parent_surface_id)
            else {
                return false;
            };
            if parent_surface_id == current {
                return false;
            }
            current = parent_surface_id;
        }

        false
    }

    fn popup_owner_for_parent(&self, parent_id: u32) -> PopupOwner {
        if self.popup_nodes.contains_key(&parent_id) {
            PopupOwner::Popup(parent_id)
        } else if self.layer_surfaces.contains_key(&parent_id) {
            PopupOwner::LayerSurface(parent_id)
        } else {
            PopupOwner::Toplevel(parent_id)
        }
    }

    pub(in crate::compositor) fn popup_node_is_alive(&self, surface_id: u32) -> bool {
        self.popup_nodes.get(&surface_id).is_some_and(|node| {
            node.lifecycle == PopupLifecycle::Alive && self.popup_surfaces.contains_key(&surface_id)
        })
    }

    fn popup_tree_root(&self, surface_id: u32) -> u32 {
        let mut current = surface_id;
        for _ in 0..self.popup_nodes.len().saturating_add(1) {
            let Some(node) = self.popup_nodes.get(&current) else {
                return current;
            };
            match node.parent {
                PopupOwner::Popup(parent_id) if parent_id != current => current = parent_id,
                _ => return current,
            }
        }
        surface_id
    }

    fn attach_popup_node(&mut self, surface_id: u32, node: PopupNode) {
        if let Some(old_node) = self.popup_nodes.get(&surface_id).cloned() {
            self.unlink_popup_from_parent(surface_id, old_node.parent);
        }
        if let PopupOwner::Popup(parent_id) = node.parent
            && self.popup_would_create_cycle(surface_id, parent_id)
        {
            popup_debug_log(|| {
                format!(
                    "popup_associate popup={surface_id} parent_popup={parent_id} rejected=cycle"
                )
            });
            return;
        }
        let parent = node.parent;
        self.popup_nodes.insert(surface_id, node);
        self.link_popup_to_parent(surface_id, parent);
    }

    pub(in crate::compositor) fn relink_popup_node(
        &mut self,
        surface_id: u32,
        parent: PopupOwner,
        owner_root_id: u32,
    ) {
        let Some(old_parent) = self.popup_nodes.get(&surface_id).map(|node| node.parent) else {
            return;
        };
        if let PopupOwner::Popup(parent_id) = parent
            && self.popup_would_create_cycle(surface_id, parent_id)
        {
            popup_debug_log(|| {
                format!(
                    "popup_associate popup={surface_id} parent_popup={parent_id} rejected=cycle"
                )
            });
            return;
        }
        self.unlink_popup_from_parent(surface_id, old_parent);
        if let Some(node) = self.popup_nodes.get_mut(&surface_id) {
            node.parent = parent;
            node.owner_root_id = owner_root_id;
        }
        self.link_popup_to_parent(surface_id, parent);
        popup_debug_log(|| {
            format!(
                "popup_associate popup={surface_id} owner_root={owner_root_id} parent={parent:?}"
            )
        });
    }

    fn link_popup_to_parent(&mut self, surface_id: u32, parent: PopupOwner) {
        if let PopupOwner::Popup(parent_id) = parent
            && let Some(parent_node) = self.popup_nodes.get_mut(&parent_id)
            && !parent_node.children.contains(&surface_id)
        {
            parent_node.children.push(surface_id);
        }
    }

    fn unlink_popup_from_parent(&mut self, surface_id: u32, parent: PopupOwner) {
        if let PopupOwner::Popup(parent_id) = parent
            && let Some(parent_node) = self.popup_nodes.get_mut(&parent_id)
        {
            parent_node
                .children
                .retain(|child_id| *child_id != surface_id);
        }
    }

    fn popup_would_create_cycle(&self, surface_id: u32, parent_id: u32) -> bool {
        let mut current = parent_id;
        for _ in 0..self.popup_nodes.len().saturating_add(1) {
            if current == surface_id {
                return true;
            }
            let Some(parent_node) = self.popup_nodes.get(&current) else {
                return false;
            };
            match parent_node.parent {
                PopupOwner::Popup(next_id) if next_id != current => current = next_id,
                _ => return false,
            }
        }
        true
    }

    fn detach_popup_node(&mut self, surface_id: u32, lifecycle: PopupLifecycle) {
        let Some(mut node) = self.popup_nodes.remove(&surface_id) else {
            return;
        };
        node.lifecycle = lifecycle;
        self.unlink_popup_from_parent(surface_id, node.parent);
        self.clear_popup_grab_for_surface_ids(&[surface_id]);
    }

    pub(in crate::compositor) fn dismiss_popup_children_for_parent(
        &mut self,
        parent_surface_id: u32,
    ) {
        let child_popup_ids = self
            .popup_nodes
            .iter()
            .filter_map(|(popup_surface_id, node)| {
                (node.parent.surface_id() == parent_surface_id).then_some(*popup_surface_id)
            })
            .collect::<Vec<_>>();
        for popup_surface_id in child_popup_ids {
            self.dismiss_popup_surface(popup_surface_id);
        }
    }

    fn destroy_popup_children_for_parent(&mut self, parent_surface_id: u32) {
        let child_popup_ids = self
            .popup_nodes
            .iter()
            .filter_map(|(popup_surface_id, node)| {
                (node.parent.surface_id() == parent_surface_id).then_some(*popup_surface_id)
            })
            .collect::<Vec<_>>();
        for popup_surface_id in child_popup_ids {
            self.destroy_popup_role(popup_surface_id);
        }
    }

    pub(in crate::compositor) fn clear_popup_grab_for_surface_ids(&mut self, surface_ids: &[u32]) {
        let active = self.popup_grab.as_ref().is_some_and(|grab| {
            surface_ids.contains(&grab.focused_popup_id)
                || surface_ids.contains(&grab.tree_root_popup_id)
                || surface_ids.contains(&grab.owner_root_id)
        });
        if active && let Some(grab) = self.popup_grab.take() {
            popup_debug_log(|| {
                format!(
                    "popup_grab_clear owner_root={} tree_root={} focused={} serial={} generation={}",
                    grab.owner_root_id,
                    grab.tree_root_popup_id,
                    grab.focused_popup_id,
                    grab.serial,
                    grab.generation
                )
            });
        }
        self.popup_grab_stack
            .retain(|surface_id| !surface_ids.contains(surface_id));
    }

    pub(in crate::compositor) fn configure_popup_surface(
        &mut self,
        surface_id: u32,
        positioner: XdgPositionerState,
        reposition_token: Option<u32>,
    ) -> bool {
        if !self.popup_node_is_alive(surface_id) {
            return false;
        }
        if let Some(popup_surface) = self.popup_surfaces.get_mut(&surface_id) {
            popup_surface.positioner = positioner;
        }
        let Some(popup_surface) = self.popup_surfaces.get(&surface_id).cloned() else {
            return false;
        };
        let geometry = positioner
            .constrained_geometry(self.popup_constraint_target(&popup_surface, positioner));
        let placement = self.store_popup_surface_placement(surface_id, &popup_surface, geometry);
        if let Some(token) = reposition_token {
            let _ = popup_surface
                .popup
                .send_event(xdg_popup::Event::Repositioned { token });
        }
        if let Err(error) = popup_surface.popup.send_event(xdg_popup::Event::Configure {
            x: geometry.x,
            y: geometry.y,
            width: geometry.width,
            height: geometry.height,
        }) && compositor_debug_surface_logging_enabled()
        {
            eprintln!("oblivion-one compositor: failed to send popup configure: {error:?}");
        }
        let serial = self.next_configure_serial();
        if let Err(error) = popup_surface
            .xdg_surface
            .send_event(xdg_surface::Event::Configure { serial })
            && compositor_debug_surface_logging_enabled()
        {
            eprintln!(
                "oblivion-one compositor: failed to send popup xdg_surface configure serial={serial}: {error:?}"
            );
        }
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: popup surface {surface_id} configured xdg={}x{}+{},{} placement={},{} parent={:?}",
                geometry.width,
                geometry.height,
                geometry.x,
                geometry.y,
                placement.local_x,
                placement.local_y,
                placement.parent_surface_id
            );
        }
        if let Some(node) = self.popup_nodes.get_mut(&surface_id) {
            node.configured = true;
        }
        popup_debug_log(|| {
            format!(
                "popup_configure popup={surface_id} owner_root={} placement={},{} size={}x{}",
                self.popup_nodes
                    .get(&surface_id)
                    .map(|node| node.owner_root_id)
                    .unwrap_or(surface_id),
                placement.local_x,
                placement.local_y,
                geometry.width,
                geometry.height
            )
        });
        true
    }

    pub(in crate::compositor) fn update_popup_surface_placement_from_committed_state(
        &mut self,
        surface_id: u32,
    ) -> bool {
        if !self.popup_node_is_alive(surface_id) {
            return false;
        }
        let Some(popup_surface) = self.popup_surfaces.get(&surface_id).cloned() else {
            return false;
        };
        let geometry = popup_surface.positioner.constrained_geometry(
            self.popup_constraint_target(&popup_surface, popup_surface.positioner),
        );
        self.store_popup_surface_placement(surface_id, &popup_surface, geometry);
        true
    }

    fn store_popup_surface_placement(
        &mut self,
        surface_id: u32,
        popup_surface: &PopupSurface,
        geometry: PopupRect,
    ) -> SurfacePlacement {
        let parent_window_geometry = popup_surface.parent_surface_id.and_then(|surface_id| {
            self.surface_window_geometries
                .get(&surface_id)
                .copied()
                .or_else(|| {
                    self.pending_surface_window_geometries
                        .get(&surface_id)
                        .copied()
                })
        });
        let popup_window_geometry = self
            .surface_window_geometries
            .get(&surface_id)
            .copied()
            .or_else(|| {
                self.pending_surface_window_geometries
                    .get(&surface_id)
                    .copied()
            });
        let local_x = parent_window_geometry
            .map(|geometry| geometry.x)
            .unwrap_or_default()
            .saturating_add(geometry.x)
            .saturating_sub(
                popup_window_geometry
                    .map(|geometry| geometry.x)
                    .unwrap_or_default(),
            );
        let local_y = parent_window_geometry
            .map(|geometry| geometry.y)
            .unwrap_or_default()
            .saturating_add(geometry.y)
            .saturating_sub(
                popup_window_geometry
                    .map(|geometry| geometry.y)
                    .unwrap_or_default(),
            );
        let placement = popup_surface
            .parent_surface_id
            .map(|parent_surface_id| {
                SurfacePlacement::subsurface(parent_surface_id, local_x, local_y)
            })
            .unwrap_or_else(|| SurfacePlacement::root_at(local_x, local_y));
        self.store_surface_placement(surface_id, placement);
        placement
    }

    pub(in crate::compositor) fn configure_xdg_surface_if_needed(
        &mut self,
        surface_id: u32,
    ) -> bool {
        if self.configured_xdg_surfaces.contains(&surface_id) {
            return false;
        }

        if let Some(toplevel) = self.toplevel_surfaces.get(&surface_id).cloned() {
            if let Err(error) = toplevel
                .toplevel
                .send_event(xdg_toplevel::Event::Configure {
                    width: 0,
                    height: 0,
                    states: Vec::new(),
                })
                && compositor_debug_surface_logging_enabled()
            {
                eprintln!("oblivion-one compositor: failed to send toplevel configure: {error:?}");
            }
            let serial = self.next_configure_serial();
            if let Err(error) = toplevel
                .xdg_surface
                .send_event(xdg_surface::Event::Configure { serial })
                && compositor_debug_surface_logging_enabled()
            {
                eprintln!(
                    "oblivion-one compositor: failed to send toplevel xdg_surface configure serial={serial}: {error:?}"
                );
            }
            self.configured_xdg_surfaces.insert(surface_id);
            return true;
        }

        let Some(positioner) = self
            .popup_surfaces
            .get(&surface_id)
            .map(|popup| popup.positioner)
        else {
            return false;
        };
        if self.configure_popup_surface(surface_id, positioner, None) {
            self.configured_xdg_surfaces.insert(surface_id);
            return true;
        }

        false
    }

    pub(in crate::compositor) fn popup_constraint_target(
        &self,
        popup_surface: &PopupSurface,
        positioner: XdgPositionerState,
    ) -> PopupRect {
        if let Some((width, height)) = positioner.parent_size {
            return PopupRect::new(0, 0, width, height);
        }

        if let Some(surface_id) = popup_surface.parent_surface_id
            && let Some(geometry) = self.surface_window_geometries.get(&surface_id).copied()
        {
            return PopupRect::new(0, 0, geometry.width, geometry.height);
        }

        if let Some(surface_id) = popup_surface.parent_surface_id
            && let Some(surface) = self
                .renderable_surfaces
                .iter()
                .find(|surface| surface.surface_id == surface_id)
        {
            return PopupRect::new(0, 0, surface.width as i32, surface.height as i32);
        }

        PopupRect::new(
            0,
            0,
            self.output_size.width as i32,
            self.output_size.height as i32,
        )
    }

    pub(in crate::compositor) fn begin_window_move_at(&mut self, x: f64, y: f64) -> bool {
        self.begin_window_interaction_at(x, y, WindowInteractionKind::Move)
    }

    pub(in crate::compositor) fn begin_window_resize_at(&mut self, x: f64, y: f64) -> bool {
        let Some(surface_id) = self.surface_id_at(x, y) else {
            self.window_interaction = None;
            return false;
        };
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        let Some((local_x, local_y, width, height)) =
            self.root_window_local_point_at(root_surface_id, x, y)
        else {
            return self.begin_window_interaction_for_root(
                root_surface_id,
                x,
                y,
                WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
            );
        };
        let edges = resize_edges_for_window_point(local_x, local_y, width, height);
        self.begin_window_interaction_for_root(
            root_surface_id,
            x,
            y,
            WindowInteractionKind::Resize(edges),
        )
    }

    pub(in crate::compositor) fn begin_window_frame_action_at(&mut self, x: f64, y: f64) -> bool {
        let Some(hit) = self.window_frame_hit_at(x, y) else {
            return false;
        };
        self.begin_window_interaction_for_root(hit.root_surface_id, x, y, hit.kind)
    }

    pub(in crate::compositor) fn begin_window_interaction_at(
        &mut self,
        x: f64,
        y: f64,
        kind: WindowInteractionKind,
    ) -> bool {
        let Some(surface_id) = self.surface_id_at(x, y) else {
            self.window_interaction = None;
            return false;
        };
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        self.begin_window_interaction_for_root(root_surface_id, x, y, kind)
    }

    pub(in crate::compositor) fn begin_client_window_move(
        &mut self,
        surface: &wl_surface::WlSurface,
        serial: u32,
    ) -> bool {
        let root_surface_id = self.root_surface_id_for_surface(compositor_surface_id(surface));
        let Some((x, y)) = self.valid_pointer_press_for_surface(root_surface_id, surface, serial)
        else {
            return false;
        };
        self.begin_window_interaction_for_root(root_surface_id, x, y, WindowInteractionKind::Move)
    }

    pub(in crate::compositor) fn begin_client_window_resize(
        &mut self,
        surface: &wl_surface::WlSurface,
        serial: u32,
        edges: ResizeEdges,
    ) -> bool {
        let root_surface_id = self.root_surface_id_for_surface(compositor_surface_id(surface));
        let Some((x, y)) = self.valid_pointer_press_for_surface(root_surface_id, surface, serial)
        else {
            return false;
        };
        self.begin_window_interaction_for_root(
            root_surface_id,
            x,
            y,
            WindowInteractionKind::Resize(edges),
        )
    }

    pub(in crate::compositor) fn begin_window_interaction_for_root(
        &mut self,
        root_surface_id: u32,
        x: f64,
        y: f64,
        kind: WindowInteractionKind,
    ) -> bool {
        let Some(root_surface) = self
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == root_surface_id)
        else {
            self.window_interaction = None;
            return false;
        };
        let fallback_geometry = WindowGeometry::new(
            root_surface.placement,
            root_surface.width,
            root_surface.height,
        );
        let resize_interaction_id = match kind {
            WindowInteractionKind::Resize(_) => {
                let id = self.allocate_resize_interaction_id();
                if let Some(flow) = self.resize_configure_flows.get_mut(&root_surface_id) {
                    let result = flow.begin_interaction(id);
                    self.resize_flow_metrics.obsolete_queued_targets_discarded = self
                        .resize_flow_metrics
                        .obsolete_queued_targets_discarded
                        .saturating_add(result.obsolete_queued_discarded as u64);
                    self.resize_flow_metrics.obsolete_finals_discarded = self
                        .resize_flow_metrics
                        .obsolete_finals_discarded
                        .saturating_add(result.obsolete_final_discarded as u64);
                }
                self.resize_flow_metrics.resize_interactions_started = self
                    .resize_flow_metrics
                    .resize_interactions_started
                    .saturating_add(1);
                if self.active_toplevel_resizes.contains_key(&root_surface_id) {
                    self.resize_flow_metrics.rapid_reresize_interactions = self
                        .resize_flow_metrics
                        .rapid_reresize_interactions
                        .saturating_add(1);
                }
                Some(id)
            }
            WindowInteractionKind::Move => None,
        };
        let start_geometry = match kind {
            WindowInteractionKind::Resize(_) => self
                .current_visual_root_window_geometry(root_surface_id)
                .unwrap_or(fallback_geometry),
            WindowInteractionKind::Move => self
                .current_root_window_geometry(root_surface_id)
                .unwrap_or(fallback_geometry),
        };
        if matches!(kind, WindowInteractionKind::Resize(_)) {
            self.resize_flow_metrics.visual_geometry_resize_starts = self
                .resize_flow_metrics
                .visual_geometry_resize_starts
                .saturating_add(1);
        }
        let start_width = start_geometry.width;
        let start_height = start_geometry.height;
        let start_placement = start_geometry.placement;
        let Some(root_resource) = self.surface_resource_by_id(root_surface_id) else {
            self.window_interaction = None;
            return false;
        };

        self.focus_surface(root_resource);
        self.window_interaction = Some(WindowInteraction {
            root_surface_id,
            kind,
            start_pointer_x: x,
            start_pointer_y: y,
            start_placement,
            start_width,
            start_height,
            drag_committed: false,
            resize_interaction_id,
        });
        true
    }

    pub(in crate::compositor) fn allocate_resize_interaction_id(&mut self) -> ResizeInteractionId {
        self.next_resize_interaction_id = self.next_resize_interaction_id.saturating_add(1);
        ResizeInteractionId::new(self.next_resize_interaction_id.max(1))
    }

    pub(in crate::compositor) fn valid_pointer_press_for_surface(
        &self,
        root_surface_id: u32,
        surface: &wl_surface::WlSurface,
        serial: u32,
    ) -> Option<(f64, f64)> {
        let press = self.last_pointer_press.as_ref()?;
        let valid_surface = press.root_surface_id == root_surface_id
            || press.surface.id().same_client_as(&surface.id());
        (press.serial == serial && valid_surface).then_some((press.output_x, press.output_y))
    }

    pub(in crate::compositor) fn window_frame_hit_at(
        &mut self,
        x: f64,
        y: f64,
    ) -> Option<WindowFrameHit> {
        if let Some(hit) = self.root_surface_hit_at(x, y) {
            let kind = window_frame_action_for_local_point(
                hit.local_x,
                hit.local_y,
                hit.width,
                hit.height,
            )?;
            return Some(WindowFrameHit {
                root_surface_id: hit.root_surface_id,
                kind,
            });
        }

        None
    }

    pub(in crate::compositor) fn update_window_interaction(&mut self, x: f64, y: f64) -> bool {
        let Some(mut interaction) = self.window_interaction else {
            return false;
        };
        let dx = (x - interaction.start_pointer_x).round() as i32;
        let dy = (y - interaction.start_pointer_y).round() as i32;

        match interaction.kind {
            WindowInteractionKind::Move => {
                let placement = SurfacePlacement::root_at(
                    interaction.start_placement.local_x + dx,
                    interaction.start_placement.local_y + dy,
                );
                self.set_surface_placement_with_cause(
                    interaction.root_surface_id,
                    placement,
                    RenderGenerationCause::WindowMove,
                )
            }
            WindowInteractionKind::Resize(edges) => {
                if !interaction.drag_committed && !resize_drag_threshold_reached(edges, dx, dy) {
                    return false;
                }
                interaction.drag_committed = true;
                self.window_interaction = Some(interaction);

                let resize = interactive_resize_geometry(interaction, edges, dx, dy);
                let update = PendingInteractiveResizeUpdate {
                    root_surface_id: interaction.root_surface_id,
                    width: resize.width,
                    height: resize.height,
                    placement: SurfacePlacement::root_at(resize.x, resize.y),
                    edges,
                    interaction_id: interaction
                        .resize_interaction_id
                        .expect("resize interaction has an ID"),
                };
                self.resize_flow_metrics.raw_pointer_resize_updates = self
                    .resize_flow_metrics
                    .raw_pointer_resize_updates
                    .saturating_add(1);
                if self.pending_interactive_resize_update == Some(update) {
                    self.resize_flow_metrics.resize_updates_skipped_unchanged = self
                        .resize_flow_metrics
                        .resize_updates_skipped_unchanged
                        .saturating_add(1);
                    return false;
                }
                if self
                    .pending_interactive_resize_update
                    .replace(update)
                    .is_some()
                {
                    self.resize_flow_metrics.pending_resize_updates_replaced = self
                        .resize_flow_metrics
                        .pending_resize_updates_replaced
                        .saturating_add(1);
                }
                true
            }
        }
    }

    pub(in crate::compositor) fn end_window_interaction(&mut self) {
        let interaction = self.window_interaction;
        if let Some(interaction) = interaction
            && interaction.drag_committed
            && let WindowInteractionKind::Resize(edges) = interaction.kind
        {
            self.apply_pending_interactive_resize_update();
            self.send_resize_end_configure(
                interaction.root_surface_id,
                edges,
                interaction
                    .resize_interaction_id
                    .expect("resize interaction has an ID"),
            );
        }
        self.window_interaction = None;
    }

    pub(in crate::compositor) fn apply_pending_interactive_resize_update(&mut self) -> bool {
        let Some(update) = self.pending_interactive_resize_update.take() else {
            return false;
        };
        let applied = self.queue_resize_root_window_to(
            update.root_surface_id,
            update.width,
            update.height,
            update.placement,
            update.edges,
            update.interaction_id,
        );
        if applied {
            self.resize_flow_metrics.resize_updates_applied = self
                .resize_flow_metrics
                .resize_updates_applied
                .saturating_add(1);
        } else {
            self.resize_flow_metrics.resize_updates_skipped_unchanged = self
                .resize_flow_metrics
                .resize_updates_skipped_unchanged
                .saturating_add(1);
        }
        applied
    }

    pub(in crate::compositor) fn window_interaction_active(&self) -> bool {
        self.window_interaction.is_some()
    }

    pub(in crate::compositor) fn resize_focused_window_to(
        &mut self,
        width: u32,
        height: u32,
    ) -> bool {
        let Some(surface_id) = self.focused_surface.as_ref().map(compositor_surface_id) else {
            return false;
        };
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        self.resize_root_window_to(root_surface_id, width, height)
    }

    pub(in crate::compositor) fn minimize_focused_window(&mut self) -> bool {
        let Some(surface_id) = self.focused_root_surface_id() else {
            return false;
        };
        self.minimize_root_window(surface_id)
    }

    pub(in crate::compositor) fn restore_next_minimized_window(&mut self) -> bool {
        let Some(surface_id) = self
            .toplevel_surfaces
            .iter()
            .find_map(|(surface_id, toplevel)| {
                toplevel.window.is_minimized().then_some(*surface_id)
            })
        else {
            return false;
        };

        self.restore_minimized_root_window(surface_id)
    }

    pub(in crate::compositor) fn activate_root_window(&mut self, surface_id: u32) -> bool {
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            return false;
        }
        if self
            .toplevel_surfaces
            .get(&surface_id)
            .is_some_and(|toplevel| toplevel.window.is_minimized())
        {
            self.restore_minimized_root_window(surface_id);
        }
        let focused = self
            .surface_resource_by_id(surface_id)
            .map(|surface| {
                self.focus_surface(surface);
                true
            })
            .unwrap_or(false);
        let raised = self.raise_root_window(surface_id);
        focused || raised
    }

    pub(in crate::compositor) fn toggle_maximize_focused_window(&mut self) -> bool {
        let Some(surface_id) = self.focused_root_surface_id() else {
            return false;
        };
        self.toggle_root_window_mode(surface_id, ToplevelMode::Maximized)
    }

    pub(in crate::compositor) fn toggle_fullscreen_focused_window(&mut self) -> bool {
        let Some(surface_id) = self.focused_root_surface_id() else {
            return false;
        };
        self.toggle_root_window_mode(surface_id, ToplevelMode::Fullscreen)
    }

    pub(in crate::compositor) fn minimize_root_window(&mut self, surface_id: u32) -> bool {
        if !self.toplevel_surfaces.contains_key(&surface_id)
            || self
                .toplevel_surfaces
                .get(&surface_id)
                .is_some_and(|toplevel| toplevel.window.is_minimized())
        {
            return false;
        }
        self.clear_resize_state_for_surfaces(&[surface_id]);

        let surface_placements = &self.surface_placements;
        let mut minimized_surfaces = Vec::new();
        let mut visible_surfaces = Vec::with_capacity(self.renderable_surfaces.len());
        for surface in self.renderable_surfaces.drain(..) {
            if root_surface_id_for_surface_in_placements(surface_placements, surface.surface_id)
                == surface_id
            {
                minimized_surfaces.push(surface);
            } else {
                visible_surfaces.push(surface);
            }
        }
        self.renderable_surfaces = visible_surfaces;

        if minimized_surfaces.is_empty() {
            return false;
        }

        if let Some(toplevel) = self.toplevel_surfaces.get_mut(&surface_id) {
            toplevel.window.minimize(minimized_surfaces);
        }
        if self.focused_root_surface_id() == Some(surface_id) {
            self.focused_surface = None;
            self.clear_keyboard_focus();
            if self.pointer_surface.as_ref().is_some_and(|surface| {
                self.root_surface_id_for_surface(compositor_surface_id(surface)) == surface_id
            }) {
                self.clear_pointer_focus();
            }
        }
        self.focus_topmost_renderable_toplevel();
        self.advance_render_generation(RenderGenerationCause::WindowMinimize);
        true
    }

    pub(in crate::compositor) fn restore_minimized_root_window(&mut self, surface_id: u32) -> bool {
        let Some(minimized_surfaces) = self
            .toplevel_surfaces
            .get_mut(&surface_id)
            .and_then(|toplevel| toplevel.window.restore_minimized())
        else {
            return false;
        };

        self.renderable_surfaces.extend(minimized_surfaces);
        if let Some(surface) = self.surface_resource_by_id(surface_id) {
            self.focus_surface(surface);
        }
        self.advance_render_generation(RenderGenerationCause::WindowRestore);
        true
    }

    pub(in crate::compositor) fn toggle_root_window_mode(
        &mut self,
        surface_id: u32,
        mode: ToplevelMode,
    ) -> bool {
        let Some(current_mode) = self
            .toplevel_surfaces
            .get(&surface_id)
            .map(|toplevel| toplevel.window.mode())
        else {
            return false;
        };

        if current_mode == mode {
            self.restore_floating_root_window(surface_id)
        } else {
            self.set_root_window_mode(surface_id, mode)
        }
    }

    pub(in crate::compositor) fn set_root_window_mode(
        &mut self,
        surface_id: u32,
        mode: ToplevelMode,
    ) -> bool {
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            return false;
        }
        self.clear_resize_state_for_surfaces(&[surface_id]);
        if self
            .toplevel_surfaces
            .get(&surface_id)
            .is_some_and(|toplevel| toplevel.window.is_minimized())
        {
            self.restore_minimized_root_window(surface_id);
        }

        let restore_geometry = self
            .current_root_window_geometry(surface_id)
            .unwrap_or_else(|| WindowGeometry::new(self.surface_placement(surface_id), 0, 0));
        if let Some(toplevel) = self.toplevel_surfaces.get_mut(&surface_id) {
            toplevel.window.capture_restore_geometry(restore_geometry);
            toplevel.window.set_mode(mode);
        }

        let states = mode.xdg_states();
        let configured = self
            .send_configure_root_window_to(
                surface_id,
                self.output_size.width,
                self.output_size.height,
                states,
            )
            .is_some();
        let fullscreen_placement = SurfacePlacement::root_at(
            -render::FIRST_SURFACE_OFFSET.0,
            -render::FIRST_SURFACE_OFFSET.1,
        );
        self.set_surface_placement_with_cause(
            surface_id,
            fullscreen_placement,
            RenderGenerationCause::WindowMode,
        );
        configured
    }

    pub(in crate::compositor) fn restore_floating_root_window(&mut self, surface_id: u32) -> bool {
        self.clear_resize_state_for_surfaces(&[surface_id]);
        let Some(restore_geometry) = self.toplevel_surfaces.get_mut(&surface_id).map(|toplevel| {
            toplevel.window.set_mode(ToplevelMode::Floating);
            toplevel.window.take_restore_geometry()
        }) else {
            return false;
        };
        let restore_geometry = restore_geometry
            .or_else(|| self.current_root_window_geometry(surface_id))
            .unwrap_or_else(|| WindowGeometry::new(self.surface_placement(surface_id), 0, 0));

        let configured = self
            .send_configure_root_window_to(
                surface_id,
                restore_geometry.width,
                restore_geometry.height,
                &[],
            )
            .is_some();
        self.set_surface_placement_with_cause(
            surface_id,
            restore_geometry.placement,
            RenderGenerationCause::WindowMode,
        );
        configured
    }

    pub(in crate::compositor) fn focused_root_surface_id(&self) -> Option<u32> {
        self.focused_surface
            .as_ref()
            .map(|surface| self.root_surface_id_for_surface(compositor_surface_id(surface)))
    }

    pub(in crate::compositor) fn current_root_window_geometry(
        &self,
        surface_id: u32,
    ) -> Option<WindowGeometry> {
        let surface = self
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == surface_id)
            .or_else(|| {
                self.toplevel_surfaces
                    .get(&surface_id)?
                    .window
                    .minimized_root_surface(surface_id)
            })?;
        let (width, height) = self
            .xdg_window_geometry_size(surface_id)
            .unwrap_or((surface.width, surface.height));

        Some(WindowGeometry::new(
            self.surface_placement(surface_id),
            width,
            height,
        ))
    }

    pub(in crate::compositor) fn current_visual_root_window_geometry(
        &self,
        surface_id: u32,
    ) -> Option<WindowGeometry> {
        if let Some(visual) = self.toplevel_visual_geometries.get(&surface_id) {
            return Some(visual.window_geometry());
        }
        let surface = self
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == surface_id)
            .or_else(|| {
                self.toplevel_surfaces
                    .get(&surface_id)?
                    .window
                    .minimized_root_surface(surface_id)
            })?;
        let (width, height) = self
            .xdg_window_geometry_size(surface_id)
            .unwrap_or((surface.width, surface.height));
        Some(WindowGeometry::new(surface.placement, width, height))
    }

    pub(in crate::compositor) fn xdg_window_geometry_size(
        &self,
        surface_id: u32,
    ) -> Option<(u32, u32)> {
        let geometry = self.surface_window_geometries.get(&surface_id)?;
        Some((
            u32::try_from(geometry.width).ok()?,
            u32::try_from(geometry.height).ok()?,
        ))
    }

    pub(in crate::compositor) fn focus_topmost_renderable_toplevel(&mut self) -> bool {
        let Some(surface_id) = self.renderable_surfaces.iter().rev().find_map(|surface| {
            let root_surface_id = self.root_surface_id_for_surface(surface.surface_id);
            self.toplevel_surfaces
                .contains_key(&root_surface_id)
                .then_some(root_surface_id)
        }) else {
            return false;
        };
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            return false;
        };
        self.focus_surface(surface);
        true
    }

    pub(in crate::compositor) fn raise_root_window(&mut self, surface_id: u32) -> bool {
        let surface_placements = &self.surface_placements;
        let mut raised_surfaces = Vec::new();
        let mut lower_surfaces = Vec::with_capacity(self.renderable_surfaces.len());
        for surface in self.renderable_surfaces.drain(..) {
            if root_surface_id_for_surface_in_placements(surface_placements, surface.surface_id)
                == surface_id
            {
                raised_surfaces.push(surface);
            } else {
                lower_surfaces.push(surface);
            }
        }
        if raised_surfaces.is_empty() {
            self.renderable_surfaces = lower_surfaces;
            return false;
        }
        lower_surfaces.extend(raised_surfaces);
        self.renderable_surfaces = lower_surfaces;
        self.advance_render_generation(RenderGenerationCause::WindowStack);
        true
    }

    pub(in crate::compositor) fn resize_root_window_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
    ) -> bool {
        self.send_resize_root_window_to(surface_id, width, height)
    }

    pub(in crate::compositor) fn queue_resize_root_window_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
        edges: ResizeEdges,
        interaction_id: ResizeInteractionId,
    ) -> bool {
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            return false;
        };
        let geometry = self.clamp_resize_geometry(
            surface_id,
            WindowGeometry::new(placement, width, height),
            edges,
        );
        let width = geometry.width;
        let height = geometry.height;
        let placement = geometry.placement;
        let pending = PendingResizeConfigure {
            surface_id,
            width,
            height,
            placement,
            edges,
            resizing: true,
            interaction_id,
        };
        self.resize_flow_metrics.configures_requested = self
            .resize_flow_metrics
            .configures_requested
            .saturating_add(1);
        let flow = self.resize_configure_flows.entry(surface_id).or_default();
        let was_blocked = flow.has_in_flight() || flow.latest_desired().is_some();
        let queued = flow.queue(pending);
        self.update_resize_retained_configure_peak(surface_id);
        if !queued {
            self.resize_flow_metrics.duplicate_configure_sizes_skipped = self
                .resize_flow_metrics
                .duplicate_configure_sizes_skipped
                .saturating_add(1);
        }
        if queued && was_blocked {
            self.resize_flow_metrics.geometries_coalesced = self
                .resize_flow_metrics
                .geometries_coalesced
                .saturating_add(1);
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: resize_flow surface={surface_id} decision=coalesced queued_serial=not-sent queued_size={}x{} final_pending=false preview_active=true",
                    pending.width, pending.height,
                );
            }
        }
        self.preview_resize_root_window_to(
            surface_id,
            width,
            height,
            placement,
            edges,
            interaction_id,
        )
    }

    pub(in crate::compositor) fn clamp_resize_geometry(
        &self,
        surface_id: u32,
        geometry: WindowGeometry,
        edges: ResizeEdges,
    ) -> WindowGeometry {
        let width = self.clamp_toplevel_width(surface_id, geometry.width);
        let height = self.clamp_toplevel_height(surface_id, geometry.height);
        let mut placement = geometry.placement;
        if edges.left && width != geometry.width {
            let requested_right = placement
                .local_x
                .saturating_add(i32::try_from(geometry.width).unwrap_or(i32::MAX));
            placement.local_x =
                requested_right.saturating_sub(i32::try_from(width).unwrap_or(i32::MAX));
        }
        if edges.top && height != geometry.height {
            let requested_bottom = placement
                .local_y
                .saturating_add(i32::try_from(geometry.height).unwrap_or(i32::MAX));
            placement.local_y =
                requested_bottom.saturating_sub(i32::try_from(height).unwrap_or(i32::MAX));
        }

        WindowGeometry::new(placement, width, height)
    }

    pub(in crate::compositor) fn clamp_toplevel_width(&self, surface_id: u32, width: u32) -> u32 {
        let constraints = self.toplevel_constraints(surface_id);
        let min_width = constraints.min_width.unwrap_or(MIN_WINDOW_WIDTH);
        let mut clamped = width.max(min_width);
        if let Some(max_width) = constraints.max_width {
            clamped = clamped.min(max_width.max(min_width));
        }
        clamped
    }

    pub(in crate::compositor) fn clamp_toplevel_height(&self, surface_id: u32, height: u32) -> u32 {
        let constraints = self.toplevel_constraints(surface_id);
        let min_height = constraints.min_height.unwrap_or(MIN_WINDOW_HEIGHT);
        let mut clamped = height.max(min_height);
        if let Some(max_height) = constraints.max_height {
            clamped = clamped.min(max_height.max(min_height));
        }
        clamped
    }

    pub(in crate::compositor) fn toplevel_constraints(
        &self,
        surface_id: u32,
    ) -> ToplevelSizeConstraints {
        self.toplevel_surfaces
            .get(&surface_id)
            .map(|toplevel| toplevel.constraints)
            .unwrap_or_default()
    }

    pub(in crate::compositor) fn preview_resize_root_window_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
        edges: ResizeEdges,
        interaction_id: ResizeInteractionId,
    ) -> bool {
        let flow_sequence = self
            .resize_configure_flows
            .get(&surface_id)
            .and_then(ResizeConfigureFlow::in_flight_sequence)
            .unwrap_or_else(|| self.next_resize_configure_sequence.saturating_add(1));
        let previous = self
            .toplevel_visual_geometries
            .get(&surface_id)
            .copied()
            .or_else(|| {
                self.current_visual_root_window_geometry(surface_id)
                    .map(|geometry| ToplevelVisualGeometry {
                        placement: geometry.placement,
                        width: geometry.width,
                        height: geometry.height,
                        active_resize: None,
                    })
            });
        if previous.is_some_and(|visual| {
            visual.width == width
                && visual.height == height
                && visual.placement == placement
                && visual.active_resize == Some(interaction_id)
        }) {
            return false;
        }

        self.toplevel_visual_geometries.insert(
            surface_id,
            ToplevelVisualGeometry {
                placement,
                width,
                height,
                active_resize: Some(interaction_id),
            },
        );
        self.update_toplevel_visual_render_assignment(surface_id);
        let previous_resize = self.active_toplevel_resizes.get(&surface_id).copied();
        if previous_resize.is_none() {
            self.active_toplevel_resizes.insert(
                surface_id,
                ActiveToplevelResize {
                    interaction_id,
                    flow_sequence,
                    edges,
                    activated_at: Instant::now(),
                },
            );
            self.resize_flow_metrics.preview_activations = self
                .resize_flow_metrics
                .preview_activations
                .saturating_add(1);
        } else if previous_resize.is_some_and(|resize| resize.interaction_id != interaction_id) {
            self.active_toplevel_resizes.insert(
                surface_id,
                ActiveToplevelResize {
                    interaction_id,
                    flow_sequence,
                    edges,
                    activated_at: Instant::now(),
                },
            );
            self.resize_flow_metrics.preview_ownership_transfers = self
                .resize_flow_metrics
                .preview_ownership_transfers
                .saturating_add(1);
        }
        self.advance_render_generation(RenderGenerationCause::WindowResize);
        true
    }

    pub(in crate::compositor) fn update_toplevel_visual_render_assignment(
        &mut self,
        root_surface_id: u32,
    ) {
        let Some(visual) = self
            .toplevel_visual_geometries
            .get(&root_surface_id)
            .copied()
        else {
            return;
        };
        let geometry = self
            .surface_window_geometries
            .get(&root_surface_id)
            .copied();
        let geometry_x = geometry.map_or(0, |geometry| geometry.x);
        let geometry_y = geometry.map_or(0, |geometry| geometry.y);
        let root_render_placement = SurfacePlacement::root_at(
            visual.placement.local_x.saturating_sub(geometry_x),
            visual.placement.local_y.saturating_sub(geometry_y),
        );
        let clip = render::SurfaceTargetRect::new(
            visual.placement.local_x,
            visual.placement.local_y,
            visual.width,
            visual.height,
        );
        let visual_clip = visual.active_resize.is_some().then_some(clip);
        let placements = &self.surface_placements;
        for surface in &mut self.renderable_surfaces {
            if root_surface_id_for_surface_in_placements(placements, surface.surface_id)
                != root_surface_id
            {
                continue;
            }
            surface.visual_clip = visual_clip;
            if surface.surface_id == root_surface_id {
                surface.render_placement = Some(root_render_placement);
            }
        }
        self.invalidate_surface_origin_cache();
    }

    pub(in crate::compositor) fn clear_toplevel_visual_render_assignment(
        &mut self,
        root_surface_id: u32,
    ) {
        let placements = &self.surface_placements;
        for surface in &mut self.renderable_surfaces {
            if root_surface_id_for_surface_in_placements(placements, surface.surface_id)
                == root_surface_id
            {
                surface.render_placement = None;
                surface.visual_clip = None;
            }
        }
        self.invalidate_surface_origin_cache();
    }

    pub(in crate::compositor) fn flush_pending_resize_configure(&mut self) -> bool {
        let surface_ids = self
            .resize_configure_flows
            .iter()
            .filter_map(|(surface_id, flow)| flow.has_sendable().then_some(*surface_id))
            .collect::<Vec<_>>();
        let mut sent = false;
        for surface_id in surface_ids {
            let desired = self
                .resize_configure_flows
                .get_mut(&surface_id)
                .and_then(ResizeConfigureFlow::take_sendable);
            if let Some(desired) = desired {
                sent |= self.send_resize_configure(desired);
            }
        }
        sent
    }

    pub(in crate::compositor) fn send_resize_end_configure(
        &mut self,
        surface_id: u32,
        edges: ResizeEdges,
        interaction_id: ResizeInteractionId,
    ) -> bool {
        let desired = self
            .resize_configure_flows
            .get(&surface_id)
            .and_then(ResizeConfigureFlow::latest_desired)
            .filter(|pending| pending.interaction_id == interaction_id)
            .map(|pending| PendingResizeConfigure {
                resizing: false,
                ..pending
            })
            .or_else(|| {
                self.current_visual_root_window_geometry(surface_id)
                    .map(|geometry| PendingResizeConfigure {
                        surface_id,
                        width: geometry.width,
                        height: geometry.height,
                        placement: geometry.placement,
                        edges,
                        resizing: false,
                        interaction_id,
                    })
            });
        let Some(desired) = desired else {
            return false;
        };
        self.resize_flow_metrics.configures_requested = self
            .resize_flow_metrics
            .configures_requested
            .saturating_add(1);
        self.resize_configure_flows
            .entry(surface_id)
            .or_default()
            .queue_final(desired);
        self.update_resize_retained_configure_peak(surface_id);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} decision=coalesced queued_serial=not-sent queued_size={}x{} final_pending=true preview_active={}",
                desired.width,
                desired.height,
                self.active_toplevel_resizes.contains_key(&surface_id),
            );
        }
        self.flush_pending_resize_configure()
    }

    pub(in crate::compositor) fn pending_resize_configure_is_flushable(&self) -> bool {
        self.resize_configure_flows
            .values()
            .any(ResizeConfigureFlow::has_sendable)
    }

    pub(in crate::compositor) fn send_resize_configure(
        &mut self,
        desired: PendingResizeConfigure,
    ) -> bool {
        let surface_id = desired.surface_id;
        let geometry = self.clamp_resize_geometry(
            surface_id,
            WindowGeometry::new(desired.placement, desired.width, desired.height),
            desired.edges,
        );
        let width = geometry.width;
        let height = geometry.height;
        let placement = geometry.placement;
        let resizing_states = [xdg_toplevel::State::Resizing];
        let states = if desired.resizing {
            &resizing_states[..]
        } else {
            &[][..]
        };
        let Some(serial) = self.send_configure_root_window_to(surface_id, width, height, states)
        else {
            return false;
        };
        let resize = PendingResizeConfigure {
            surface_id,
            width: width.max(MIN_WINDOW_WIDTH),
            height: height.max(MIN_WINDOW_HEIGHT),
            placement,
            edges: desired.edges,
            resizing: desired.resizing,
            interaction_id: desired.interaction_id,
        };
        self.next_resize_configure_sequence = self.next_resize_configure_sequence.saturating_add(1);
        let sequence = self.next_resize_configure_sequence;
        self.resize_configure_flows
            .entry(surface_id)
            .or_default()
            .mark_sent(resize, serial, sequence);
        self.update_resize_retained_configure_peak(surface_id);
        if !resize.resizing {
            self.resize_flow_metrics.final_configures_sent = self
                .resize_flow_metrics
                .final_configures_sent
                .saturating_add(1);
        }
        self.resize_flow_metrics.configures_sent =
            self.resize_flow_metrics.configures_sent.saturating_add(1);
        self.resize_flow_metrics.max_in_flight_configures =
            self.resize_flow_metrics.max_in_flight_configures.max(
                self.resize_configure_flows
                    .get(&surface_id)
                    .map_or(0, ResizeConfigureFlow::in_flight_configure_count),
            );
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} decision=sent serial={serial} sequence={sequence} size={}x{} placement={},{} edges={:?} resizing={} in_flight_serial={serial}",
                resize.width,
                resize.height,
                resize.placement.local_x,
                resize.placement.local_y,
                resize.edges,
                resize.resizing,
            );
        }
        true
    }
}

fn popup_debug_log(message: impl FnOnce() -> String) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("OBLIVION_ONE_POPUP_DEBUG").is_some()) {
        eprintln!("oblivion-one popup: {}", message());
    }
}
