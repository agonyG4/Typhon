//! Stable semantic animation identities and built-in capability catalog.

use crate::presentation_animation_policy::PresentationAnimationKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AnimationSlot {
    WindowMove,
    WindowResize,
    LayoutReflow,
    WindowMaximize,
    WindowFullscreen,
    WindowOpen,
    WindowClose,
    WindowMinimize,
    WindowRestore,
    WorkspaceSwitch,
    WorkspaceWindowMove,
}

impl AnimationSlot {
    pub const ALL: [Self; 11] = [
        Self::WindowMove,
        Self::WindowResize,
        Self::LayoutReflow,
        Self::WindowMaximize,
        Self::WindowFullscreen,
        Self::WindowOpen,
        Self::WindowClose,
        Self::WindowMinimize,
        Self::WindowRestore,
        Self::WorkspaceSwitch,
        Self::WorkspaceWindowMove,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::WindowMove => "window.move",
            Self::WindowResize => "window.resize",
            Self::LayoutReflow => "layout.reflow",
            Self::WindowMaximize => "window.maximize",
            Self::WindowFullscreen => "window.fullscreen",
            Self::WindowOpen => "window.open",
            Self::WindowClose => "window.close",
            Self::WindowMinimize => "window.minimize",
            Self::WindowRestore => "window.restore",
            Self::WorkspaceSwitch => "workspace.switch",
            Self::WorkspaceWindowMove => "workspace.window-move",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|slot| slot.id() == value)
    }

    pub const fn geometry_kind(self) -> Option<PresentationAnimationKind> {
        match self {
            Self::WindowMove => Some(PresentationAnimationKind::ProgrammaticMove),
            Self::WindowResize => Some(PresentationAnimationKind::ProgrammaticResize),
            Self::LayoutReflow => Some(PresentationAnimationKind::LayoutReflow),
            Self::WindowMaximize => Some(PresentationAnimationKind::MaximizeEnter),
            Self::WindowFullscreen => Some(PresentationAnimationKind::FullscreenEnter),
            _ => None,
        }
    }

    pub const fn from_geometry_kind(kind: PresentationAnimationKind) -> Self {
        match kind {
            PresentationAnimationKind::ProgrammaticMove => Self::WindowMove,
            PresentationAnimationKind::ProgrammaticResize => Self::WindowResize,
            PresentationAnimationKind::LayoutReflow => Self::LayoutReflow,
            PresentationAnimationKind::MaximizeEnter | PresentationAnimationKind::MaximizeExit => {
                Self::WindowMaximize
            }
            PresentationAnimationKind::FullscreenEnter
            | PresentationAnimationKind::FullscreenExit => Self::WindowFullscreen,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AnimationEffect {
    None,
    GeometryKde,
    GeometryMacos,
    WindowScale,
    WindowGlide,
    MinimizeLamp,
    MinimizeSquash,
    WorkspaceSlide,
}

impl AnimationEffect {
    pub const ALL: [Self; 8] = [
        Self::None,
        Self::GeometryKde,
        Self::GeometryMacos,
        Self::WindowScale,
        Self::WindowGlide,
        Self::MinimizeLamp,
        Self::MinimizeSquash,
        Self::WorkspaceSlide,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::GeometryKde => "geometry.kde",
            Self::GeometryMacos => "geometry.macos",
            Self::WindowScale => "window.scale",
            Self::WindowGlide => "window.glide",
            Self::MinimizeLamp => "minimize.lamp",
            Self::MinimizeSquash => "minimize.squash",
            Self::WorkspaceSlide => "workspace.slide",
        }
    }

    pub const fn is_available(self) -> bool {
        matches!(self, Self::None | Self::GeometryKde | Self::GeometryMacos)
    }

    pub const fn availability(self) -> &'static str {
        if self.is_available() { "available" } else { "planned" }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|effect| effect.id() == value)
    }

    pub const fn compatible_with(self, slot: AnimationSlot) -> bool {
        match self {
            Self::None => true,
            Self::GeometryKde | Self::GeometryMacos => slot.geometry_kind().is_some(),
            Self::WindowScale | Self::WindowGlide => {
                matches!(slot, AnimationSlot::WindowOpen | AnimationSlot::WindowClose)
            }
            Self::MinimizeLamp | Self::MinimizeSquash => {
                matches!(slot, AnimationSlot::WindowMinimize | AnimationSlot::WindowRestore)
            }
            Self::WorkspaceSlide => matches!(
                slot,
                AnimationSlot::WorkspaceSwitch | AnimationSlot::WorkspaceWindowMove
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AnimationPreset {
    Astrea,
    Kde,
    Macos,
}

impl AnimationPreset {
    pub const ALL: [Self; 3] = [Self::Astrea, Self::Kde, Self::Macos];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Astrea => "astrea",
            Self::Kde => "kde",
            Self::Macos => "macos",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|preset| preset.id() == value)
    }

    pub const fn requested_effect(self, slot: AnimationSlot) -> AnimationEffect {
        match (self, slot) {
            (
                Self::Astrea | Self::Kde,
                AnimationSlot::WindowMove
                | AnimationSlot::WindowResize
                | AnimationSlot::LayoutReflow
                | AnimationSlot::WindowMaximize
                | AnimationSlot::WindowFullscreen,
            ) => AnimationEffect::GeometryKde,
            (Self::Astrea, AnimationSlot::WindowMinimize | AnimationSlot::WindowRestore) => {
                AnimationEffect::MinimizeLamp
            }
            (
                Self::Macos,
                AnimationSlot::WindowMove
                | AnimationSlot::WindowResize
                | AnimationSlot::LayoutReflow
                | AnimationSlot::WindowMaximize
                | AnimationSlot::WindowFullscreen,
            ) => AnimationEffect::GeometryMacos,
            _ => AnimationEffect::None,
        }
    }
}

pub const fn effect_for_request(
    slot: AnimationSlot,
    requested: AnimationEffect,
    enabled: bool,
) -> AnimationEffect {
    if enabled && requested.is_available() && requested.compatible_with(slot) {
        requested
    } else {
        AnimationEffect::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_catalog_contains_all_v1_ids() {
        assert_eq!(AnimationSlot::ALL[0].id(), "window.move");
        assert_eq!(AnimationSlot::ALL[10].id(), "workspace.window-move");
        assert_eq!(AnimationEffect::ALL[5].id(), "minimize.lamp");
        assert_eq!(AnimationPreset::ALL.map(AnimationPreset::id), ["astrea", "kde", "macos"]);
    }

    #[test]
    fn astrea_reserves_planned_lamp_but_effectively_resolves_to_none() {
        let slot = AnimationSlot::WindowMinimize;
        let requested = AnimationPreset::Astrea.requested_effect(slot);
        assert_eq!(requested, AnimationEffect::MinimizeLamp);
        assert_eq!(effect_for_request(slot, requested, true), AnimationEffect::None);
    }

    #[test]
    fn geometry_presets_preserve_the_existing_families() {
        for slot in [
            AnimationSlot::WindowMove,
            AnimationSlot::WindowResize,
            AnimationSlot::LayoutReflow,
            AnimationSlot::WindowMaximize,
            AnimationSlot::WindowFullscreen,
        ] {
            assert_eq!(AnimationPreset::Astrea.requested_effect(slot), AnimationEffect::GeometryKde);
            assert_eq!(AnimationPreset::Kde.requested_effect(slot), AnimationEffect::GeometryKde);
            assert_eq!(AnimationPreset::Macos.requested_effect(slot), AnimationEffect::GeometryMacos);
        }
    }

    #[test]
    fn planned_and_incompatible_effects_are_not_available_for_manual_use() {
        assert!(!AnimationEffect::MinimizeLamp.is_available());
        assert!(!AnimationEffect::GeometryKde.compatible_with(AnimationSlot::WindowOpen));
    }
}
