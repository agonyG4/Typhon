use astrea_framework::{
    component::ComponentState,
    geometry::{Rect, Size},
    material::{Material, MaterialRole},
    theme::AstreaTheme,
    typography::TypographyTokens,
};

use crate::components::ListRow;

pub use astrea_framework::typography::FontWeight as SpotlightFontWeight;

#[derive(Debug, Clone, PartialEq)]
pub struct Topbar {
    leading_text: String,
    trailing_text: Option<String>,
}

impl Topbar {
    pub fn new(leading_text: impl Into<String>) -> Self {
        Self {
            leading_text: leading_text.into(),
            trailing_text: None,
        }
    }

    pub fn trailing_text(mut self, trailing_text: impl Into<String>) -> Self {
        self.trailing_text = Some(trailing_text.into());
        self
    }

    pub fn layout(&self, viewport: Size, theme: &AstreaTheme) -> TopbarLayout {
        let margin_x = 12.0;
        let top = 10.0;
        let height = 34.0;
        let width = (viewport.width - margin_x * 2.0).max(0.0);
        TopbarLayout {
            bar_rect: Rect::new(margin_x, top, width, height),
            leading_text: self.leading_text.clone(),
            trailing_text: self.trailing_text.clone(),
            material: theme.material(MaterialRole::Panel),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TopbarLayout {
    pub bar_rect: Rect,
    pub leading_text: String,
    pub trailing_text: Option<String>,
    pub material: Material,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spotlight {
    query: String,
    weather: SpotlightWeather,
    results: Vec<SpotlightResult>,
}

impl Spotlight {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn query(mut self, query: impl Into<String>) -> Self {
        self.query = query.into();
        self
    }

    pub fn weather(mut self, weather: SpotlightWeather) -> Self {
        self.weather = weather;
        self
    }

    pub fn results(mut self, results: impl IntoIterator<Item = SpotlightResult>) -> Self {
        self.results = results.into_iter().collect();
        self
    }

    pub fn layout(&self, viewport: Size, theme: &AstreaTheme) -> SpotlightLayout {
        let panel_width = (viewport.width - 80.0).clamp(240.0, 600.0);
        let x = (viewport.width - panel_width) / 2.0;
        let y = viewport.height * 0.25;
        let content_margin = 14.0;
        let search_row_height = 30.0;
        let weather_width = 68.0_f32.min((panel_width - content_margin * 2.0).max(0.0));
        let search_row_rect = Rect::new(
            x + content_margin,
            y + content_margin,
            (panel_width - content_margin * 2.0).max(0.0),
            search_row_height,
        );
        let weather_rect = Rect::new(
            x + panel_width - content_margin - weather_width,
            search_row_rect.y,
            weather_width,
            search_row_height,
        );
        let field_x = search_row_rect.x + 44.0;
        let field_rect = Rect::new(
            field_x,
            search_row_rect.y,
            (weather_rect.x - field_x - 8.0).max(0.0),
            search_row_height,
        );
        let visible_results = if self.query.trim().is_empty() {
            Vec::new()
        } else {
            self.results.iter().take(6).cloned().collect::<Vec<_>>()
        };
        let divider_rect = (!visible_results.is_empty()).then(|| {
            Rect::new(
                search_row_rect.x,
                search_row_rect.y + search_row_height + 12.0,
                search_row_rect.width,
                1.0,
            )
        });
        let result_rows = divider_rect
            .map(|divider| {
                visible_results
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, result)| {
                        let row_rect = Rect::new(
                            search_row_rect.x,
                            divider.y + divider.height + 8.0 + index as f32 * 50.0,
                            search_row_rect.width,
                            50.0,
                        );
                        let selected = result.selected;
                        let row_layout = ListRow::new(result.label.clone())
                            .state(ComponentState::default().selected(selected))
                            .layout(row_rect, theme);

                        SpotlightResultLayout {
                            rect: row_layout.row_rect,
                            icon_rect: row_layout.icon_rect,
                            text_rect: row_layout.text_rect,
                            selected,
                            result,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let panel_height = if result_rows.is_empty() {
            58.0
        } else {
            let content_height =
                search_row_height + 12.0 + 1.0 + 8.0 + result_rows.len() as f32 * 50.0;
            (content_height + content_margin * 2.0).min(450.0)
        };
        SpotlightLayout {
            panel_rect: Rect::new(x, y, panel_width, panel_height),
            search_row_rect,
            field_rect,
            weather_rect,
            divider_rect,
            query_text: self.query.clone(),
            placeholder_text: "Spotlight Search".to_string(),
            weather: self.weather.clone(),
            result_rows,
            typography: SpotlightTypography::from_theme(theme),
            material: theme.material(MaterialRole::Overlay),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightTypography {
    pub font_family: String,
    pub search_icon_pixel_size: u32,
    pub search_pixel_size: u32,
    pub search_weight: SpotlightFontWeight,
    pub result_pixel_size: u32,
    pub result_weight: SpotlightFontWeight,
    pub weather_pixel_size: u32,
    pub weather_weight: SpotlightFontWeight,
}

impl SpotlightTypography {
    pub fn astrea_default() -> Self {
        Self::from_tokens(&TypographyTokens::default())
    }

    pub fn from_theme(theme: &AstreaTheme) -> Self {
        Self::from_tokens(theme.typography())
    }

    pub fn from_tokens(tokens: &TypographyTokens) -> Self {
        Self {
            font_family: tokens.spotlight_search.family.to_string(),
            search_icon_pixel_size: tokens.spotlight_icon.pixel_size,
            search_pixel_size: tokens.spotlight_search.pixel_size,
            search_weight: tokens.spotlight_search.weight,
            result_pixel_size: tokens.spotlight_result.pixel_size,
            result_weight: tokens.spotlight_result.weight,
            weather_pixel_size: tokens.spotlight_weather.pixel_size,
            weather_weight: tokens.spotlight_weather.weight,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpotlightWeather {
    pub ready: bool,
    pub condition: String,
    pub temperature_c: i32,
}

impl SpotlightWeather {
    pub fn loading() -> Self {
        Self::default()
    }

    pub fn ready(condition: impl Into<String>, temperature_c: i32) -> Self {
        Self {
            ready: true,
            condition: condition.into(),
            temperature_c,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightResult {
    pub label: String,
    pub command: String,
    pub selected: bool,
}

impl SpotlightResult {
    pub fn new(label: impl Into<String>, command: impl Into<String>, selected: bool) -> Self {
        Self {
            label: label.into(),
            command: command.into(),
            selected,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpotlightLayout {
    pub panel_rect: Rect,
    pub search_row_rect: Rect,
    pub field_rect: Rect,
    pub weather_rect: Rect,
    pub divider_rect: Option<Rect>,
    pub query_text: String,
    pub placeholder_text: String,
    pub weather: SpotlightWeather,
    pub result_rows: Vec<SpotlightResultLayout>,
    pub typography: SpotlightTypography,
    pub material: Material,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpotlightResultLayout {
    pub rect: Rect,
    pub icon_rect: Rect,
    pub text_rect: Rect,
    pub result: SpotlightResult,
    pub selected: bool,
}
