use super::{
    layout::DecorationLayout,
    theme::DecorationThemeSnapshot,
    types::{DecorationButtonKind, DecorationButtonVisualState, DecorationRect},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecorationRenderState {
    pub active: bool,
    pub maximized: bool,
    pub hovered: Option<DecorationButtonKind>,
    pub pressed: Option<DecorationButtonKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecorationRenderPrimitive {
    SolidRect {
        rect: DecorationRect,
        color: [u8; 4],
    },
    Image {
        rect: DecorationRect,
        asset: String,
    },
    Text {
        rect: DecorationRect,
        clip: DecorationRect,
        text: String,
        color: [u8; 4],
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecorationRenderPlan {
    pub layout: DecorationLayout,
    pub primitives: Vec<DecorationRenderPrimitive>,
    pub theme_generation: u64,
}

pub(crate) fn build_render_plan(
    layout: &DecorationLayout,
    theme: &DecorationThemeSnapshot,
    title: &str,
    state: DecorationRenderState,
) -> DecorationRenderPlan {
    let mut primitives = Vec::new();
    if layout.titlebar.height == 0 {
        return DecorationRenderPlan {
            layout: layout.clone(),
            primitives,
            theme_generation: theme.generation(),
        };
    }

    let colors = theme.colors();
    primitives.push(DecorationRenderPrimitive::SolidRect {
        rect: layout.titlebar,
        color: if state.active {
            colors.active_background
        } else {
            colors.inactive_background
        },
    });
    primitives.extend(layout.visible_border.iter().copied().map(|rect| {
        DecorationRenderPrimitive::SolidRect {
            rect,
            color: colors.border,
        }
    }));

    for button in &layout.buttons {
        let selected_state = if state.pressed == Some(button.kind) {
            DecorationButtonVisualState::Pressed
        } else if state.hovered == Some(button.kind) {
            DecorationButtonVisualState::Hovered
        } else {
            DecorationButtonVisualState::Normal
        };
        let (kind, restore) = match button.kind {
            DecorationButtonKind::MaximizeRestore if state.maximized => ("restore", true),
            DecorationButtonKind::MaximizeRestore => ("maximize", false),
            DecorationButtonKind::Minimize => ("minimize", false),
            DecorationButtonKind::Close => ("close", false),
        };
        if let Some(asset) = theme.asset(kind, !state.active, restore, selected_state) {
            primitives.push(DecorationRenderPrimitive::Image {
                rect: button.visual,
                asset: asset.to_string(),
            });
        }
    }

    if layout.title_safe.width > 0 {
        primitives.push(DecorationRenderPrimitive::Text {
            rect: layout.title_safe,
            clip: layout.title_safe,
            text: ellipsize_title(title, layout.title_safe.width),
            color: if state.active {
                colors.title
            } else {
                colors.inactive_title
            },
        });
    }

    DecorationRenderPlan {
        layout: layout.clone(),
        primitives,
        theme_generation: theme.generation(),
    }
}

fn ellipsize_title(title: &str, width: u32) -> String {
    let max_chars = usize::try_from(width / 8).unwrap_or(usize::MAX);
    if title.chars().count() <= max_chars {
        return title.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let mut result = title.chars().take(max_chars - 1).collect::<String>();
    result.push('…');
    result
}

#[cfg(test)]
mod tests {
    use super::{DecorationRenderPrimitive, DecorationRenderState, build_render_plan};
    use crate::compositor::decoration::{
        layout::DecorationLayout,
        theme::DecorationThemeSnapshot,
        types::{DecorationButtonKind, DecorationMetrics, DecorationMode},
    };

    fn plan() -> super::DecorationRenderPlan {
        let theme = DecorationThemeSnapshot::builtin_mac_tahoe(1);
        let layout = DecorationLayout::for_window(
            640,
            480,
            DecorationMode::ServerSide,
            false,
            false,
            DecorationMetrics::mac_tahoe(),
        )
        .unwrap();
        build_render_plan(
            &layout,
            &theme,
            "A very long window title that must remain bounded and clipped before the buttons",
            DecorationRenderState {
                active: true,
                maximized: false,
                hovered: Some(DecorationButtonKind::Close),
                pressed: None,
            },
        )
    }

    #[test]
    fn shared_plan_contains_title_and_three_button_primitives() {
        let plan = plan();
        assert!(
            plan.primitives
                .iter()
                .any(|primitive| matches!(primitive, DecorationRenderPrimitive::Text { .. }))
        );
        assert_eq!(
            plan.primitives
                .iter()
                .filter(|primitive| matches!(primitive, DecorationRenderPrimitive::Image { .. }))
                .count(),
            3
        );
    }

    #[test]
    fn shared_plan_is_renderer_independent_and_title_is_clipped() {
        let left = plan();
        let right = plan();
        assert_eq!(left, right);
        let text = left
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                DecorationRenderPrimitive::Text { text, clip, .. } => Some((text, clip)),
                _ => None,
            })
            .unwrap();
        assert!(text.0.ends_with('…'));
        assert_eq!(text.1, &left.layout.title_safe);
    }
}
