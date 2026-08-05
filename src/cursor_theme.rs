//! Shared compositor-owned XCursor image loading.

use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::PathBuf,
    sync::{Arc, OnceLock, RwLock},
};

#[cfg(test)]
use std::path::Path;

use xcursor::{CursorTheme, parser::Image};

pub const DEFAULT_CURSOR_THEME: &str = "default";
pub const DEFAULT_CURSOR_SIZE: u32 = 24;
pub const MIN_CURSOR_SIZE: u32 = 8;
pub const MAX_CURSOR_SIZE: u32 = 256;
pub const MAX_CURSOR_THEME_BYTES: usize = 128;
pub const MAX_CURSOR_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CURSOR_FRAME_DIMENSION: u32 = 1024;
pub const MAX_CURSOR_FRAME_PIXELS: usize = 1024 * 1024;
pub const MAX_CURSOR_FRAMES_PER_FILE: usize = 256;
pub const MAX_CURSOR_UNIQUE_IMAGES: usize = 6;
pub const MAX_CURSOR_TOTAL_FRAME_PIXELS: usize = MAX_CURSOR_UNIQUE_IMAGES * MAX_CURSOR_FRAME_PIXELS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompositorCursorShape {
    Pointer,
    Move,
    ResizeHorizontal,
    ResizeVertical,
    ResizeDiagonalNwSe,
    ResizeDiagonalNeSw,
}

impl CompositorCursorShape {
    pub const ALL: [Self; 6] = [
        Self::Pointer,
        Self::Move,
        Self::ResizeHorizontal,
        Self::ResizeVertical,
        Self::ResizeDiagonalNwSe,
        Self::ResizeDiagonalNeSw,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pointer => "pointer",
            Self::Move => "move",
            Self::ResizeHorizontal => "resize_horizontal",
            Self::ResizeVertical => "resize_vertical",
            Self::ResizeDiagonalNwSe => "resize_diagonal_nw_se",
            Self::ResizeDiagonalNeSw => "resize_diagonal_ne_sw",
        }
    }

    pub const fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Pointer => &["left_ptr", "default", "arrow"],
            Self::Move => &["move", "fleur", "all-scroll"],
            Self::ResizeHorizontal => &[
                "ew-resize",
                "size_hor",
                "sb_h_double_arrow",
                "left_side",
                "right_side",
            ],
            Self::ResizeVertical => &[
                "ns-resize",
                "size_ver",
                "sb_v_double_arrow",
                "top_side",
                "bottom_side",
            ],
            Self::ResizeDiagonalNwSe => &[
                "nwse-resize",
                "size_fdiag",
                "top_left_corner",
                "bottom_right_corner",
            ],
            Self::ResizeDiagonalNeSw => &[
                "nesw-resize",
                "size_bdiag",
                "top_right_corner",
                "bottom_left_corner",
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorThemeLoadError {
    ThemeNotFound,
    RequiredPointerMissing,
    CursorFileReadFailed,
    CursorFileInvalid,
    CursorFileTooLarge,
    FrameBoundsExceeded,
}

impl CursorThemeLoadError {
    pub const fn detail(self) -> &'static str {
        match self {
            Self::ThemeNotFound => "theme_not_found",
            Self::RequiredPointerMissing => "required_pointer_missing",
            Self::CursorFileReadFailed => "cursor_file_read_failed",
            Self::CursorFileInvalid => "cursor_file_invalid",
            Self::CursorFileTooLarge => "cursor_file_too_large",
            Self::FrameBoundsExceeded => "cursor_frame_bounds_exceeded",
        }
    }
}

impl std::fmt::Display for CursorThemeLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.detail())
    }
}

impl std::error::Error for CursorThemeLoadError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorConfigurationError {
    InvalidTheme,
    InvalidSize,
}

impl std::fmt::Display for CursorConfigurationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTheme => formatter.write_str("invalid cursor theme"),
            Self::InvalidSize => formatter.write_str("invalid cursor size"),
        }
    }
}

impl std::error::Error for CursorConfigurationError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorConfiguration {
    pub theme: String,
    pub size_px: u32,
}

impl CursorConfiguration {
    pub fn new(theme: impl AsRef<str>, size_px: u32) -> Result<Self, CursorConfigurationError> {
        validate_cursor_theme(theme.as_ref())?;
        validate_cursor_size(size_px)?;
        Ok(Self {
            theme: theme.as_ref().to_string(),
            size_px,
        })
    }
}

pub fn validate_cursor_theme(theme: &str) -> Result<(), CursorConfigurationError> {
    if theme.is_empty() || theme.len() > MAX_CURSOR_THEME_BYTES || theme.contains("..") {
        return Err(CursorConfigurationError::InvalidTheme);
    }
    if !theme
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(CursorConfigurationError::InvalidTheme);
    }
    Ok(())
}

pub fn validate_cursor_size(size_px: u32) -> Result<(), CursorConfigurationError> {
    if (MIN_CURSOR_SIZE..=MAX_CURSOR_SIZE).contains(&size_px) {
        Ok(())
    } else {
        Err(CursorConfigurationError::InvalidSize)
    }
}

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

#[derive(Debug, Clone)]
pub struct CursorShapeImages {
    images: [Arc<CompositorCursorImage>; 6],
}

impl CursorShapeImages {
    pub fn from_pointer(pointer: Arc<CompositorCursorImage>) -> Self {
        Self {
            images: [
                pointer.clone(),
                pointer.clone(),
                pointer.clone(),
                pointer.clone(),
                pointer.clone(),
                pointer,
            ],
        }
    }

    pub fn from_images(
        pointer: Arc<CompositorCursorImage>,
        movement: Arc<CompositorCursorImage>,
        horizontal: Arc<CompositorCursorImage>,
        vertical: Arc<CompositorCursorImage>,
        diagonal_nw_se: Arc<CompositorCursorImage>,
        diagonal_ne_sw: Arc<CompositorCursorImage>,
    ) -> Self {
        Self {
            images: [
                pointer,
                movement,
                horizontal,
                vertical,
                diagonal_nw_se,
                diagonal_ne_sw,
            ],
        }
    }

    pub fn image(&self, shape: CompositorCursorShape) -> Arc<CompositorCursorImage> {
        self.images[shape as usize].clone()
    }

    pub fn has_external_owner(&self) -> bool {
        for (index, image) in self.images.iter().enumerate() {
            if self.images[..index]
                .iter()
                .find(|candidate| Arc::ptr_eq(candidate, image))
                .is_some()
            {
                continue;
            }
            let internal_references = self
                .images
                .iter()
                .filter(|candidate| Arc::ptr_eq(candidate, image))
                .count();
            if Arc::strong_count(image) > internal_references {
                return true;
            }
        }
        false
    }

    pub fn all(
        &self,
    ) -> impl Iterator<Item = (CompositorCursorShape, Arc<CompositorCursorImage>)> + '_ {
        CompositorCursorShape::ALL
            .into_iter()
            .zip(self.images.iter().cloned())
    }
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

static SHARED_CURSOR_IMAGE: OnceLock<RwLock<Arc<CompositorCursorImage>>> = OnceLock::new();

pub fn install_shared_compositor_cursor(image: Arc<CompositorCursorImage>) {
    let shared = SHARED_CURSOR_IMAGE
        .get_or_init(|| RwLock::new(Arc::new(CompositorCursorImage::builtin_fallback())));
    if let Ok(mut current) = shared.write() {
        *current = image;
    }
}

pub fn shared_compositor_cursor_image() -> Arc<CompositorCursorImage> {
    let shared = SHARED_CURSOR_IMAGE
        .get_or_init(|| RwLock::new(Arc::new(CompositorCursorImage::builtin_fallback())));
    shared
        .read()
        .map(|current| current.clone())
        .unwrap_or_else(|_| Arc::new(CompositorCursorImage::builtin_fallback()))
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
        .unwrap_or(DEFAULT_CURSOR_THEME)
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
        Ok(size) if validate_cursor_size(size).is_ok() => size,
        _ => DEFAULT_CURSOR_SIZE,
    }
}

pub fn default_cursor_configuration() -> CursorConfiguration {
    let environment = CursorEnvironment::from_process();
    CursorConfiguration::new(
        resolve_theme_name(&environment),
        resolve_requested_size(&environment),
    )
    .unwrap_or_else(|_| {
        CursorConfiguration::new(DEFAULT_CURSOR_THEME, DEFAULT_CURSOR_SIZE)
            .expect("built-in cursor default is valid")
    })
}

pub fn load_compositor_cursor_from_environment() -> CompositorCursorImage {
    let configuration = default_cursor_configuration();
    match load_cursor_theme(&configuration.theme, configuration.size_px) {
        Ok(theme) => {
            let image = theme.image(CompositorCursorShape::Pointer);
            eprintln!(
                "cursor theme: loaded theme={} size={} image={}x{} hotspot={},{} source=system",
                image.theme,
                image.requested_size,
                image.width,
                image.height,
                image.hotspot_x,
                image.hotspot_y,
            );
            image.as_ref().clone()
        }
        Err(reason) => {
            eprintln!("cursor theme: using built-in fallback reason={reason}");
            CompositorCursorImage::builtin_fallback()
        }
    }
}

pub(crate) fn load_cursor_theme(
    theme_name: &str,
    requested_size: u32,
) -> Result<CursorShapeImages, CursorThemeLoadError> {
    let theme = CursorTheme::load(theme_name);
    let theme_exists = cursor_theme_directory_exists(theme_name);
    let mut cache = HashMap::new();
    let pointer = load_shape_image(
        &theme,
        theme_name,
        CompositorCursorShape::Pointer,
        requested_size,
        true,
        theme_exists,
        &mut cache,
    )?
    .expect("required pointer image must be present");
    let optional = [
        CompositorCursorShape::Move,
        CompositorCursorShape::ResizeHorizontal,
        CompositorCursorShape::ResizeVertical,
        CompositorCursorShape::ResizeDiagonalNwSe,
        CompositorCursorShape::ResizeDiagonalNeSw,
    ];
    let mut images: [Arc<CompositorCursorImage>; 5] = std::array::from_fn(|_| pointer.clone());
    for (slot, shape) in images.iter_mut().zip(optional) {
        if let Ok(Some(image)) = load_shape_image(
            &theme,
            theme_name,
            shape,
            requested_size,
            false,
            theme_exists,
            &mut cache,
        ) {
            *slot = image;
        }
    }
    Ok(CursorShapeImages::from_images(
        pointer,
        images[0].clone(),
        images[1].clone(),
        images[2].clone(),
        images[3].clone(),
        images[4].clone(),
    ))
}

#[cfg(test)]
fn load_cursor_from_theme(
    theme_name: &str,
    requested_size: u32,
) -> Result<CompositorCursorImage, String> {
    load_cursor_theme(theme_name, requested_size)
        .map(|theme| theme.image(CompositorCursorShape::Pointer).as_ref().clone())
        .map_err(|error| error.to_string())
}

fn load_shape_image(
    theme: &CursorTheme,
    theme_name: &str,
    shape: CompositorCursorShape,
    requested_size: u32,
    required: bool,
    theme_exists: bool,
    cache: &mut HashMap<PathBuf, Result<Arc<CompositorCursorImage>, CursorThemeLoadError>>,
) -> Result<Option<Arc<CompositorCursorImage>>, CursorThemeLoadError> {
    let Some((path, _)) = shape
        .aliases()
        .iter()
        .find_map(|name| theme.load_icon_with_depth(name))
    else {
        return if required {
            Err(if !theme_exists {
                CursorThemeLoadError::ThemeNotFound
            } else if matches!(shape, CompositorCursorShape::Pointer) {
                CursorThemeLoadError::RequiredPointerMissing
            } else {
                CursorThemeLoadError::ThemeNotFound
            })
        } else {
            Ok(None)
        };
    };
    let source_path =
        std::fs::canonicalize(&path).map_err(|_| CursorThemeLoadError::CursorFileReadFailed)?;
    let parsed = if let Some(cached) = cache.get(&source_path) {
        cached.clone()
    } else {
        if cache.len() >= MAX_CURSOR_UNIQUE_IMAGES {
            return Err(CursorThemeLoadError::CursorFileInvalid);
        }
        let parsed = read_cursor_file(&source_path).and_then(|content| {
            validate_xcursor_bounds(&content)?;
            let frames = xcursor::parser::parse_xcursor(&content)
                .ok_or(CursorThemeLoadError::CursorFileInvalid)?;
            let image = select_nearest_frame(&frames, requested_size)
                .map_err(|_| CursorThemeLoadError::CursorFileInvalid)?;
            compositor_image_from_frame(image, theme_name, requested_size, source_path.clone())
                .map(Arc::new)
                .map_err(|_| CursorThemeLoadError::CursorFileInvalid)
        });
        cache.insert(source_path, parsed.clone());
        parsed
    }?;
    Ok(Some(parsed))
}

fn read_cursor_file(path: &std::path::Path) -> Result<Vec<u8>, CursorThemeLoadError> {
    let file = File::open(path).map_err(|_| CursorThemeLoadError::CursorFileReadFailed)?;
    read_bounded_cursor_bytes(file)
}

fn read_bounded_cursor_bytes<R: Read>(mut reader: R) -> Result<Vec<u8>, CursorThemeLoadError> {
    let mut content = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let remaining = MAX_CURSOR_FILE_BYTES + 1 - content.len();
        if remaining == 0 {
            return Err(CursorThemeLoadError::CursorFileTooLarge);
        }
        let read_len = remaining.min(buffer.len());
        let count = reader
            .read(&mut buffer[..read_len])
            .map_err(|_| CursorThemeLoadError::CursorFileReadFailed)?;
        if count == 0 {
            return Ok(content);
        }
        if content.len().saturating_add(count) > MAX_CURSOR_FILE_BYTES {
            return Err(CursorThemeLoadError::CursorFileTooLarge);
        }
        content.extend_from_slice(&buffer[..count]);
    }
}

fn validate_xcursor_bounds(content: &[u8]) -> Result<(), CursorThemeLoadError> {
    const HEADER_BYTES: usize = 16;
    const TOC_BYTES: usize = 12;
    const IMAGE_HEADER_BYTES: usize = 36;

    if content.len() < HEADER_BYTES || &content[..4] != b"Xcur" {
        return Err(CursorThemeLoadError::CursorFileInvalid);
    }
    let header = read_u32(content, 4).ok_or(CursorThemeLoadError::CursorFileInvalid)?;
    let toc_count = read_u32(content, 12).ok_or(CursorThemeLoadError::CursorFileInvalid)?;
    let toc_count =
        usize::try_from(toc_count).map_err(|_| CursorThemeLoadError::FrameBoundsExceeded)?;
    if toc_count > MAX_CURSOR_FRAMES_PER_FILE {
        return Err(CursorThemeLoadError::FrameBoundsExceeded);
    }
    let toc_start = usize::try_from(header).map_err(|_| CursorThemeLoadError::CursorFileInvalid)?;
    let toc_end = toc_start
        .checked_add(
            toc_count
                .checked_mul(TOC_BYTES)
                .ok_or(CursorThemeLoadError::FrameBoundsExceeded)?,
        )
        .ok_or(CursorThemeLoadError::FrameBoundsExceeded)?;
    if toc_start < HEADER_BYTES || toc_end > content.len() {
        return Err(CursorThemeLoadError::CursorFileInvalid);
    }

    let mut frame_count = 0_usize;
    let mut total_frame_pixels = 0_usize;
    for index in 0..toc_count {
        let toc = toc_start + index * TOC_BYTES;
        let kind = read_u32(content, toc).ok_or(CursorThemeLoadError::CursorFileInvalid)?;
        if kind != 0xfffd_0002 {
            continue;
        }
        frame_count = frame_count.saturating_add(1);
        if frame_count > MAX_CURSOR_FRAMES_PER_FILE {
            return Err(CursorThemeLoadError::FrameBoundsExceeded);
        }
        let position = read_u32(content, toc + 8)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(CursorThemeLoadError::CursorFileInvalid)?;
        let frame_end = position
            .checked_add(IMAGE_HEADER_BYTES)
            .ok_or(CursorThemeLoadError::FrameBoundsExceeded)?;
        if frame_end > content.len() {
            return Err(CursorThemeLoadError::CursorFileInvalid);
        }
        let width =
            read_u32(content, position + 16).ok_or(CursorThemeLoadError::CursorFileInvalid)?;
        let height =
            read_u32(content, position + 20).ok_or(CursorThemeLoadError::CursorFileInvalid)?;
        let xhot =
            read_u32(content, position + 24).ok_or(CursorThemeLoadError::CursorFileInvalid)?;
        let yhot =
            read_u32(content, position + 28).ok_or(CursorThemeLoadError::CursorFileInvalid)?;
        if width == 0 || height == 0 || xhot >= width || yhot >= height {
            return Err(CursorThemeLoadError::CursorFileInvalid);
        }
        if width > MAX_CURSOR_FRAME_DIMENSION || height > MAX_CURSOR_FRAME_DIMENSION {
            return Err(CursorThemeLoadError::FrameBoundsExceeded);
        }
        let pixel_count = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or(CursorThemeLoadError::FrameBoundsExceeded)?;
        if pixel_count > MAX_CURSOR_FRAME_PIXELS {
            return Err(CursorThemeLoadError::FrameBoundsExceeded);
        }
        total_frame_pixels = total_frame_pixels
            .checked_add(pixel_count)
            .ok_or(CursorThemeLoadError::FrameBoundsExceeded)?;
        if total_frame_pixels > MAX_CURSOR_TOTAL_FRAME_PIXELS {
            return Err(CursorThemeLoadError::FrameBoundsExceeded);
        }
        let payload_end = frame_end
            .checked_add(
                pixel_count
                    .checked_mul(4)
                    .ok_or(CursorThemeLoadError::FrameBoundsExceeded)?,
            )
            .ok_or(CursorThemeLoadError::FrameBoundsExceeded)?;
        if payload_end > content.len() {
            return Err(CursorThemeLoadError::CursorFileInvalid);
        }
    }
    Ok(())
}

fn read_u32(content: &[u8], offset: usize) -> Option<u32> {
    let bytes = content.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn cursor_theme_directory_exists(theme_name: &str) -> bool {
    const MAX_SEARCH_PATHS: usize = 32;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut paths = Vec::with_capacity(MAX_SEARCH_PATHS);
    if let Some(value) = std::env::var_os("XCURSOR_PATH") {
        for entry in value.to_string_lossy().split(':') {
            if entry.is_empty() || paths.len() == MAX_SEARCH_PATHS {
                continue;
            }
            let path = if entry == "~" {
                home.clone()
            } else if let Some(rest) = entry.strip_prefix("~/") {
                home.as_ref().map(|home| home.join(rest))
            } else {
                Some(PathBuf::from(entry))
            };
            if let Some(path) = path {
                push_bounded_theme_path(&mut paths, path, MAX_SEARCH_PATHS);
            }
        }
    } else {
        if let Some(value) = std::env::var_os("XDG_DATA_HOME") {
            push_bounded_theme_path(&mut paths, PathBuf::from(value), MAX_SEARCH_PATHS);
        } else if let Some(home) = home.as_ref() {
            push_bounded_theme_path(
                &mut paths,
                home.join(".local/share/icons"),
                MAX_SEARCH_PATHS,
            );
        }
        if let Some(home) = home.as_ref() {
            push_bounded_theme_path(&mut paths, home.join(".icons"), MAX_SEARCH_PATHS);
        }
        if let Some(value) = std::env::var_os("XDG_DATA_DIRS") {
            for entry in value.to_string_lossy().split(':') {
                if entry.is_empty() || paths.len() == MAX_SEARCH_PATHS {
                    continue;
                }
                push_bounded_theme_path(
                    &mut paths,
                    PathBuf::from(entry).join("icons"),
                    MAX_SEARCH_PATHS,
                );
            }
        } else {
            push_bounded_theme_path(
                &mut paths,
                PathBuf::from("/usr/local/share/icons"),
                MAX_SEARCH_PATHS,
            );
            push_bounded_theme_path(
                &mut paths,
                PathBuf::from("/usr/share/icons"),
                MAX_SEARCH_PATHS,
            );
        }
        push_bounded_theme_path(
            &mut paths,
            PathBuf::from("/usr/share/pixmaps"),
            MAX_SEARCH_PATHS,
        );
        if let Some(home) = home {
            push_bounded_theme_path(&mut paths, home.join(".cursors"), MAX_SEARCH_PATHS);
        }
        push_bounded_theme_path(
            &mut paths,
            PathBuf::from("/usr/share/cursors/xorg-x11"),
            MAX_SEARCH_PATHS,
        );
    }
    paths.iter().any(|path| path.join(theme_name).is_dir())
}

fn push_bounded_theme_path(paths: &mut Vec<PathBuf>, path: PathBuf, limit: usize) {
    if paths.len() < limit {
        paths.push(path);
    }
}

fn select_nearest_frame(frames: &[Image], requested_size: u32) -> Result<Image, String> {
    let mut selected: Option<&Image> = None;
    for frame in frames {
        let replace = match selected {
            None => true,
            Some(current) => {
                let frame_distance = frame.size.abs_diff(requested_size);
                let current_distance = current.size.abs_diff(requested_size);
                frame_distance < current_distance
                    || (frame_distance == current_distance && frame.size < current.size)
            }
        };
        if replace {
            selected = Some(frame);
        }
    }
    selected
        .cloned()
        .ok_or_else(|| "cursor file has no image frames".to_string())
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
    let _guard = cursor_env_lock().lock().unwrap();
    let previous = std::env::var_os("XCURSOR_PATH");
    // SAFETY: this test-only environment override is serialized by ENV_LOCK.
    unsafe { std::env::set_var("XCURSOR_PATH", search_path) };
    let result = load_cursor_from_theme(theme, size);
    match previous {
        Some(value) => {
            // SAFETY: this test-only environment restore is serialized by ENV_LOCK.
            unsafe { std::env::set_var("XCURSOR_PATH", value) }
        }
        None => {
            // SAFETY: this test-only environment restore is serialized by ENV_LOCK.
            unsafe { std::env::remove_var("XCURSOR_PATH") }
        }
    }
    result
}

#[cfg(test)]
fn load_cursor_theme_from_search_path(
    theme: &str,
    size: u32,
    search_path: &Path,
) -> Result<CursorShapeImages, CursorThemeLoadError> {
    let _guard = cursor_env_lock().lock().unwrap();
    let previous = std::env::var_os("XCURSOR_PATH");
    // SAFETY: this test-only environment override is serialized by ENV_LOCK.
    unsafe { std::env::set_var("XCURSOR_PATH", search_path) };
    let result = load_cursor_theme(theme, size);
    match previous {
        Some(value) => {
            // SAFETY: this test-only environment restore is serialized by ENV_LOCK.
            unsafe { std::env::set_var("XCURSOR_PATH", value) }
        }
        None => {
            // SAFETY: this test-only environment restore is serialized by ENV_LOCK.
            unsafe { std::env::remove_var("XCURSOR_PATH") }
        }
    }
    result
}

#[cfg(test)]
fn cursor_env_lock() -> &'static std::sync::Mutex<()> {
    static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    ENV_LOCK.get_or_init(|| std::sync::Mutex::new(()))
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
    use std::io::Cursor;

    #[test]
    fn cursor_shape_bundle_is_exhaustive_and_optional_shapes_fall_back_to_pointer() {
        let pointer = Arc::new(CompositorCursorImage::builtin_fallback());
        let images = CursorShapeImages::from_pointer(pointer.clone());

        assert_eq!(CompositorCursorShape::ALL.len(), 6);
        for shape in CompositorCursorShape::ALL {
            assert!(Arc::ptr_eq(&images.image(shape), &pointer));
        }
    }

    #[test]
    fn bounded_cursor_reader_rejects_the_first_byte_above_the_file_cap() {
        let content = vec![0_u8; MAX_CURSOR_FILE_BYTES + 1];

        let result = read_bounded_cursor_bytes(Cursor::new(content));

        assert_eq!(result, Err(CursorThemeLoadError::CursorFileTooLarge));
    }

    #[test]
    fn cursor_frame_dimension_at_the_cap_is_accepted() {
        let content = cursor_file(&[(MAX_CURSOR_FRAME_DIMENSION, 1, 0, 0, 1)]);

        assert!(validate_xcursor_bounds(&content).is_ok());
    }

    #[test]
    fn cursor_frame_dimension_above_the_cap_is_rejected() {
        let content = cursor_file(&[(MAX_CURSOR_FRAME_DIMENSION + 1, 1, 0, 0, 1)]);

        assert_eq!(
            validate_xcursor_bounds(&content),
            Err(CursorThemeLoadError::FrameBoundsExceeded)
        );
    }

    #[test]
    fn cursor_frame_count_above_the_cap_is_rejected_before_parsing() {
        let frames = (0..=MAX_CURSOR_FRAMES_PER_FILE)
            .map(|_| (1, 1, 0, 0, 1))
            .collect::<Vec<_>>();

        assert_eq!(
            validate_xcursor_bounds(&cursor_file(&frames)),
            Err(CursorThemeLoadError::FrameBoundsExceeded)
        );
    }

    #[test]
    fn cursor_frame_count_at_the_cap_is_accepted() {
        let frames = (0..MAX_CURSOR_FRAMES_PER_FILE)
            .map(|_| (1, 1, 0, 0, 1))
            .collect::<Vec<_>>();

        assert!(validate_xcursor_bounds(&cursor_file(&frames)).is_ok());
    }

    #[test]
    fn cursor_shape_bundle_retirement_counts_each_unique_external_image_owner() {
        let images =
            CursorShapeImages::from_pointer(Arc::new(CompositorCursorImage::builtin_fallback()));
        assert!(!images.has_external_owner());
        let external_shape = images.image(CompositorCursorShape::Move);
        assert!(images.has_external_owner());
        drop(external_shape);
        assert!(!images.has_external_owner());
    }

    fn test_shape_bundle() -> CursorShapeImages {
        CursorShapeImages::from_images(
            Arc::new(CompositorCursorImage::from_argb8888(vec![1], 1, 1, 0, 0).unwrap()),
            Arc::new(CompositorCursorImage::from_argb8888(vec![2], 1, 1, 0, 0).unwrap()),
            Arc::new(CompositorCursorImage::from_argb8888(vec![3], 1, 1, 0, 0).unwrap()),
            Arc::new(CompositorCursorImage::from_argb8888(vec![4], 1, 1, 0, 0).unwrap()),
            Arc::new(CompositorCursorImage::from_argb8888(vec![5], 1, 1, 0, 0).unwrap()),
            Arc::new(CompositorCursorImage::from_argb8888(vec![6], 1, 1, 0, 0).unwrap()),
        )
    }

    #[test]
    fn one_hundred_pointer_move_shape_transitions_select_exact_images() {
        let images = test_shape_bundle();
        for index in 0..100 {
            let shape = if index % 2 == 0 {
                CompositorCursorShape::Pointer
            } else {
                CompositorCursorShape::Move
            };
            assert_eq!(
                images.image(shape).pixels_argb8888[0],
                if index % 2 == 0 { 1 } else { 2 }
            );
        }
    }

    #[test]
    fn one_hundred_horizontal_vertical_shape_transitions_select_exact_images() {
        let images = test_shape_bundle();
        for index in 0..100 {
            let shape = if index % 2 == 0 {
                CompositorCursorShape::ResizeHorizontal
            } else {
                CompositorCursorShape::ResizeVertical
            };
            assert_eq!(
                images.image(shape).pixels_argb8888[0],
                if index % 2 == 0 { 3 } else { 4 }
            );
        }
    }

    #[test]
    fn one_hundred_diagonal_shape_transitions_select_exact_images() {
        let images = test_shape_bundle();
        for index in 0..100 {
            let shape = if index % 2 == 0 {
                CompositorCursorShape::ResizeDiagonalNwSe
            } else {
                CompositorCursorShape::ResizeDiagonalNeSw
            };
            assert_eq!(
                images.image(shape).pixels_argb8888[0],
                if index % 2 == 0 { 5 } else { 6 }
            );
        }
    }

    #[test]
    fn cursor_theme_loads_every_required_interaction_shape_at_requested_size() {
        let fixture = CursorFixture::new();
        fixture.write_theme("Theme", None);
        for (name, pixel) in [
            ("left_ptr", 11),
            ("move", 22),
            ("ew-resize", 33),
            ("ns-resize", 44),
            ("nwse-resize", 55),
            ("nesw-resize", 66),
        ] {
            fixture.write_cursor("Theme", name, &[pixel], 24, 24, 3, 4);
        }

        let theme = load_cursor_theme_from_search_path("Theme", 24, &fixture.root).unwrap();
        for (shape, pixel) in [
            (CompositorCursorShape::Pointer, 11),
            (CompositorCursorShape::Move, 22),
            (CompositorCursorShape::ResizeHorizontal, 33),
            (CompositorCursorShape::ResizeVertical, 44),
            (CompositorCursorShape::ResizeDiagonalNwSe, 55),
            (CompositorCursorShape::ResizeDiagonalNeSw, 66),
        ] {
            let image = theme.image(shape);
            assert_eq!(image.requested_size, 24);
            assert_eq!((image.pixels_argb8888[0] >> 16) & 0xff, pixel);
            assert_eq!((image.hotspot_x, image.hotspot_y), (3, 4));
        }
    }

    #[test]
    fn malformed_optional_shape_falls_back_without_rejecting_theme() {
        let fixture = CursorFixture::new();
        fixture.write_theme("Theme", None);
        fixture.write_cursor("Theme", "left_ptr", &[11], 24, 24, 3, 4);
        fixture.write_cursor_raw("Theme", "move", malformed_cursor(24, 24, 24, 0));

        let theme = load_cursor_theme_from_search_path("Theme", 24, &fixture.root).unwrap();
        assert!(Arc::ptr_eq(
            &theme.image(CompositorCursorShape::Move),
            &theme.image(CompositorCursorShape::Pointer)
        ));
    }

    #[test]
    fn aliases_resolving_to_one_cursor_file_share_the_selected_image() {
        let fixture = CursorFixture::new();
        fixture.write_theme("Theme", None);
        fixture.write_cursor("Theme", "left_ptr", &[11], 24, 24, 3, 4);
        std::os::unix::fs::symlink(
            fixture.root.join("Theme/cursors/left_ptr"),
            fixture.root.join("Theme/cursors/move"),
        )
        .unwrap();

        let theme = load_cursor_theme_from_search_path("Theme", 24, &fixture.root).unwrap();

        assert!(Arc::ptr_eq(
            &theme.image(CompositorCursorShape::Pointer),
            &theme.image(CompositorCursorShape::Move)
        ));
    }

    #[test]
    fn malformed_required_pointer_rejects_theme_with_typed_error() {
        let fixture = CursorFixture::new();
        fixture.write_theme("Theme", None);
        fixture.write_cursor_raw("Theme", "left_ptr", malformed_cursor(24, 24, 24, 0));

        assert!(matches!(
            load_cursor_theme_from_search_path("Theme", 24, &fixture.root),
            Err(CursorThemeLoadError::CursorFileInvalid)
        ));
    }

    #[test]
    fn missing_theme_is_reported_as_a_typed_theme_not_found_error() {
        let fixture = CursorFixture::new();

        assert!(matches!(
            load_cursor_theme_from_search_path("Missing", 24, &fixture.root),
            Err(CursorThemeLoadError::ThemeNotFound)
        ));
    }

    #[test]
    fn cursor_configuration_accepts_bounded_logical_theme_names_and_sizes() {
        assert!(CursorConfiguration::new("Bibata-Modern-Ice", 24).is_ok());
        assert!(CursorConfiguration::new("a", 8).is_ok());
        assert!(CursorConfiguration::new("a".repeat(128), 256).is_ok());
        assert!(CursorConfiguration::new("a", 24).is_ok());
    }

    #[test]
    fn cursor_configuration_rejects_invalid_theme_syntax() {
        for theme in [
            "",
            &"a".repeat(129),
            "theme/name",
            r"theme\\name",
            "theme name",
            "..",
            "theme\0name",
            "theme\nname",
            "тема",
            "/absolute/theme",
            "../theme",
        ] {
            assert!(
                CursorConfiguration::new(theme, 24).is_err(),
                "theme should be rejected: {theme:?}"
            );
        }
    }

    #[test]
    fn cursor_configuration_rejects_sizes_outside_the_runtime_range() {
        for size in [0, 1, 7, 257, u32::MAX] {
            assert!(CursorConfiguration::new("default", size).is_err());
        }
        assert!(CursorConfiguration::new("default", 8).is_ok());
        assert!(CursorConfiguration::new("default", 24).is_ok());
        assert!(CursorConfiguration::new("default", 256).is_ok());
    }

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
