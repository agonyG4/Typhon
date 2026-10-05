use std::time::Instant;

use super::super::decoration::{
    layout::DecorationLayout,
    render_plan::{DecorationRenderState, build_render_plan},
    types::{
        CapturedXdgDecorationCommit, CapturedXdgDecorationCommitState,
        ConfiguredXdgDecorationState, DecorationButtonKind, DecorationHit, DecorationMode,
        DecorationObjectGeneration, DecorationPreference, DecorationResizeEdge,
    },
};
use super::super::{
    BeginWindowInteraction, DecorationRenderInstance, DesktopWindow, DesktopWindowKind,
    RenderGenerationCause, RenderableSurface, ResizeEdges, ToplevelMode, WindowBackend, WindowId,
    WindowInteractionKind, WindowInteractionSource,
};
use super::hit_testing::PointerSceneHit;
use super::surface_focus::WindowFocusReason;
use crate::compositor::render;
use crate::compositor::runtime_files::compositor_debug_surface_logging_enabled;
use crate::compositor::{WEnum, zxdg_toplevel_decoration_v1};
use crate::wm::WindowDecorationPolicy;
use wayland_server::Resource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct WindowDecorationState {
    preference: DecorationPreference,
    applied_mode: DecorationMode,
    applied_generation: Option<DecorationObjectGeneration>,
    current_generation: Option<DecorationObjectGeneration>,
    next_generation: u64,
    destruction_pending_commit: Option<DecorationObjectGeneration>,
    destruction_publication_pending: Option<DecorationObjectGeneration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct DecorationButtonCapture {
    pub window_id: WindowId,
    pub root_surface_id: u32,
    pub kind: DecorationButtonKind,
    pub button: u32,
}

impl Default for WindowDecorationState {
    fn default() -> Self {
        Self {
            preference: DecorationPreference::Unset,
            applied_mode: DecorationMode::ClientSide,
            applied_generation: Some(DecorationObjectGeneration(1)),
            current_generation: Some(DecorationObjectGeneration(1)),
            next_generation: 2,
            destruction_pending_commit: None,
            destruction_publication_pending: None,
        }
    }
}

impl WindowDecorationState {
    pub(in crate::compositor) const fn new() -> Self {
        Self {
            preference: DecorationPreference::Unset,
            applied_mode: DecorationMode::ClientSide,
            applied_generation: Some(DecorationObjectGeneration(1)),
            current_generation: Some(DecorationObjectGeneration(1)),
            next_generation: 2,
            destruction_pending_commit: None,
            destruction_publication_pending: None,
        }
    }

    pub(in crate::compositor) const fn with_published_mode(applied_mode: DecorationMode) -> Self {
        Self {
            applied_mode,
            ..Self::new()
        }
    }

    pub(in crate::compositor) fn requested_mode(self, fullscreen: bool) -> DecorationMode {
        self.preference
            .effective_mode(self.current_generation.is_some(), fullscreen)
    }

    pub(in crate::compositor) const fn applied_mode(self) -> DecorationMode {
        self.applied_mode
    }

    pub(in crate::compositor) const fn has_published_object_mode(self) -> bool {
        self.current_generation.is_some()
            || self.destruction_pending_commit.is_some()
            || self.destruction_publication_pending.is_some()
    }

    pub(in crate::compositor) fn set_preference(
        &mut self,
        preference: DecorationPreference,
    ) -> bool {
        if self.preference == preference {
            return false;
        }
        self.preference = preference;
        true
    }

    pub(in crate::compositor) fn set_preference_for_generation(
        &mut self,
        generation: DecorationObjectGeneration,
        preference: DecorationPreference,
    ) -> bool {
        if self.current_generation != Some(generation) {
            return false;
        }
        self.set_preference(preference)
    }

    pub(in crate::compositor) fn apply_configured_mode(&mut self, mode: DecorationMode) -> bool {
        if self.applied_mode == mode {
            return false;
        }
        self.applied_mode = mode;
        true
    }

    pub(in crate::compositor) const fn current_generation(
        self,
    ) -> Option<DecorationObjectGeneration> {
        self.current_generation
    }

    pub(in crate::compositor) fn destroy_object(
        &mut self,
        generation: DecorationObjectGeneration,
    ) -> bool {
        if self.current_generation != Some(generation) {
            return false;
        }
        self.current_generation = None;
        self.destruction_pending_commit = Some(generation);
        true
    }

    #[cfg(test)]
    pub(in crate::compositor) fn recreate_object(&mut self) -> DecorationObjectGeneration {
        let baseline = self.applied_mode;
        self.recreate_object_with_published_mode(baseline)
    }

    pub(in crate::compositor) fn recreate_object_with_published_mode(
        &mut self,
        applied_mode: DecorationMode,
    ) -> DecorationObjectGeneration {
        if let Some(generation) = self.current_generation {
            return generation;
        }
        if self.destruction_pending_commit.is_none()
            && self.destruction_publication_pending.is_none()
        {
            self.applied_mode = applied_mode;
        }
        let generation = DecorationObjectGeneration(self.next_generation);
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .expect("XDG decoration generation exhausted");
        self.current_generation = Some(generation);
        self.preference = DecorationPreference::Unset;
        if self.destruction_pending_commit.is_none()
            && self.destruction_publication_pending.is_none()
        {
            self.applied_generation = Some(generation);
        }
        generation
    }

    pub(in crate::compositor) fn capture_surface_commit_decoration(
        &mut self,
        acknowledged: Option<ConfiguredXdgDecorationState>,
    ) -> (
        Option<CapturedXdgDecorationCommitState>,
        Option<DecorationObjectGeneration>,
    ) {
        let stale_generation = acknowledged
            .filter(|configured| self.current_generation != Some(configured.generation))
            .map(|configured| configured.generation);
        let configured = acknowledged
            .filter(|configured| self.current_generation == Some(configured.generation))
            .map(CapturedXdgDecorationCommitState::Configured);

        let captured = if let Some(generation) = self.destruction_pending_commit.take()
            && self.current_generation.is_none()
        {
            self.destruction_publication_pending = Some(generation);
            Some(CapturedXdgDecorationCommitState::DecorationDestroyed { generation })
        } else {
            configured
        };
        (captured, stale_generation)
    }

    fn apply_captured_commit(&mut self, captured: CapturedXdgDecorationCommitState) -> bool {
        match captured {
            CapturedXdgDecorationCommitState::Configured(configured) => {
                if self.current_generation != Some(configured.generation) {
                    return false;
                }
                let changed = self.apply_configured_mode(configured.mode);
                self.applied_generation = Some(configured.generation);
                changed
            }
            CapturedXdgDecorationCommitState::DecorationDestroyed { generation } => {
                if self.destruction_publication_pending != Some(generation) {
                    return false;
                }
                self.destruction_publication_pending = None;
                if self.applied_generation != Some(generation) {
                    return false;
                }
                let changed = self.apply_configured_mode(DecorationMode::ClientSide);
                self.applied_generation = None;
                changed
            }
        }
    }
}

fn effective_decoration_mode(
    window: &DesktopWindow,
    xdg_state: Option<&WindowDecorationState>,
    mode: ToplevelMode,
) -> DecorationMode {
    if window.kind != DesktopWindowKind::Managed {
        return DecorationMode::None;
    }
    match window.backend {
        WindowBackend::Xdg(_) => {
            if let Some(decoration_state) = xdg_state
                && decoration_state.has_published_object_mode()
                && decoration_state.applied_generation.is_some()
            {
                return decoration_state.applied_mode();
            }
            if mode == ToplevelMode::Fullscreen {
                return DecorationMode::None;
            }
            match window.decoration_policy {
                WindowDecorationPolicy::ClientPreference => DecorationMode::ClientSide,
                WindowDecorationPolicy::Server => DecorationMode::ServerSide,
            }
        }
        WindowBackend::X11(_) => {
            if !window.is_normal_x11_role() || mode == ToplevelMode::Fullscreen {
                return DecorationMode::None;
            }
            if window.decoration_policy == WindowDecorationPolicy::Server {
                return DecorationMode::ServerSide;
            }
            if window
                .x11_decoration_hints
                .gtk_frame_extents
                .is_some_and(|extents| extents.is_non_zero())
            {
                return DecorationMode::ClientSide;
            }
            if window.x11_decoration_hints.motif
                == crate::xwayland::xwm::X11MotifDecorationHint::Undecorated
            {
                return DecorationMode::None;
            }
            DecorationMode::ServerSide
        }
    }
}

impl super::super::CompositorState {
    fn effective_decoration_mode_for_window(
        &self,
        window: &DesktopWindow,
        mode: ToplevelMode,
    ) -> DecorationMode {
        let xdg_state = matches!(window.backend, WindowBackend::Xdg(_))
            .then(|| self.xdg_decoration_states.get(&window.root_surface_id))
            .flatten();
        effective_decoration_mode(window, xdg_state, mode)
    }

    pub(in crate::compositor) fn effective_window_decoration_mode(
        &self,
        window_id: WindowId,
    ) -> DecorationMode {
        self.window(window_id)
            .map_or(DecorationMode::None, |window| {
                self.effective_decoration_mode_for_window(window, window.state.mode())
            })
    }

    pub(in crate::compositor) fn effective_window_decoration_mode_for_surface(
        &self,
        surface_id: u32,
        mode: ToplevelMode,
    ) -> DecorationMode {
        self.window_id_for_surface(surface_id)
            .and_then(|window_id| self.window(window_id))
            .map_or(DecorationMode::None, |window| {
                self.effective_decoration_mode_for_window(window, mode)
            })
    }

    pub(in crate::compositor) fn set_window_decoration_policy(
        &mut self,
        window_id: WindowId,
        policy: WindowDecorationPolicy,
    ) -> bool {
        let Some(window) = self.window(window_id) else {
            return false;
        };
        let old_policy = window.decoration_policy;
        if old_policy == policy {
            return false;
        }
        let root_surface_id = window.root_surface_id;
        let backend = window.backend;
        let mode = window.state.mode();
        let old_effective = self.effective_decoration_mode_for_window(window, mode);
        let old_xdg_configure = matches!(backend, WindowBackend::Xdg(_))
            .then(|| self.xdg_decoration_mode_for_configure(root_surface_id))
            .flatten()
            .map(|configured| configured.mode);
        let old_frame_extents = matches!(backend, WindowBackend::X11(_))
            .then(|| match backend {
                WindowBackend::X11(handle) => self.x11_decoration_frame_extents(handle),
                WindowBackend::Xdg(_) => [0; 4],
            })
            .unwrap_or([0; 4]);

        self.window_mut(window_id)
            .expect("window was resolved before policy update")
            .decoration_policy = policy;

        let new_effective = self.effective_window_decoration_mode(window_id);
        if let WindowBackend::Xdg(_) = backend {
            let new_xdg_configure = self
                .xdg_decoration_mode_for_configure(root_surface_id)
                .map(|configured| configured.mode);
            if old_xdg_configure != new_xdg_configure
                && self.xdg_decoration_resources.contains_key(&root_surface_id)
            {
                self.configure_xdg_surface_for_decoration(root_surface_id);
            }
        }

        if let WindowBackend::X11(handle) = backend {
            let new_frame_extents = self.x11_decoration_frame_extents(handle);
            if (old_effective != new_effective || old_frame_extents != new_frame_extents)
                && let Some(geometry) = self
                    .window(window_id)
                    .and_then(|window| window.x11_geometry)
                    .map(|geometry| geometry.frame)
            {
                self.queue_backend_configure(window_id, geometry, mode, false);
            }
            if old_effective != new_effective {
                self.reconcile_x11_decoration_transition(handle, old_effective, new_effective);
            }
        } else if old_effective != new_effective {
            self.reconcile_native_decoration_transition(window_id, root_surface_id, new_effective);
        }

        if old_effective != new_effective {
            self.advance_render_generation(RenderGenerationCause::WindowDecoration);
            self.refresh_pointer_focus_at_last_position();
        }

        if compositor_debug_surface_logging_enabled() {
            let backend_name = match backend {
                WindowBackend::Xdg(_) => "Xdg",
                WindowBackend::X11(_) => "X11",
            };
            let has_xdg_decoration_object =
                self.xdg_decoration_resources.contains_key(&root_surface_id);
            let chrome = self
                .window(window_id)
                .and_then(|window| window.management)
                .map_or(crate::wm::WindowChromePolicy::Full, |management| {
                    management.chrome_policy()
                });
            eprintln!(
                "oblivion-one compositor: event=window_decoration_policy_change window_id={} backend={backend_name} old_policy={old_policy:?} new_policy={policy:?} old_effective={old_effective:?} new_effective={new_effective:?} has_xdg_decoration_object={has_xdg_decoration_object} fullscreen={} chrome={chrome:?}",
                window_id.get(),
                mode == ToplevelMode::Fullscreen,
            );
        }
        true
    }

    pub(in crate::compositor) fn x11_effective_decoration_mode(
        &self,
        handle: crate::xwayland::X11WindowHandle,
    ) -> DecorationMode {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return DecorationMode::None;
        };
        self.effective_window_decoration_mode(window_id)
    }

    pub(in crate::compositor) fn reconcile_x11_decoration_transition(
        &mut self,
        handle: crate::xwayland::X11WindowHandle,
        old_mode: DecorationMode,
        new_mode: DecorationMode,
    ) {
        if old_mode == new_mode {
            return;
        }
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return;
        };
        let Some(root_surface_id) = self.window(window_id).map(|window| window.root_surface_id)
        else {
            return;
        };
        self.decoration_button_capture = self
            .decoration_button_capture
            .filter(|capture| capture.root_surface_id != root_surface_id);
        self.decoration_button_hover = self
            .decoration_button_hover
            .filter(|(hover_window_id, _)| *hover_window_id != window_id);
        self.decoration_titlebar_click_capture = self
            .decoration_titlebar_click_capture
            .filter(|(captured_window_id, _)| *captured_window_id != window_id);
        self.decoration_last_titlebar_click = self
            .decoration_last_titlebar_click
            .filter(|(clicked_window_id, _, _, _)| *clicked_window_id != window_id);

        let interaction_is_decoration_owned =
            self.window_interaction_debug_snapshot()
                .is_some_and(|interaction| {
                    interaction.root_surface_id == root_surface_id
                        && interaction.source == WindowInteractionSource::NativeBinding
                        && interaction.decoration_owned
                        && matches!(
                            interaction.kind,
                            WindowInteractionKind::Move | WindowInteractionKind::Resize(_)
                        )
                });
        if new_mode != DecorationMode::ServerSide && interaction_is_decoration_owned {
            self.clear_window_interaction_state(
                super::super::WindowInteractionEndReason::ModeTransition,
            );
        }
    }

    pub(in crate::compositor) fn surface_uses_server_side_decorations(
        &self,
        surface_id: u32,
        mode: ToplevelMode,
    ) -> bool {
        let Some(window_id) = self.window_id_for_surface(surface_id) else {
            return false;
        };
        let Some(window) = self.window(window_id) else {
            return false;
        };
        let _ = window;
        self.effective_window_decoration_mode_for_surface(surface_id, mode)
            == DecorationMode::ServerSide
    }

    pub(in crate::compositor) fn reconcile_native_decoration_transition(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        new_mode: DecorationMode,
    ) {
        self.decoration_button_capture = self
            .decoration_button_capture
            .filter(|capture| capture.root_surface_id != root_surface_id);
        self.decoration_button_hover = self
            .decoration_button_hover
            .filter(|(hover_window_id, _)| *hover_window_id != window_id);
        self.decoration_titlebar_click_capture = self
            .decoration_titlebar_click_capture
            .filter(|(captured_window_id, _)| *captured_window_id != window_id);
        self.decoration_last_titlebar_click = self
            .decoration_last_titlebar_click
            .filter(|(clicked_window_id, _, _, _)| *clicked_window_id != window_id);

        let interaction_is_decoration_owned =
            self.window_interaction_debug_snapshot()
                .is_some_and(|interaction| {
                    interaction.root_surface_id == root_surface_id
                        && interaction.source == WindowInteractionSource::NativeBinding
                        && interaction.decoration_owned
                        && matches!(
                            interaction.kind,
                            WindowInteractionKind::Move | WindowInteractionKind::Resize(_)
                        )
                });
        if new_mode != DecorationMode::ServerSide && interaction_is_decoration_owned {
            self.clear_window_interaction_state(
                super::super::WindowInteractionEndReason::ModeTransition,
            );
        }
    }

    pub(in crate::compositor) fn xdg_decoration_mode_for_configure(
        &self,
        surface_id: u32,
    ) -> Option<ConfiguredXdgDecorationState> {
        let decoration_state = self.xdg_decoration_states.get(&surface_id)?;
        let generation = decoration_state.current_generation()?;
        let window = self
            .window_id_for_surface(surface_id)
            .and_then(|window_id| self.window(window_id))?;
        let fullscreen = window.state.mode() == ToplevelMode::Fullscreen;
        let mode = if fullscreen {
            DecorationMode::None
        } else if window.decoration_policy == WindowDecorationPolicy::Server {
            DecorationMode::ServerSide
        } else {
            decoration_state.requested_mode(false)
        };
        Some(ConfiguredXdgDecorationState { generation, mode })
    }

    pub(in crate::compositor) fn xdg_decoration_configure_event_needed(
        &self,
        surface_id: u32,
        decoration: ConfiguredXdgDecorationState,
        force: bool,
    ) -> bool {
        force
            || self
                .xdg_surface_lifecycle(surface_id)
                .and_then(|lifecycle| lifecycle.last_configured_decoration)
                != Some(decoration)
    }

    pub(in crate::compositor) fn send_xdg_decoration_configure(
        &self,
        surface_id: u32,
        decoration_state: ConfiguredXdgDecorationState,
    ) {
        if self
            .xdg_decoration_states
            .get(&surface_id)
            .and_then(|state| state.current_generation())
            != Some(decoration_state.generation)
        {
            return;
        }
        let Some(decoration) = self.xdg_decoration_resources.get(&surface_id) else {
            return;
        };
        let wire_mode = match decoration_state.mode {
            DecorationMode::ServerSide | DecorationMode::None => {
                zxdg_toplevel_decoration_v1::Mode::ServerSide
            }
            DecorationMode::ClientSide => zxdg_toplevel_decoration_v1::Mode::ClientSide,
        };
        let _ = decoration.send_event(zxdg_toplevel_decoration_v1::Event::Configure {
            mode: WEnum::Value(wire_mode),
        });
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: event=xdg_decoration_configure surface={surface_id} generation={} mode={:?}",
                decoration_state.generation.0, decoration_state.mode,
            );
        }
    }

    pub(in crate::compositor) fn capture_xdg_decoration_commit_state(
        &mut self,
        surface_id: u32,
        commit_sequence: super::super::SurfaceCommitSequence,
    ) -> Option<CapturedXdgDecorationCommit> {
        let acknowledged = self.take_acked_xdg_decoration(surface_id);
        let Some(decoration_state) = self.xdg_decoration_states.get_mut(&surface_id) else {
            if let Some(acknowledged) = acknowledged
                && compositor_debug_surface_logging_enabled()
            {
                eprintln!(
                    "oblivion-one compositor: event=xdg_decoration_stale_ack surface={surface_id} commit_sequence={} generation={} current_generation=none",
                    commit_sequence.0, acknowledged.generation.0,
                );
            }
            return None;
        };
        let current_generation = decoration_state.current_generation();
        let (captured, stale_generation) =
            decoration_state.capture_surface_commit_decoration(acknowledged);
        if let Some(generation) = stale_generation
            && compositor_debug_surface_logging_enabled()
        {
            eprintln!(
                "oblivion-one compositor: event=xdg_decoration_stale_ack surface={surface_id} commit_sequence={} generation={} current_generation={:?}",
                commit_sequence.0,
                generation.0,
                current_generation.map(|generation| generation.0),
            );
        }
        let captured = captured.map(|state| CapturedXdgDecorationCommit {
            state,
            commit_sequence,
        });
        if compositor_debug_surface_logging_enabled()
            && let Some(captured) = captured
        {
            match captured.state {
                CapturedXdgDecorationCommitState::Configured(configured) => eprintln!(
                    "oblivion-one compositor: event=xdg_decoration_commit_capture surface={surface_id} commit_sequence={} generation={} mode={:?}",
                    captured.commit_sequence.0, configured.generation.0, configured.mode,
                ),
                CapturedXdgDecorationCommitState::DecorationDestroyed { generation } => eprintln!(
                    "oblivion-one compositor: event=xdg_decoration_commit_capture surface={surface_id} commit_sequence={} generation={} destroyed=true mode=ClientSide",
                    captured.commit_sequence.0, generation.0,
                ),
            }
        }
        captured
    }

    pub(in crate::compositor) fn apply_captured_xdg_decoration(
        &mut self,
        surface_id: u32,
        commit_sequence: super::super::SurfaceCommitSequence,
        captured: Option<CapturedXdgDecorationCommit>,
    ) -> bool {
        let Some(captured) = captured else {
            return false;
        };
        let captured_commit_sequence = captured.commit_sequence;
        let (generation, mode) = match captured.state {
            CapturedXdgDecorationCommitState::Configured(configured) => {
                (configured.generation, configured.mode)
            }
            CapturedXdgDecorationCommitState::DecorationDestroyed { generation } => {
                (generation, DecorationMode::ClientSide)
            }
        };
        let window_id = self.window_id_for_surface(surface_id);
        let old_effective_mode =
            window_id.map(|window_id| self.effective_window_decoration_mode(window_id));
        let Some(decoration_state) = self.xdg_decoration_states.get_mut(&surface_id) else {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: event=xdg_decoration_publish_dropped surface={surface_id} commit_sequence={} captured_commit_sequence={} generation={} reason=decoration_state_retired",
                    commit_sequence.0, captured_commit_sequence.0, generation.0,
                );
            }
            return false;
        };
        let raw_mode_changed = decoration_state.apply_captured_commit(captured.state);
        let new_effective_mode =
            window_id.map(|window_id| self.effective_window_decoration_mode(window_id));
        let effective_mode_changed = old_effective_mode != new_effective_mode;
        if effective_mode_changed
            && let (Some(window_id), Some(new_mode)) = (window_id, new_effective_mode)
        {
            self.reconcile_native_decoration_transition(window_id, surface_id, new_mode);
        }
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: event=xdg_decoration_publish surface={surface_id} commit_sequence={} captured_commit_sequence={} generation={} mode={mode:?} applied={raw_mode_changed} effective_changed={effective_mode_changed}",
                commit_sequence.0, captured_commit_sequence.0, generation.0,
            );
        }
        effective_mode_changed
    }

    pub(in crate::compositor) fn update_decoration_hover(&mut self) {
        let hit = self.pointer_scene_hit_at(self.last_pointer_x, self.last_pointer_y);
        self.update_decoration_hover_for_scene_hit(&hit);
    }

    pub(in crate::compositor) fn update_decoration_hover_for_scene_hit(
        &mut self,
        hit: &PointerSceneHit,
    ) {
        let next = match hit {
            PointerSceneHit::Decoration {
                window_id,
                hit: DecorationHit::Button(kind),
                ..
            } => Some((*window_id, *kind)),
            _ => None,
        };
        if self.decoration_button_hover == next {
            return;
        }
        self.decoration_button_hover = next;
        self.advance_render_generation(RenderGenerationCause::WindowDecoration);
    }

    pub(in crate::compositor) fn decoration_hit_for_root_at(
        &self,
        root_surface_id: u32,
        root_origin: (i32, i32),
        x: f64,
        y: f64,
    ) -> Option<DecorationHit> {
        let window_id = self.window_id_for_surface(root_surface_id)?;
        let window = self.window(window_id)?;
        let visual_geometry = self.current_visual_root_window_geometry(root_surface_id)?;
        let mode = window.state.mode();
        let fullscreen = mode == ToplevelMode::Fullscreen;
        let decoration_mode = self.effective_decoration_mode_for_window(window, mode);
        let decoration_fullscreen = fullscreen && decoration_mode == DecorationMode::None;
        let chrome_policy = window
            .management
            .map_or(crate::wm::WindowChromePolicy::Full, |management| {
                management.chrome_policy()
            });
        let layout = DecorationLayout::for_window_with_chrome_policy(
            visual_geometry.width,
            visual_geometry.height,
            decoration_mode,
            mode == ToplevelMode::Maximized,
            decoration_fullscreen,
            chrome_policy,
            self.decoration_theme.metrics(),
        )?;
        let local_x = x - f64::from(root_origin.0) + f64::from(layout.client.x);
        let local_y = y - f64::from(root_origin.1) + f64::from(layout.client.y);
        layout.hit_test(local_x, local_y).or_else(|| {
            let tiled_minimal = chrome_policy == crate::wm::WindowChromePolicy::Minimal
                && window.management.is_some_and(|management| {
                    management.layout() == crate::wm::LayoutMembership::Tiled
                });
            if !tiled_minimal {
                return None;
            }
            let edge = layout.logical_resize_edge_at(local_x, local_y)?;
            let edges = resize_edges_for_decoration_edge(edge);
            self.prepare_tiled_resize(window_id, edges)
                .is_some()
                .then_some(DecorationHit::Resize(edge))
        })
    }

    pub(in crate::compositor) fn decoration_theme_status(
        &self,
    ) -> (String, String, u32, u64, String, Option<String>) {
        (
            self.decoration_theme.name().to_string(),
            self.decoration_theme.name().to_string(),
            self.decoration_theme.schema_version(),
            self.decoration_theme.generation(),
            self.decoration_theme.source().to_string(),
            self.decoration_theme_error.clone(),
        )
    }

    pub(in crate::compositor) fn available_decoration_themes(&self) -> Vec<String> {
        super::super::decoration::theme::available_theme_names()
    }

    pub(in crate::compositor) fn set_decoration_theme(&mut self, name: &str) -> Result<(), String> {
        let generation = self.decoration_theme.generation().saturating_add(1);
        let theme = super::super::decoration::theme::load_theme_by_name(name, generation)
            .map_err(|error| error.to_string())?;
        if let Err(error) = super::super::decoration::theme::write_selected_theme(name) {
            self.decoration_theme_error = Some(error.to_string());
            return Err(error.to_string());
        }
        self.decoration_theme = theme;
        self.decoration_theme_error = None;
        let x11_windows = self
            .desktop_windows
            .values()
            .filter(|window| matches!(window.backend, WindowBackend::X11(_)))
            .map(|window| window.id)
            .collect::<Vec<_>>();
        for window_id in x11_windows {
            self.queue_backend_state(window_id);
        }
        self.advance_render_generation(RenderGenerationCause::WindowDecoration);
        Ok(())
    }

    pub(in crate::compositor) fn reload_decoration_theme(&mut self) -> Result<(), String> {
        let name = self.decoration_theme.name().to_string();
        self.set_decoration_theme(&name)
    }

    pub(in crate::compositor) fn load_persisted_decoration_theme(&mut self) {
        let name = match super::super::decoration::theme::read_selected_theme() {
            Ok(Some(name)) => name,
            Ok(None) => return,
            Err(error) => {
                self.decoration_theme_error = Some(error.to_string());
                return;
            }
        };
        let generation = self.decoration_theme.generation().saturating_add(1);
        match super::super::decoration::theme::load_theme_by_name(&name, generation) {
            Ok(theme) => {
                self.decoration_theme = theme;
                self.decoration_theme_error = None;
            }
            Err(error) => self.decoration_theme_error = Some(error.to_string()),
        }
    }

    #[cfg(test)]
    pub(in crate::compositor) fn handle_decoration_button(
        &mut self,
        button: u32,
        pressed: bool,
    ) -> bool {
        let hit = self.pointer_scene_hit_at(self.last_pointer_x, self.last_pointer_y);
        self.handle_decoration_button_with_hit(Some(&hit), button, pressed)
    }

    pub(in crate::compositor) fn handle_decoration_button_with_hit(
        &mut self,
        scene_hit: Option<&PointerSceneHit>,
        button: u32,
        pressed: bool,
    ) -> bool {
        const LEFT_BUTTON: u32 = 0x110;
        if !pressed
            && self
                .decoration_titlebar_click_capture
                .is_some_and(|(_, captured_button)| captured_button == button)
        {
            self.decoration_titlebar_click_capture = None;
            self.advance_render_generation(RenderGenerationCause::WindowDecoration);
            return true;
        }
        if pressed {
            let Some(PointerSceneHit::Decoration {
                window_id,
                root_surface_id,
                hit: decoration_hit,
            }) = scene_hit
            else {
                return false;
            };
            if button != LEFT_BUTTON {
                return true;
            }
            if let DecorationHit::Titlebar = decoration_hit {
                const DOUBLE_CLICK_WINDOW: std::time::Duration =
                    std::time::Duration::from_millis(500);
                const DOUBLE_CLICK_DISTANCE: f64 = 8.0;
                let now = Instant::now();
                let double_click = self.decoration_last_titlebar_click.take().is_some_and(
                    |(prior_window_id, prior_time, prior_x, prior_y)| {
                        let delta_x = self.last_pointer_x - prior_x;
                        let delta_y = self.last_pointer_y - prior_y;
                        prior_window_id == *window_id
                            && now.duration_since(prior_time) <= DOUBLE_CLICK_WINDOW
                            && delta_x.mul_add(delta_x, delta_y * delta_y)
                                <= DOUBLE_CLICK_DISTANCE * DOUBLE_CLICK_DISTANCE
                    },
                );
                if double_click {
                    let _ = self.toggle_maximize_desktop_window(*window_id);
                    self.decoration_titlebar_click_capture = Some((*window_id, button));
                    self.advance_render_generation(RenderGenerationCause::WindowDecoration);
                    return true;
                }
                self.decoration_last_titlebar_click =
                    Some((*window_id, now, self.last_pointer_x, self.last_pointer_y));
                let _ = self.begin_window_interaction_for_root(BeginWindowInteraction {
                    window_id: Some(*window_id),
                    root_surface_id: *root_surface_id,
                    x: self.last_pointer_x,
                    y: self.last_pointer_y,
                    kind: WindowInteractionKind::Move,
                    source: WindowInteractionSource::NativeBinding,
                    trigger_button: Some(button),
                    trigger_serial: None,
                    pointer_motion_surface_id: None,
                    decoration_owned: true,
                });
                return true;
            }
            return match decoration_hit {
                DecorationHit::Button(kind) => {
                    let _ =
                        self.activate_desktop_window(*window_id, WindowFocusReason::PointerPress);
                    self.decoration_button_capture = Some(DecorationButtonCapture {
                        window_id: *window_id,
                        root_surface_id: *root_surface_id,
                        kind: *kind,
                        button,
                    });
                    self.advance_render_generation(RenderGenerationCause::WindowDecoration);
                    true
                }
                DecorationHit::Resize(edge) => {
                    let _ =
                        self.activate_desktop_window(*window_id, WindowFocusReason::PointerPress);
                    let _ = self.begin_window_interaction_for_root(BeginWindowInteraction {
                        window_id: Some(*window_id),
                        root_surface_id: *root_surface_id,
                        x: self.last_pointer_x,
                        y: self.last_pointer_y,
                        kind: WindowInteractionKind::Resize(resize_edges_for_decoration_edge(
                            *edge,
                        )),
                        source: WindowInteractionSource::NativeBinding,
                        trigger_button: Some(button),
                        trigger_serial: None,
                        pointer_motion_surface_id: None,
                        decoration_owned: true,
                    });
                    true
                }
                DecorationHit::Titlebar => unreachable!("titlebar handled above"),
            };
        }

        let Some(capture) = self.decoration_button_capture.take() else {
            return scene_hit.is_some_and(|hit| matches!(hit, PointerSceneHit::Decoration { .. }));
        };
        if capture.button != button {
            self.advance_render_generation(RenderGenerationCause::WindowDecoration);
            return true;
        }
        let same_button = matches!(
            scene_hit,
            Some(PointerSceneHit::Decoration {
                window_id,
                hit: DecorationHit::Button(kind),
                ..
            }) if *window_id == capture.window_id && *kind == capture.kind
        );
        if same_button {
            match capture.kind {
                DecorationButtonKind::Minimize => {
                    let _ = self.minimize_desktop_window_outcome(capture.window_id);
                }
                DecorationButtonKind::MaximizeRestore => {
                    let _ = self.toggle_maximize_desktop_window(capture.window_id);
                }
                DecorationButtonKind::Close => {
                    let _ = self.close_desktop_window_outcome(capture.window_id);
                }
            }
        }
        self.advance_render_generation(RenderGenerationCause::WindowDecoration);
        true
    }

    pub(in crate::compositor) fn decoration_hit_at(
        &mut self,
        x: f64,
        y: f64,
    ) -> Option<(WindowId, u32, DecorationHit)> {
        match self.pointer_scene_hit_at(x, y) {
            PointerSceneHit::Decoration {
                window_id,
                root_surface_id,
                hit,
            } => Some((window_id, root_surface_id, hit)),
            PointerSceneHit::Client { .. } | PointerSceneHit::None => None,
        }
    }

    pub(in crate::compositor) fn native_decoration_render_instances(
        &self,
        surfaces: &[RenderableSurface],
    ) -> Vec<DecorationRenderInstance> {
        self.native_decoration_render_instances_for_scale(surfaces, 1.0)
    }

    pub(in crate::compositor) fn native_decoration_render_instances_for_scale(
        &self,
        surfaces: &[RenderableSurface],
        output_scale: f64,
    ) -> Vec<DecorationRenderInstance> {
        let origins = render::surface_origins(surfaces);
        self.native_decoration_render_instances_for_scale_with_origins(
            surfaces,
            &origins,
            output_scale,
        )
    }

    pub(in crate::compositor) fn native_decoration_render_instances_for_scale_with_origins(
        &self,
        surfaces: &[RenderableSurface],
        origins: &[(i32, i32)],
        output_scale: f64,
    ) -> Vec<DecorationRenderInstance> {
        let metrics = self.decoration_theme.metrics();
        surfaces
            .iter()
            .enumerate()
            .filter(|(_, surface)| surface.placement.parent_surface_id.is_none())
            .filter_map(|(index, surface)| {
                let window_id = self.window_id_for_surface(surface.surface_id)?;
                let window = self.window(window_id)?;
                let mode = window.state.mode();
                let fullscreen = mode == ToplevelMode::Fullscreen;
                let decoration_mode = self.effective_decoration_mode_for_window(window, mode);
                let decoration_fullscreen = fullscreen && decoration_mode == DecorationMode::None;
                if decoration_mode != DecorationMode::ServerSide {
                    return None;
                }
                let visual_geometry =
                    self.current_visual_root_window_geometry(surface.surface_id)?;
                let chrome_policy = window
                    .management
                    .map_or(crate::wm::WindowChromePolicy::Full, |management| {
                        management.chrome_policy()
                    });
                let layout = DecorationLayout::for_window_with_chrome_policy(
                    visual_geometry.width,
                    visual_geometry.height,
                    decoration_mode,
                    mode == ToplevelMode::Maximized,
                    decoration_fullscreen,
                    chrome_policy,
                    metrics,
                )?;
                let (root_origin_x, root_origin_y) = origins.get(index).copied()?;
                let instance_origin_x = root_origin_x.saturating_sub(layout.client.x);
                let instance_origin_y = root_origin_y.saturating_sub(layout.client.y);
                let pressed = self
                    .decoration_button_capture
                    .filter(|capture| capture.window_id == window_id)
                    .filter(|capture| {
                        matches!(
                            layout.hit_test(
                                self.last_pointer_x - f64::from(instance_origin_x),
                                self.last_pointer_y - f64::from(instance_origin_y),
                            ),
                            Some(DecorationHit::Button(kind)) if kind == capture.kind
                        )
                    })
                    .map(|capture| capture.kind);
                let plan = build_render_plan(
                    &layout,
                    &self.decoration_theme,
                    window.metadata.title.as_deref().unwrap_or_default(),
                    DecorationRenderState {
                        active: self.focused_window_id == Some(window_id),
                        maximized: mode == ToplevelMode::Maximized,
                        hovered: self
                            .decoration_button_hover
                            .filter(|(hover_window_id, _)| *hover_window_id == window_id)
                            .map(|(_, kind)| kind),
                        pressed,
                    },
                    output_scale,
                );
                let scene_node_id = self
                    .scene_node_id_for_server_decoration(window_id)
                    .expect("server decoration has no canonical scene node");
                Some(DecorationRenderInstance {
                    origin_x: root_origin_x.saturating_sub(layout.client.x),
                    origin_y: root_origin_y.saturating_sub(layout.client.y),
                    plan,
                    window_id,
                    root_surface_id: surface.surface_id,
                    scene_node_id,
                })
            })
            .collect()
    }

    pub(in crate::compositor) fn x11_decoration_frame_extents(
        &self,
        handle: crate::xwayland::X11WindowHandle,
    ) -> [u32; 4] {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return [0; 4];
        };
        let Some(window) = self.window(window_id) else {
            return [0; 4];
        };
        let decoration_mode =
            self.effective_decoration_mode_for_window(window, window.state.mode());
        if decoration_mode != DecorationMode::ServerSide {
            return [0; 4];
        }
        let mode = window.state.mode();
        let chrome_policy = window
            .management
            .map_or(crate::wm::WindowChromePolicy::Full, |management| {
                management.chrome_policy()
            });
        let Some(layout) = DecorationLayout::for_window_with_chrome_policy(
            1,
            1,
            decoration_mode,
            mode == ToplevelMode::Maximized,
            mode == ToplevelMode::Fullscreen,
            chrome_policy,
            self.decoration_theme.metrics(),
        ) else {
            return [0; 4];
        };
        [
            layout.extents.left,
            layout.extents.right,
            layout.extents.top,
            layout.extents.bottom,
        ]
    }
}

pub(in crate::compositor) fn resize_edges_for_decoration_edge(
    edge: DecorationResizeEdge,
) -> ResizeEdges {
    match edge {
        DecorationResizeEdge::Top => ResizeEdges::new(true, false, false, false),
        DecorationResizeEdge::Right => ResizeEdges::new(false, false, false, true),
        DecorationResizeEdge::Bottom => ResizeEdges::new(false, true, false, false),
        DecorationResizeEdge::Left => ResizeEdges::new(false, false, true, false),
        DecorationResizeEdge::TopRight => ResizeEdges::new(true, false, false, true),
        DecorationResizeEdge::BottomRight => ResizeEdges::new(false, true, false, true),
        DecorationResizeEdge::BottomLeft => ResizeEdges::new(false, true, true, false),
        DecorationResizeEdge::TopLeft => ResizeEdges::new(true, false, true, false),
    }
}
