use super::{
    layout::DecorationLayout,
    raster::DecorationRasterAsset,
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
        asset: DecorationRasterAsset,
    },
    Text {
        rect: DecorationRect,
        clip: DecorationRect,
        text: String,
        color: [u8; 4],
        asset: DecorationRasterAsset,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecorationRenderPlan {
    pub layout: DecorationLayout,
    pub primitives: Vec<DecorationRenderPrimitive>,
    pub theme_generation: u64,
}

impl DecorationRenderPlan {
    pub(crate) fn visual_signature(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        hash_u64(&mut hash, self.theme_generation);
        hash_layout(&mut hash, &self.layout);
        for primitive in &self.primitives {
            match primitive {
                DecorationRenderPrimitive::SolidRect { rect, color } => {
                    hash_u8(&mut hash, 0);
                    hash_rect(&mut hash, *rect);
                    for component in color {
                        hash_u8(&mut hash, *component);
                    }
                }
                DecorationRenderPrimitive::Image { rect, asset } => {
                    hash_u8(&mut hash, 1);
                    hash_rect(&mut hash, *rect);
                    hash_u64(&mut hash, asset.asset_id());
                }
                DecorationRenderPrimitive::Text {
                    rect,
                    clip,
                    text,
                    color,
                    asset,
                } => {
                    hash_u8(&mut hash, 2);
                    hash_rect(&mut hash, *rect);
                    hash_rect(&mut hash, *clip);
                    hash_bytes(&mut hash, text.as_bytes());
                    for component in color {
                        hash_u8(&mut hash, *component);
                    }
                    hash_u64(&mut hash, asset.asset_id());
                }
            }
        }
        hash
    }
}

fn hash_layout(hash: &mut u64, layout: &DecorationLayout) {
    for rect in [
        layout.outer,
        layout.client,
        layout.titlebar,
        layout.title_safe,
        layout.resize_input,
    ] {
        hash_rect(hash, rect);
    }
    for rect in &layout.visible_border {
        hash_rect(hash, *rect);
    }
    for button in &layout.buttons {
        hash_u8(hash, button.kind as u8);
        hash_rect(hash, button.visual);
        hash_rect(hash, button.input);
    }
    for extent in [
        layout.extents.top,
        layout.extents.right,
        layout.extents.bottom,
        layout.extents.left,
    ] {
        hash_u32(hash, extent);
    }
}

fn hash_rect(hash: &mut u64, rect: super::types::DecorationRect) {
    hash_u32(hash, rect.x as u32);
    hash_u32(hash, rect.y as u32);
    hash_u32(hash, rect.width);
    hash_u32(hash, rect.height);
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        hash_u8(hash, *byte);
    }
    hash_u8(hash, 0xff);
}

fn hash_u64(hash: &mut u64, value: u64) {
    for byte in value.to_le_bytes() {
        hash_u8(hash, byte);
    }
}

fn hash_u32(hash: &mut u64, value: u32) {
    for byte in value.to_le_bytes() {
        hash_u8(hash, byte);
    }
}

fn hash_u8(hash: &mut u64, value: u8) {
    *hash ^= u64::from(value);
    *hash = hash.wrapping_mul(0x1000_0000_01b3);
}

pub(crate) fn build_render_plan(
    layout: &DecorationLayout,
    theme: &DecorationThemeSnapshot,
    title: &str,
    state: DecorationRenderState,
    output_scale: f64,
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
            color: if state.active {
                colors.active_border
            } else {
                colors.inactive_border
            },
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
        if let Some(path) = theme.asset(kind, !state.active, restore, selected_state)
            && let Some(asset) = theme.raster_asset(path, output_scale)
        {
            primitives.push(DecorationRenderPrimitive::Image {
                rect: button.visual,
                asset: asset.clone(),
            });
        }
    }

    if layout.title_safe.width > 0 {
        let color = if state.active {
            colors.title
        } else {
            colors.inactive_title
        };
        if let Some(raster) = theme.rasterize_title(title, layout.title_safe, color, output_scale) {
            let width = raster.logical_width.min(layout.title_safe.width);
            let centered_x = (layout.outer.width.saturating_sub(width) / 2) as i32;
            let minimum_x = layout.title_safe.x;
            let maximum_x = layout.title_safe.right().saturating_sub(width as i32);
            let x = centered_x.clamp(minimum_x, maximum_x.max(minimum_x));
            primitives.push(DecorationRenderPrimitive::Text {
                rect: DecorationRect::new(x, layout.title_safe.y, width, layout.title_safe.height),
                clip: layout.title_safe,
                text: raster.text,
                color,
                asset: raster.asset,
            });
        }
    }

    DecorationRenderPlan {
        layout: layout.clone(),
        primitives,
        theme_generation: theme.generation(),
    }
}

#[cfg(test)]
mod tests {
    use super::{DecorationRenderPrimitive, DecorationRenderState, build_render_plan};
    use crate::compositor::decoration::{
        layout::DecorationLayout,
        theme::load_theme_package,
        types::{DecorationButtonKind, DecorationMetrics, DecorationMode},
    };

    fn plan() -> super::DecorationRenderPlan {
        let theme = load_theme_package(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("resources/decorations/MacTahoe-Dark"),
            1,
        )
        .expect("bundled MacTahoe theme");
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
            "A very long window title that must remain bounded and clipped before the buttons and remain safe around the controls in every state",
            DecorationRenderState {
                active: true,
                maximized: false,
                hovered: Some(DecorationButtonKind::Close),
                pressed: None,
            },
            1.0,
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
