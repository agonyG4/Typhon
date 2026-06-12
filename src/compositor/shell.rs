mod canvas;
mod dock;
mod font_text;
mod spotlight;
mod topbar;

pub use canvas::blend_shell_overlay_argb;
pub use dock::{ShellDockItem, dock_item_at};
pub use spotlight::{ShellLaunchSuggestion, SpotlightModel, launcher_suggestions};
pub use topbar::ShellTopbarModel;

use canvas::Rect;
use dock::{dock_bounds, draw_dock_at};
use spotlight::{draw_spotlight_at, spotlight_bounds};
use topbar::{draw_topbar_at, topbar_bounds};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellOverlayState {
    pub topbar: ShellTopbarModel,
    pub dock_items: Vec<ShellDockItem>,
    pub spotlight: SpotlightModel,
    pub generation: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ShellOverlayImage {
    pub width: u32,
    pub height: u32,
    pub generation: u64,
    pub pixels: Vec<u32>,
    content_bounds: Option<ShellOverlayBounds>,
    regions: Vec<ShellOverlayRegion>,
}

impl ShellOverlayImage {
    pub const fn content_bounds(&self) -> Option<ShellOverlayBounds> {
        self.content_bounds
    }

    pub fn regions(&self) -> &[ShellOverlayRegion] {
        &self.regions
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellOverlayBounds {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ShellOverlayBounds {
    fn union(self, other: Self) -> Self {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self
            .x
            .saturating_add(self.width)
            .max(other.x.saturating_add(other.width));
        let bottom = self
            .y
            .saturating_add(self.height)
            .max(other.y.saturating_add(other.height));
        Self {
            x: left,
            y: top,
            width: right.saturating_sub(left),
            height: bottom.saturating_sub(top),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellOverlayRegion {
    pub output: ShellOverlayBounds,
    pub texture: ShellOverlayBounds,
}

#[derive(Debug, Default)]
pub struct ShellOverlayRenderer {
    image: ShellOverlayImage,
}

impl ShellOverlayRenderer {
    pub fn render(
        &mut self,
        width: u32,
        height: u32,
        state: &ShellOverlayState,
    ) -> &ShellOverlayImage {
        let output_width = width.max(1);
        let output_height = height.max(1);
        let generation = overlay_generation(output_width, output_height, state);
        if self.image.generation == generation {
            return &self.image;
        }

        let parts = shell_overlay_parts(output_width, output_height, state);
        let Some(content_bounds) = shell_overlay_content_bounds(&parts) else {
            self.image.width = 0;
            self.image.height = 0;
            self.image.generation = generation;
            self.image.pixels.clear();
            self.image.content_bounds = None;
            self.image.regions.clear();
            return &self.image;
        };
        let atlas_width = parts
            .iter()
            .map(|part| part.bounds.width)
            .max()
            .unwrap_or(0);
        let atlas_height = parts
            .iter()
            .map(|part| part.bounds.height)
            .fold(0_u32, u32::saturating_add);
        let regions = shell_overlay_regions(&parts, atlas_width);
        let pixel_count = atlas_width.saturating_mul(atlas_height) as usize;
        if self.image.width == atlas_width
            && self.image.height == atlas_height
            && self.image.generation == generation
            && self.image.content_bounds == Some(content_bounds)
            && self.image.regions == regions
            && self.image.pixels.len() == pixel_count
        {
            return &self.image;
        }

        self.image.width = atlas_width;
        self.image.height = atlas_height;
        self.image.generation = generation;
        self.image.pixels.resize(pixel_count, 0);
        self.image.pixels.fill(0);
        self.image.content_bounds = Some(content_bounds);
        self.image.regions = regions;
        for (part, region) in parts.iter().zip(self.image.regions.iter().copied()) {
            let origin = (
                part.bounds.x as i32 - region.texture.x as i32,
                part.bounds.y as i32 - region.texture.y as i32,
            );
            match part.kind {
                ShellOverlayPartKind::Topbar => draw_topbar_at(
                    &mut self.image.pixels,
                    atlas_width,
                    atlas_height,
                    output_width,
                    output_height,
                    origin,
                    &state.topbar,
                ),
                ShellOverlayPartKind::Dock => draw_dock_at(
                    &mut self.image.pixels,
                    atlas_width,
                    atlas_height,
                    output_width,
                    output_height,
                    origin,
                    &state.dock_items,
                ),
                ShellOverlayPartKind::Spotlight => draw_spotlight_at(
                    &mut self.image.pixels,
                    atlas_width,
                    atlas_height,
                    output_width,
                    output_height,
                    origin,
                    &state.spotlight,
                ),
            }
        }
        &self.image
    }
}

#[derive(Debug, Clone, Copy)]
struct ShellOverlayPart {
    kind: ShellOverlayPartKind,
    bounds: ShellOverlayBounds,
}

#[derive(Debug, Clone, Copy)]
enum ShellOverlayPartKind {
    Topbar,
    Dock,
    Spotlight,
}

fn shell_overlay_parts(
    width: u32,
    height: u32,
    state: &ShellOverlayState,
) -> Vec<ShellOverlayPart> {
    [
        (
            ShellOverlayPartKind::Topbar,
            topbar_bounds(width, height, &state.topbar),
        ),
        (
            ShellOverlayPartKind::Dock,
            dock_bounds(width, height, &state.dock_items),
        ),
        (
            ShellOverlayPartKind::Spotlight,
            spotlight_bounds(width, height, &state.spotlight),
        ),
    ]
    .into_iter()
    .filter_map(|(kind, rect)| {
        rect.and_then(|rect| shell_overlay_bounds_from_rect(rect, width, height))
            .map(|bounds| ShellOverlayPart { kind, bounds })
    })
    .collect()
}

fn shell_overlay_bounds_from_rect(
    rect: Rect,
    width: u32,
    height: u32,
) -> Option<ShellOverlayBounds> {
    let rect = rect.clipped_to(width, height)?;
    Some(ShellOverlayBounds {
        x: rect.x as u32,
        y: rect.y as u32,
        width: rect.width,
        height: rect.height,
    })
}

fn shell_overlay_content_bounds(parts: &[ShellOverlayPart]) -> Option<ShellOverlayBounds> {
    parts
        .iter()
        .map(|part| part.bounds)
        .reduce(ShellOverlayBounds::union)
}

fn shell_overlay_regions(parts: &[ShellOverlayPart], atlas_width: u32) -> Vec<ShellOverlayRegion> {
    let mut next_y = 0_u32;
    parts
        .iter()
        .map(|part| {
            let region = ShellOverlayRegion {
                output: part.bounds,
                texture: ShellOverlayBounds {
                    x: 0,
                    y: next_y,
                    width: part.bounds.width.min(atlas_width),
                    height: part.bounds.height,
                },
            };
            next_y = next_y.saturating_add(part.bounds.height);
            region
        })
        .collect()
}

fn overlay_generation(width: u32, height: u32, state: &ShellOverlayState) -> u64 {
    let mut generation = state
        .generation
        .wrapping_mul(31)
        .wrapping_add(u64::from(width))
        .wrapping_mul(31)
        .wrapping_add(u64::from(height));
    generation = generation
        .wrapping_mul(31)
        .wrapping_add(u64::from(state.topbar.is_visible() as u8));
    for byte in state.topbar.title().as_bytes() {
        generation = generation.wrapping_mul(31).wrapping_add(u64::from(*byte));
    }
    for byte in state.topbar.trailing_text().as_bytes() {
        generation = generation.wrapping_mul(31).wrapping_add(u64::from(*byte));
    }
    for item in &state.dock_items {
        generation = generation
            .wrapping_mul(31)
            .wrapping_add(u64::from(item.surface_id));
        generation = generation
            .wrapping_mul(31)
            .wrapping_add(u64::from(item.active as u8));
        generation = generation
            .wrapping_mul(31)
            .wrapping_add(u64::from(item.minimized as u8));
        for byte in item.label.as_bytes() {
            generation = generation.wrapping_mul(31).wrapping_add(u64::from(*byte));
        }
    }
    generation = generation
        .wrapping_mul(31)
        .wrapping_add(u64::from(state.spotlight.is_visible() as u8));
    for byte in state.spotlight.query().as_bytes() {
        generation = generation.wrapping_mul(31).wrapping_add(u64::from(*byte));
    }
    generation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_overlay_image_draws_dock_for_open_apps() {
        let state = ShellOverlayState {
            topbar: ShellTopbarModel::default(),
            dock_items: vec![ShellDockItem::new(1, "brave", true, false)],
            spotlight: SpotlightModel::default(),
            generation: 1,
        };
        let mut renderer = ShellOverlayRenderer::default();
        let image = renderer.render(320, 200, &state);
        let bounds = image.content_bounds().expect("dock should draw");

        assert_eq!(image.width, bounds.width);
        assert_eq!(image.height, bounds.height);
        assert!(bounds.width < 320);
        assert!(bounds.height < 200);
        assert!(image.pixels.iter().any(|pixel| pixel >> 24 != 0));
    }

    #[test]
    fn shell_overlay_image_handles_empty_transparent_overlay() {
        let state = ShellOverlayState::default();
        let mut renderer = ShellOverlayRenderer::default();
        let image = renderer.render(320, 200, &state);

        assert_eq!(image.content_bounds(), None);
    }

    #[test]
    fn shell_overlay_image_draws_visible_topbar_at_top_edge() {
        let state = ShellOverlayState {
            topbar: ShellTopbarModel::visible("Oblivion One"),
            dock_items: Vec::new(),
            spotlight: SpotlightModel::default(),
            generation: 1,
        };
        let mut renderer = ShellOverlayRenderer::default();
        let image = renderer.render(640, 360, &state);

        let bounds = image.content_bounds().expect("visible topbar should draw");
        assert!(bounds.y <= 12);
        assert!(bounds.height <= 40);
    }

    #[test]
    fn shell_overlay_image_crops_pixels_to_content_bounds() {
        let state = ShellOverlayState {
            topbar: ShellTopbarModel::visible("Oblivion One"),
            dock_items: vec![ShellDockItem::new(1, "brave", true, false)],
            spotlight: SpotlightModel::default(),
            generation: 1,
        };
        let mut renderer = ShellOverlayRenderer::default();
        let image = renderer.render(1920, 1080, &state);
        let bounds = image.content_bounds().expect("shell should draw");

        assert!(image.width < 1920);
        assert!(image.height < 1080);
        assert!(image.width <= bounds.width);
        assert!(image.height < bounds.height);
        assert_eq!(image.regions().len(), 2);
        assert_eq!(
            image.pixels.len(),
            image.width as usize * image.height as usize
        );
    }

    #[test]
    fn shell_overlay_image_packs_disjoint_topbar_and_dock_without_gap() {
        let state = ShellOverlayState {
            topbar: ShellTopbarModel::visible("Oblivion One"),
            dock_items: vec![ShellDockItem::new(1, "brave", true, false)],
            spotlight: SpotlightModel::default(),
            generation: 1,
        };
        let mut renderer = ShellOverlayRenderer::default();
        let image = renderer.render(1920, 1080, &state);

        assert!(image.height < 180);
        assert!(image.pixels.len() < 1920 * 180);
    }

    #[test]
    fn shell_overlay_image_draws_astrea_spotlight_panel_shape() {
        let mut spotlight = SpotlightModel::default();
        spotlight.toggle();
        let state = ShellOverlayState {
            topbar: ShellTopbarModel::default(),
            dock_items: Vec::new(),
            spotlight,
            generation: 1,
        };
        let mut renderer = ShellOverlayRenderer::default();
        let image = renderer.render(1280, 720, &state);

        assert_eq!(
            image.content_bounds(),
            Some(ShellOverlayBounds {
                x: 340,
                y: 180,
                width: 600,
                height: 58,
            })
        );
    }

    #[test]
    fn shell_overlay_image_expands_spotlight_for_query_results() {
        let mut spotlight = SpotlightModel::default();
        spotlight.toggle();
        spotlight.push_text("fire");
        let state = ShellOverlayState {
            topbar: ShellTopbarModel::default(),
            dock_items: Vec::new(),
            spotlight,
            generation: 1,
        };
        let mut renderer = ShellOverlayRenderer::default();
        let image = renderer.render(1280, 720, &state);

        let bounds = image.content_bounds().expect("spotlight should draw");
        assert_eq!(bounds.x, 340);
        assert_eq!(bounds.y, 180);
        assert_eq!(bounds.width, 600);
        assert!(bounds.height > 58);
    }
}
