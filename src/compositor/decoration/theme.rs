use super::{
    raster::{DecorationRasterAsset, rasterize_svg},
    types::{DecorationButtonVisualState, DecorationMetrics},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

pub(crate) const MAX_THEME_JSON_BYTES: usize = 64 * 1024;
const MAX_THEME_NAME_BYTES: usize = 128;
const MAX_ASSET_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecorationThemeError {
    TooLarge,
    InvalidSchema(String),
    InvalidMetrics(String),
    InvalidColor(String),
    InvalidAssetPath(String),
    AssetTooLarge(String),
    AssetIo(String),
    ExternalResource(String),
    Raster(String),
    Persistence(String),
}

impl fmt::Display for DecorationThemeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("theme JSON exceeds 64 KiB"),
            Self::InvalidSchema(reason) => write!(formatter, "invalid theme schema: {reason}"),
            Self::InvalidMetrics(reason) => write!(formatter, "invalid theme metrics: {reason}"),
            Self::InvalidColor(name) => write!(formatter, "invalid theme color: {name}"),
            Self::InvalidAssetPath(path) => write!(formatter, "invalid theme asset path: {path}"),
            Self::AssetTooLarge(path) => write!(formatter, "theme asset exceeds 256 KiB: {path}"),
            Self::AssetIo(reason) => write!(formatter, "theme asset I/O failed: {reason}"),
            Self::ExternalResource(path) => {
                write!(
                    formatter,
                    "theme asset references an external resource: {path}"
                )
            }
            Self::Raster(reason) => write!(formatter, "theme asset rasterization failed: {reason}"),
            Self::Persistence(reason) => {
                write!(formatter, "theme selection persistence failed: {reason}")
            }
        }
    }
}

impl std::error::Error for DecorationThemeError {}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DecorationThemeDefinition {
    pub schema_version: u32,
    pub name: String,
    pub metrics: ThemeMetricsDefinition,
    pub colors: ThemeColorsDefinition,
    pub title: ThemeTitleDefinition,
    pub buttons: ThemeButtonsDefinition,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThemeMetricsDefinition {
    pub titlebar_height: u32,
    pub button_visual_size: u32,
    pub button_spacing: u32,
    pub right_padding: u32,
    pub horizontal_padding: u32,
    pub border_width: u32,
}

impl ThemeMetricsDefinition {
    fn into_metrics(self) -> Result<DecorationMetrics, DecorationThemeError> {
        let metrics = DecorationMetrics {
            titlebar_height: self.titlebar_height,
            button_visual_size: self.button_visual_size,
            button_spacing: self.button_spacing,
            right_padding: self.right_padding,
            horizontal_padding: self.horizontal_padding,
            border_width: self.border_width,
            resize_hit_width: 6,
            minimum_button_hit_width: 24,
        };
        metrics
            .validate()
            .then_some(metrics)
            .ok_or_else(|| DecorationThemeError::InvalidMetrics("value outside v1 bounds".into()))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThemeColorsDefinition {
    pub active_background: String,
    pub inactive_background: String,
    pub border: String,
    #[serde(default)]
    pub active_border: Option<String>,
    #[serde(default)]
    pub inactive_border: Option<String>,
    pub title: String,
    pub inactive_title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThemeColors {
    pub active_background: [u8; 4],
    pub inactive_background: [u8; 4],
    pub border: [u8; 4],
    pub active_border: [u8; 4],
    pub inactive_border: [u8; 4],
    pub title: [u8; 4],
    pub inactive_title: [u8; 4],
}

impl ThemeColorsDefinition {
    fn parse(self) -> Result<ThemeColors, DecorationThemeError> {
        Ok(ThemeColors {
            active_background: parse_color("active_background", &self.active_background)?,
            inactive_background: parse_color("inactive_background", &self.inactive_background)?,
            border: parse_color("border", &self.border)?,
            active_border: parse_color(
                "active_border",
                self.active_border.as_deref().unwrap_or(&self.border),
            )?,
            inactive_border: parse_color(
                "inactive_border",
                self.inactive_border.as_deref().unwrap_or(&self.border),
            )?,
            title: parse_color("title", &self.title)?,
            inactive_title: parse_color("inactive_title", &self.inactive_title)?,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThemeTitleDefinition {
    pub font_family: String,
    pub font_style: String,
    pub font_size: u32,
    pub alignment: ThemeTitleAlignment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ThemeTitleAlignment {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThemeButtonsDefinition {
    pub minimize: ThemeButtonDefinition,
    pub maximize: ThemeButtonDefinition,
    pub restore: ThemeButtonDefinition,
    pub close: ThemeButtonDefinition,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThemeButtonDefinition {
    pub active: ThemeButtonStateDefinition,
    #[serde(default)]
    pub inactive: Option<ThemeButtonStateDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThemeButtonStateDefinition {
    pub normal: String,
    #[serde(default)]
    pub hover: Option<String>,
    #[serde(default)]
    pub pressed: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct DecorationThemeSnapshot {
    name: String,
    schema_version: u32,
    source: String,
    generation: u64,
    metrics: DecorationMetrics,
    colors: ThemeColors,
    title: ThemeTitleDefinition,
    asset_paths: BTreeMap<String, String>,
    asset_bytes: BTreeMap<String, Arc<[u8]>>,
    raster_assets: BTreeMap<String, DecorationRasterAsset>,
    font_bytes: Option<Arc<[u8]>>,
    text_cache: Arc<Mutex<BTreeMap<String, super::text::RasterizedTitle>>>,
}

impl Default for DecorationThemeSnapshot {
    fn default() -> Self {
        load_theme_by_name("MacTahoe-Dark", 1).unwrap_or_else(|_| Self::builtin_mac_tahoe(1))
    }
}

impl DecorationThemeSnapshot {
    pub(crate) fn builtin_mac_tahoe(generation: u64) -> Self {
        let metrics = DecorationMetrics::mac_tahoe();
        let colors = ThemeColors {
            active_background: [51, 51, 51, 255],
            inactive_background: [36, 36, 36, 255],
            border: [1, 1, 1, 255],
            active_border: [1, 1, 1, 255],
            inactive_border: [44, 44, 44, 255],
            title: [255, 255, 255, 255],
            inactive_title: [255, 255, 255, 153],
        };
        let title = ThemeTitleDefinition {
            font_family: "Sans".into(),
            font_style: "Regular".into(),
            font_size: 13,
            alignment: ThemeTitleAlignment::Center,
        };
        let mut asset_paths = BTreeMap::new();
        for kind in ["minimize", "maximize", "restore", "close"] {
            for inactive in [false, true] {
                for state in ["normal", "hover", "pressed"] {
                    asset_paths.insert(
                        asset_key(kind, inactive, false, state),
                        format!("builtin/mactahoe/{kind}-{state}"),
                    );
                    if kind == "maximize" || kind == "restore" {
                        asset_paths.insert(
                            asset_key(kind, inactive, true, state),
                            format!("builtin/mactahoe/restore-{state}"),
                        );
                    }
                }
            }
        }
        Self {
            name: "Emergency-Fallback".into(),
            schema_version: 1,
            source: "emergency-fallback".into(),
            generation,
            metrics,
            colors,
            title,
            asset_paths,
            asset_bytes: BTreeMap::new(),
            raster_assets: BTreeMap::new(),
            font_bytes: None,
            text_cache: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) const fn metrics(&self) -> DecorationMetrics {
        self.metrics
    }

    pub(crate) const fn colors(&self) -> ThemeColors {
        self.colors
    }

    pub(crate) fn title(&self) -> &ThemeTitleDefinition {
        &self.title
    }

    pub(crate) fn active_asset(&self, kind: &str, inactive: bool, restore: bool) -> Option<&str> {
        self.asset(kind, inactive, restore, DecorationButtonVisualState::Normal)
    }

    pub(crate) fn asset(
        &self,
        kind: &str,
        inactive: bool,
        restore: bool,
        state: DecorationButtonVisualState,
    ) -> Option<&str> {
        let states = match state {
            DecorationButtonVisualState::Pressed => ["pressed", "hover", "normal"],
            DecorationButtonVisualState::Hovered => ["hover", "normal", "normal"],
            DecorationButtonVisualState::Normal => ["normal", "normal", "normal"],
        };
        for state_name in states {
            if let Some(path) = self
                .asset_paths
                .get(&asset_key(kind, inactive, restore, state_name))
            {
                return Some(path);
            }
            if inactive
                && let Some(path) = self
                    .asset_paths
                    .get(&asset_key(kind, false, restore, state_name))
            {
                return Some(path);
            }
            if restore
                && let Some(path) = self
                    .asset_paths
                    .get(&asset_key(kind, inactive, false, state_name))
            {
                return Some(path);
            }
        }
        None
    }

    pub(crate) fn asset_bytes(&self, path: &str) -> Option<&[u8]> {
        self.asset_bytes.get(path).map(AsRef::as_ref)
    }

    pub(crate) fn raster_asset(&self, path: &str, scale: f64) -> Option<&DecorationRasterAsset> {
        self.raster_assets.get(&raster_asset_key(path, scale))
    }

    pub(crate) fn rasterize_title(
        &self,
        title: &str,
        title_rect: super::types::DecorationRect,
        color: [u8; 4],
        output_scale: f64,
    ) -> Option<super::text::RasterizedTitle> {
        let font_bytes = self.font_bytes.as_deref()?;
        let key = format!(
            "{title}\0{}:{}:{}:{}:{}:{}:{}:{}:{}",
            title_rect.width,
            title_rect.height,
            color[0],
            color[1],
            color[2],
            color[3],
            self.title.font_size,
            self.title.alignment as u8,
            output_scale.to_bits(),
        );
        if let Ok(cache) = self.text_cache.lock()
            && let Some(raster) = cache.get(&key)
        {
            return Some(raster.clone());
        }
        let raster = super::text::rasterize_title(
            font_bytes,
            title,
            title_rect,
            color,
            self.title.font_size,
            self.title.alignment,
            output_scale,
        )
        .ok()?;
        if let Ok(mut cache) = self.text_cache.lock() {
            cache.insert(key, raster.clone());
        }
        Some(raster)
    }
}

pub(crate) fn parse_theme_json(
    bytes: &[u8],
) -> Result<DecorationThemeDefinition, DecorationThemeError> {
    if bytes.len() > MAX_THEME_JSON_BYTES {
        return Err(DecorationThemeError::TooLarge);
    }
    let definition = serde_json::from_slice::<DecorationThemeDefinition>(bytes)
        .map_err(|error| DecorationThemeError::InvalidSchema(error.to_string()))?;
    validate_definition(&definition)?;
    Ok(definition)
}

pub(crate) fn snapshot_from_definition(
    definition: DecorationThemeDefinition,
    generation: u64,
) -> Result<DecorationThemeSnapshot, DecorationThemeError> {
    let metrics = definition.metrics.into_metrics()?;
    let colors = definition.colors.parse()?;
    let font_bytes = select_font_bytes(&definition.title)?;
    let mut asset_paths = BTreeMap::new();
    insert_button_paths(
        &mut asset_paths,
        "minimize",
        &definition.buttons.minimize,
        false,
    );
    insert_button_paths(
        &mut asset_paths,
        "maximize",
        &definition.buttons.maximize,
        false,
    );
    insert_button_paths(
        &mut asset_paths,
        "restore",
        &definition.buttons.restore,
        false,
    );
    insert_button_paths(&mut asset_paths, "close", &definition.buttons.close, false);
    Ok(DecorationThemeSnapshot {
        name: definition.name,
        schema_version: definition.schema_version,
        source: "package".into(),
        generation,
        metrics,
        colors,
        title: definition.title,
        asset_paths,
        asset_bytes: BTreeMap::new(),
        raster_assets: BTreeMap::new(),
        font_bytes: Some(font_bytes),
        text_cache: Arc::new(Mutex::new(BTreeMap::new())),
    })
}

pub(crate) fn load_theme_package(
    root: &Path,
    generation: u64,
) -> Result<DecorationThemeSnapshot, DecorationThemeError> {
    let root =
        fs::canonicalize(root).map_err(|error| DecorationThemeError::AssetIo(error.to_string()))?;
    let document = root.join("theme.json");
    let bytes =
        fs::read(&document).map_err(|error| DecorationThemeError::AssetIo(error.to_string()))?;
    let definition = parse_theme_json(&bytes)?;
    let mut snapshot = snapshot_from_definition(definition, generation)?;
    let mut asset_bytes = BTreeMap::new();
    for path in snapshot.asset_paths.values() {
        let resolved = resolve_asset_path(&root, path)?;
        let metadata = fs::metadata(&resolved)
            .map_err(|error| DecorationThemeError::AssetIo(error.to_string()))?;
        if metadata.len() > MAX_ASSET_BYTES {
            return Err(DecorationThemeError::AssetTooLarge(path.clone()));
        }
        let data = fs::read(&resolved)
            .map_err(|error| DecorationThemeError::AssetIo(error.to_string()))?;
        validate_asset_bytes(path, &data)?;
        asset_bytes.insert(path.clone(), Arc::<[u8]>::from(data));
    }
    snapshot.asset_bytes = asset_bytes;
    for (path, bytes) in &snapshot.asset_bytes {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let raster = rasterize_svg(path, bytes, scale).map_err(DecorationThemeError::Raster)?;
            snapshot
                .raster_assets
                .insert(raster_asset_key(path, scale), raster);
        }
    }
    Ok(snapshot)
}

pub(crate) fn load_theme_by_name(
    name: &str,
    generation: u64,
) -> Result<DecorationThemeSnapshot, DecorationThemeError> {
    validate_theme_name(name)?;
    for root in theme_search_roots(name) {
        if root.is_dir() {
            let theme = load_theme_package(&root, generation)?;
            if theme.name() != name {
                return Err(DecorationThemeError::InvalidSchema(
                    "theme name does not match its package directory".into(),
                ));
            }
            return Ok(theme);
        }
    }
    Err(DecorationThemeError::AssetIo(format!(
        "theme package not found: {name}"
    )))
}

fn validate_theme_name(name: &str) -> Result<(), DecorationThemeError> {
    if name.is_empty()
        || name.len() > MAX_THEME_NAME_BYTES
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(DecorationThemeError::InvalidSchema(
            "theme name contains unsupported characters".into(),
        ));
    }
    Ok(())
}

fn theme_search_roots(name: &str) -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/decorations")
            .join(name),
    ];
    let mut standard_roots = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        standard_roots.push(PathBuf::from(data_home));
    } else if let Some(home) = std::env::var_os("HOME") {
        standard_roots.push(PathBuf::from(home).join(".local/share"));
    }
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        standard_roots.push(PathBuf::from(config_home));
    }
    standard_roots.extend([
        PathBuf::from("/usr/local/share"),
        PathBuf::from("/usr/share"),
    ]);
    roots.extend(
        standard_roots
            .into_iter()
            .map(|root| root.join("astrea/typhon/decorations").join(name)),
    );
    roots
}

pub(crate) fn available_theme_names() -> Vec<String> {
    let mut names = vec!["MacTahoe-Dark".to_string()];
    let mut roots = theme_search_roots("__unused__");
    for root in &mut roots {
        root.pop();
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if path.is_dir() && validate_theme_name(name).is_ok() {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

pub(crate) fn read_selected_theme() -> Result<Option<String>, DecorationThemeError> {
    let Some(path) = theme_selection_path() else {
        return Ok(None);
    };
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DecorationThemeError::Persistence(error.to_string())),
    };
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Selection {
        version: u32,
        theme: String,
    }
    let selection = serde_json::from_slice::<Selection>(&bytes)
        .map_err(|error| DecorationThemeError::Persistence(error.to_string()))?;
    if selection.version != 1 {
        return Err(DecorationThemeError::Persistence(
            "unsupported selection version".into(),
        ));
    }
    validate_theme_name(&selection.theme)?;
    Ok(Some(selection.theme))
}

pub(crate) fn write_selected_theme(name: &str) -> Result<(), DecorationThemeError> {
    validate_theme_name(name)?;
    let Some(path) = theme_selection_path() else {
        return Err(DecorationThemeError::Persistence(
            "no secure configuration home is available".into(),
        ));
    };
    let parent = path
        .parent()
        .ok_or_else(|| DecorationThemeError::Persistence("invalid selection path".into()))?;
    fs::create_dir_all(parent)
        .map_err(|error| DecorationThemeError::Persistence(error.to_string()))?;
    let document = serde_json::to_vec(&serde_json::json!({"version": 1, "theme": name}))
        .map_err(|error| DecorationThemeError::Persistence(error.to_string()))?;
    let temporary = parent.join(format!(".decoration.json.tmp-{}", std::process::id()));
    fs::write(&temporary, document)
        .map_err(|error| DecorationThemeError::Persistence(error.to_string()))?;
    #[cfg(unix)]
    fs::set_permissions(
        &temporary,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .map_err(|error| DecorationThemeError::Persistence(error.to_string()))?;
    fs::rename(&temporary, &path)
        .map_err(|error| DecorationThemeError::Persistence(error.to_string()))?;
    Ok(())
}

fn theme_selection_path() -> Option<PathBuf> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    if !config_home.is_absolute() {
        return None;
    }
    Some(config_home.join("AstreaOS/typhon/decoration.json"))
}

fn validate_definition(definition: &DecorationThemeDefinition) -> Result<(), DecorationThemeError> {
    if definition.schema_version != 1 {
        return Err(DecorationThemeError::InvalidSchema(
            "schema_version must be 1".into(),
        ));
    }
    validate_theme_name(&definition.name)?;
    if definition.title.font_family.is_empty()
        || definition.title.font_family.len() > MAX_THEME_NAME_BYTES
        || definition.title.font_style.len() > MAX_THEME_NAME_BYTES
        || !(8..=48).contains(&definition.title.font_size)
    {
        return Err(DecorationThemeError::InvalidSchema(
            "title font settings are outside v1 bounds".into(),
        ));
    }
    definition.colors.clone().parse()?;
    definition.metrics.clone().into_metrics()?;
    for (kind, button) in [
        ("minimize", &definition.buttons.minimize),
        ("maximize", &definition.buttons.maximize),
        ("restore", &definition.buttons.restore),
        ("close", &definition.buttons.close),
    ] {
        validate_button_paths(kind, &button.active)?;
        if let Some(inactive) = &button.inactive {
            validate_button_paths(kind, inactive)?;
        }
    }
    Ok(())
}

fn validate_button_paths(
    kind: &str,
    state: &ThemeButtonStateDefinition,
) -> Result<(), DecorationThemeError> {
    validate_relative_asset_path(&state.normal)?;
    for path in [state.hover.as_ref(), state.pressed.as_ref()]
        .into_iter()
        .flatten()
    {
        validate_relative_asset_path(path)?;
    }
    if state.normal.is_empty() {
        return Err(DecorationThemeError::InvalidSchema(format!(
            "{kind} normal asset is required"
        )));
    }
    Ok(())
}

fn insert_button_paths(
    table: &mut BTreeMap<String, String>,
    kind: &str,
    definition: &ThemeButtonDefinition,
    restore: bool,
) {
    insert_button_state(table, kind, false, restore, &definition.active);
    if let Some(inactive) = &definition.inactive {
        insert_button_state(table, kind, true, restore, inactive);
    }
}

fn insert_button_state(
    table: &mut BTreeMap<String, String>,
    kind: &str,
    inactive: bool,
    restore: bool,
    state: &ThemeButtonStateDefinition,
) {
    table.insert(
        asset_key(kind, inactive, restore, "normal"),
        state.normal.clone(),
    );
    if let Some(path) = &state.hover {
        table.insert(asset_key(kind, inactive, restore, "hover"), path.clone());
    }
    if let Some(path) = &state.pressed {
        table.insert(asset_key(kind, inactive, restore, "pressed"), path.clone());
    }
}

fn asset_key(kind: &str, inactive: bool, restore: bool, state: &str) -> String {
    format!("{kind}:{inactive}:{restore}:{state}")
}

fn parse_color(name: &str, value: &str) -> Result<[u8; 4], DecorationThemeError> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 && value.len() != 8 {
        return Err(DecorationThemeError::InvalidColor(name.into()));
    }
    let mut bytes = [0; 4];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        bytes[index] = u8::from_str_radix(
            std::str::from_utf8(chunk)
                .map_err(|_| DecorationThemeError::InvalidColor(name.into()))?,
            16,
        )
        .map_err(|_| DecorationThemeError::InvalidColor(name.into()))?;
    }
    if value.len() == 6 {
        bytes[3] = 255;
    }
    Ok(bytes)
}

fn validate_relative_asset_path(path: &str) -> Result<(), DecorationThemeError> {
    let path_ref = Path::new(path);
    if path.is_empty()
        || path_ref.is_absolute()
        || path_ref.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(DecorationThemeError::InvalidAssetPath(path.into()));
    }
    Ok(())
}

fn resolve_asset_path(root: &Path, path: &str) -> Result<PathBuf, DecorationThemeError> {
    validate_relative_asset_path(path)?;
    let candidate = root.join(path);
    let resolved = fs::canonicalize(&candidate)
        .map_err(|error| DecorationThemeError::AssetIo(error.to_string()))?;
    if !resolved.starts_with(root) {
        return Err(DecorationThemeError::InvalidAssetPath(path.into()));
    }
    Ok(resolved)
}

fn raster_asset_key(path: &str, scale: f64) -> String {
    format!("{path}@{}", (scale * 100.0).round() as u32)
}

fn select_font_bytes(title: &ThemeTitleDefinition) -> Result<Arc<[u8]>, DecorationThemeError> {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    let mut families = title
        .font_family
        .split(',')
        .map(str::trim)
        .filter(|family| !family.is_empty())
        .map(fontdb::Family::Name)
        .collect::<Vec<_>>();
    families.push(fontdb::Family::Name("Inter"));
    families.push(fontdb::Family::Name("Noto Sans"));
    families.push(fontdb::Family::SansSerif);
    let id = database
        .query(&fontdb::Query {
            families: &families,
            ..fontdb::Query::default()
        })
        .or_else(|| {
            database
                .faces()
                .find(|face| {
                    face.families
                        .iter()
                        .any(|(family, _)| family.contains("Sans") || family.contains("Adwaita"))
                })
                .map(|face| face.id)
        })
        .ok_or_else(|| DecorationThemeError::Raster("no usable title font was found".into()))?;
    database
        .with_face_data(id, |data, _| Arc::<[u8]>::from(data.to_vec()))
        .ok_or_else(|| {
            DecorationThemeError::Raster("selected title font bytes are unavailable".into())
        })
}

fn validate_asset_bytes(path: &str, bytes: &[u8]) -> Result<(), DecorationThemeError> {
    if path.ends_with(".svg") {
        let text = String::from_utf8_lossy(bytes);
        let lower = text
            .to_ascii_lowercase()
            .lines()
            .filter(|line| !line.contains("xmlns"))
            .collect::<String>();
        if lower.contains("http://")
            || lower.contains("https://")
            || lower.contains("file:")
            || lower.contains("<image")
            || lower.contains("@import")
            || lower.contains("url(")
        {
            return Err(DecorationThemeError::ExternalResource(path.into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DecorationThemeError, DecorationThemeSnapshot, load_theme_by_name, load_theme_package,
        parse_theme_json, snapshot_from_definition,
    };

    fn valid_theme() -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "name": "Test",
            "metrics": {
                "titlebar_height": 32,
                "button_visual_size": 16,
                "button_spacing": 9,
                "right_padding": 12,
                "horizontal_padding": 12,
                "border_width": 1
            },
            "colors": {
                "active_background": "#18202aff",
                "inactive_background": "#101419ff",
                "border": "#0a0d12ff",
                "title": "#ffffffff",
                "inactive_title": "#8a929eff"
            },
            "title": {
                "font_family": "Sans",
                "font_style": "Regular",
                "font_size": 13,
                "alignment": "center"
            },
            "buttons": {
                "minimize": {"active": {"normal": "min.svg"}},
                "maximize": {"active": {"normal": "max.svg"}},
                "restore": {"active": {"normal": "restore.svg"}},
                "close": {"active": {"normal": "close.svg"}}
            }
        })
    }

    #[test]
    fn schema_v1_rejects_unknown_fields_and_invalid_colors() {
        let mut unknown = valid_theme();
        unknown["unexpected"] = serde_json::json!(true);
        assert!(matches!(
            parse_theme_json(&serde_json::to_vec(&unknown).unwrap()),
            Err(DecorationThemeError::InvalidSchema(_))
        ));

        let mut invalid_color = valid_theme();
        invalid_color["colors"]["border"] = serde_json::json!("red");
        assert!(parse_theme_json(&serde_json::to_vec(&invalid_color).unwrap()).is_err());
    }

    #[test]
    fn schema_v1_rejects_path_like_theme_names() {
        let mut value = valid_theme();
        value["name"] = serde_json::json!("../escape");
        assert!(matches!(
            parse_theme_json(&serde_json::to_vec(&value).unwrap()),
            Err(DecorationThemeError::InvalidSchema(_))
        ));
    }

    #[test]
    fn snapshot_generation_changes_only_after_valid_activation() {
        let definition =
            parse_theme_json(&serde_json::to_vec(&valid_theme()).unwrap()).expect("valid theme");
        let snapshot = snapshot_from_definition(definition, 7).expect("snapshot");
        assert_eq!(snapshot.generation(), 7);
        assert_eq!(snapshot.metrics().titlebar_height, 32);
        assert_eq!(
            snapshot.active_asset("close", false, false),
            Some("close.svg")
        );
        assert_eq!(
            snapshot.active_asset("close", true, false),
            Some("close.svg")
        );
    }

    #[test]
    fn schema_and_asset_limits_are_explicit() {
        let mut value = valid_theme();
        value["metrics"]["titlebar_height"] = serde_json::json!(10);
        let error = parse_theme_json(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert!(matches!(error, DecorationThemeError::InvalidMetrics(_)));

        let oversized = vec![b' '; 65 * 1024];
        assert!(matches!(
            parse_theme_json(&oversized),
            Err(DecorationThemeError::TooLarge)
        ));
    }

    #[test]
    fn snapshot_keeps_a_bounded_immutable_asset_table() {
        let snapshot = DecorationThemeSnapshot::builtin_mac_tahoe(3);
        assert_eq!(snapshot.generation(), 3);
        assert!(snapshot.active_asset("minimize", false, false).is_some());
        assert!(snapshot.active_asset("maximize", true, true).is_some());
    }

    #[test]
    fn bundled_mac_tahoe_package_loads_local_assets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources/decorations/MacTahoe-Dark");
        let snapshot = load_theme_package(&root, 9).expect("bundled theme package");
        assert_eq!(snapshot.name(), "MacTahoe-Dark");
        assert_eq!(snapshot.source(), "package");
        assert_eq!(snapshot.metrics().titlebar_height, 26);
        assert!(snapshot.asset_bytes("assets/close-active.svg").is_some());
        assert!(
            snapshot
                .raster_asset("assets/close-active.svg", 1.25)
                .is_some()
        );
    }

    #[test]
    fn mactahoe_name_uses_the_bundled_package_loader() {
        let snapshot = load_theme_by_name("MacTahoe-Dark", 12).expect("bundled MacTahoe");
        assert_eq!(snapshot.source(), "package");
        assert_eq!(snapshot.generation(), 12);
        assert!(snapshot.asset_bytes("assets/minimize-active.svg").is_some());
    }
}
