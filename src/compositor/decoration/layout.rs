#[cfg(test)]
mod tests {
    use super::super::types::{
        DecorationButtonKind, DecorationHit, DecorationMetrics, DecorationMode,
        DecorationResizeEdge,
    };
    use super::DecorationLayout;
    use crate::wm::WindowChromePolicy;

    #[test]
    fn mac_tahoe_layout_keeps_buttons_on_right_in_product_order() {
        let layout = DecorationLayout::for_window(
            640,
            480,
            DecorationMode::ServerSide,
            false,
            false,
            DecorationMetrics::mac_tahoe(),
        )
        .expect("valid floating layout");

        assert_eq!(layout.titlebar.height, 26);
        assert_eq!(
            layout
                .buttons
                .iter()
                .map(|button| button.kind)
                .collect::<Vec<_>>(),
            vec![
                DecorationButtonKind::Minimize,
                DecorationButtonKind::MaximizeRestore,
                DecorationButtonKind::Close,
            ]
        );
        assert_eq!(layout.buttons[0].visual.width, 16);
        assert_eq!(layout.buttons[1].visual.x - layout.buttons[0].visual.x, 25);
        assert_eq!(
            layout.outer.width as i32 - layout.buttons[2].visual.right(),
            12
        );
    }

    #[test]
    fn mac_tahoe_is_borderless_but_keeps_resize_hit_area() {
        let layout = DecorationLayout::for_window(
            640,
            480,
            DecorationMode::ServerSide,
            false,
            false,
            DecorationMetrics::mac_tahoe(),
        )
        .expect("valid floating layout");

        assert_eq!(layout.extents.left, 0);
        assert_eq!(layout.extents.right, 0);
        assert_eq!(layout.extents.bottom, 0);
        assert_eq!(layout.extents.top, 26);
        assert!(matches!(
            layout.hit_test(2.0, 2.0),
            Some(DecorationHit::Resize(_))
        ));
    }

    #[test]
    fn title_safe_area_stops_before_button_cluster_on_narrow_windows() {
        let layout = DecorationLayout::for_window(
            64,
            120,
            DecorationMode::ServerSide,
            false,
            false,
            DecorationMetrics::mac_tahoe(),
        )
        .expect("narrow layout remains representable");

        assert!(layout.title_safe.width as i32 <= layout.buttons[0].input.x);
        assert!(
            layout
                .buttons
                .windows(2)
                .all(|pair| pair[0].input.right() <= pair[1].input.x)
        );
    }

    #[test]
    fn fullscreen_and_csd_have_no_visible_server_decoration() {
        for mode in [DecorationMode::ClientSide, DecorationMode::None] {
            let layout = DecorationLayout::for_window(
                640,
                480,
                mode,
                false,
                false,
                DecorationMetrics::mac_tahoe(),
            )
            .expect("undecorated layout");
            assert_eq!(layout.extents, Default::default());
            assert_eq!(layout.titlebar.height, 0);
            assert!(layout.buttons.is_empty());
        }

        let layout = DecorationLayout::for_window(
            640,
            480,
            DecorationMode::ServerSide,
            false,
            true,
            DecorationMetrics::mac_tahoe(),
        )
        .expect("fullscreen layout");
        assert_eq!(layout.extents, Default::default());
        assert!(layout.buttons.is_empty());
    }

    #[test]
    fn hit_testing_keeps_resize_region_separate_from_visible_border() {
        let layout = DecorationLayout::for_window(
            640,
            480,
            DecorationMode::ServerSide,
            false,
            false,
            DecorationMetrics::mac_tahoe(),
        )
        .expect("valid layout");

        assert!(matches!(
            layout.hit_test(2.0, 2.0),
            Some(DecorationHit::Resize(_))
        ));
        assert_eq!(layout.hit_test(100.0, 16.0), Some(DecorationHit::Titlebar));
        assert_eq!(
            layout.hit_test(
                f64::from(layout.buttons[2].input.x + 2),
                f64::from(layout.buttons[2].input.y + 2)
            ),
            Some(DecorationHit::Button(DecorationButtonKind::Close))
        );
    }

    #[test]
    fn minimal_server_chrome_has_no_titlebar_or_rendered_resize_target() {
        let layout = DecorationLayout::for_window_with_chrome_policy(
            640,
            480,
            DecorationMode::ServerSide,
            false,
            false,
            WindowChromePolicy::Minimal,
            DecorationMetrics::mac_tahoe(),
        )
        .expect("minimal layout");

        assert_eq!(layout.titlebar.height, 0);
        assert!(layout.buttons.is_empty());
        assert_eq!(layout.extents.top, 0);
        assert_eq!(layout.hit_test(2.0, 2.0), None);
        assert_eq!(
            layout.logical_resize_edge_at(2.0, 2.0),
            Some(DecorationResizeEdge::TopLeft)
        );
    }

    #[test]
    fn button_input_wins_over_overlapping_top_resize_region() {
        let layout = DecorationLayout::for_window(
            640,
            480,
            DecorationMode::ServerSide,
            false,
            false,
            DecorationMetrics::mac_tahoe(),
        )
        .expect("valid layout");
        let button = layout.buttons.first().expect("minimize button");

        assert_eq!(
            layout.hit_test(
                f64::from(button.input.x + button.input.width as i32 / 2),
                1.0,
            ),
            Some(DecorationHit::Button(button.kind))
        );
    }
}
use super::types::{
    DecorationButtonKind, DecorationExtents, DecorationHit, DecorationMetrics, DecorationMode,
    DecorationRect, DecorationResizeEdge,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecorationButtonLayout {
    pub kind: DecorationButtonKind,
    pub visual: DecorationRect,
    pub input: DecorationRect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecorationLayout {
    pub outer: DecorationRect,
    pub client: DecorationRect,
    pub titlebar: DecorationRect,
    pub title_safe: DecorationRect,
    pub resize_input: DecorationRect,
    pub visible_border: Vec<DecorationRect>,
    pub buttons: Vec<DecorationButtonLayout>,
    pub extents: DecorationExtents,
}

impl DecorationLayout {
    #[cfg(test)]
    pub(crate) fn for_window(
        client_width: u32,
        client_height: u32,
        mode: DecorationMode,
        maximized: bool,
        fullscreen: bool,
        metrics: DecorationMetrics,
    ) -> Option<Self> {
        Self::for_window_with_chrome_policy(
            client_width,
            client_height,
            mode,
            maximized,
            fullscreen,
            crate::wm::WindowChromePolicy::Full,
            metrics,
        )
    }

    pub(crate) fn for_window_with_chrome_policy(
        client_width: u32,
        client_height: u32,
        mode: DecorationMode,
        maximized: bool,
        fullscreen: bool,
        chrome_policy: crate::wm::WindowChromePolicy,
        metrics: DecorationMetrics,
    ) -> Option<Self> {
        if !metrics.validate() || client_width == 0 || client_height == 0 {
            return None;
        }

        let server_side = mode == DecorationMode::ServerSide && !fullscreen;
        let visible = server_side && chrome_policy == crate::wm::WindowChromePolicy::Full;
        let border = if server_side && !maximized {
            metrics.border_width
        } else {
            0
        };
        let titlebar_height = if visible { metrics.titlebar_height } else { 0 };
        let extents = DecorationExtents {
            top: titlebar_height.checked_add(border)?,
            right: border,
            bottom: border,
            left: border,
        };
        let outer_width = client_width
            .checked_add(extents.left)?
            .checked_add(extents.right)?;
        let outer_height = client_height
            .checked_add(extents.top)?
            .checked_add(extents.bottom)?;
        if outer_width > i32::MAX as u32 || outer_height > i32::MAX as u32 {
            return None;
        }

        let outer = DecorationRect::new(0, 0, outer_width, outer_height);
        let client = DecorationRect::new(
            extents.left.min(i32::MAX as u32) as i32,
            extents.top.min(i32::MAX as u32) as i32,
            client_width,
            client_height,
        );
        let titlebar = DecorationRect::new(0, 0, outer_width, titlebar_height);
        let mut buttons = Vec::new();
        let mut title_safe = DecorationRect::new(
            metrics.horizontal_padding.min(i32::MAX as u32) as i32,
            0,
            outer_width.saturating_sub(metrics.horizontal_padding.saturating_mul(2)),
            titlebar_height,
        );

        if visible {
            let (button_size, spacing, cluster_width) =
                button_cluster_metrics(outer_width, metrics);
            let start_x = outer_width
                .saturating_sub(metrics.right_padding)
                .saturating_sub(cluster_width);
            let visual_y = (titlebar_height.saturating_sub(button_size)) / 2;
            let input_height = button_size.max(24).min(titlebar_height.max(button_size));
            let input_y = (titlebar_height.saturating_sub(input_height)) / 2;
            for (index, kind) in DecorationButtonKind::ORDER.into_iter().enumerate() {
                let index = u32::try_from(index).ok()?;
                let x =
                    start_x.checked_add(index.checked_mul(button_size.checked_add(spacing)?)?)?;
                let visual =
                    DecorationRect::new(x as i32, visual_y as i32, button_size, button_size);
                let input_width = button_size
                    .saturating_add(spacing)
                    .min(metrics.minimum_button_hit_width.max(button_size));
                let centered_offset = input_width.saturating_sub(button_size) / 2;
                let input = DecorationRect::new(
                    x.saturating_sub(centered_offset) as i32,
                    input_y as i32,
                    input_width,
                    input_height,
                );
                buttons.push(DecorationButtonLayout {
                    kind,
                    visual,
                    input,
                });
            }
            if let Some(first) = buttons.first() {
                let safe_right = first
                    .input
                    .x
                    .saturating_sub(metrics.horizontal_padding as i32);
                title_safe.width = safe_right.saturating_sub(title_safe.x).max(0) as u32;
            }
        }

        let resize = if visible && !maximized {
            metrics.resize_hit_width
        } else {
            0
        };
        let resize_input = DecorationRect::new(
            -(resize.min(i32::MAX as u32) as i32),
            -(resize.min(i32::MAX as u32) as i32),
            outer_width.saturating_add(resize.saturating_mul(2)),
            outer_height.saturating_add(resize.saturating_mul(2)),
        );
        let visible_border = if border == 0 {
            Vec::new()
        } else {
            vec![
                DecorationRect::new(0, 0, outer_width, border),
                DecorationRect::new(
                    0,
                    outer_height.saturating_sub(border) as i32,
                    outer_width,
                    border,
                ),
                DecorationRect::new(
                    0,
                    border as i32,
                    border,
                    client_height.saturating_add(titlebar_height),
                ),
                DecorationRect::new(
                    outer_width.saturating_sub(border) as i32,
                    border as i32,
                    border,
                    client_height.saturating_add(titlebar_height),
                ),
            ]
        };

        Some(Self {
            outer,
            client,
            titlebar,
            title_safe,
            resize_input,
            visible_border,
            buttons,
            extents,
        })
    }

    pub(crate) fn hit_test(&self, x: f64, y: f64) -> Option<DecorationHit> {
        if self.titlebar.height == 0 {
            return None;
        }
        if let Some(button) = self
            .buttons
            .iter()
            .find(|button| button.input.contains(x, y))
        {
            return Some(DecorationHit::Button(button.kind));
        }
        if let Some(edge) = self.resize_edge_at(x, y) {
            return Some(DecorationHit::Resize(edge));
        }
        self.titlebar
            .contains(x, y)
            .then_some(DecorationHit::Titlebar)
    }

    pub(crate) fn logical_resize_edge_at(&self, x: f64, y: f64) -> Option<DecorationResizeEdge> {
        const LOGICAL_EDGE_WIDTH: f64 = 6.0;

        if !self.outer.contains(x, y) {
            return None;
        }
        let left = x < f64::from(self.outer.x) + LOGICAL_EDGE_WIDTH;
        let right = x >= f64::from(self.outer.right()) - LOGICAL_EDGE_WIDTH;
        let top = y < f64::from(self.outer.y) + LOGICAL_EDGE_WIDTH;
        let bottom = y >= f64::from(self.outer.bottom()) - LOGICAL_EDGE_WIDTH;
        Some(match (top, right, bottom, left) {
            (true, true, false, false) => DecorationResizeEdge::TopRight,
            (false, true, true, false) => DecorationResizeEdge::BottomRight,
            (false, false, true, true) => DecorationResizeEdge::BottomLeft,
            (true, false, false, true) => DecorationResizeEdge::TopLeft,
            (true, _, _, _) => DecorationResizeEdge::Top,
            (_, true, _, _) => DecorationResizeEdge::Right,
            (_, _, true, _) => DecorationResizeEdge::Bottom,
            (_, _, _, true) => DecorationResizeEdge::Left,
            _ => return None,
        })
    }

    fn resize_edge_at(&self, x: f64, y: f64) -> Option<DecorationResizeEdge> {
        let width = self.resize_input.x.unsigned_abs();
        if width == 0 || !self.resize_input.contains(x, y) {
            return None;
        }
        let left = x < f64::from(self.outer.x + width as i32);
        let right = x >= f64::from(self.outer.right() - width as i32);
        let top = y < f64::from(self.outer.y + width as i32);
        let bottom = y >= f64::from(self.outer.bottom() - width as i32);
        Some(match (top, right, bottom, left) {
            (true, true, false, false) => DecorationResizeEdge::TopRight,
            (false, true, true, false) => DecorationResizeEdge::BottomRight,
            (false, false, true, true) => DecorationResizeEdge::BottomLeft,
            (true, false, false, true) => DecorationResizeEdge::TopLeft,
            (true, _, _, _) => DecorationResizeEdge::Top,
            (_, true, _, _) => DecorationResizeEdge::Right,
            (_, _, true, _) => DecorationResizeEdge::Bottom,
            (_, _, _, true) => DecorationResizeEdge::Left,
            _ => return None,
        })
    }
}

fn button_cluster_metrics(outer_width: u32, metrics: DecorationMetrics) -> (u32, u32, u32) {
    let available = outer_width.saturating_sub(metrics.horizontal_padding.saturating_mul(2));
    let requested = metrics
        .button_visual_size
        .saturating_mul(3)
        .saturating_add(metrics.button_spacing.saturating_mul(2));
    if available >= requested {
        return (
            metrics.button_visual_size,
            metrics.button_spacing,
            requested,
        );
    }
    let button_size = metrics.button_visual_size.min(available / 3).max(1);
    let spacing = metrics
        .button_spacing
        .min(available.saturating_sub(button_size.saturating_mul(3)) / 2);
    (
        button_size,
        spacing,
        button_size
            .saturating_mul(3)
            .saturating_add(spacing.saturating_mul(2)),
    )
}
