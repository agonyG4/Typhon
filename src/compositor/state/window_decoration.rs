use std::time::Instant;

use super::super::decoration::{
    layout::DecorationLayout,
    render_plan::{DecorationRenderState, build_render_plan},
    types::{
        DecorationButtonKind, DecorationHit, DecorationMode, DecorationPreference,
        DecorationResizeEdge,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct WindowDecorationState {
    preference: DecorationPreference,
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
        }
    }
}

impl WindowDecorationState {
    pub(in crate::compositor) const fn new() -> Self {
        Self {
            preference: DecorationPreference::Unset,
        }
    }

    pub(in crate::compositor) const fn preference(self) -> DecorationPreference {
        self.preference
    }

    pub(in crate::compositor) const fn effective_mode(self, fullscreen: bool) -> DecorationMode {
        self.preference.effective_mode(true, fullscreen)
    }

    pub(in crate::compositor) fn set_preference(&mut self, preference: DecorationPreference) {
        self.preference = preference;
    }
}

fn effective_x11_decoration_mode(window: &DesktopWindow, mode: ToplevelMode) -> DecorationMode {
    if !matches!(window.backend, WindowBackend::X11(_))
        || window.kind != DesktopWindowKind::Managed
        || !window.is_normal_x11_role()
    {
        return DecorationMode::None;
    }
    if mode == ToplevelMode::Fullscreen {
        return DecorationMode::None;
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

impl super::super::CompositorState {
    pub(in crate::compositor) fn x11_effective_decoration_mode(
        &self,
        handle: crate::xwayland::X11WindowHandle,
    ) -> DecorationMode {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return DecorationMode::None;
        };
        self.window(window_id)
            .map_or(DecorationMode::None, |window| {
                effective_x11_decoration_mode(window, window.state.mode())
            })
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
        if mode == ToplevelMode::Fullscreen {
            return false;
        }
        let Some(window_id) = self.window_id_for_surface(surface_id) else {
            return false;
        };
        let Some(window) = self.window(window_id) else {
            return false;
        };
        if let Some(decoration_state) = self.xdg_decoration_states.get(&surface_id) {
            return decoration_state.preference().effective_mode(true, false)
                == DecorationMode::ServerSide;
        }
        effective_x11_decoration_mode(window, mode) == DecorationMode::ServerSide
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
        let decoration_mode =
            if let Some(decoration_state) = self.xdg_decoration_states.get(&root_surface_id) {
                decoration_state
                    .preference()
                    .effective_mode(true, fullscreen)
            } else if matches!(window.backend, WindowBackend::X11(_)) {
                effective_x11_decoration_mode(window, mode)
            } else {
                return None;
            };
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
            fullscreen,
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
        let metrics = self.decoration_theme.metrics();
        let origins = render::surface_origins(surfaces);
        surfaces
            .iter()
            .enumerate()
            .filter(|(_, surface)| surface.placement.parent_surface_id.is_none())
            .filter_map(|(index, surface)| {
                let window_id = self.window_id_for_surface(surface.surface_id)?;
                let window = self.window(window_id)?;
                let mode = window.state.mode();
                let fullscreen = mode == ToplevelMode::Fullscreen;
                let decoration_mode = if let Some(decoration_state) =
                    self.xdg_decoration_states.get(&surface.surface_id)
                {
                    decoration_state
                        .preference()
                        .effective_mode(true, fullscreen)
                } else if matches!(window.backend, WindowBackend::X11(_)) {
                    effective_x11_decoration_mode(window, mode)
                } else {
                    return None;
                };
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
                    fullscreen,
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
                Some(DecorationRenderInstance {
                    origin_x: root_origin_x.saturating_sub(layout.client.x),
                    origin_y: root_origin_y.saturating_sub(layout.client.y),
                    plan,
                    window_id,
                    root_surface_id: surface.surface_id,
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
        let decoration_mode = effective_x11_decoration_mode(window, window.state.mode());
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
