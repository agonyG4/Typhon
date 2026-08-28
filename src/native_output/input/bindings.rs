use super::*;
use oblivion_one::wm::WorkspaceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl std::ops::BitOr for ModifierMask {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingTrigger {
    Press,
    Release,
    PointerPress,
    PointerRelease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingInput {
    Key(u16),
    PointerButton(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BindingAction {
    ExitCompositor,
    CloseActiveWindow,
    ToggleFullscreen,
    ToggleFocusedWindowLayout,
    SwitchWorkspace(WorkspaceId),
    MoveFocusedWindowToWorkspace(WorkspaceId),
    ToggleDefaultSpecialWorkspace,
    MoveFocusedWindowToOrFromSpecialWorkspace,
    LaunchCommand(Vec<String>),
    LaunchSessionCommand(u8),
    BeginMove,
    BeginResize,
    EmitShortcut { namespace: String, name: String },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) modifiers: ModifierMask,
    pub(crate) trigger: BindingTrigger,
    pub(crate) input: BindingInput,
    pub(crate) action: BindingAction,
    pub(crate) repeat: RepeatPolicy,
    pub(crate) inhibition: InhibitionPolicy,
    pub(crate) reserved: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ActiveBindingState {
    pub(crate) alt_tab_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AstreaBindingManager {
    bindings: Vec<Binding>,
    active_sequences: ActiveBindingState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AstreaBindingMatch {
    Consumed {
        action: BindingAction,
        phase: AstreaShortcutPhase,
    },
    Pass,
}

impl Default for AstreaBindingManager {
    fn default() -> Self {
        Self::with_default_bindings()
    }
}

impl AstreaBindingManager {
    pub(crate) fn with_default_bindings() -> Self {
        Self {
            bindings: default_astrea_bindings(),
            active_sequences: ActiveBindingState::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_bindings(bindings: Vec<Binding>) -> Self {
        Self {
            bindings,
            active_sequences: ActiveBindingState::default(),
        }
    }

    pub(crate) fn handle_key(
        &mut self,
        modifiers: ModifierMask,
        code: u16,
        pressed: bool,
        repeated: bool,
        inhibited: bool,
    ) -> AstreaBindingMatch {
        let trigger = if pressed {
            BindingTrigger::Press
        } else {
            BindingTrigger::Release
        };
        let input = BindingInput::Key(code);
        let Some(binding) = self.match_binding(modifiers, trigger, input, repeated, inhibited)
        else {
            return AstreaBindingMatch::Pass;
        };
        let trigger = binding.trigger;
        let action = binding.action.clone();
        let phase = shortcut_phase(trigger, repeated);
        if matches!(
            action,
            BindingAction::EmitShortcut { ref namespace, ref name }
                if namespace == "astrea-shell" && name.starts_with("alt_tab_")
        ) && name_is_alt_tab_step(&action)
        {
            self.active_sequences.alt_tab_active = true;
        }
        AstreaBindingMatch::Consumed { action, phase }
    }

    pub(crate) fn handle_pointer_button(
        &mut self,
        modifiers: ModifierMask,
        button: u32,
        pressed: bool,
        inhibited: bool,
    ) -> AstreaBindingMatch {
        let trigger = if pressed {
            BindingTrigger::PointerPress
        } else {
            BindingTrigger::PointerRelease
        };
        let input = BindingInput::PointerButton(button);
        self.match_binding(modifiers, trigger, input, false, inhibited)
            .map(|binding| AstreaBindingMatch::Consumed {
                action: binding.action.clone(),
                phase: shortcut_phase(binding.trigger, false),
            })
            .unwrap_or(AstreaBindingMatch::Pass)
    }

    pub(crate) fn handle_modifier_release(&mut self, released: ModifierMask) -> AstreaBindingMatch {
        if released == ModifierMask::ALT && self.active_sequences.alt_tab_active {
            self.active_sequences.alt_tab_active = false;
            return AstreaBindingMatch::Consumed {
                action: BindingAction::EmitShortcut {
                    namespace: "astrea-shell".to_string(),
                    name: "alt_tab_commit".to_string(),
                },
                phase: AstreaShortcutPhase::Pressed,
            };
        }
        AstreaBindingMatch::Pass
    }

    pub(crate) fn cancel_shortcut_sequences_for_inhibition(&mut self) {
        // Inhibition transfers ownership of subsequent keyboard input to the
        // focused client.  Cancel stateful compositor sequences without
        // emitting their completion action; physical modifier truth remains
        // owned by NativeInputState.
        self.active_sequences.alt_tab_active = false;
    }

    fn match_binding(
        &self,
        modifiers: ModifierMask,
        trigger: BindingTrigger,
        input: BindingInput,
        repeated: bool,
        inhibited: bool,
    ) -> Option<&Binding> {
        self.bindings.iter().rev().find(|binding| {
            binding.trigger == trigger
                && binding.input == input
                && binding.modifiers.matches(modifiers)
                && (!repeated || binding.repeat == RepeatPolicy::Enabled)
                && (!inhibited || binding.inhibition == InhibitionPolicy::Bypass)
        })
    }
}

const fn shortcut_phase(trigger: BindingTrigger, repeated: bool) -> AstreaShortcutPhase {
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

fn name_is_alt_tab_step(action: &BindingAction) -> bool {
    matches!(
        action,
        BindingAction::EmitShortcut { namespace, name }
            if namespace == "astrea-shell"
                && matches!(name.as_str(), "alt_tab_next" | "alt_tab_previous")
    )
}

pub(crate) fn default_astrea_bindings() -> Vec<Binding> {
    let mut bindings = vec![
        Binding {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_Q),
            action: BindingAction::LaunchCommand(vec!["kitty".to_string()]),
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        Binding {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_S),
            action: BindingAction::ToggleDefaultSpecialWorkspace,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        Binding {
            modifiers: ModifierMask::SUPER | ModifierMask::SHIFT,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_S),
            action: BindingAction::MoveFocusedWindowToOrFromSpecialWorkspace,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        Binding {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_C),
            action: BindingAction::CloseActiveWindow,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        Binding {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_F),
            action: BindingAction::ToggleFullscreen,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        Binding {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_V),
            action: BindingAction::ToggleFocusedWindowLayout,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        Binding {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_SPACE),
            action: BindingAction::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "spotlight_toggle".to_string(),
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        Binding {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::PointerPress,
            input: BindingInput::PointerButton(u32::from(BTN_LEFT)),
            action: BindingAction::BeginMove,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        Binding {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::PointerPress,
            input: BindingInput::PointerButton(u32::from(BTN_RIGHT)),
            action: BindingAction::BeginResize,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        Binding {
            modifiers: ModifierMask::ALT,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_TAB),
            action: BindingAction::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "alt_tab_next".to_string(),
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        Binding {
            modifiers: ModifierMask::ALT | ModifierMask::SHIFT,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_TAB),
            action: BindingAction::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "alt_tab_previous".to_string(),
            },
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        },
        Binding {
            modifiers: ModifierMask::ALT,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_P),
            action: BindingAction::ExitCompositor,
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        },
        Binding {
            modifiers: ModifierMask::CTRL | ModifierMask::SHIFT | ModifierMask::ALT,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_1),
            action: BindingAction::LaunchSessionCommand(1),
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        },
        Binding {
            modifiers: ModifierMask::CTRL | ModifierMask::SHIFT | ModifierMask::ALT,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_2),
            action: BindingAction::LaunchSessionCommand(2),
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        },
        Binding {
            modifiers: ModifierMask::CTRL | ModifierMask::SHIFT | ModifierMask::ALT,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(KEY_3),
            action: BindingAction::LaunchSessionCommand(3),
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Bypass,
            reserved: true,
        },
    ];
    for workspace in 1..=10 {
        let workspace = WorkspaceId::new(workspace).expect("workspace binding id");
        let key = workspace_key(workspace);
        bindings.push(Binding {
            modifiers: ModifierMask::SUPER,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(key),
            action: BindingAction::SwitchWorkspace(workspace),
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        });
        bindings.push(Binding {
            modifiers: ModifierMask::SUPER | ModifierMask::SHIFT,
            trigger: BindingTrigger::Press,
            input: BindingInput::Key(key),
            action: BindingAction::MoveFocusedWindowToWorkspace(workspace),
            repeat: RepeatPolicy::Disabled,
            inhibition: InhibitionPolicy::Respect,
            reserved: false,
        });
    }
    bindings
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

    #[test]
    fn default_workspace_bindings_are_typed_non_repeating_and_not_reserved() {
        let mut manager = AstreaBindingManager::default();
        assert_eq!(
            manager.handle_key(ModifierMask::SUPER, KEY_0, true, false, false),
            AstreaBindingMatch::Consumed {
                action: BindingAction::SwitchWorkspace(WorkspaceId::new(10).unwrap()),
                phase: AstreaShortcutPhase::Pressed,
            }
        );
        assert_eq!(
            manager.handle_key(ModifierMask::SUPER, KEY_0, true, true, false),
            AstreaBindingMatch::Pass
        );
        assert_eq!(
            manager.handle_key(
                ModifierMask::SUPER | ModifierMask::SHIFT,
                KEY_4,
                true,
                false,
                false,
            ),
            AstreaBindingMatch::Consumed {
                action: BindingAction::MoveFocusedWindowToWorkspace(WorkspaceId::new(4).unwrap(),),
                phase: AstreaShortcutPhase::Pressed,
            }
        );
    }

    #[test]
    fn session_switch_bindings_keep_their_reserved_modifier_boundary() {
        let mut manager = AstreaBindingManager::default();
        assert_eq!(
            manager.handle_key(
                ModifierMask::CTRL | ModifierMask::SHIFT | ModifierMask::ALT,
                KEY_1,
                true,
                false,
                true,
            ),
            AstreaBindingMatch::Consumed {
                action: BindingAction::LaunchSessionCommand(1),
                phase: AstreaShortcutPhase::Pressed,
            }
        );
    }

    #[test]
    fn special_workspace_bindings_are_press_only_exact_and_inhibition_aware() {
        let mut manager = AstreaBindingManager::default();
        assert_eq!(
            manager.handle_key(ModifierMask::SUPER, KEY_S, true, false, false),
            AstreaBindingMatch::Consumed {
                action: BindingAction::ToggleDefaultSpecialWorkspace,
                phase: AstreaShortcutPhase::Pressed,
            }
        );
        assert_eq!(
            manager.handle_key(ModifierMask::SUPER, KEY_S, true, true, false),
            AstreaBindingMatch::Pass
        );
        assert_eq!(
            manager.handle_key(
                ModifierMask::SUPER | ModifierMask::SHIFT,
                KEY_S,
                true,
                false,
                false
            ),
            AstreaBindingMatch::Consumed {
                action: BindingAction::MoveFocusedWindowToOrFromSpecialWorkspace,
                phase: AstreaShortcutPhase::Pressed,
            }
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
