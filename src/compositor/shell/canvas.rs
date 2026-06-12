#[derive(Debug, Clone, Copy)]
pub(super) struct Rect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl Rect {
    pub(super) const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub(super) fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.width as i32)
            && y < self.y.saturating_add(self.height as i32)
    }

    pub(super) fn translated(self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            width: self.width,
            height: self.height,
        }
    }

    pub(super) fn clipped_to(self, width: u32, height: u32) -> Option<Self> {
        let left = self.x.max(0);
        let top = self.y.max(0);
        let right = self.x.saturating_add(self.width as i32).min(width as i32);
        let bottom = self.y.saturating_add(self.height as i32).min(height as i32);
        (right > left && bottom > top).then_some(Self {
            x: left,
            y: top,
            width: right.saturating_sub(left) as u32,
            height: bottom.saturating_sub(top) as u32,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct FrameSize {
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TextStyle {
    pub(super) scale: i32,
    pub(super) color: u32,
}

pub(super) fn draw_rounded_rect(
    frame: &mut [u32],
    width: u32,
    height: u32,
    rect: Rect,
    radius: i32,
    color: u32,
) {
    let left = rect.x.max(0);
    let top = rect.y.max(0);
    let right = rect.x.saturating_add(rect.width as i32).min(width as i32);
    let bottom = rect.y.saturating_add(rect.height as i32).min(height as i32);
    if left >= right || top >= bottom {
        return;
    }

    let radius = radius.max(0);
    for y in top..bottom {
        for x in left..right {
            if rounded_rect_contains(rect, radius, x, y) {
                blend_pixel(frame, width, x, y, color);
            }
        }
    }
}

pub(super) fn draw_char(
    frame: &mut [u32],
    frame_size: FrameSize,
    x: i32,
    y: i32,
    character: char,
    style: TextStyle,
) {
    let glyph = glyph_rows(character);
    for (row_index, row) in glyph.iter().enumerate() {
        for (column_index, byte) in row.as_bytes().iter().enumerate() {
            if *byte != b'1' {
                continue;
            }
            let rect = Rect::new(
                x + column_index as i32 * style.scale,
                y + row_index as i32 * style.scale,
                style.scale as u32,
                style.scale as u32,
            );
            fill_rect(
                frame,
                frame_size.width,
                frame_size.height,
                rect,
                style.color,
            );
        }
    }
}

pub(super) fn premul_argb(alpha: u8, red: u8, green: u8, blue: u8) -> u32 {
    let alpha_u32 = u32::from(alpha);
    let premul = |channel: u8| (u32::from(channel) * alpha_u32 + 127) / 255;
    (alpha_u32 << 24) | (premul(red) << 16) | (premul(green) << 8) | premul(blue)
}

pub fn blend_shell_overlay_argb(source: u32, target: u32) -> u32 {
    blend_premul_argb(source, target)
}

fn rounded_rect_contains(rect: Rect, radius: i32, x: i32, y: i32) -> bool {
    if radius == 0 {
        return true;
    }
    let right = rect.x + rect.width as i32 - 1;
    let bottom = rect.y + rect.height as i32 - 1;
    let inner_left = rect.x + radius;
    let inner_right = right - radius;
    let inner_top = rect.y + radius;
    let inner_bottom = bottom - radius;
    if (inner_left..=inner_right).contains(&x) || (inner_top..=inner_bottom).contains(&y) {
        return true;
    }

    let center_x = if x < inner_left {
        inner_left
    } else {
        inner_right
    };
    let center_y = if y < inner_top {
        inner_top
    } else {
        inner_bottom
    };
    let dx = x - center_x;
    let dy = y - center_y;
    dx * dx + dy * dy <= radius * radius
}

fn fill_rect(frame: &mut [u32], width: u32, height: u32, rect: Rect, color: u32) {
    let left = rect.x.max(0);
    let top = rect.y.max(0);
    let right = rect.x.saturating_add(rect.width as i32).min(width as i32);
    let bottom = rect.y.saturating_add(rect.height as i32).min(height as i32);
    for y in top..bottom {
        for x in left..right {
            blend_pixel(frame, width, x, y, color);
        }
    }
}

fn blend_pixel(frame: &mut [u32], width: u32, x: i32, y: i32, source: u32) {
    let index = y as usize * width as usize + x as usize;
    if let Some(target) = frame.get_mut(index) {
        *target = blend_premul_argb(source, *target);
    }
}

pub(super) fn blend_pixel_checked(
    frame: &mut [u32],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    source: u32,
) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    blend_pixel(frame, width, x, y, source);
}

fn blend_premul_argb(source: u32, target: u32) -> u32 {
    let source_alpha = (source >> 24) & 0xff;
    if source_alpha == 0 {
        return target;
    }
    if source_alpha == 0xff {
        return source;
    }
    let target_alpha = (target >> 24) & 0xff;
    let inverse_alpha = 255 - source_alpha;
    let blend = |shift: u32| {
        let source_channel = (source >> shift) & 0xff;
        let target_channel = (target >> shift) & 0xff;
        source_channel + (target_channel * inverse_alpha + 127) / 255
    };
    let alpha = source_alpha + (target_alpha * inverse_alpha + 127) / 255;
    (alpha << 24) | (blend(16) << 16) | (blend(8) << 8) | blend(0)
}

fn glyph_rows(character: char) -> [&'static str; 7] {
    match character.to_ascii_uppercase() {
        'A' => [
            "01110", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'B' => [
            "11110", "10001", "10001", "11110", "10001", "10001", "11110",
        ],
        'C' => [
            "01111", "10000", "10000", "10000", "10000", "10000", "01111",
        ],
        'D' => [
            "11110", "10001", "10001", "10001", "10001", "10001", "11110",
        ],
        'E' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "11111",
        ],
        'F' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "10000",
        ],
        'G' => [
            "01111", "10000", "10000", "10111", "10001", "10001", "01111",
        ],
        'H' => [
            "10001", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'I' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "11111",
        ],
        'J' => [
            "00111", "00010", "00010", "00010", "10010", "10010", "01100",
        ],
        'K' => [
            "10001", "10010", "10100", "11000", "10100", "10010", "10001",
        ],
        'L' => [
            "10000", "10000", "10000", "10000", "10000", "10000", "11111",
        ],
        'M' => [
            "10001", "11011", "10101", "10101", "10001", "10001", "10001",
        ],
        'N' => [
            "10001", "11001", "10101", "10011", "10001", "10001", "10001",
        ],
        'O' => [
            "01110", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'P' => [
            "11110", "10001", "10001", "11110", "10000", "10000", "10000",
        ],
        'Q' => [
            "01110", "10001", "10001", "10001", "10101", "10010", "01101",
        ],
        'R' => [
            "11110", "10001", "10001", "11110", "10100", "10010", "10001",
        ],
        'S' => [
            "01111", "10000", "10000", "01110", "00001", "00001", "11110",
        ],
        'T' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "00100",
        ],
        'U' => [
            "10001", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'V' => [
            "10001", "10001", "10001", "10001", "10001", "01010", "00100",
        ],
        'W' => [
            "10001", "10001", "10001", "10101", "10101", "10101", "01010",
        ],
        'X' => [
            "10001", "10001", "01010", "00100", "01010", "10001", "10001",
        ],
        'Y' => [
            "10001", "10001", "01010", "00100", "00100", "00100", "00100",
        ],
        'Z' => [
            "11111", "00001", "00010", "00100", "01000", "10000", "11111",
        ],
        '0' => [
            "01110", "10001", "10011", "10101", "11001", "10001", "01110",
        ],
        '1' => [
            "00100", "01100", "00100", "00100", "00100", "00100", "01110",
        ],
        '2' => [
            "01110", "10001", "00001", "00010", "00100", "01000", "11111",
        ],
        '3' => [
            "11110", "00001", "00001", "01110", "00001", "00001", "11110",
        ],
        '4' => [
            "00010", "00110", "01010", "10010", "11111", "00010", "00010",
        ],
        '5' => [
            "11111", "10000", "10000", "11110", "00001", "00001", "11110",
        ],
        '6' => [
            "01110", "10000", "10000", "11110", "10001", "10001", "01110",
        ],
        '7' => [
            "11111", "00001", "00010", "00100", "01000", "01000", "01000",
        ],
        '8' => [
            "01110", "10001", "10001", "01110", "10001", "10001", "01110",
        ],
        '9' => [
            "01110", "10001", "10001", "01111", "00001", "00001", "01110",
        ],
        ':' => [
            "00000", "00100", "00100", "00000", "00100", "00100", "00000",
        ],
        '-' => [
            "00000", "00000", "00000", "11111", "00000", "00000", "00000",
        ],
        '.' => [
            "00000", "00000", "00000", "00000", "00000", "01100", "01100",
        ],
        '/' => [
            "00001", "00010", "00010", "00100", "01000", "01000", "10000",
        ],
        _ => [
            "11111", "00001", "00010", "00100", "00100", "00000", "00100",
        ],
    }
}
