use super::*;
use crate::system_action::{AstreaSystemAction, AstreaSystemActionCapabilities};
#[cfg(test)]
use std::cell::Cell;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BindingId(u32);

impl BindingId {
    pub(super) fn from_index(index: usize) -> Option<Self> {
        u32::try_from(index).ok().map(Self)
    }

    pub(super) const fn index(self) -> usize {
        self.0 as usize
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BindingActionId(u32);

impl BindingActionId {
    pub(super) fn from_index(index: usize) -> Option<Self> {
        u32::try_from(index).ok().map(Self)
    }

    pub(super) const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BindingActionCatalog {
    actions: Vec<BindingActionDefinition>,
}

impl BindingActionCatalog {
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            actions: Vec::new(),
        }
    }

    pub(crate) fn action(&self, id: BindingActionId) -> Option<&BindingActionDefinition> {
        self.actions.get(id.index())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BindingActionInvocation {
    pub(crate) action: BindingActionId,
    pub(crate) phase: AstreaShortcutPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingSequenceEffect {
    None,
    AltTabStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BindingLookupKey {
    trigger: BindingTrigger,
    input: BindingInput,
    modifiers: ModifierMask,
}

impl BindingLookupKey {
    const fn of(trigger: BindingTrigger, input: BindingInput, modifiers: ModifierMask) -> Self {
        Self {
            trigger,
            input,
            modifiers,
        }
    }
}

impl PartialOrd for BindingLookupKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BindingLookupKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.trigger, self.input, self.modifiers).cmp(&(
            other.trigger,
            other.input,
            other.modifiers,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindingAvailability {
    Always,
    SystemAction(AstreaSystemAction),
}

impl BindingAvailability {
    const fn from_action(action: &BindingActionDefinition) -> Self {
        match action {
            BindingActionDefinition::SystemAction(action) => Self::SystemAction(*action),
            _ => Self::Always,
        }
    }

    const fn is_available(self, capabilities: AstreaSystemActionCapabilities) -> bool {
        match self {
            Self::Always => true,
            Self::SystemAction(action) => capabilities.contains(action),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompiledBinding {
    pub(crate) id: BindingId,
    pub(crate) trigger: BindingTrigger,
    pub(crate) input: BindingInput,
    pub(crate) modifiers: ModifierMask,
    pub(crate) action: BindingActionId,
    availability: BindingAvailability,
    pub(crate) repeat: RepeatPolicy,
    pub(crate) inhibition: InhibitionPolicy,
    pub(crate) reserved: bool,
    pub(crate) definition_order: u32,
    pub(crate) sequence_effect: BindingSequenceEffect,
}

impl CompiledBinding {
    fn lookup_key(&self) -> BindingLookupKey {
        BindingLookupKey::of(self.trigger, self.input, self.modifiers)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BindingBucket {
    key: BindingLookupKey,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompiledBindingMatch {
    pub(crate) binding: BindingId,
    pub(crate) action: BindingActionId,
    pub(crate) phase: AstreaShortcutPhase,
    pub(crate) repeat: RepeatPolicy,
    pub(crate) inhibition: InhibitionPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AstreaBindingMatch {
    Consumed {
        binding: Option<BindingId>,
        action: BindingActionId,
        phase: AstreaShortcutPhase,
        repeat: RepeatPolicy,
        inhibition: InhibitionPolicy,
    },
    Pass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingTableCompileError {
    TooManyBindings,
    TooManyActions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompiledBindingTable {
    bindings: Vec<CompiledBinding>,
    buckets: Vec<BindingBucket>,
    candidates: Vec<BindingId>,
    action_catalog: BindingActionCatalog,
    alt_tab_commit_action: BindingActionId,
}

impl CompiledBindingTable {
    pub(crate) fn compile(specs: Vec<BindingSpec>) -> Result<Self, BindingTableCompileError> {
        let mut actions = Vec::with_capacity(
            specs
                .len()
                .checked_add(1)
                .ok_or(BindingTableCompileError::TooManyActions)?,
        );
        let mut bindings = Vec::with_capacity(specs.len());

        for (index, spec) in specs.into_iter().enumerate() {
            let id =
                BindingId::from_index(index).ok_or(BindingTableCompileError::TooManyBindings)?;
            let action = BindingActionId::from_index(actions.len())
                .ok_or(BindingTableCompileError::TooManyActions)?;
            let availability = BindingAvailability::from_action(&spec.action);
            let sequence_effect = classify_sequence_effect(&spec.action);
            let action_definition = resolve_action_definition(spec.action);
            let definition_order =
                u32::try_from(index).map_err(|_| BindingTableCompileError::TooManyBindings)?;
            actions.push(action_definition);
            bindings.push(CompiledBinding {
                id,
                trigger: spec.trigger,
                input: spec.input,
                modifiers: spec.modifiers,
                action,
                availability,
                repeat: spec.repeat,
                inhibition: spec.inhibition,
                reserved: spec.reserved,
                definition_order,
                sequence_effect,
            });
        }

        let alt_tab_commit_action = BindingActionId::from_index(actions.len())
            .ok_or(BindingTableCompileError::TooManyActions)?;
        actions.push(BindingActionDefinition::EmitShortcut {
            namespace: "astrea-shell".to_string(),
            name: "alt_tab_commit".to_string(),
        });

        let mut definition_indices: Vec<usize> = (0..bindings.len()).collect();
        definition_indices.sort_by(|left, right| {
            let left = &bindings[*left];
            let right = &bindings[*right];
            left.lookup_key()
                .cmp(&right.lookup_key())
                .then_with(|| right.definition_order.cmp(&left.definition_order))
        });

        let mut buckets: Vec<BindingBucket> = Vec::new();
        let mut candidates = Vec::with_capacity(bindings.len());
        for definition_index in definition_indices {
            let binding = &bindings[definition_index];
            let key = binding.lookup_key();
            if buckets.last().map(|bucket| bucket.key) != Some(key) {
                buckets.push(BindingBucket {
                    key,
                    start: candidates.len(),
                    end: candidates.len(),
                });
            }
            candidates.push(binding.id);
            if let Some(bucket) = buckets.last_mut() {
                bucket.end += 1;
            }
        }

        Ok(Self {
            bindings,
            buckets,
            candidates,
            action_catalog: BindingActionCatalog { actions },
            alt_tab_commit_action,
        })
    }

    pub(crate) fn binding(&self, id: BindingId) -> Option<&CompiledBinding> {
        self.bindings
            .get(id.index())
            .filter(|binding| binding.id == id)
    }

    pub(crate) fn action_catalog(&self) -> &BindingActionCatalog {
        &self.action_catalog
    }

    pub(crate) fn match_binding(
        &self,
        modifiers: ModifierMask,
        trigger: BindingTrigger,
        input: BindingInput,
        repeated: bool,
        inhibited: bool,
        system_capabilities: AstreaSystemActionCapabilities,
    ) -> Option<CompiledBindingMatch> {
        let key = BindingLookupKey::of(trigger, input, modifiers);
        let bucket_index = self
            .buckets
            .binary_search_by(|bucket| bucket.key.cmp(&key))
            .ok()?;
        let bucket = self.buckets.get(bucket_index)?;
        let candidates = self.candidates.get(bucket.start..bucket.end)?;

        candidates.iter().find_map(|id| {
            let binding = self.binding(*id)?;
            if (repeated && binding.repeat != RepeatPolicy::Enabled)
                || (inhibited && binding.inhibition != InhibitionPolicy::Bypass)
                || !binding.availability.is_available(system_capabilities)
            {
                return None;
            }
            Some(CompiledBindingMatch {
                binding: binding.id,
                action: binding.action,
                phase: shortcut_phase(binding.trigger, repeated),
                repeat: binding.repeat,
                inhibition: binding.inhibition,
            })
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AstreaBindingManager {
    table: CompiledBindingTable,
    system_capabilities: AstreaSystemActionCapabilities,
    active_sequences: ActiveBindingState,
    #[cfg(test)]
    modifier_release_call_counts: [usize; 4],
    #[cfg(test)]
    generic_lookup_count: Cell<usize>,
}

impl Default for AstreaBindingManager {
    fn default() -> Self {
        Self::with_default_bindings()
    }
}

impl AstreaBindingManager {
    pub(crate) fn with_default_bindings() -> Self {
        Self::from_specs(default_astrea_binding_specs())
            .expect("built-in binding table must compile")
    }

    #[cfg(test)]
    pub(crate) fn with_default_bindings_and_system_capabilities(
        capabilities: AstreaSystemActionCapabilities,
    ) -> Self {
        Self::from_specs_with_system_capabilities(default_astrea_binding_specs(), capabilities)
            .expect("built-in binding table must compile")
    }

    pub(crate) fn from_specs(specs: Vec<BindingSpec>) -> Result<Self, BindingTableCompileError> {
        Self::from_specs_with_system_capabilities(specs, AstreaSystemActionCapabilities::EMPTY)
    }

    fn from_specs_with_system_capabilities(
        specs: Vec<BindingSpec>,
        system_capabilities: AstreaSystemActionCapabilities,
    ) -> Result<Self, BindingTableCompileError> {
        Ok(Self {
            table: CompiledBindingTable::compile(specs)?,
            system_capabilities,
            active_sequences: ActiveBindingState::default(),
            #[cfg(test)]
            modifier_release_call_counts: [0; 4],
            #[cfg(test)]
            generic_lookup_count: Cell::new(0),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_specs(specs: Vec<BindingSpec>) -> Self {
        Self::from_specs(specs).expect("test binding table must compile")
    }

    #[cfg(test)]
    pub(crate) fn with_specs_and_system_capabilities(
        specs: Vec<BindingSpec>,
        capabilities: AstreaSystemActionCapabilities,
    ) -> Self {
        Self::from_specs_with_system_capabilities(specs, capabilities)
            .expect("test binding table must compile")
    }

    pub(crate) fn action_catalog(&self) -> &BindingActionCatalog {
        self.table.action_catalog()
    }

    #[cfg(test)]
    pub(crate) fn binding(&self, id: BindingId) -> Option<&CompiledBinding> {
        self.table.binding(id)
    }

    pub(crate) fn handle_keyboard_key(
        &mut self,
        physical_modifiers: ModifierMask,
        code: u16,
        symbolic: Option<KeyboardSymbolicSnapshot>,
        pressed: bool,
        repeated: bool,
        inhibited: bool,
    ) -> AstreaBindingMatch {
        let trigger = if pressed {
            BindingTrigger::Press
        } else {
            BindingTrigger::Release
        };
        let Some(matched) = self.match_keyboard_binding(
            physical_modifiers,
            trigger,
            code,
            symbolic,
            repeated,
            inhibited,
        ) else {
            return AstreaBindingMatch::Pass;
        };
        if self
            .table
            .binding(matched.binding)
            .is_some_and(|binding| binding.sequence_effect == BindingSequenceEffect::AltTabStep)
        {
            self.active_sequences.alt_tab_active = true;
        }
        AstreaBindingMatch::Consumed {
            binding: Some(matched.binding),
            action: matched.action,
            phase: matched.phase,
            repeat: matched.repeat,
            inhibition: matched.inhibition,
        }
    }

    #[cfg(test)]
    pub(crate) fn handle_key(
        &mut self,
        modifiers: ModifierMask,
        code: u16,
        pressed: bool,
        repeated: bool,
        inhibited: bool,
    ) -> AstreaBindingMatch {
        self.handle_keyboard_key(modifiers, code, None, pressed, repeated, inhibited)
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
        self.match_binding(
            modifiers,
            trigger,
            BindingInput::PointerButton(button),
            false,
            inhibited,
        )
        .map(|matched| AstreaBindingMatch::Consumed {
            binding: Some(matched.binding),
            action: matched.action,
            phase: matched.phase,
            repeat: matched.repeat,
            inhibition: matched.inhibition,
        })
        .unwrap_or(AstreaBindingMatch::Pass)
    }

    pub(crate) fn handle_modifier_release(&mut self, released: ModifierMask) -> AstreaBindingMatch {
        #[cfg(test)]
        if let Some(index) = modifier_release_index(released) {
            self.modifier_release_call_counts[index] += 1;
        }

        if released == ModifierMask::ALT && self.active_sequences.alt_tab_active {
            self.active_sequences.alt_tab_active = false;
            return AstreaBindingMatch::Consumed {
                binding: None,
                action: self.table.alt_tab_commit_action,
                phase: AstreaShortcutPhase::Pressed,
                repeat: RepeatPolicy::Disabled,
                inhibition: InhibitionPolicy::Respect,
            };
        }
        AstreaBindingMatch::Pass
    }

    pub(crate) fn cancel_shortcut_sequences_for_inhibition(&mut self) {
        self.active_sequences.alt_tab_active = false;
    }

    pub(crate) fn match_repeat_binding(
        &self,
        id: BindingId,
        code: u16,
        physical_modifiers: ModifierMask,
        symbolic: Option<KeyboardSymbolicSnapshot>,
        inhibited: bool,
    ) -> Option<AstreaBindingMatch> {
        let binding = self.table.binding(id)?;
        let trigger_matches = match binding.input {
            BindingInput::PhysicalKey(binding_code) => {
                binding_code == code && binding.modifiers.matches(physical_modifiers)
            }
            BindingInput::KeySym(expected) => symbolic.is_some_and(|snapshot| {
                [snapshot.raw, snapshot.translated]
                    .into_iter()
                    .flatten()
                    .any(|identity| {
                        identity.keysym == expected && binding.modifiers.matches(identity.modifiers)
                    })
            }),
            BindingInput::PointerButton(_) => false,
        };
        if binding.trigger != BindingTrigger::Press
            || !trigger_matches
            || binding.repeat != RepeatPolicy::Enabled
            || (inhibited && binding.inhibition != InhibitionPolicy::Bypass)
            || !binding.availability.is_available(self.system_capabilities)
        {
            return None;
        }
        Some(AstreaBindingMatch::Consumed {
            binding: Some(binding.id),
            action: binding.action,
            phase: AstreaShortcutPhase::Repeated,
            repeat: binding.repeat,
            inhibition: binding.inhibition,
        })
    }

    fn match_binding(
        &self,
        modifiers: ModifierMask,
        trigger: BindingTrigger,
        input: BindingInput,
        repeated: bool,
        inhibited: bool,
    ) -> Option<CompiledBindingMatch> {
        #[cfg(test)]
        self.generic_lookup_count
            .set(self.generic_lookup_count.get().saturating_add(1));
        self.table.match_binding(
            modifiers,
            trigger,
            input,
            repeated,
            inhibited,
            self.system_capabilities,
        )
    }

    fn match_keyboard_binding(
        &self,
        physical_modifiers: ModifierMask,
        trigger: BindingTrigger,
        code: u16,
        symbolic: Option<KeyboardSymbolicSnapshot>,
        repeated: bool,
        inhibited: bool,
    ) -> Option<CompiledBindingMatch> {
        let physical = self.match_binding(
            physical_modifiers,
            trigger,
            BindingInput::PhysicalKey(code),
            repeated,
            inhibited,
        );
        let raw = symbolic
            .and_then(|snapshot| snapshot.raw)
            .and_then(|identity| {
                self.match_binding(
                    identity.modifiers,
                    trigger,
                    BindingInput::KeySym(identity.keysym),
                    repeated,
                    inhibited,
                )
            });
        let translated = symbolic
            .and_then(|snapshot| snapshot.translated)
            .and_then(|identity| {
                self.match_binding(
                    identity.modifiers,
                    trigger,
                    BindingInput::KeySym(identity.keysym),
                    repeated,
                    inhibited,
                )
            });

        let mut winner = None;
        for candidate in [physical, raw, translated].into_iter().flatten() {
            let candidate_order = self
                .table
                .binding(candidate.binding)
                .map_or(0, |binding| binding.definition_order);
            let winner_order = winner
                .and_then(|matched: CompiledBindingMatch| self.table.binding(matched.binding))
                .map_or(0, |binding| binding.definition_order);
            if winner.is_none() || candidate_order > winner_order {
                winner = Some(candidate);
            }
        }
        winner
    }

    pub(crate) fn repeat_binding_requires_symbolic_translation(&self, id: BindingId) -> bool {
        self.table
            .binding(id)
            .is_some_and(|binding| matches!(binding.input, BindingInput::KeySym(_)))
    }

    #[cfg(test)]
    pub(crate) fn modifier_release_call_count(&self, family: ModifierMask) -> usize {
        self.modifier_release_call_counts
            [modifier_release_index(family).expect("test query must name one modifier family")]
    }

    #[cfg(test)]
    pub(crate) fn generic_lookup_count_for_tests(&self) -> usize {
        self.generic_lookup_count.get()
    }
}

fn resolve_action_definition(action: BindingActionDefinition) -> BindingActionDefinition {
    match action {
        BindingActionDefinition::LaunchSessionCommand { index, .. } => {
            BindingActionDefinition::LaunchSessionCommand {
                index,
                command: crate::native_output::external_session_switch_command(index),
            }
        }
        action => action,
    }
}

fn classify_sequence_effect(action: &BindingActionDefinition) -> BindingSequenceEffect {
    if matches!(
        action,
        BindingActionDefinition::EmitShortcut { namespace, name }
            if namespace == "astrea-shell"
                && matches!(name.as_str(), "alt_tab_next" | "alt_tab_previous")
    ) {
        BindingSequenceEffect::AltTabStep
    } else {
        BindingSequenceEffect::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(
        modifiers: ModifierMask,
        trigger: BindingTrigger,
        input: BindingInput,
        repeat: RepeatPolicy,
        inhibition: InhibitionPolicy,
    ) -> BindingSpec {
        BindingSpec {
            modifiers,
            trigger,
            input,
            action: BindingActionDefinition::ExitCompositor,
            repeat,
            inhibition,
            reserved: false,
        }
    }

    fn reference_match(
        bindings: &[BindingSpec],
        modifiers: ModifierMask,
        trigger: BindingTrigger,
        input: BindingInput,
        repeated: bool,
        inhibited: bool,
    ) -> Option<usize> {
        bindings
            .iter()
            .enumerate()
            .rev()
            .find(|(_, binding)| {
                binding.trigger == trigger
                    && binding.input == input
                    && binding.modifiers.matches(modifiers)
                    && (!repeated || binding.repeat == RepeatPolicy::Enabled)
                    && (!inhibited || binding.inhibition == InhibitionPolicy::Bypass)
            })
            .map(|(order, _)| order)
    }

    #[test]
    fn compiled_lookup_matches_the_reverse_definition_reference() {
        let specs = vec![
            spec(
                ModifierMask::EMPTY,
                BindingTrigger::Press,
                BindingInput::PhysicalKey(KEY_Z),
                RepeatPolicy::Enabled,
                InhibitionPolicy::Respect,
            ),
            spec(
                ModifierMask::SUPER,
                BindingTrigger::Press,
                BindingInput::PhysicalKey(KEY_Z),
                RepeatPolicy::Enabled,
                InhibitionPolicy::Bypass,
            ),
            spec(
                ModifierMask::SUPER,
                BindingTrigger::Press,
                BindingInput::PhysicalKey(KEY_Z),
                RepeatPolicy::Disabled,
                InhibitionPolicy::Respect,
            ),
            spec(
                ModifierMask::EMPTY,
                BindingTrigger::Release,
                BindingInput::PhysicalKey(KEY_Z),
                RepeatPolicy::Disabled,
                InhibitionPolicy::Respect,
            ),
            spec(
                ModifierMask::SUPER,
                BindingTrigger::PointerPress,
                BindingInput::PointerButton(u32::from(BTN_LEFT)),
                RepeatPolicy::Disabled,
                InhibitionPolicy::Respect,
            ),
        ];
        let table = CompiledBindingTable::compile(specs.clone()).unwrap();
        let keys = [
            (BindingTrigger::Press, BindingInput::PhysicalKey(KEY_Z)),
            (BindingTrigger::Release, BindingInput::PhysicalKey(KEY_Z)),
            (
                BindingTrigger::PointerPress,
                BindingInput::PointerButton(u32::from(BTN_LEFT)),
            ),
            (
                BindingTrigger::PointerRelease,
                BindingInput::PointerButton(u32::from(BTN_RIGHT)),
            ),
        ];

        for (trigger, input) in keys {
            for modifiers in [ModifierMask::EMPTY, ModifierMask::SUPER, ModifierMask::ALT] {
                for repeated in [false, true] {
                    for inhibited in [false, true] {
                        let expected =
                            reference_match(&specs, modifiers, trigger, input, repeated, inhibited);
                        let actual = table
                            .match_binding(
                                modifiers,
                                trigger,
                                input,
                                repeated,
                                inhibited,
                                AstreaSystemActionCapabilities::EMPTY,
                            )
                            .map(|matched| {
                                table
                                    .binding(matched.binding)
                                    .expect("compiled match has a valid binding ID")
                                    .definition_order as usize
                            });
                        assert_eq!(actual, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn later_eligible_candidate_wins_and_ineligible_candidates_fall_back() {
        let specs = vec![
            spec(
                ModifierMask::SUPER,
                BindingTrigger::Press,
                BindingInput::PhysicalKey(KEY_Z),
                RepeatPolicy::Enabled,
                InhibitionPolicy::Bypass,
            ),
            spec(
                ModifierMask::SUPER,
                BindingTrigger::Press,
                BindingInput::PhysicalKey(KEY_Z),
                RepeatPolicy::Disabled,
                InhibitionPolicy::Respect,
            ),
        ];
        let table = CompiledBindingTable::compile(specs).unwrap();

        let selected_order = |repeated, inhibited| {
            let matched = table
                .match_binding(
                    ModifierMask::SUPER,
                    BindingTrigger::Press,
                    BindingInput::PhysicalKey(KEY_Z),
                    repeated,
                    inhibited,
                    AstreaSystemActionCapabilities::EMPTY,
                )
                .expect("one candidate remains eligible");
            table.binding(matched.binding).unwrap().definition_order
        };

        assert_eq!(selected_order(false, false), 1);
        assert_eq!(selected_order(false, true), 0);
        assert_eq!(selected_order(true, false), 0);
    }

    #[test]
    fn match_result_is_copy_and_action_ids_resolve_catalog_owned_payloads() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<CompiledBindingMatch>();
        assert_copy::<AstreaBindingMatch>();
        assert_copy::<BindingActionInvocation>();

        let specs = vec![
            BindingSpec {
                modifiers: ModifierMask::EMPTY,
                trigger: BindingTrigger::Press,
                input: BindingInput::PhysicalKey(KEY_Z),
                action: BindingActionDefinition::EmitShortcut {
                    namespace: "astrea-shell".to_string(),
                    name: "catalog-test".to_string(),
                },
                repeat: RepeatPolicy::Enabled,
                inhibition: InhibitionPolicy::Bypass,
                reserved: true,
            },
            BindingSpec {
                modifiers: ModifierMask::SUPER,
                trigger: BindingTrigger::PointerPress,
                input: BindingInput::PointerButton(u32::from(BTN_LEFT)),
                action: BindingActionDefinition::LaunchCommand(vec!["kitty".to_string()]),
                repeat: RepeatPolicy::Disabled,
                inhibition: InhibitionPolicy::Respect,
                reserved: false,
            },
        ];
        let table = CompiledBindingTable::compile(specs).unwrap();
        let key_match = table
            .match_binding(
                ModifierMask::EMPTY,
                BindingTrigger::Press,
                BindingInput::PhysicalKey(KEY_Z),
                false,
                false,
                AstreaSystemActionCapabilities::EMPTY,
            )
            .unwrap();
        let pointer_match = table
            .match_binding(
                ModifierMask::SUPER,
                BindingTrigger::PointerPress,
                BindingInput::PointerButton(u32::from(BTN_LEFT)),
                false,
                false,
                AstreaSystemActionCapabilities::EMPTY,
            )
            .unwrap();

        assert_eq!(key_match.repeat, RepeatPolicy::Enabled);
        assert_eq!(key_match.inhibition, InhibitionPolicy::Bypass);
        assert_eq!(key_match.phase, AstreaShortcutPhase::Pressed);
        assert!(table.binding(key_match.binding).unwrap().reserved);
        assert_ne!(key_match.binding, pointer_match.binding);
        assert_ne!(key_match.action, pointer_match.action);
        assert_eq!(
            table.action_catalog().action(key_match.action),
            Some(&BindingActionDefinition::EmitShortcut {
                namespace: "astrea-shell".to_string(),
                name: "catalog-test".to_string(),
            })
        );
        assert_eq!(
            table.action_catalog().action(pointer_match.action),
            Some(&BindingActionDefinition::LaunchCommand(vec![
                "kitty".to_string()
            ]))
        );
        assert!(
            table
                .action_catalog()
                .action(BindingActionId(u32::MAX))
                .is_none()
        );
        assert!(table.binding(BindingId(u32::MAX)).is_none());
    }

    #[test]
    fn alt_tab_sequence_classification_is_compiled_once() {
        let table = CompiledBindingTable::compile(vec![BindingSpec {
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
        }])
        .unwrap();
        let matched = table
            .match_binding(
                ModifierMask::ALT,
                BindingTrigger::Press,
                BindingInput::PhysicalKey(KEY_TAB),
                false,
                false,
                AstreaSystemActionCapabilities::EMPTY,
            )
            .unwrap();

        assert_eq!(
            table.binding(matched.binding).unwrap().sequence_effect,
            BindingSequenceEffect::AltTabStep
        );
    }

    #[test]
    fn compiling_the_same_specs_produces_an_equivalent_table() {
        let specs = default_astrea_binding_specs();
        let first = CompiledBindingTable::compile(specs.clone()).unwrap();
        let second = CompiledBindingTable::compile(specs).unwrap();

        assert_eq!(first, second);
    }
}
