#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        let right = self.x + self.width as i32;
        let bottom = self.y + self.height as i32;
        x >= self.x && x < right && y >= self.y && y < bottom
    }
}
