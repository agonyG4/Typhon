//! Shared compositor-owned XCursor image loading.

use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

#[cfg(test)]
use std::path::Path;

use xcursor::{CursorTheme, parser::Image};

const DEFAULT_CURSOR_SIZE: u32 = 24;
const MAX_CURSOR_SIZE: u32 = 256;
const CURSOR_NAMES: [&str; 3] = ["left_ptr", "default", "arrow"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositorCursorImage {
    pub pixels_argb8888: Vec<u32>,
    pub width: u32,
    pub height: u32,
    pub hotspot_x: i32,
    pub hotspot_y: i32,
    pub(crate) requested_size: u32,
    pub(crate) theme: String,
    pub(crate) source: Option<PathBuf>,
}

impl CompositorCursorImage {
    pub fn from_argb8888(
        pixels_argb8888: Vec<u32>,
        width: u32,
        height: u32,
        hotspot_x: i32,
        hotspot_y: i32,
    ) -> Result<Self, String> {
        let pixel_count = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or_else(|| "cursor dimensions overflow".to_string())?;
        if width == 0
            || height == 0
            || hotspot_x < 0
            || hotspot_y < 0
            || hotspot_x >= i32::try_from(width).map_err(|_| "cursor width overflow")?
            || hotspot_y >= i32::try_from(height).map_err(|_| "cursor height overflow")?
            || pixels_argb8888.len() != pixel_count
        {
            return Err("cursor image dimensions, hotspot, or pixel count are invalid".to_string());
        }
        Ok(Self {
            pixels_argb8888,
            width,
            height,
            hotspot_x,
            hotspot_y,
            requested_size: width.max(height),
            theme: "test".to_string(),
            source: None,
        })
    }

    pub fn builtin_fallback() -> Self {
        let width = CURSOR_PATTERN
            .iter()
            .map(|line| line.len() as u32)
            .max()
            .unwrap_or(0);
        let height = CURSOR_PATTERN.len() as u32;
        let mut pixels = vec![0; width.saturating_mul(height) as usize];
        for (row, line) in CURSOR_PATTERN.iter().enumerate() {
            for (column, marker) in line.bytes().enumerate() {
                let color = match marker {
                    b'X' => BUILTIN_CURSOR_OUTLINE,
                    b'O' => BUILTIN_CURSOR_FILL,
                    _ => continue,
                };
                let index = row * width as usize + column;
                pixels[index] = color;
            }
        }
        Self {
            pixels_argb8888: pixels,
            width,
            height,
            hotspot_x: 0,
            hotspot_y: 0,
            requested_size: DEFAULT_CURSOR_SIZE,
            theme: "builtin".to_string(),
            source: None,
        }
    }

    pub fn top_left(&self, pointer_x: i32, pointer_y: i32) -> (i32, i32) {
        (
            pointer_x.saturating_sub(self.hotspot_x),
            pointer_y.saturating_sub(self.hotspot_y),
        )
    }
}

static SHARED_CURSOR_IMAGE: OnceLock<Arc<CompositorCursorImage>> = OnceLock::new();

pub fn install_shared_compositor_cursor(image: Arc<CompositorCursorImage>) {
    let _ = SHARED_CURSOR_IMAGE.set(image);
}

pub fn shared_compositor_cursor_image() -> Arc<CompositorCursorImage> {
    SHARED_CURSOR_IMAGE
        .get_or_init(|| Arc::new(CompositorCursorImage::builtin_fallback()))
        .clone()
}

#[derive(Debug, Default, Clone)]
struct CursorEnvironment {
    override_theme: Option<String>,
    xcursor_theme: Option<String>,
    override_size: Option<String>,
    xcursor_size: Option<String>,
}

impl CursorEnvironment {
    fn from_process() -> Self {
        Self {
            override_theme: non_empty_env("OBLIVION_ONE_CURSOR_THEME"),
            xcursor_theme: non_empty_env("XCURSOR_THEME"),
            override_size: std::env::var("OBLIVION_ONE_CURSOR_SIZE").ok(),
            xcursor_size: std::env::var("XCURSOR_SIZE").ok(),
        }
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn resolve_theme_name(environment: &CursorEnvironment) -> String {
    environment
        .override_theme
        .as_deref()
        .or(environment.xcursor_theme.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("default")
        .to_string()
}

fn resolve_requested_size(environment: &CursorEnvironment) -> u32 {
    let value = environment
        .override_size
        .as_deref()
        .or(environment.xcursor_size.as_deref());
    let Some(value) = value else {
        return DEFAULT_CURSOR_SIZE;
    };
    match value.trim().parse::<u32>() {
        Ok(size) if (1..=MAX_CURSOR_SIZE).contains(&size) => size,
        _ => DEFAULT_CURSOR_SIZE,
    }
}

pub fn load_compositor_cursor_from_environment() -> CompositorCursorImage {
    let environment = CursorEnvironment::from_process();
    let theme = resolve_theme_name(&environment);
    let requested_size = resolve_requested_size(&environment);
    match load_cursor_from_theme(&theme, requested_size) {
        Ok(image) => {
            eprintln!(
                "cursor theme: loaded theme={} size={} image={}x{} hotspot={},{} source={}",
                image.theme,
                image.requested_size,
                image.width,
                image.height,
                image.hotspot_x,
                image.hotspot_y,
                image
                    .source
                    .as_deref()
                    .map_or_else(|| "unknown".to_string(), |path| path.display().to_string()),
            );
            image
        }
        Err(reason) => {
            eprintln!("cursor theme: using built-in fallback reason={reason}");
            CompositorCursorImage::builtin_fallback()
        }
    }
}

fn load_cursor_from_theme(
    theme_name: &str,
    requested_size: u32,
) -> Result<CompositorCursorImage, String> {
    let theme = CursorTheme::load(theme_name);
    let (path, _) = CURSOR_NAMES
        .iter()
        .find_map(|name| theme.load_icon_with_depth(name))
        .ok_or_else(|| format!("theme {theme_name:?} has no left pointer cursor"))?;
    let content =
        std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let frames = xcursor::parser::parse_xcursor(&content)
        .ok_or_else(|| format!("parse {}", path.display()))?;
    let image = select_nearest_frame(frames, requested_size)?;
    compositor_image_from_frame(image, theme_name, requested_size, path)
}

fn select_nearest_frame(frames: Vec<Image>, requested_size: u32) -> Result<Image, String> {
    let mut selected = None;
    for frame in frames {
        let replace = selected.as_ref().is_none_or(|current: &Image| {
            let frame_distance = frame.size.abs_diff(requested_size);
            let current_distance = current.size.abs_diff(requested_size);
            frame_distance < current_distance
                || (frame_distance == current_distance && frame.size < current.size)
        });
        if replace {
            selected = Some(frame);
        }
    }
    selected.ok_or_else(|| "cursor file has no image frames".to_string())
}

fn compositor_image_from_frame(
    frame: Image,
    theme_name: &str,
    requested_size: u32,
    source: PathBuf,
) -> Result<CompositorCursorImage, String> {
    if frame.width == 0
        || frame.height == 0
        || frame.xhot >= frame.width
        || frame.yhot >= frame.height
    {
        return Err("cursor hotspot or dimensions are outside the image".to_string());
    }
    let pixel_count = usize::try_from(frame.width)
        .ok()
        .and_then(|width| {
            usize::try_from(frame.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| "cursor dimensions overflow".to_string())?;
    let byte_count = pixel_count
        .checked_mul(4)
        .ok_or_else(|| "cursor pixel count overflow".to_string())?;
    if frame.pixels_argb.len() != byte_count {
        return Err("cursor pixel count does not match dimensions".to_string());
    }
    let pixels_argb8888 = frame
        .pixels_argb
        .chunks_exact(4)
        .map(|pixel| u32::from_be_bytes([pixel[0], pixel[1], pixel[2], pixel[3]]))
        .collect::<Vec<_>>();
    if pixels_argb8888.len() != pixel_count {
        return Err("cursor pixel conversion produced an invalid count".to_string());
    }
    Ok(CompositorCursorImage {
        pixels_argb8888,
        width: frame.width,
        height: frame.height,
        hotspot_x: i32::try_from(frame.xhot).map_err(|_| "cursor hotspot overflow")?,
        hotspot_y: i32::try_from(frame.yhot).map_err(|_| "cursor hotspot overflow")?,
        requested_size,
        theme: theme_name.to_string(),
        source: Some(source),
    })
}

#[cfg(test)]
fn load_cursor_from_search_path(
    theme: &str,
    size: u32,
    search_path: &Path,
) -> Result<CompositorCursorImage, String> {
    use std::sync::{Mutex, OnceLock};
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let previous = std::env::var_os("XCURSOR_PATH");
    unsafe { std::env::set_var("XCURSOR_PATH", search_path) };
    let result = load_cursor_from_theme(theme, size);
    match previous {
        Some(value) => unsafe { std::env::set_var("XCURSOR_PATH", value) },
        None => unsafe { std::env::remove_var("XCURSOR_PATH") },
    }
    result
}

const BUILTIN_CURSOR_FILL: u32 = 0xffff_ffff;
const BUILTIN_CURSOR_OUTLINE: u32 = 0xff10_1116;
const CURSOR_PATTERN: [&str; 17] = [
    "X",
    "XX",
    "XOX",
    "XOOX",
    "XOOOX",
    "XOOOOX",
    "XOOOOOX",
    "XOOOOOOX",
    "XOOOOOOOX",
    "XOOOOOOOOX",
    "XOOOOXXXXX",
    "XOOXOOX",
    "XOX XOOX",
    "XX  XOOX",
    "X    XOOX",
    "     XOOX",
    "      XX",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_theme_precedes_xcursor_theme() {
        let environment = CursorEnvironment {
            override_theme: Some("override".into()),
            xcursor_theme: Some("environment".into()),
            ..CursorEnvironment::default()
        };
        assert_eq!(resolve_theme_name(&environment), "override");
    }

    #[test]
    fn override_size_precedes_xcursor_size() {
        let environment = CursorEnvironment {
            override_size: Some("31".into()),
            xcursor_size: Some("19".into()),
            ..CursorEnvironment::default()
        };
        assert_eq!(resolve_requested_size(&environment), 31);
    }

    #[test]
    fn invalid_size_uses_24() {
        for value in ["0", "-1", "not-a-size", "999999"] {
            let environment = CursorEnvironment {
                override_size: Some(value.into()),
                ..CursorEnvironment::default()
            };
            assert_eq!(resolve_requested_size(&environment), 24);
        }
    }

    #[test]
    fn left_ptr_is_preferred_over_aliases() {
        let fixture = CursorFixture::new();
        fixture.write_theme("Theme", None);
        fixture.write_cursor("Theme", "default", &[8], 8, 8, 1, 1);
        fixture.write_cursor("Theme", "arrow", &[16], 8, 8, 1, 1);
        fixture.write_cursor("Theme", "left_ptr", &[32], 8, 8, 1, 1);

        let image = fixture.load("Theme", 8);
        assert_eq!((image.pixels_argb8888[0] >> 16) & 0xff, 32);
    }

    #[test]
    fn nearest_size_is_selected() {
        let fixture = CursorFixture::new();
        fixture.write_theme("Theme", None);
        fixture.write_cursor("Theme", "left_ptr", &[16], 16, 16, 1, 1);
        fixture.append_cursor_frame("Theme", "left_ptr", &[32], 32, 32, 2, 2);

        let image = fixture.load("Theme", 27);
        assert_eq!((image.width, image.height), (32, 32));
        assert_eq!((image.pixels_argb8888[0] >> 16) & 0xff, 32);
    }

    #[test]
    fn equal_distance_prefers_smaller_size() {
        let fixture = CursorFixture::new();
        fixture.write_theme("Theme", None);
        fixture.write_cursor("Theme", "left_ptr", &[16], 16, 16, 1, 1);
        fixture.append_cursor_frame("Theme", "left_ptr", &[32], 32, 32, 2, 2);

        let image = fixture.load("Theme", 24);
        assert_eq!((image.width, image.height), (16, 16));
        assert_eq!((image.pixels_argb8888[0] >> 16) & 0xff, 16);
    }

    #[test]
    fn hotspot_is_preserved() {
        let fixture = CursorFixture::new();
        fixture.write_theme("Theme", None);
        fixture.write_cursor("Theme", "left_ptr", &[1; 12], 3, 4, 2, 3);

        let image = fixture.load("Theme", 4);
        assert_eq!((image.hotspot_x, image.hotspot_y), (2, 3));
    }

    #[test]
    fn malformed_hotspot_uses_builtin_fallback() {
        let fixture = CursorFixture::new();
        fixture.write_theme("Theme", None);
        fixture.write_cursor_raw("Theme", "left_ptr", malformed_cursor(4, 4, 4, 0));

        let image = fixture.load("Theme", 4);
        assert_eq!(image, CompositorCursorImage::builtin_fallback());
    }

    #[test]
    fn missing_theme_uses_builtin_fallback() {
        let fixture = CursorFixture::new();
        let image = fixture.load("missing", 24);
        assert_eq!(image, CompositorCursorImage::builtin_fallback());
    }

    #[test]
    fn theme_inheritance_resolves_left_ptr_from_parent() {
        let fixture = CursorFixture::new();
        fixture.write_theme("Parent", None);
        fixture.write_cursor("Parent", "left_ptr", &[77], 8, 8, 2, 3);
        fixture.write_theme("Child", Some("Parent"));

        let image = fixture.load("Child", 8);
        assert_eq!((image.hotspot_x, image.hotspot_y), (2, 3));
        assert_eq!((image.pixels_argb8888[0] >> 16) & 0xff, 77);
    }

    // The test fixture helpers deliberately use the XCursor binary layout so
    // selection and conversion are tested through the dependency parser.
    struct CursorFixture {
        root: std::path::PathBuf,
    }

    impl CursorFixture {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "typhon-xcursor-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write_theme(&self, name: &str, inherits: Option<&str>) {
            let theme = self.root.join(name);
            std::fs::create_dir_all(theme.join("cursors")).unwrap();
            let inherits =
                inherits.map_or_else(String::new, |value| format!("\nInherits={value}\n"));
            std::fs::write(
                theme.join("index.theme"),
                format!("[Icon Theme]\nName={name}{inherits}"),
            )
            .unwrap();
        }

        #[allow(clippy::too_many_arguments)]
        fn write_cursor(
            &self,
            theme: &str,
            name: &str,
            pixels: &[u8],
            width: u32,
            height: u32,
            hotspot_x: u32,
            hotspot_y: u32,
        ) {
            self.write_cursor_raw(
                theme,
                name,
                cursor_file(&[(width, height, hotspot_x, hotspot_y, pixels[0])]),
            );
        }

        #[allow(clippy::too_many_arguments)]
        fn append_cursor_frame(
            &self,
            theme: &str,
            name: &str,
            pixels: &[u8],
            width: u32,
            height: u32,
            hotspot_x: u32,
            hotspot_y: u32,
        ) {
            self.write_cursor_raw(
                theme,
                name,
                cursor_file(&[
                    (16, 16, 1, 1, 16),
                    (width, height, hotspot_x, hotspot_y, pixels[0]),
                ]),
            );
        }

        fn write_cursor_raw(&self, theme: &str, name: &str, bytes: Vec<u8>) {
            std::fs::write(self.root.join(theme).join("cursors").join(name), bytes).unwrap();
        }

        fn load(&self, theme: &str, size: u32) -> CompositorCursorImage {
            load_cursor_from_search_path(theme, size, &self.root)
                .unwrap_or_else(|_| CompositorCursorImage::builtin_fallback())
        }
    }

    impl Drop for CursorFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn cursor_file(frames: &[(u32, u32, u32, u32, u8)]) -> Vec<u8> {
        let header_size = 16u32;
        let toc_size = 12u32;
        let image_size = 36u32;
        let mut result = Vec::new();
        result.extend_from_slice(b"Xcur");
        result.extend_from_slice(&header_size.to_le_bytes());
        result.extend_from_slice(&0x0001_0000u32.to_le_bytes());
        result.extend_from_slice(&(frames.len() as u32).to_le_bytes());
        let mut offset = header_size + toc_size * frames.len() as u32;
        for (width, height, _, _, _) in frames {
            result.extend_from_slice(&0xfffd_0002u32.to_le_bytes());
            result.extend_from_slice(&width.to_le_bytes());
            result.extend_from_slice(&offset.to_le_bytes());
            offset = offset.saturating_add(
                image_size.saturating_add(width.saturating_mul(*height).saturating_mul(4)),
            );
        }
        for (width, height, hotspot_x, hotspot_y, pixel) in frames {
            result.extend_from_slice(&image_size.to_le_bytes());
            result.extend_from_slice(&0xfffd_0002u32.to_le_bytes());
            result.extend_from_slice(&width.to_le_bytes());
            result.extend_from_slice(&0x0000_0001u32.to_le_bytes());
            result.extend_from_slice(&width.to_le_bytes());
            result.extend_from_slice(&height.to_le_bytes());
            result.extend_from_slice(&hotspot_x.to_le_bytes());
            result.extend_from_slice(&hotspot_y.to_le_bytes());
            result.extend_from_slice(&0u32.to_le_bytes());
            for _ in 0..width.saturating_mul(*height) {
                result.extend_from_slice(&[*pixel, 0, 0, 255]);
            }
        }
        result
    }

    fn malformed_cursor(width: u32, height: u32, hotspot_x: u32, hotspot_y: u32) -> Vec<u8> {
        cursor_file(&[(width, height, hotspot_x, hotspot_y, 255)])
    }
}
