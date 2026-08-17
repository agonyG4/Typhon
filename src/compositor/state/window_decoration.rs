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
    DecorationRenderInstance, RenderGenerationCause, RenderableSurface, ResizeEdges, ToplevelMode,
    WindowBackend, WindowId,
};
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

impl super::super::CompositorState {
    pub(in crate::compositor) fn update_decoration_hover(&mut self) {
        let next = match self.decoration_hit_at(self.last_pointer_x, self.last_pointer_y) {
            Some((window_id, _, DecorationHit::Button(kind))) => Some((window_id, kind)),
            _ => None,
        };
        if self.decoration_button_hover == next {
            return;
        }
        self.decoration_button_hover = next;
        self.advance_render_generation(RenderGenerationCause::WindowDecoration);
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

    pub(in crate::compositor) fn handle_decoration_button(
        &mut self,
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
        if button != LEFT_BUTTON && self.decoration_button_capture.is_none() {
            return false;
        }
        if pressed {
            let hit = self.decoration_hit_at(self.last_pointer_x, self.last_pointer_y);
            if button == LEFT_BUTTON
                && let Some((window_id, _, DecorationHit::Titlebar)) = hit
            {
                const DOUBLE_CLICK_WINDOW: std::time::Duration =
                    std::time::Duration::from_millis(500);
                let now = Instant::now();
                let double_click = self.decoration_last_titlebar_click.take().is_some_and(
                    |(prior_window_id, prior_time)| {
                        prior_window_id == window_id
                            && now.duration_since(prior_time) <= DOUBLE_CLICK_WINDOW
                    },
                );
                if double_click {
                    let _ = self.toggle_maximize_desktop_window(window_id);
                    self.decoration_titlebar_click_capture = Some((window_id, button));
                    self.advance_render_generation(RenderGenerationCause::WindowDecoration);
                    return true;
                }
                self.decoration_last_titlebar_click = Some((window_id, now));
                return self.begin_window_move_at_with_trigger(
                    self.last_pointer_x,
                    self.last_pointer_y,
                    button,
                );
            }
            let Some((window_id, root_surface_id, DecorationHit::Button(kind))) = hit else {
                return false;
            };
            self.decoration_button_capture = Some(DecorationButtonCapture {
                window_id,
                root_surface_id,
                kind,
                button,
            });
            self.advance_render_generation(RenderGenerationCause::WindowDecoration);
            return true;
        }

        let Some(capture) = self.decoration_button_capture.take() else {
            return false;
        };
        if capture.button != button {
            self.advance_render_generation(RenderGenerationCause::WindowDecoration);
            return true;
        }
        let same_button = matches!(
            self.decoration_hit_at(self.last_pointer_x, self.last_pointer_y),
            Some((window_id, _, DecorationHit::Button(kind)))
                if window_id == capture.window_id && kind == capture.kind
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
        self.refresh_surface_origin_cache();
        let origins = self.surface_origin_cache.clone();
        for (index, renderable) in self.renderable_surfaces.iter().enumerate().rev() {
            let root_surface_id = self.root_surface_id_for_surface(renderable.surface_id);
            let root_index = self
                .renderable_surfaces
                .iter()
                .position(|surface| surface.surface_id == root_surface_id)?;
            let root_origin = origins.get(root_index).copied()?;

            if renderable.surface_id != root_surface_id
                && let Some((surface_x, surface_y)) = render::surface_local_point_at_origin(
                    renderable,
                    origins.get(index).copied()?,
                    x,
                    y,
                )
                && self.surface_accepts_input_at(renderable, surface_x, surface_y)
            {
                return None;
            }
            if renderable.surface_id != root_surface_id {
                continue;
            }

            let Some(window_id) = self.window_id_for_surface(root_surface_id) else {
                continue;
            };
            let Some(window) = self.window(window_id) else {
                continue;
            };
            let mode = window.state.mode();
            let fullscreen = mode == ToplevelMode::Fullscreen;
            let decoration_mode =
                if let Some(decoration_state) = self.xdg_decoration_states.get(&root_surface_id) {
                    decoration_state
                        .preference()
                        .effective_mode(true, fullscreen)
                } else if matches!(window.backend, WindowBackend::X11(_))
                    && window.is_normal_x11_role()
                    && !window.x11_window_types.no_decorations
                {
                    if fullscreen {
                        DecorationMode::None
                    } else {
                        DecorationMode::ServerSide
                    }
                } else {
                    continue;
                };
            let Some(layout) = DecorationLayout::for_window(
                renderable.width,
                renderable.height,
                decoration_mode,
                mode == ToplevelMode::Maximized,
                mode == ToplevelMode::Fullscreen,
                self.decoration_theme.metrics(),
            ) else {
                continue;
            };
            let local_x = x - f64::from(root_origin.0) + f64::from(layout.client.x);
            let local_y = y - f64::from(root_origin.1) + f64::from(layout.client.y);
            if let Some(hit) = layout.hit_test(local_x, local_y) {
                return Some((window_id, root_surface_id, hit));
            }
            if let Some((surface_x, surface_y)) =
                render::surface_local_point_at_origin(renderable, root_origin, x, y)
                && self.surface_accepts_input_at(renderable, surface_x, surface_y)
            {
                return None;
            }
        }
        None
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
                } else if matches!(window.backend, WindowBackend::X11(_))
                    && window.is_normal_x11_role()
                    && !window.x11_window_types.no_decorations
                {
                    if fullscreen {
                        DecorationMode::None
                    } else {
                        DecorationMode::ServerSide
                    }
                } else {
                    return None;
                };
                if decoration_mode != DecorationMode::ServerSide {
                    return None;
                }
                let layout = DecorationLayout::for_window(
                    surface.width,
                    surface.height,
                    decoration_mode,
                    mode == ToplevelMode::Maximized,
                    fullscreen,
                    metrics,
                )?;
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
                        pressed: self
                            .decoration_button_capture
                            .filter(|capture| capture.window_id == window_id)
                            .map(|capture| capture.kind),
                    },
                    output_scale,
                );
                let (root_origin_x, root_origin_y) = origins.get(index).copied()?;
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
        if !matches!(window.backend, WindowBackend::X11(_))
            || !window.is_normal_x11_role()
            || window.x11_window_types.no_decorations
        {
            return [0; 4];
        }
        let mode = window.state.mode();
        let Some(layout) = DecorationLayout::for_window(
            1,
            1,
            if mode == ToplevelMode::Fullscreen {
                DecorationMode::None
            } else {
                DecorationMode::ServerSide
            },
            mode == ToplevelMode::Maximized,
            mode == ToplevelMode::Fullscreen,
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
