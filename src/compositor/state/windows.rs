use super::*;

impl CompositorState {
    pub(in crate::compositor) fn focus_desktop_window(&mut self, window_id: WindowId) -> bool {
        let Some(window) = self.window(window_id) else {
            return false;
        };
        if window.kind == DesktopWindowKind::OverrideRedirect {
            return false;
        }
        let surface_id = window.root_surface_id;
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            return false;
        };
        self.focus_surface(surface);
        let _ = self.raise_root_window(surface_id);
        true
    }

    pub(in crate::compositor) fn x11_focus_request_allowed(&self, handle: X11WindowHandle) -> bool {
        let Some(target_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some(target) = self.window(target_id) else {
            return false;
        };
        if target.kind == DesktopWindowKind::OverrideRedirect {
            return false;
        }
        self.focused_window_id.is_none_or(|focused| {
            focused == target_id || target.relationships.transient_for == Some(focused)
        })
    }

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
        self.clear_resize_state_for_surfaces(&[surface_id]);
        let Ok(window_id) = self.allocate_window_id() else {
            return;
        };
        if self
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
            .is_err()
        {
            return;
        }
        self.toplevel_surfaces.insert(
            surface_id,
            ToplevelSurface {
                window_id,
                xdg_surface,
                toplevel,
                pending_constraints: None,
                wm_capabilities_sent: false,
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
        let window_id = self
            .toplevel_surfaces
            .get(&surface_id)
            .map(|toplevel| toplevel.window_id);
        if self.toplevel_surfaces.contains_key(&surface_id)
            || self
                .surface_role_lifecycle(surface_id)
                .live_instance
                .is_some_and(|role| role == LiveRoleInstance::XdgToplevel)
        {
            self.retire_unpublished_work_for_xdg_role(
                surface_id,
                AcquireWatchCancelReason::RoleDestroyed,
            );
        }
        self.unmap_xdg_role_surfaces(surface_id);
        self.toplevel_surfaces.remove(&surface_id);
        if let Some(window_id) = window_id {
            self.remove_desktop_window(window_id);
        }
        self.clear_fullscreen_presentation_owner(surface_id);
        self.deactivate_role_instance_if(surface_id, SurfaceRole::XdgToplevel);
        self.surface_placements.remove(&surface_id);
        self.xdg_configure_serials.remove(&surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
    }

    pub(in crate::compositor) fn apply_pending_toplevel_constraints(&mut self, surface_id: u32) {
        let Some((window_id, pending)) =
            self.toplevel_surfaces
                .get_mut(&surface_id)
                .and_then(|toplevel| {
                    toplevel
                        .pending_constraints
                        .take()
                        .map(|pending| (toplevel.window_id, pending))
                })
        else {
            return;
        };
        if let Some(window) = self.window_mut(window_id) {
            window.constraints.min_width = pending.min_width;
            window.constraints.min_height = pending.min_height;
            window.constraints.max_width = pending.max_width;
            window.constraints.max_height = pending.max_height;
        }
    }

    pub(in crate::compositor) fn set_toplevel_parent(
        &mut self,
        surface_id: u32,
        parent_surface_id: Option<u32>,
    ) -> Result<(), ()> {
        if parent_surface_id == Some(surface_id) {
            return Err(());
        }
        let mut current = parent_surface_id;
        while let Some(candidate) = current {
            if candidate == surface_id {
                return Err(());
            }
            current = self
                .window_id_for_surface(candidate)
                .and_then(|window_id| self.window(window_id))
                .and_then(|window| window.relationships.parent)
                .and_then(|window_id| self.window(window_id))
                .map(|window| window.root_surface_id);
        }
        let Some(window_id) = self
            .toplevel_surfaces
            .get(&surface_id)
            .map(|toplevel| toplevel.window_id)
        else {
            return Err(());
        };
        let parent = parent_surface_id
            .and_then(|parent_surface_id| self.window_id_for_surface(parent_surface_id));
        if let Some(window) = self.window_mut(window_id) {
            window.relationships.parent = parent;
        }
        Ok(())
    }

    pub(in crate::compositor) fn unregister_xdg_surface_role(&mut self, surface_id: u32) {
        self.destroy_popup_children_for_parent(surface_id);

        self.unregister_toplevel_surface(surface_id);
        self.unregister_popup_surface(surface_id);
        self.surface_window_geometries.remove(&surface_id);
        self.pending_surface_window_geometries.remove(&surface_id);
        self.surface_placements.remove(&surface_id);
        self.clear_popup_grab_for_surface_ids(&[surface_id]);
        self.popup_grab_stack.retain(|id| *id != surface_id);
        self.recent_input_serials
            .retain(|input| compositor_surface_id(&input.surface) != surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
        self.xdg_surface_resources.remove(&surface_id);
        self.xdg_surface_wm_bases.remove(&surface_id);
        self.xdg_surface_lifecycles.remove(&surface_id);
        self.destroy_xdg_association(surface_id);
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
            || !self.validate_popup_grab_serial(serial, surface)
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
        if self.popup_surfaces.contains_key(&surface_id)
            || self
                .surface_role_lifecycle(surface_id)
                .live_instance
                .is_some_and(|role| role == LiveRoleInstance::XdgPopup)
        {
            self.retire_unpublished_work_for_xdg_role(
                surface_id,
                AcquireWatchCancelReason::RoleDestroyed,
            );
        }
        self.destroy_popup_role(surface_id);
    }

    pub(in crate::compositor) fn popup_destroy_is_topmost(&self, surface_id: u32) -> bool {
        self.popup_grab_stack.last().is_none_or(|topmost| {
            *topmost == surface_id || !self.popup_grab_stack.contains(&surface_id)
        })
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
        if self.popup_surfaces.contains_key(&surface_id)
            || self
                .surface_role_lifecycle(surface_id)
                .live_instance
                .is_some_and(|role| role == LiveRoleInstance::XdgPopup)
        {
            self.retire_unpublished_work_for_xdg_role(
                surface_id,
                AcquireWatchCancelReason::RoleDestroyed,
            );
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
        self.deactivate_role_instance_if(surface_id, SurfaceRole::XdgPopup);
        self.detach_popup_node(surface_id, PopupLifecycle::Destroyed);
        self.surface_placements.remove(&surface_id);
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
                self.focused_window_id = None;
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
        self.record_xdg_configure(surface_id, serial);
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
        let Some(lifecycle) = self.xdg_surface_lifecycle(surface_id) else {
            return false;
        };
        if !lifecycle.needs_configure() || lifecycle.has_outstanding_configure() {
            return false;
        }

        if let Some(toplevel) = self.toplevel_surfaces.get(&surface_id).cloned() {
            self.send_wm_capabilities_if_needed(surface_id);
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
            self.record_xdg_configure(surface_id, serial);
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
            return true;
        }

        false
    }

    pub(in crate::compositor) fn send_wm_capabilities_if_needed(&mut self, surface_id: u32) {
        let Some(toplevel) = self.toplevel_surfaces.get_mut(&surface_id) else {
            return;
        };
        if toplevel.toplevel.version() < 5 || toplevel.wm_capabilities_sent {
            return;
        }

        let mut capabilities = Vec::with_capacity(2 * std::mem::size_of::<u32>());
        for capability in [
            xdg_toplevel::WmCapabilities::Maximize,
            xdg_toplevel::WmCapabilities::Fullscreen,
        ] {
            capabilities.extend_from_slice(&(capability as u32).to_ne_bytes());
        }
        let _ = toplevel
            .toplevel
            .send_event(xdg_toplevel::Event::WmCapabilities { capabilities });
        toplevel.wm_capabilities_sent = true;
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
        let Some(surface_id) = self.toplevel_surfaces.iter().find_map(|(surface_id, _)| {
            self.toplevel_window_state(*surface_id)
                .is_some_and(WindowState::is_minimized)
                .then_some(*surface_id)
        }) else {
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
            .is_some_and(|toplevel| {
                self.window(toplevel.window_id)
                    .is_some_and(|window| window.state.is_minimized())
            })
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
        let Some(window_id) = self
            .toplevel_surfaces
            .get(&surface_id)
            .map(|toplevel| toplevel.window_id)
        else {
            return false;
        };
        self.minimize_desktop_window(window_id)
    }

    pub(in crate::compositor) fn minimize_desktop_window(&mut self, window_id: WindowId) -> bool {
        let Some(root_surface_id) = self.window(window_id).map(|window| window.root_surface_id)
        else {
            return false;
        };
        if self
            .window(window_id)
            .is_some_and(|window| window.state.is_minimized())
        {
            return false;
        }
        self.clear_resize_state_for_surfaces_with_reason(
            &[root_surface_id],
            WindowInteractionEndReason::ExplicitCancel,
        );
        self.clear_fullscreen_presentation_owner(root_surface_id);

        let surface_placements = &self.surface_placements;
        let mut minimized_surfaces = Vec::new();
        let mut visible_surfaces = Vec::with_capacity(self.renderable_surfaces.len());
        for surface in self.renderable_surfaces.drain(..) {
            if root_surface_id_for_surface_in_placements(surface_placements, surface.surface_id)
                == root_surface_id
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

        if let Some(window) = self.window_mut(window_id) {
            window.state.minimize(minimized_surfaces);
        }
        if self.focused_root_surface_id() == Some(root_surface_id) {
            self.focused_surface = None;
            self.focused_window_id = None;
            self.clear_keyboard_focus();
            if self.pointer_surface.as_ref().is_some_and(|surface| {
                self.root_surface_id_for_surface(compositor_surface_id(surface)) == root_surface_id
            }) {
                self.clear_pointer_focus();
            }
        }
        self.focus_topmost_renderable_toplevel();
        self.advance_render_generation(RenderGenerationCause::WindowMinimize);
        true
    }

    pub(in crate::compositor) fn restore_minimized_root_window(&mut self, surface_id: u32) -> bool {
        let Some(window_id) = self.toplevel_window_id(surface_id) else {
            return false;
        };
        self.restore_minimized_desktop_window(window_id)
    }

    pub(in crate::compositor) fn restore_minimized_desktop_window(
        &mut self,
        window_id: WindowId,
    ) -> bool {
        let Some(minimized_surfaces) = self
            .window_mut(window_id)
            .and_then(|window| window.state.restore_minimized())
        else {
            return false;
        };

        self.renderable_surfaces.extend(minimized_surfaces);
        if let Some(surface_id) = self.window(window_id).map(|window| window.root_surface_id)
            && let Some(surface) = self.surface_resource_by_id(surface_id)
        {
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
            .and_then(|_| self.toplevel_window_state(surface_id))
            .map(WindowState::mode)
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
        let restore_geometry = self
            .current_visual_root_window_geometry(surface_id)
            .or_else(|| self.current_root_window_geometry(surface_id))
            .unwrap_or_else(|| WindowGeometry::new(self.surface_placement(surface_id), 0, 0));
        self.clear_resize_state_for_surfaces_with_reason(
            &[surface_id],
            WindowInteractionEndReason::ModeTransition,
        );
        if self
            .toplevel_surfaces
            .get(&surface_id)
            .is_some_and(|toplevel| {
                self.window(toplevel.window_id)
                    .is_some_and(|window| window.state.is_minimized())
            })
        {
            self.restore_minimized_root_window(surface_id);
        }

        if let Some(window) = self.toplevel_window_state_mut(surface_id) {
            window.capture_restore_geometry(restore_geometry);
            window.set_mode(mode);
        }

        let geometry = self.window_geometry_for_mode(mode);
        let states = mode.xdg_states();
        let configured = self
            .send_configure_root_window_to(surface_id, geometry.width, geometry.height, states)
            .is_some();
        if mode == ToplevelMode::Fullscreen {
            self.set_fullscreen_presentation_owner(surface_id);
        } else {
            self.clear_fullscreen_presentation_owner(surface_id);
        }
        self.set_surface_placement_with_cause(
            surface_id,
            geometry.placement,
            RenderGenerationCause::WindowMode,
        );
        configured
    }

    pub(in crate::compositor) fn restore_floating_root_window(&mut self, surface_id: u32) -> bool {
        self.clear_resize_state_for_surfaces_with_reason(
            &[surface_id],
            WindowInteractionEndReason::ModeTransition,
        );
        self.clear_fullscreen_presentation_owner(surface_id);
        let Some(window) = self.toplevel_window_state_mut(surface_id) else {
            return false;
        };
        window.set_mode(ToplevelMode::Floating);
        let restore_geometry = window.take_restore_geometry();
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
        if let Some(window_id) = self.focused_window_id
            && let Some(window) = self.window(window_id)
        {
            return Some(window.root_surface_id);
        }
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
                self.toplevel_window_state(surface_id)?
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
                self.toplevel_window_state(surface_id)?
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
        let Some(surface_id) = self.window_stacking.iter().rev().find_map(|window_id| {
            let root_surface_id = self.window(*window_id)?.root_surface_id;
            self.renderable_surfaces
                .iter()
                .any(|surface| surface.surface_id == root_surface_id)
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
        if let Some(window_id) = self.window_id_for_surface(surface_id) {
            let _ = self.raise_window_id(window_id);
        }
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
}

fn popup_debug_log(message: impl FnOnce() -> String) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("OBLIVION_ONE_POPUP_DEBUG").is_some()) {
        eprintln!("oblivion-one popup: {}", message());
    }
}
