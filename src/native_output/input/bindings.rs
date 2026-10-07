use super::*;
use crate::system_action::AstreaSystemAction;
use oblivion_one::wm::WorkspaceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModifierMask(u8);

impl ModifierMask {
    pub(crate) const EMPTY: Self = Self(0);
    pub(crate) const ALT: Self = Self(1 << 0);
    pub(crate) const SHIFT: Self = Self(1 << 1);
    pub(crate) const SUPER: Self = Self(1 << 2);
    pub(crate) const CTRL: Self = Self(1 << 3);

    pub(crate) const fn matches(self, active: Self) -> bool {
        self.0 == active.0
    }

    pub(crate) const fn contains(self, family: Self) -> bool {
        self.0 & family.0 == family.0
    }
}

impl std::ops::BitOr for ModifierMask {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum BindingTrigger {
    Press,
    Release,
    PointerPress,
    PointerRelease,
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct BindingKeySym(u32);

impl BindingKeySym {
    pub(crate) const fn new(keysym: u32) -> Self {
        Self(keysym)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum BindingInput {
    PhysicalKey(u16),
    KeySym(BindingKeySym),
    PointerButton(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BindingActionDefinition {
    ExitCompositor,
    CloseActiveWindow,
    ToggleFullscreen,
    ToggleFocusedWindowLayout,
    SwitchWorkspace(WorkspaceId),
    MoveFocusedWindowToWorkspace(WorkspaceId),
    ToggleDefaultSpecialWorkspace,
    MoveFocusedWindowToOrFromSpecialWorkspace,
    LaunchCommand(Vec<String>),
    LaunchSessionCommand {
        index: u8,
        command: Option<Vec<String>>,
    },
    SystemAction(AstreaSystemAction),
    BeginMove,
    BeginResize,
    EmitShortcut {
        namespace: String,
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepeatPolicy {
    Disabled,
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InhibitionPolicy {
    Respect,
    Bypass,
}

/// Cold definition input. Compiled bindings retain only their action ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingSpec {
    pub(crate) modifiers: ModifierMask,
    pub(crate) trigger: BindingTrigger,
    pub(crate) input: BindingInput,
    pub(crate) action: BindingActionDefinition,
    pub(crate) repeat: RepeatPolicy,
    pub(crate) inhibition: InhibitionPolicy,
    pub(crate) reserved: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ActiveBindingState {
    pub(crate) alt_tab_active: bool,
}

#[cfg(test)]
pub(super) fn modifier_release_index(family: ModifierMask) -> Option<usize> {
    if family == ModifierMask::ALT {
        Some(0)
    } else if family == ModifierMask::CTRL {
        Some(1)
    } else if family == ModifierMask::SHIFT {
        Some(2)
    } else if family == ModifierMask::SUPER {
        Some(3)
    } else {
        None
    }
}

pub(super) const fn shortcut_phase(trigger: BindingTrigger, repeated: bool) -> AstreaShortcutPhase {
    match trigger {
        BindingTrigger::Press | BindingTrigger::PointerPress => {
            if repeated {
                AstreaShortcutPhase::Repeated
            } else {
                AstreaShortcutPhase::Pressed
            }
        }
        BindingTrigger::Release | BindingTrigger::PointerRelease => AstreaShortcutPhase::Released,
    }
}

pub(crate) fn default_astrea_binding_specs() -> Vec<BindingSpec> {
    let mut specs = vec![
        BindingSpec {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_Q),
            action: BindingActionDefinition::LaunchCommand(vec!["kitty".to_string()]),
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_S),
            action: BindingActionDefinition::ToggleDefaultSpecialWorkspace,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::SUPER | ModifierMask::SHIFT,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_S),
            action: BindingActionDefinition::MoveFocusedWindowToOrFromSpecialWorkspace,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_C),
            action: BindingActionDefinition::CloseActiveWindow,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_F),
            action: BindingActionDefinition::ToggleFullscreen,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_V),
            action: BindingActionDefinition::ToggleFocusedWindowLayout,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_SPACE),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "spotlight_toggle".to_string(),
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::EMPTY,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_SYSRQ),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "screenshot_quick".to_string(),
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        },
        BindingSpec {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_SYSRQ),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "screenshot_region_frozen".to_string(),
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        },
        BindingSpec {
            modifiers: ModifierMask::SUPER | ModifierMask::SHIFT,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_SYSRQ),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "screenshot_region_live".to_string(),
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        },
        BindingSpec {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::PointerPress,
            input: BindingInput::PointerButton(u32::from(BTN_LEFT)),
            action: BindingActionDefinition::BeginMove,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::PointerPress,
            input: BindingInput::PointerButton(u32::from(BTN_RIGHT)),
            action: BindingActionDefinition::BeginResize,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::ALT,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_TAB),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "alt_tab_next".to_string(),
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::ALT | ModifierMask::SHIFT,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_TAB),
            action: BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "alt_tab_previous".to_string(),
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        BindingSpec {
            modifiers: ModifierMask::ALT,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_P),
            action: BindingActionDefinition::ExitCompositor,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        },
        BindingSpec {
            modifiers: ModifierMask::CTRL | ModifierMask::SHIFT | ModifierMask::ALT,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_1),
            action: BindingActionDefinition::LaunchSessionCommand {
                index: 1,
                command: None,
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        },
        BindingSpec {
            modifiers: ModifierMask::CTRL | ModifierMask::SHIFT | ModifierMask::ALT,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_2),
            action: BindingActionDefinition::LaunchSessionCommand {
                index: 2,
                command: None,
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        },
        BindingSpec {
            modifiers: ModifierMask::CTRL | ModifierMask::SHIFT | ModifierMask::ALT,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(KEY_3),
            action: BindingActionDefinition::LaunchSessionCommand {
                index: 3,
                command: None,
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        },
    ];
    for workspace_number in 1..=10 {
        let workspace = WorkspaceId::new(workspace_number).expect("workspace binding id");
        let key = workspace_key(workspace);
        specs.push(BindingSpec {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(key),
            action: BindingActionDefinition::SwitchWorkspace(workspace),
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        });
        specs.push(BindingSpec {
            modifiers: ModifierMask::SUPER | ModifierMask::SHIFT,
            trigger: BindingTrigger::Press,
            input: BindingInput::PhysicalKey(key),
            action: BindingActionDefinition::MoveFocusedWindowToWorkspace(workspace),
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        });
    }

    use xkbcommon::xkb::keysyms;
    for (keysym, action, repeat) in [
        (
            keysyms::KEY_XF86AudioRaiseVolume,
            AstreaSystemAction::OutputVolumeUp,
            RepeatPolicy::Enabled,
        ),
        (
            keysyms::KEY_XF86AudioLowerVolume,
            AstreaSystemAction::OutputVolumeDown,
            RepeatPolicy::Enabled,
        ),
        (
            keysyms::KEY_XF86AudioMute,
            AstreaSystemAction::ToggleOutputMute,
            RepeatPolicy::Disabled,
        ),
        (
            keysyms::KEY_XF86AudioMicMute,
            AstreaSystemAction::ToggleMicrophoneMute,
            RepeatPolicy::Disabled,
        ),
        (
            keysyms::KEY_XF86AudioPlay,
            AstreaSystemAction::MediaPlayPause,
            RepeatPolicy::Disabled,
        ),
        (
            keysyms::KEY_XF86AudioPause,
            AstreaSystemAction::MediaPause,
            RepeatPolicy::Disabled,
        ),
        (
            keysyms::KEY_XF86AudioStop,
            AstreaSystemAction::MediaStop,
            RepeatPolicy::Disabled,
        ),
        (
            keysyms::KEY_XF86AudioNext,
            AstreaSystemAction::MediaNext,
            RepeatPolicy::Disabled,
        ),
        (
            keysyms::KEY_XF86AudioPrev,
            AstreaSystemAction::MediaPrevious,
            RepeatPolicy::Disabled,
        ),
        (
            keysyms::KEY_XF86AudioRewind,
            AstreaSystemAction::MediaRewind,
            RepeatPolicy::Disabled,
        ),
        (
            keysyms::KEY_XF86AudioForward,
            AstreaSystemAction::MediaFastForward,
            RepeatPolicy::Disabled,
        ),
        (
            keysyms::KEY_XF86MonBrightnessUp,
            AstreaSystemAction::DisplayBrightnessUp,
            RepeatPolicy::Enabled,
        ),
        (
            keysyms::KEY_XF86MonBrightnessDown,
            AstreaSystemAction::DisplayBrightnessDown,
            RepeatPolicy::Enabled,
        ),
        (
            keysyms::KEY_XF86KbdBrightnessUp,
            AstreaSystemAction::KeyboardBrightnessUp,
            RepeatPolicy::Enabled,
        ),
        (
            keysyms::KEY_XF86KbdBrightnessDown,
            AstreaSystemAction::KeyboardBrightnessDown,
            RepeatPolicy::Enabled,
        ),
        (
            keysyms::KEY_XF86KbdLightOnOff,
            AstreaSystemAction::ToggleKeyboardBacklight,
            RepeatPolicy::Disabled,
        ),
        (
            keysyms::KEY_XF86TouchpadToggle,
            AstreaSystemAction::ToggleTouchpad,
            RepeatPolicy::Disabled,
        ),
    ] {
        specs.push(BindingSpec {
            modifiers: ModifierMask::EMPTY,
            trigger: BindingTrigger::Press,
            input: BindingInput::KeySym(BindingKeySym::new(keysym)),
            action: BindingActionDefinition::SystemAction(action),
            repeat,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        });
    }

    specs
}

const fn workspace_key(workspace: WorkspaceId) -> u16 {
    match workspace.get() {
        1 => KEY_1,
        2 => KEY_2,
        3 => KEY_3,
        4 => KEY_4,
        5 => KEY_5,
        6 => KEY_6,
        7 => KEY_7,
        8 => KEY_8,
        9 => KEY_9,
        10 => KEY_0,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action_for_match<'a>(
        manager: &'a AstreaBindingManager,
        matched: AstreaBindingMatch,
    ) -> Option<&'a BindingActionDefinition> {
        match matched {
            AstreaBindingMatch::Consumed { action, .. } => manager.action_catalog().action(action),
            AstreaBindingMatch::Pass => None,
        }
    }

    #[test]
    fn default_workspace_bindings_are_typed_non_repeating_and_not_reserved() {
        let mut manager = AstreaBindingManager::default();
        let matched = manager.handle_key(ModifierMask::SUPER, KEY_0, true, false, false);
        assert_eq!(
            action_for_match(&manager, matched),
            Some(&BindingActionDefinition::SwitchWorkspace(
                WorkspaceId::new(10).unwrap()
            ))
        );
        assert_eq!(
            manager.handle_key(ModifierMask::SUPER, KEY_0, true, true, false),
            AstreaBindingMatch::Pass
        );
        let matched = manager.handle_key(
            ModifierMask::SUPER | ModifierMask::SHIFT,
            KEY_4,
            true,
            false,
            false,
        );
        assert_eq!(
            action_for_match(&manager, matched),
            Some(&BindingActionDefinition::MoveFocusedWindowToWorkspace(
                WorkspaceId::new(4).unwrap()
            ))
        );
    }

    #[test]
    fn default_pointer_bindings_keep_move_resize_and_inhibition_policy() {
        let mut manager = AstreaBindingManager::default();

        for (button, expected) in [
            (BTN_LEFT, BindingActionDefinition::BeginMove),
            (BTN_RIGHT, BindingActionDefinition::BeginResize),
        ] {
            let matched =
                manager.handle_pointer_button(ModifierMask::SUPER, u32::from(button), true, false);
            assert_eq!(action_for_match(&manager, matched), Some(&expected));
            assert_eq!(
                manager.handle_pointer_button(ModifierMask::SUPER, u32::from(button), true, true,),
                AstreaBindingMatch::Pass
            );
        }
    }

    #[test]
    fn session_switch_bindings_keep_their_reserved_modifier_boundary() {
        let mut manager = AstreaBindingManager::default();
        let matched = manager.handle_key(
            ModifierMask::CTRL | ModifierMask::SHIFT | ModifierMask::ALT,
            KEY_1,
            true,
            false,
            true,
        );
        assert!(matches!(
            action_for_match(&manager, matched),
            Some(BindingActionDefinition::LaunchSessionCommand { index: 1, .. })
        ));
    }

    #[test]
    fn special_workspace_bindings_are_press_only_exact_and_inhibition_aware() {
        let mut manager = AstreaBindingManager::default();
        let matched = manager.handle_key(ModifierMask::SUPER, KEY_S, true, false, false);
        assert_eq!(
            action_for_match(&manager, matched),
            Some(&BindingActionDefinition::ToggleDefaultSpecialWorkspace)
        );
        assert_eq!(
            manager.handle_key(ModifierMask::SUPER, KEY_S, true, true, false),
            AstreaBindingMatch::Pass
        );
        let matched = manager.handle_key(
            ModifierMask::SUPER | ModifierMask::SHIFT,
            KEY_S,
            true,
            false,
            false,
        );
        assert_eq!(
            action_for_match(&manager, matched),
            Some(&BindingActionDefinition::MoveFocusedWindowToOrFromSpecialWorkspace)
        );
        assert_eq!(
            manager.handle_key(ModifierMask::SUPER, KEY_S, true, false, true),
            AstreaBindingMatch::Pass
        );
        assert_eq!(
            manager.handle_key(ModifierMask::SUPER, KEY_S, false, false, false),
            AstreaBindingMatch::Pass
        );
    }
}
