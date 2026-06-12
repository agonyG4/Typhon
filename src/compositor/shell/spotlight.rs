use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use astrea_framework::{
    geometry::{Rect as LayoutRect, Size},
    theme::AstreaTheme,
};
use exodus::shell::{
    Spotlight, SpotlightFontWeight, SpotlightLayout, SpotlightResult, SpotlightResultLayout,
    SpotlightWeather,
};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

use super::{
    canvas::{FrameSize, Rect as CanvasRect, draw_rounded_rect, premul_argb},
    font_text::{NativeTextStyle, draw_native_text_in_rect, fit_native_text_to_width},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellLaunchSuggestion {
    label: String,
    command: String,
    entry_key: String,
    launch: LaunchKind,
}

impl ShellLaunchSuggestion {
    fn desktop(entry: &DesktopEntry, argv: Vec<String>) -> Self {
        Self {
            label: entry.name.clone(),
            command: entry
                .exec
                .as_deref()
                .unwrap_or(entry.desktop_id.as_str())
                .to_string(),
            entry_key: entry.entry_key(),
            launch: LaunchKind::Argv { argv },
        }
    }

    fn exec_argv(entry: &DesktopEntry, argv: Vec<String>) -> Self {
        Self {
            label: entry.name.clone(),
            command: entry.exec.clone().unwrap_or_default(),
            entry_key: entry.entry_key(),
            launch: LaunchKind::Argv { argv },
        }
    }

    fn typed_command(command: &str) -> Self {
        Self {
            label: command.to_string(),
            command: command.to_string(),
            entry_key: command.to_string(),
            launch: LaunchKind::Command {
                command: command.to_string(),
            },
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    fn entry_key(&self) -> &str {
        &self.entry_key
    }

    pub fn argv(&self) -> Vec<String> {
        match &self.launch {
            LaunchKind::Argv { argv } => argv.clone(),
            LaunchKind::Command { command } => {
                vec!["sh".to_string(), "-lc".to_string(), command.clone()]
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LaunchKind {
    Argv { argv: Vec<String> },
    Command { command: String },
}

pub fn launcher_suggestions(query: &str) -> Vec<ShellLaunchSuggestion> {
    DesktopEntryIndex::system().search(query, &UsageCounts::load())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightModel {
    visible: bool,
    query: String,
    selected_index: usize,
    index: Arc<DesktopEntryIndex>,
    usage_counts: UsageCounts,
    persist_usage: bool,
}

impl Default for SpotlightModel {
    fn default() -> Self {
        Self {
            visible: false,
            query: String::new(),
            selected_index: 0,
            index: Arc::new(DesktopEntryIndex::system()),
            usage_counts: UsageCounts::load(),
            persist_usage: true,
        }
    }
}

impl SpotlightModel {
    pub fn show(&mut self) {
        self.visible = true;
        self.query.clear();
        self.selected_index = 0;
        self.index = Arc::new(DesktopEntryIndex::system());
        self.usage_counts = UsageCounts::load();
    }

    pub fn toggle(&mut self) {
        if self.visible {
            self.hide();
        } else {
            self.show();
        }
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.query.clear();
        self.selected_index = 0;
    }

    pub const fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn push_text(&mut self, text: &str) {
        let old_len = self.query.len();
        self.query
            .extend(text.chars().filter(|character| !character.is_control()));
        if self.query.len() != old_len {
            self.selected_index = 0;
        }
    }

    pub fn backspace(&mut self) -> bool {
        let changed = self.query.pop().is_some();
        if changed {
            self.selected_index = 0;
        }
        changed
    }

    pub fn select_next(&mut self) -> bool {
        let result_count = self.search_results().len();
        if result_count == 0 {
            return false;
        }
        let next_index = (self.selected_index + 1) % result_count;
        let changed = next_index != self.selected_index;
        self.selected_index = next_index;
        changed
    }

    pub fn select_previous(&mut self) -> bool {
        let result_count = self.search_results().len();
        if result_count == 0 {
            return false;
        }
        let next_index = (self.selected_index + result_count - 1) % result_count;
        let changed = next_index != self.selected_index;
        self.selected_index = next_index;
        changed
    }

    pub fn selected_label(&self) -> Option<String> {
        self.selected_suggestion()
            .map(|suggestion| suggestion.label().to_string())
    }

    pub fn selected_launch_command(&mut self) -> Option<Vec<String>> {
        let query = self.query.trim();
        if query.is_empty() {
            return None;
        }

        if let Some(suggestion) = self.selected_suggestion() {
            self.usage_counts.bump(suggestion.entry_key());
            if self.persist_usage {
                self.usage_counts.save();
            }
            return Some(suggestion.argv());
        }

        Some(ShellLaunchSuggestion::typed_command(query).argv())
    }

    const fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn selected_suggestion(&self) -> Option<ShellLaunchSuggestion> {
        let query = self.query.trim();
        if query.is_empty() {
            return None;
        }

        let suggestions = self.search_results();
        suggestions
            .get(self.selected_index.min(suggestions.len().saturating_sub(1)))
            .cloned()
    }

    fn search_results(&self) -> Vec<ShellLaunchSuggestion> {
        self.index.search(self.query(), &self.usage_counts)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DesktopEntryIndex {
    entries: Vec<DesktopEntry>,
}

impl DesktopEntryIndex {
    fn system() -> Self {
        Self::from_application_dirs(application_dirs())
    }

    fn from_application_dirs<I, P>(dirs: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        for dir in dirs {
            let Ok(read_dir) = fs::read_dir(dir.as_ref()) else {
                continue;
            };
            let mut paths = read_dir
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("desktop"))
                .collect::<Vec<_>>();
            paths.sort();

            for path in paths {
                let Some(entry) = DesktopEntry::from_file(&path) else {
                    continue;
                };
                if seen.insert(entry.entry_key()) {
                    entries.push(entry);
                }
            }
        }

        Self { entries }
    }

    fn search(&self, query: &str, usage_counts: &UsageCounts) -> Vec<ShellLaunchSuggestion> {
        rank_desktop_entries(&self.entries, query, usage_counts)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesktopEntry {
    desktop_id: String,
    name: String,
    generic: String,
    comment: String,
    keywords: Vec<String>,
    categories: Vec<String>,
    startup_wm_class: String,
    exec: Option<String>,
}

impl DesktopEntry {
    fn from_file(path: &Path) -> Option<Self> {
        let values = desktop_entry_values(path)?;
        if values
            .get("Type")
            .map(String::as_str)
            .unwrap_or("Application")
            != "Application"
        {
            return None;
        }
        if truthy(values.get("NoDisplay")) || truthy(values.get("Hidden")) {
            return None;
        }

        let name = desktop_value(&values, "Name")?;
        let exec = values
            .get("Exec")
            .filter(|value| !value.is_empty())
            .cloned();
        let desktop_id = path.file_name()?.to_string_lossy().to_string();
        Some(Self {
            desktop_id,
            name,
            generic: desktop_value(&values, "GenericName").unwrap_or_default(),
            comment: desktop_value(&values, "Comment").unwrap_or_default(),
            keywords: desktop_list(values.get("Keywords")),
            categories: desktop_list(values.get("Categories")),
            startup_wm_class: values.get("StartupWMClass").cloned().unwrap_or_default(),
            exec,
        })
    }

    fn entry_key(&self) -> String {
        if !self.desktop_id.is_empty() {
            self.desktop_id.clone()
        } else {
            format!("{}|{}", self.name, self.exec.as_deref().unwrap_or_default())
        }
    }

    fn suggestion(&self) -> Option<ShellLaunchSuggestion> {
        let argv = parse_exec_line(self.exec.as_deref()?)?;
        if argv.is_empty() {
            return None;
        }
        if !self.desktop_id.is_empty() {
            return Some(ShellLaunchSuggestion::desktop(self, argv));
        }
        Some(ShellLaunchSuggestion::exec_argv(self, argv))
    }

    #[cfg(test)]
    fn for_test(desktop_id: &str, name: &str, generic: &str) -> Self {
        Self {
            desktop_id: desktop_id.to_string(),
            name: name.to_string(),
            generic: generic.to_string(),
            comment: String::new(),
            keywords: Vec::new(),
            categories: Vec::new(),
            startup_wm_class: String::new(),
            exec: Some(format!("{desktop_id} --open")),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageCounts {
    values: HashMap<String, u64>,
}

impl UsageCounts {
    fn load() -> Self {
        let Ok(text) = fs::read_to_string(usage_path()) else {
            return Self::default();
        };
        let Ok(serde_json::Value::Object(payload)) = serde_json::from_str(&text) else {
            return Self::default();
        };
        let values = payload
            .into_iter()
            .filter_map(|(key, value)| value.as_u64().map(|count| (key, count)))
            .collect();
        Self { values }
    }

    fn save(&self) {
        let path = usage_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string(&self.values) {
            let _ = fs::write(path, format!("{text}\n"));
        }
    }

    fn count_for(&self, entry: &DesktopEntry) -> u64 {
        self.values.get(&entry.entry_key()).copied().unwrap_or(0)
    }

    fn bump(&mut self, key: &str) {
        *self.values.entry(key.to_string()).or_insert(0) += 1;
    }

    #[cfg(test)]
    fn from_pairs<const N: usize>(pairs: [(&str, u64); N]) -> Self {
        Self {
            values: pairs
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        }
    }
}

fn rank_desktop_entries(
    entries: &[DesktopEntry],
    query: &str,
    usage_counts: &UsageCounts,
) -> Vec<ShellLaunchSuggestion> {
    let query = normalize_search(query.trim());
    if query.is_empty() {
        return Vec::new();
    }

    let search_terms = search_tokens(&query);
    let mut seen = HashSet::new();
    let mut items = entries
        .iter()
        .filter(|entry| !entry.name.trim().is_empty())
        .filter(|entry| seen.insert(entry.entry_key()))
        .filter_map(|entry| {
            let score = entry_search_score(entry, &query, &search_terms)?;
            Some(RankedDesktopEntry {
                entry,
                name: normalize_search(&entry.name),
                score,
                usage: usage_counts.count_for(entry),
            })
        })
        .collect::<Vec<_>>();

    items.sort_by(|left, right| {
        left.score
            .cmp(&right.score)
            .then_with(|| right.usage.cmp(&left.usage))
            .then_with(|| left.name.cmp(&right.name))
    });

    if items.iter().any(|item| item.score < 12) {
        items.retain(|item| item.score < 12);
    }

    items
        .into_iter()
        .take(6)
        .filter_map(|item| item.entry.suggestion())
        .collect()
}

#[derive(Debug, Clone)]
struct RankedDesktopEntry<'a> {
    entry: &'a DesktopEntry,
    name: String,
    score: u32,
    usage: u64,
}

fn entry_search_score(entry: &DesktopEntry, query: &str, search_terms: &[String]) -> Option<u32> {
    let aliases = aliases_for_name(&entry.name);
    let mut best = score_text(&entry.name, query, search_terms, 0, true);

    for alias in aliases {
        let alias_score = score_text(&alias, query, search_terms, 1, false);
        if alias_score.is_some_and(|score| best.is_none_or(|best| score < best)) {
            best = alias_score;
        }
    }

    let metadata = [
        entry.keywords.join(" "),
        entry.generic.clone(),
        entry.comment.clone(),
        entry.categories.join(" "),
    ]
    .into_iter()
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    let metadata_score = score_text(&metadata, query, search_terms, 12, false);
    if metadata_score.is_some_and(|score| best.is_none_or(|best| score < best)) {
        best = metadata_score;
    }

    let identifiers = [
        entry.desktop_id.clone(),
        entry.startup_wm_class.clone(),
        entry.exec.clone().unwrap_or_default(),
    ]
    .into_iter()
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    let identifier_score = score_text(&identifiers, query, search_terms, 24, false);
    if identifier_score.is_some_and(|score| best.is_none_or(|best| score < best)) {
        best = identifier_score;
    }

    best
}

fn score_text(
    value: &str,
    query: &str,
    search_terms: &[String],
    base_score: u32,
    allow_fuzzy: bool,
) -> Option<u32> {
    let text = normalize_search(value);
    if text.is_empty() {
        return None;
    }

    let parts = search_tokens(&text);
    if text == query {
        return Some(base_score);
    }
    if text.starts_with(query) {
        return Some(base_score + 2);
    }
    if parts.iter().any(|part| part.starts_with(query)) {
        return Some(base_score + 5);
    }
    if parts
        .iter()
        .any(|part| query.starts_with(part) && part.len() >= 4 && query.len() - part.len() <= 2)
    {
        return Some(base_score + 7);
    }
    if search_terms.len() > 1
        && search_terms
            .iter()
            .all(|term| parts.iter().any(|part| part.starts_with(term)))
    {
        return Some(base_score + 8);
    }
    if search_terms.iter().all(|term| text.contains(term)) {
        return Some(base_score + 14);
    }
    if allow_fuzzy && query.len() >= 3 && is_subsequence(query, &text) {
        return Some(base_score + 34 + text.len().saturating_sub(query.len()) as u32);
    }
    None
}

fn aliases_for_name(name: &str) -> Vec<String> {
    let tokens = search_tokens(name);
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut aliases = vec![tokens.join(""), acronym_for_tokens(&tokens)];
    if tokens.len() >= 2 {
        let mut compact = tokens[..tokens.len() - 1]
            .iter()
            .filter_map(|part| part.chars().next())
            .collect::<String>();
        compact.push_str(&tokens[tokens.len() - 1]);
        aliases.push(compact);
    }
    aliases
}

fn acronym_for_tokens(tokens: &[String]) -> String {
    tokens
        .iter()
        .filter_map(|part| part.chars().next())
        .collect()
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut needle = needle.chars();
    let Some(mut expected) = needle.next() else {
        return true;
    };
    for character in haystack.chars() {
        if character == expected {
            let Some(next) = needle.next() else {
                return true;
            };
            expected = next;
        }
    }
    false
}

fn normalize_search(value: &str) -> String {
    value
        .to_lowercase()
        .nfd()
        .filter(|character| !is_combining_mark(*character))
        .collect()
}

fn search_tokens(value: &str) -> Vec<String> {
    normalize_search(value)
        .split(is_search_separator)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_search_separator(character: char) -> bool {
    character.is_whitespace() || matches!(character, '.' | '_' | ':' | '/' | '\\' | '-')
}

fn desktop_entry_values(path: &Path) -> Option<HashMap<String, String>> {
    let text = fs::read_to_string(path).ok()?;
    let mut values = HashMap::new();
    let mut in_entry = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            values.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    Some(values)
}

fn desktop_value(values: &HashMap<String, String>, base: &str) -> Option<String> {
    values.get(base).filter(|value| !value.is_empty()).cloned()
}

fn desktop_list(value: Option<&String>) -> Vec<String> {
    value
        .map(|value| {
            value
                .split(';')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn truthy(value: Option<&String>) -> bool {
    value.is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "true" | "1"))
}

fn parse_exec_line(line: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut quote = None;
    let mut arg_started = false;
    let mut suppress_arg = false;

    while let Some(character) = chars.next() {
        match character {
            '\'' | '"' if quote.is_none() => {
                quote = Some(character);
                arg_started = true;
            }
            '\'' | '"' if quote == Some(character) => {
                quote = None;
                arg_started = true;
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    arg_started = true;
                    suppress_arg = false;
                }
            }
            '%' => {
                arg_started = true;
                match chars.next() {
                    Some('%') => {
                        current.push('%');
                        suppress_arg = false;
                    }
                    Some(code) if "fFuUick".contains(code) => {
                        if current.is_empty() {
                            suppress_arg = true;
                        }
                    }
                    Some(_) | None => {}
                }
            }
            character if character.is_whitespace() && quote.is_none() => {
                finish_exec_arg(&mut args, &mut current, &mut arg_started, &mut suppress_arg);
            }
            _ => {
                current.push(character);
                arg_started = true;
                suppress_arg = false;
            }
        }
    }

    if quote.is_some() {
        return None;
    }
    finish_exec_arg(&mut args, &mut current, &mut arg_started, &mut suppress_arg);
    Some(args)
}

fn finish_exec_arg(
    args: &mut Vec<String>,
    current: &mut String,
    arg_started: &mut bool,
    suppress_arg: &mut bool,
) {
    if *arg_started && !*suppress_arg {
        args.push(std::mem::take(current));
    } else {
        current.clear();
    }
    *arg_started = false;
    *suppress_arg = false;
}

pub(super) fn spotlight_bounds(
    width: u32,
    height: u32,
    spotlight: &SpotlightModel,
) -> Option<CanvasRect> {
    if !spotlight.is_visible() {
        return None;
    }

    Some(canvas_rect(
        spotlight_layout(width, height, spotlight).panel_rect,
    ))
}

pub(super) fn draw_spotlight_at(
    frame: &mut [u32],
    width: u32,
    height: u32,
    output_width: u32,
    output_height: u32,
    origin: (i32, i32),
    spotlight: &SpotlightModel,
) {
    let layout = spotlight_layout(output_width, output_height, spotlight);
    let panel_rect = canvas_rect_with_origin(layout.panel_rect, origin);

    draw_panel_with_border(frame, width, height, panel_rect);
    draw_search_icon(
        frame,
        FrameSize { width, height },
        canvas_rect_with_origin(layout.search_row_rect, origin),
        layout.typography.search_icon_pixel_size,
        premul_argb(153, 255, 255, 255),
    );
    draw_weather_chip(
        frame,
        FrameSize { width, height },
        canvas_rect_with_origin(layout.weather_rect, origin),
        &layout.weather,
        layout.typography.weather_pixel_size,
        layout.typography.weather_weight,
    );

    let query_text = if layout.query_text.is_empty() {
        layout.placeholder_text.as_str()
    } else {
        layout.query_text.as_str()
    };
    let query_color = if layout.query_text.is_empty() {
        premul_argb(102, 255, 255, 255)
    } else {
        premul_argb(255, 255, 255, 255)
    };
    let field_rect = canvas_rect_with_origin(layout.field_rect, origin);
    let query_style = NativeTextStyle {
        pixel_size: layout.typography.search_pixel_size as f32,
        color: query_color,
        weight: layout.typography.search_weight,
    };
    let fitted_query = fit_native_text_to_width(query_text, field_rect.width, query_style);
    draw_native_text_in_rect(
        frame,
        FrameSize { width, height },
        field_rect,
        &fitted_query,
        query_style,
    );

    if let Some(divider_rect) = layout.divider_rect {
        draw_rounded_rect(
            frame,
            width,
            height,
            canvas_rect_with_origin(divider_rect, origin),
            0,
            premul_argb(21, 255, 255, 255),
        );
    }
    for row in &layout.result_rows {
        draw_result_row(
            frame,
            FrameSize { width, height },
            row,
            origin,
            layout.typography.result_pixel_size,
            layout.typography.result_weight,
        );
    }
}

fn draw_panel_with_border(frame: &mut [u32], width: u32, height: u32, rect: CanvasRect) {
    draw_rounded_rect(
        frame,
        width,
        height,
        rect,
        24,
        premul_argb(51, 255, 255, 255),
    );
    draw_rounded_rect(
        frame,
        width,
        height,
        CanvasRect::new(
            rect.x + 1,
            rect.y + 1,
            rect.width.saturating_sub(2),
            rect.height.saturating_sub(2),
        ),
        23,
        premul_argb(128, 52, 52, 52),
    );
}

fn draw_search_icon(
    frame: &mut [u32],
    frame_size: FrameSize,
    search_row_rect: CanvasRect,
    pixel_size: u32,
    color: u32,
) {
    draw_native_text_in_rect(
        frame,
        frame_size,
        CanvasRect::new(
            search_row_rect.x + 8,
            search_row_rect.y,
            28,
            search_row_rect.height,
        ),
        "⌕",
        NativeTextStyle {
            pixel_size: pixel_size as f32,
            color,
            weight: SpotlightFontWeight::Regular,
        },
    );
}

fn draw_weather_chip(
    frame: &mut [u32],
    frame_size: FrameSize,
    rect: CanvasRect,
    weather: &SpotlightWeather,
    pixel_size: u32,
    weight: SpotlightFontWeight,
) {
    let color = premul_argb(153, 255, 255, 255);
    draw_native_text_in_rect(
        frame,
        frame_size,
        CanvasRect::new(rect.x + 4, rect.y, 20, rect.height),
        if weather.ready { "●" } else { "○" },
        NativeTextStyle {
            pixel_size: pixel_size as f32,
            color,
            weight: SpotlightFontWeight::Regular,
        },
    );

    let temperature = if weather.ready {
        format!("{}°", weather.temperature_c)
    } else {
        "--°".to_string()
    };
    draw_native_text_in_rect(
        frame,
        frame_size,
        CanvasRect::new(
            rect.x + 25,
            rect.y,
            rect.width.saturating_sub(25),
            rect.height,
        ),
        &temperature,
        NativeTextStyle {
            pixel_size: pixel_size as f32,
            color,
            weight,
        },
    );
}

fn draw_result_row(
    frame: &mut [u32],
    frame_size: FrameSize,
    row: &SpotlightResultLayout,
    origin: (i32, i32),
    pixel_size: u32,
    weight: SpotlightFontWeight,
) {
    let rect = canvas_rect_with_origin(row.rect, origin);
    if row.selected {
        draw_rounded_rect(
            frame,
            frame_size.width,
            frame_size.height,
            rect,
            7,
            premul_argb(255, 0, 122, 255),
        );
    }

    let icon_rect = canvas_rect_with_origin(row.icon_rect, origin);
    draw_rounded_rect(
        frame,
        frame_size.width,
        frame_size.height,
        icon_rect,
        10,
        premul_argb(34, 255, 255, 255),
    );
    let icon_label = row.result.label.chars().next().unwrap_or('?').to_string();
    draw_native_text_in_rect(
        frame,
        frame_size,
        CanvasRect::new(icon_rect.x + 12, icon_rect.y, 16, icon_rect.height),
        &icon_label,
        NativeTextStyle {
            pixel_size: 17.0,
            color: premul_argb(255, 255, 255, 255),
            weight: SpotlightFontWeight::Medium,
        },
    );

    let text_rect = canvas_rect_with_origin(row.text_rect, origin);
    let available_width = text_rect.width;
    let label_style = NativeTextStyle {
        pixel_size: pixel_size as f32,
        color: premul_argb(255, 255, 255, 255),
        weight,
    };
    let fitted_label = fit_native_text_to_width(&row.result.label, available_width, label_style);
    draw_native_text_in_rect(frame, frame_size, text_rect, &fitted_label, label_style);
}

fn canvas_rect(rect: LayoutRect) -> CanvasRect {
    CanvasRect::new(
        rect.x.round() as i32,
        rect.y.round() as i32,
        rect.width.round().max(0.0) as u32,
        rect.height.round().max(0.0) as u32,
    )
}

fn canvas_rect_with_origin(rect: LayoutRect, origin: (i32, i32)) -> CanvasRect {
    canvas_rect(rect).translated(-origin.0, -origin.1)
}

fn spotlight_layout(width: u32, height: u32, spotlight: &SpotlightModel) -> SpotlightLayout {
    let theme = AstreaTheme::default_dark();
    let selected_index = spotlight.selected_index();
    let results = spotlight
        .search_results()
        .into_iter()
        .enumerate()
        .map(|(index, suggestion)| {
            SpotlightResult::new(
                suggestion.label(),
                suggestion.command(),
                index == selected_index,
            )
        });
    Spotlight::new()
        .query(spotlight.query())
        .weather(SpotlightWeather::loading())
        .results(results)
        .layout(Size::new(width as f32, height as f32), &theme)
}

fn usage_path() -> PathBuf {
    xdg_state_home().join("Astrea").join("spotlight-usage.json")
}

fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();
    push_unique_path(&mut dirs, &mut seen, xdg_data_home().join("applications"));

    let data_dirs = env::var_os("XDG_DATA_DIRS")
        .map(|value| env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_else(|| {
            vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]
        });

    for dir in data_dirs {
        push_unique_path(&mut dirs, &mut seen, dir.join("applications"));
    }

    dirs
}

fn push_unique_path(paths: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, path: PathBuf) {
    if seen.insert(path.clone()) {
        paths.push(path);
    }
}

fn xdg_data_home() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local").join("share"))
}

fn xdg_state_home() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local").join("state"))
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotlight_query_launches_typed_command_when_no_app_matches() {
        let mut spotlight = open_test_spotlight(Vec::new());
        spotlight.push_text("kitty --class astrea-test");

        assert_eq!(
            spotlight.selected_launch_command(),
            Some(vec![
                "sh".to_string(),
                "-lc".to_string(),
                "kitty --class astrea-test".to_string()
            ])
        );
    }

    #[test]
    fn spotlight_opens_like_a_system_launcher() {
        let mut spotlight = SpotlightModel::default();
        spotlight.push_text("brave");
        spotlight.show();

        assert!(spotlight.is_visible());
        assert_eq!(spotlight.query(), "");
        assert_eq!(spotlight.selected_launch_command(), None);
    }

    #[test]
    fn spotlight_enter_launches_selected_desktop_entry_from_its_exec_line() {
        let mut spotlight = open_test_spotlight(vec![DesktopEntry::for_test(
            "brave-browser.desktop",
            "Brave",
            "Browser",
        )]);
        spotlight.push_text("br");

        let command = spotlight.selected_launch_command().unwrap();

        assert_eq!(
            command,
            vec!["brave-browser.desktop".to_string(), "--open".to_string()]
        );
    }

    #[test]
    fn spotlight_arrow_selection_tracks_filtered_results_with_wrapping() {
        let mut spotlight = open_test_spotlight(vec![
            DesktopEntry::for_test("brave-browser.desktop", "Brave", "Browser"),
            DesktopEntry::for_test("zen-browser.desktop", "Zen Browser", "Browser"),
        ]);
        spotlight.push_text("br");

        assert_eq!(spotlight.selected_label().as_deref(), Some("Brave"));
        spotlight.select_next();
        assert_eq!(spotlight.selected_label().as_deref(), Some("Zen Browser"));
        spotlight.select_next();
        assert_eq!(spotlight.selected_label().as_deref(), Some("Brave"));
        spotlight.select_previous();
        assert_eq!(spotlight.selected_label().as_deref(), Some("Zen Browser"));
    }

    #[test]
    fn astrea_spotlight_ranking_uses_name_metadata_usage_then_label() {
        let entries = vec![
            DesktopEntry::for_test("alpha.desktop", "Code Alpha", "Editor"),
            DesktopEntry::for_test("beta.desktop", "Code Beta", "Editor"),
            DesktopEntry::for_test("ocean.desktop", "Ocean", "Code Browser"),
        ];
        let usage_counts = UsageCounts::from_pairs([("beta.desktop", 4)]);

        let labels = rank_desktop_entries(&entries, "code", &usage_counts)
            .into_iter()
            .map(|suggestion| suggestion.label().to_string())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["Code Beta", "Code Alpha"]);
    }

    #[test]
    fn astrea_desktop_entry_index_reads_xdg_application_dirs() {
        let dir = unique_test_dir("oblivion-one-desktop-entries");
        fs::create_dir_all(&dir).unwrap();
        let desktop_file = dir.join("org.astrea.Example.desktop");
        fs::write(
            &desktop_file,
            "\
[Desktop Entry]
Type=Application
Name=Astrea Example
GenericName=Launcher
Comment=Launches an example app
Keywords=spotlight;astrea;
Exec=astrea-example --open %U
",
        )
        .unwrap();

        let index = DesktopEntryIndex::from_application_dirs([&dir]);
        let result = index
            .search("launcher", &UsageCounts::default())
            .into_iter()
            .next()
            .unwrap();

        assert_eq!(result.label(), "Astrea Example");
        assert_eq!(
            result.argv(),
            vec!["astrea-example".to_string(), "--open".to_string()]
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn astrea_search_matches_hyphenated_browser_names() {
        let entries = vec![DesktopEntry::for_test(
            "zen-browser.desktop",
            "Zen Browser",
            "Browser",
        )];

        let labels = rank_desktop_entries(&entries, "Zen-Browser", &UsageCounts::default())
            .into_iter()
            .map(|suggestion| suggestion.label().to_string())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["Zen Browser"]);
    }

    #[test]
    fn astrea_exec_parser_removes_desktop_field_codes() {
        let argv = parse_exec_line("app --name \"Hello World\" %U %% --flag").unwrap();

        assert_eq!(argv, vec!["app", "--name", "Hello World", "%", "--flag"]);
    }

    fn open_test_spotlight(entries: Vec<DesktopEntry>) -> SpotlightModel {
        SpotlightModel {
            visible: true,
            query: String::new(),
            selected_index: 0,
            index: Arc::new(DesktopEntryIndex { entries }),
            usage_counts: UsageCounts::default(),
            persist_usage: false,
        }
    }

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{suffix}", std::process::id()))
    }
}
