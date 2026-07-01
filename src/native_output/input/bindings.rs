use super::*;

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
    LaunchCommand(Vec<String>),
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
    Consumed(BindingAction),
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
        let action = binding.action.clone();
        if matches!(
            action,
            BindingAction::EmitShortcut { ref namespace, ref name }
                if namespace == "astrea-shell" && name.starts_with("alt_tab_")
        ) && name_is_alt_tab_step(&action)
        {
            self.active_sequences.alt_tab_active = true;
        }
        AstreaBindingMatch::Consumed(action)
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
            .map(|binding| AstreaBindingMatch::Consumed(binding.action.clone()))
            .unwrap_or(AstreaBindingMatch::Pass)
    }

    pub(crate) fn handle_modifier_release(&mut self, released: ModifierMask) -> AstreaBindingMatch {
        if released == ModifierMask::ALT && self.active_sequences.alt_tab_active {
            self.active_sequences.alt_tab_active = false;
            return AstreaBindingMatch::Consumed(BindingAction::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "alt_tab_commit".to_string(),
            });
        }
        AstreaBindingMatch::Pass
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

fn name_is_alt_tab_step(action: &BindingAction) -> bool {
    matches!(
        action,
        BindingAction::EmitShortcut { namespace, name }
            if namespace == "astrea-shell"
                && matches!(name.as_str(), "alt_tab_next" | "alt_tab_previous")
    )
}

pub(crate) fn default_astrea_bindings() -> Vec<Binding> {
    vec![
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
    ]
}
