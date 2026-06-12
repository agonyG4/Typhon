use astrea_framework::{
    color::Rgba,
    component::ComponentState,
    geometry::{CornerRadii, Rect, Size},
    layout::{BoxConstraints, Insets},
    material::{Material, MaterialRole},
    theme::AstreaTheme,
    typography::TextStyle,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    frame_rect: Rect,
    padding: Insets,
    role: MaterialRole,
}

impl Panel {
    pub fn new(frame_rect: Rect) -> Self {
        Self {
            frame_rect,
            padding: Insets::all(14.0),
            role: MaterialRole::Panel,
        }
    }

    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = Insets::all(padding);
        self
    }

    pub fn material_role(mut self, role: MaterialRole) -> Self {
        self.role = role;
        self
    }

    pub fn layout(&self, theme: &AstreaTheme) -> PanelLayout {
        PanelLayout {
            frame_rect: self.frame_rect,
            content_rect: self.padding.inset_rect(self.frame_rect),
            material: theme.material(self.role),
            radii: theme.radius().panel,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PanelLayout {
    pub frame_rect: Rect,
    pub content_rect: Rect,
    pub material: Material,
    pub radii: CornerRadii,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Ghost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    label: String,
    variant: ButtonVariant,
    state: ComponentState,
}

impl Button {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            variant: ButtonVariant::Secondary,
            state: ComponentState::default(),
        }
    }

    pub const fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub const fn state(mut self, state: ComponentState) -> Self {
        self.state = state;
        self
    }

    pub fn layout(&self, rect: Rect, theme: &AstreaTheme) -> ButtonLayout {
        let size = BoxConstraints::new(Size::new(96.0, 36.0), Size::new(f32::MAX, f32::MAX))
            .constrain(Size::new(rect.width, rect.height));
        let frame_rect = Rect::new(rect.x, rect.y, size.width, size.height);
        let content_rect = Insets::symmetric(14.0, 8.0).inset_rect(frame_rect);
        let base_background = match self.variant {
            ButtonVariant::Primary => Rgba::rgb(0, 122, 255),
            ButtonVariant::Secondary => theme.material(MaterialRole::Control).background,
            ButtonVariant::Ghost => Rgba::TRANSPARENT,
        };

        ButtonLayout {
            frame_rect,
            content_rect,
            label: self.label.clone(),
            variant: self.variant,
            state: self.state,
            text_style: theme.typography().label,
            background: self.state.surface_alpha(base_background),
            radii: theme.radius().control,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ButtonLayout {
    pub frame_rect: Rect,
    pub content_rect: Rect,
    pub label: String,
    pub variant: ButtonVariant,
    pub state: ComponentState,
    pub text_style: TextStyle,
    pub background: Rgba,
    pub radii: CornerRadii,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextField {
    placeholder: String,
    value: String,
    state: ComponentState,
}

impl TextField {
    pub fn new(placeholder: impl Into<String>) -> Self {
        Self {
            placeholder: placeholder.into(),
            value: String::new(),
            state: ComponentState::default(),
        }
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    pub const fn state(mut self, state: ComponentState) -> Self {
        self.state = state;
        self
    }

    pub fn layout(&self, rect: Rect, theme: &AstreaTheme) -> TextFieldLayout {
        let height = rect.height.max(36.0);
        let frame_rect = Rect::new(rect.x, rect.y, rect.width.max(120.0), height);
        let text_rect = Insets::symmetric(14.0, 8.0).inset_rect(frame_rect);
        let cursor_rect = self.state.focused.then(|| {
            Rect::new(
                text_rect.x + self.value.chars().count() as f32 * 8.5,
                text_rect.y + 1.0,
                1.0,
                18.0,
            )
        });

        TextFieldLayout {
            frame_rect,
            text_rect,
            cursor_rect,
            placeholder: self.placeholder.clone(),
            value: self.value.clone(),
            state: self.state,
            text_style: theme.typography().spotlight_search,
            background: self
                .state
                .surface_alpha(theme.material(MaterialRole::Control).background),
            radii: theme.radius().control,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextFieldLayout {
    pub frame_rect: Rect,
    pub text_rect: Rect,
    pub cursor_rect: Option<Rect>,
    pub placeholder: String,
    pub value: String,
    pub state: ComponentState,
    pub text_style: TextStyle,
    pub background: Rgba,
    pub radii: CornerRadii,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToggleSwitch {
    on: bool,
    state: ComponentState,
}

impl ToggleSwitch {
    pub fn new(on: bool) -> Self {
        Self {
            on,
            state: ComponentState::default(),
        }
    }

    pub const fn state(mut self, state: ComponentState) -> Self {
        self.state = state;
        self
    }

    pub fn layout(&self, rect: Rect, _theme: &AstreaTheme) -> ToggleSwitchLayout {
        let track_rect = Rect::new(rect.x, rect.y, 46.0, 28.0);
        let thumb_x = if self.on { rect.x + 21.0 } else { rect.x + 3.0 };
        let thumb_rect = Rect::new(thumb_x, rect.y + 3.0, 22.0, 22.0);

        ToggleSwitchLayout {
            track_rect,
            thumb_rect,
            on: self.on,
            state: self.state,
            track_color: if self.on {
                self.state.surface_alpha(Rgba::rgb(0, 122, 255))
            } else {
                self.state.surface_alpha(Rgba::new(255, 255, 255, 34))
            },
            thumb_color: self.state.content_alpha(Rgba::rgb(255, 255, 255)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToggleSwitchLayout {
    pub track_rect: Rect,
    pub thumb_rect: Rect,
    pub on: bool,
    pub state: ComponentState,
    pub track_color: Rgba,
    pub thumb_color: Rgba,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slider {
    value: f32,
    state: ComponentState,
}

impl Slider {
    pub fn new(value: f32) -> Self {
        Self {
            value,
            state: ComponentState::default(),
        }
    }

    pub const fn state(mut self, state: ComponentState) -> Self {
        self.state = state;
        self
    }

    pub fn layout(&self, rect: Rect, _theme: &AstreaTheme) -> SliderLayout {
        let value = self.value.clamp(0.0, 1.0);
        let track_rect = Rect::new(rect.x, rect.y + (rect.height - 4.0) / 2.0, rect.width, 4.0);
        let fill_rect = Rect::new(track_rect.x, track_rect.y, track_rect.width * value, 4.0);
        let thumb_rect = Rect::new(
            track_rect.x + value * (track_rect.width - 2.0) - 10.0,
            rect.y + (rect.height - 20.0) / 2.0,
            20.0,
            20.0,
        );

        SliderLayout {
            value,
            track_rect,
            fill_rect,
            thumb_rect,
            state: self.state,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderLayout {
    pub value: f32,
    pub track_rect: Rect,
    pub fill_rect: Rect,
    pub thumb_rect: Rect,
    pub state: ComponentState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    title: String,
    subtitle: Option<String>,
    state: ComponentState,
}

impl ListRow {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            state: ComponentState::default(),
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub const fn state(mut self, state: ComponentState) -> Self {
        self.state = state;
        self
    }

    pub fn layout(&self, row_rect: Rect, theme: &AstreaTheme) -> ListRowLayout {
        let icon_rect = Rect::new(row_rect.x + 12.0, row_rect.y + 5.0, 40.0, 40.0);
        let text_x = icon_rect.x + icon_rect.width + 15.0;
        let text_rect = Rect::new(
            text_x,
            row_rect.y,
            (row_rect.x + row_rect.width - text_x - 12.0).max(0.0),
            row_rect.height,
        );

        ListRowLayout {
            row_rect,
            icon_rect,
            text_rect,
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            selected: self.state.selected,
            state: self.state,
            text_style: theme.typography().spotlight_result,
            selected_background: Rgba::rgb(0, 122, 255),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListRowLayout {
    pub row_rect: Rect,
    pub icon_rect: Rect,
    pub text_rect: Rect,
    pub title: String,
    pub subtitle: Option<String>,
    pub selected: bool,
    pub state: ComponentState,
    pub text_style: TextStyle,
    pub selected_background: Rgba,
}
