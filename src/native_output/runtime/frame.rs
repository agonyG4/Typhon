use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeRepaintInputs {
    pub(crate) accepted_clients: bool,
    pub(crate) render_generation_changed: bool,
    pub(crate) pending_frame_work: bool,
    pub(crate) only_pending_surface_frame_callbacks: bool,
    pub(crate) redraw_requested: bool,
    pub(crate) page_flip_pending: bool,
}

pub(crate) fn earliest_native_deadline(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeRepaintDecision {
    pub(crate) repaint: bool,
    pub(crate) protocol_only_present: bool,
}

pub(crate) fn native_repaint_decision(inputs: NativeRepaintInputs) -> NativeRepaintDecision {
    if inputs.page_flip_pending {
        return NativeRepaintDecision {
            repaint: false,
            protocol_only_present: false,
        };
    }

    let protocol_only_present = inputs.pending_frame_work
        && inputs.only_pending_surface_frame_callbacks
        && !inputs.accepted_clients
        && !inputs.render_generation_changed
        && !inputs.redraw_requested;
    NativeRepaintDecision {
        repaint: inputs.accepted_clients
            || inputs.render_generation_changed
            || inputs.redraw_requested
            || (inputs.pending_frame_work && !protocol_only_present),
        protocol_only_present,
    }
}

pub(crate) fn normalize_refresh_hz(refresh_hz: u32) -> u32 {
    if refresh_hz == 0 {
        60
    } else {
        refresh_hz.clamp(30, 360)
    }
}

#[derive(Debug, Default)]
pub(crate) struct NativeFrameRenderer {
    pub(crate) scene_renderer: DesktopSceneRenderer,
    pub(crate) shell_overlay_renderer: ShellOverlayRenderer,
    pub(crate) frame: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorRenderMode {
    Software,
    Hardware,
}

impl NativeCursorRenderMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Software => "software",
            Self::Hardware => "hardware",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorPreference {
    Auto,
    Hardware,
    Software,
}

impl NativeCursorPreference {
    pub(crate) fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_CURSOR") {
            Ok(value) if matches!(value.as_str(), "hardware" | "hw" | "drm") => Self::Hardware,
            Ok(value) if matches!(value.as_str(), "software" | "sw" | "cpu") => Self::Software,
            Ok(value) if value == "auto" => Self::Auto,
            Ok(value) => {
                eprintln!("native cursor: unknown OBLIVION_ONE_CURSOR={value:?}; using auto");
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Hardware => "hardware",
            Self::Software => "software",
        }
    }
}

#[derive(Debug)]
pub(crate) struct NativePointerConstraintBackend {
    pub(crate) active: Option<NativePointerConstraint>,
    pub(crate) cursor_visible: bool,
}

pub(crate) fn native_pointer_debug_log(message: impl AsRef<str>) {
    if std::env::var_os("TYPHON_POINTER_DEBUG").is_some() {
        eprintln!("typhon pointer: {}", message.as_ref());
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NativePointerConstraint {
    pub(crate) id: PointerConstraintBackendId,
    pub(crate) mode: PointerConstraintMode,
    pub(crate) anchor: CompositorOutputPosition,
    pub(crate) region: Option<OutputRegion>,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct NativePointerConstraintBackendAction {
    pub(crate) activated: Option<NativePointerConstraint>,
    pub(crate) deactivated: Option<PointerConstraintBackendId>,
    pub(crate) failed: Option<(PointerConstraintBackendId, &'static str)>,
    pub(crate) restore_position: Option<CompositorOutputPosition>,
    pub(crate) cursor_position: Option<CompositorOutputPosition>,
    pub(crate) cursor_visibility_changed: Option<bool>,
}

impl NativePointerConstraintBackend {
    pub(crate) fn new() -> Self {
        Self {
            active: None,
            cursor_visible: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn active_locked(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|constraint| constraint.mode == PointerConstraintMode::Locked)
    }

    pub(crate) fn active_constraint_state(&self) -> NativePointerConstraintState {
        match self.active.as_ref() {
            Some(NativePointerConstraint {
                mode: PointerConstraintMode::Locked,
                anchor,
                ..
            }) => NativePointerConstraintState::Locked { anchor: *anchor },
            Some(NativePointerConstraint {
                mode: PointerConstraintMode::Confined,
                region: Some(region),
                ..
            }) => NativePointerConstraintState::Confined {
                region: region.clone(),
            },
            _ => NativePointerConstraintState::None,
        }
    }

    pub(crate) fn handle_request(
        &mut self,
        request: PointerConstraintBackendRequest,
        cursor_position: CompositorOutputPosition,
    ) -> NativePointerConstraintBackendAction {
        match request {
            PointerConstraintBackendRequest::ActivateLocked { id, anchor } => {
                self.activate_locked(id, anchor)
            }
            PointerConstraintBackendRequest::ActivateConfined { id, region } => {
                self.activate_confined(id, cursor_position, region)
            }
            PointerConstraintBackendRequest::UpdateConfinedRegion { id, region } => {
                self.update_confined_region(id, cursor_position, region)
            }
            PointerConstraintBackendRequest::Deactivate {
                id,
                restore_position,
            } => self.deactivate(id, restore_position),
            PointerConstraintBackendRequest::WarpPointer { position } => {
                native_pointer_debug_log(format!(
                    "backend warp requested position=({},{})",
                    position.x, position.y
                ));
                NativePointerConstraintBackendAction {
                    cursor_position: Some(position),
                    ..NativePointerConstraintBackendAction::default()
                }
            }
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible } => {
                if self.cursor_visible == visible {
                    NativePointerConstraintBackendAction::default()
                } else {
                    self.cursor_visible = visible;
                    NativePointerConstraintBackendAction {
                        cursor_visibility_changed: Some(visible),
                        ..NativePointerConstraintBackendAction::default()
                    }
                }
            }
        }
    }

    pub(crate) fn activate_locked(
        &mut self,
        id: PointerConstraintBackendId,
        anchor: CompositorOutputPosition,
    ) -> NativePointerConstraintBackendAction {
        if let Some(active) = self.active.as_ref() {
            if active.id == id {
                return NativePointerConstraintBackendAction::default();
            }
            return NativePointerConstraintBackendAction {
                failed: Some((id, "native pointer constraint already active")),
                ..NativePointerConstraintBackendAction::default()
            };
        }
        let constraint = NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Locked,
            anchor,
            region: None,
        };
        self.active = Some(constraint.clone());
        NativePointerConstraintBackendAction {
            activated: Some(constraint),
            ..NativePointerConstraintBackendAction::default()
        }
    }

    pub(crate) fn activate_confined(
        &mut self,
        id: PointerConstraintBackendId,
        anchor: CompositorOutputPosition,
        region: OutputRegion,
    ) -> NativePointerConstraintBackendAction {
        if let Some(active) = self.active.as_ref() {
            if active.id == id {
                return NativePointerConstraintBackendAction::default();
            }
            return NativePointerConstraintBackendAction {
                failed: Some((id, "native pointer constraint already active")),
                ..NativePointerConstraintBackendAction::default()
            };
        }
        let constraint = NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Confined,
            anchor,
            region: Some(region),
        };
        self.active = Some(constraint.clone());
        NativePointerConstraintBackendAction {
            activated: Some(constraint),
            ..NativePointerConstraintBackendAction::default()
        }
    }

    pub(crate) fn deactivate(
        &mut self,
        id: PointerConstraintBackendId,
        restore_position: Option<CompositorOutputPosition>,
    ) -> NativePointerConstraintBackendAction {
        let Some(active) = self.active.as_ref().cloned() else {
            return NativePointerConstraintBackendAction::default();
        };
        if active.id != id {
            return NativePointerConstraintBackendAction::default();
        }
        self.active = None;
        let restore_position = (active.mode == PointerConstraintMode::Locked)
            .then(|| restore_position.unwrap_or(active.anchor));
        NativePointerConstraintBackendAction {
            deactivated: Some(id),
            restore_position,
            ..NativePointerConstraintBackendAction::default()
        }
    }

    pub(crate) fn update_confined_region(
        &mut self,
        id: PointerConstraintBackendId,
        cursor_position: CompositorOutputPosition,
        region: OutputRegion,
    ) -> NativePointerConstraintBackendAction {
        let Some(active) = self.active.as_mut() else {
            return NativePointerConstraintBackendAction::default();
        };
        if active.id != id || active.mode != PointerConstraintMode::Confined {
            return NativePointerConstraintBackendAction::default();
        }
        active.region = Some(region.clone());
        let constrained = region.closest_point(cursor_position);
        NativePointerConstraintBackendAction {
            cursor_position: (constrained != cursor_position).then_some(constrained),
            ..NativePointerConstraintBackendAction::default()
        }
    }
}

impl Default for NativePointerConstraintBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeFrameRenderer {
    pub(crate) fn render_server_frame(
        &mut self,
        width: u32,
        height: u32,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
    ) -> NativeRenderedFrame<'_> {
        self.render_frame(NativeFrameRequest {
            width,
            height,
            surfaces: server.renderable_surfaces(),
            dock_items: server.shell_dock_items(),
            spotlight: input_state.spotlight(),
            shell_generation: input_state.shell_generation(),
            visual_state: input_state.desktop_visual_state(cursor_mode),
            render_generation: server.scene_render_generation(),
            client_cursor: server.client_cursor_render_state(),
        })
    }

    pub(crate) fn render_frame(
        &mut self,
        request: NativeFrameRequest<'_>,
    ) -> NativeRenderedFrame<'_> {
        let NativeFrameRequest {
            width,
            height,
            surfaces,
            dock_items,
            spotlight,
            shell_generation,
            visual_state,
            render_generation,
            client_cursor,
        } = request;
        let pixel_count = width.saturating_mul(height) as usize;
        self.frame.resize(pixel_count, 0);
        let shell_state = ShellOverlayState {
            topbar: ShellTopbarModel::visible("Oblivion One").with_trailing_text("Super+Space"),
            dock_items,
            spotlight: spotlight.clone(),
            generation: shell_generation,
        };
        let shell_overlay = self
            .shell_overlay_renderer
            .render(width, height, &shell_state);
        self.scene_renderer
            .compose_reusing_frame(DesktopComposeRequest {
                frame: &mut self.frame,
                frame_width: width,
                frame_height: height,
                output_scale: 1.0,
                surfaces,
                content_generation: native_scene_content_generation(
                    render_generation,
                    shell_overlay.generation,
                ),
                visual_state,
                shell_overlay: Some(shell_overlay),
                client_cursor,
            });
        NativeRenderedFrame {
            pixels: &self.frame,
            scene_rebuild_kind: self.scene_renderer.last_rebuild_kind(),
            frame_copy_kind: self.scene_renderer.last_frame_copy_kind(),
        }
    }

    pub(crate) fn egl_scene_draw_request<'a>(
        &'a mut self,
        width: u32,
        height: u32,
        server: &'a OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        current_damage: Option<OutputDamage>,
    ) -> EglSceneDrawRequest<'a> {
        let shell_state = ShellOverlayState {
            topbar: ShellTopbarModel::visible("Oblivion One").with_trailing_text("Super+Space"),
            dock_items: server.shell_dock_items(),
            spotlight: input_state.spotlight().clone(),
            generation: input_state.shell_generation(),
        };
        let shell_overlay = self
            .shell_overlay_renderer
            .render(width, height, &shell_state);
        EglSceneDrawRequest {
            width,
            height,
            surfaces: server.renderable_surfaces(),
            content_generation: native_scene_content_generation(
                server.scene_render_generation(),
                shell_overlay.generation,
            ),
            visual_state: input_state.desktop_visual_state(cursor_mode),
            output_scale: 1.0,
            shell_overlay: Some(shell_overlay),
            client_cursor: server.client_cursor_render_state(),
            current_damage,
        }
    }
}

pub(crate) struct NativeRenderedFrame<'a> {
    pub(crate) pixels: &'a [u32],
    pub(crate) scene_rebuild_kind: DesktopSceneRebuildKind,
    pub(crate) frame_copy_kind: DesktopFrameCopyKind,
}

pub(crate) struct NativeFrameRequest<'a> {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) surfaces: &'a [RenderableSurface],
    pub(crate) dock_items: Vec<ShellDockItem>,
    pub(crate) spotlight: &'a SpotlightModel,
    pub(crate) shell_generation: u64,
    pub(crate) visual_state: DesktopVisualState,
    pub(crate) render_generation: u64,
    pub(crate) client_cursor: Option<oblivion_one::compositor::ClientCursorRenderState<'a>>,
}

pub(crate) const fn native_scene_content_generation(
    render_generation: u64,
    shell_overlay_generation: u64,
) -> u64 {
    render_generation
        .wrapping_mul(1_000_003)
        .wrapping_add(shell_overlay_generation)
}
