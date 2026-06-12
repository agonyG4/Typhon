use super::canvas::{FrameSize, Rect, TextStyle, draw_char, draw_rounded_rect, premul_argb};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellDockItem {
    pub surface_id: u32,
    pub label: String,
    pub active: bool,
    pub minimized: bool,
}

impl ShellDockItem {
    pub fn new(surface_id: u32, label: impl Into<String>, active: bool, minimized: bool) -> Self {
        Self {
            surface_id,
            label: label.into(),
            active,
            minimized,
        }
    }

    fn initial(&self) -> char {
        self.label
            .chars()
            .find(|character| character.is_ascii_alphanumeric())
            .unwrap_or('?')
            .to_ascii_uppercase()
    }
}

pub fn dock_item_at(
    width: u32,
    height: u32,
    items: &[ShellDockItem],
    x: i32,
    y: i32,
) -> Option<u32> {
    items
        .iter()
        .take(12)
        .enumerate()
        .find(|(index, _)| {
            dock_item_rect(width, height, items, *index).is_some_and(|rect| rect.contains(x, y))
        })
        .map(|(_, item)| item.surface_id)
}

pub(super) fn dock_bounds(width: u32, height: u32, items: &[ShellDockItem]) -> Option<Rect> {
    dock_rect(width, height, items)
}

pub(super) fn draw_dock_at(
    frame: &mut [u32],
    width: u32,
    height: u32,
    output_width: u32,
    output_height: u32,
    origin: (i32, i32),
    items: &[ShellDockItem],
) {
    if items.is_empty() || output_width < 120 || output_height < 100 {
        return;
    }

    let Some(output_dock_rect) = dock_rect(output_width, output_height, items) else {
        return;
    };
    let dock_rect = output_dock_rect.translated(-origin.0, -origin.1);

    draw_rounded_rect(
        frame,
        width,
        height,
        dock_rect,
        20,
        premul_argb(190, 18, 22, 30),
    );

    for (index, item) in items.iter().take(12).enumerate() {
        let Some(item_rect) = dock_item_rect(output_width, output_height, items, index) else {
            continue;
        };
        let item_rect = item_rect.translated(-origin.0, -origin.1);
        let color = if item.active {
            premul_argb(255, 32, 132, 226)
        } else if item.minimized {
            premul_argb(210, 100, 106, 120)
        } else {
            premul_argb(235, 54, 60, 74)
        };
        draw_rounded_rect(frame, width, height, item_rect, 12, color);
        draw_char(
            frame,
            FrameSize { width, height },
            item_rect.x + 16,
            item_rect.y + 13,
            item.initial(),
            TextStyle {
                scale: 3,
                color: premul_argb(255, 245, 248, 255),
            },
        );
    }
}

fn dock_rect(width: u32, height: u32, items: &[ShellDockItem]) -> Option<Rect> {
    if items.is_empty() || width < 120 || height < 100 {
        return None;
    }
    let item_size = 48_i32;
    let gap = 12_i32;
    let padding = 14_i32;
    let count = items.len().min(12) as i32;
    let dock_width = count * item_size + (count - 1).max(0) * gap + padding * 2;
    let dock_height = item_size + padding * 2;
    Some(Rect::new(
        (width as i32 - dock_width) / 2,
        height as i32 - dock_height - 20,
        dock_width as u32,
        dock_height as u32,
    ))
}

fn dock_item_rect(width: u32, height: u32, items: &[ShellDockItem], index: usize) -> Option<Rect> {
    let dock_rect = dock_rect(width, height, items)?;
    let item_size = 48_i32;
    let gap = 12_i32;
    let padding = 14_i32;
    if index >= items.len().min(12) {
        return None;
    }
    let x = dock_rect.x + padding + index as i32 * (item_size + gap);
    Some(Rect::new(
        x,
        dock_rect.y + padding,
        item_size as u32,
        item_size as u32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dock_hit_test_returns_clicked_open_app() {
        let items = vec![ShellDockItem::new(42, "kitty", false, false)];

        assert_eq!(dock_item_at(320, 200, &items, 150, 132), Some(42));
    }
}
