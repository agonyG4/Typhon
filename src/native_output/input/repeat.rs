use super::{BindingId, InhibitionPolicy, ModifierMask, RepeatPolicy};

const NANOS_PER_MILLISECOND: u64 = 1_000_000;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyboardRepeatConfig {
    rate: u32,
    delay_ms: u32,
}

impl KeyboardRepeatConfig {
    pub(crate) const fn new(rate: i32, delay_ms: i32) -> Self {
        Self {
            rate: if rate > 0 { rate as u32 } else { 0 },
            delay_ms: if delay_ms > 0 { delay_ms as u32 } else { 0 },
        }
    }

    const fn delay_ns(self) -> u64 {
        (self.delay_ms as u64).saturating_mul(NANOS_PER_MILLISECOND)
    }

    const fn interval_ns(self) -> Option<u64> {
        if self.rate == 0 {
            None
        } else {
            let interval_ns = NANOS_PER_SECOND / self.rate as u64;
            Some(if interval_ns == 0 { 1 } else { interval_ns })
        }
    }
}

impl Default for KeyboardRepeatConfig {
    fn default() -> Self {
        Self::new(25, 600)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyboardRepeatPhase {
    InitialDelay,
    Repeating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActiveKeyboardRepeat {
    pub(crate) binding: BindingId,
    pub(crate) code: u16,
    pub(crate) modifiers: ModifierMask,
    pub(crate) inhibition: InhibitionPolicy,
    pub(crate) repeat: RepeatPolicy,
    phase: KeyboardRepeatPhase,
    next_deadline_ns: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct KeyboardRepeatState {
    config: Option<KeyboardRepeatConfig>,
    active: Option<ActiveKeyboardRepeat>,
    generation: u64,
}

impl KeyboardRepeatState {
    pub(crate) const fn with_config(config: KeyboardRepeatConfig) -> Self {
        Self {
            config: Some(config),
            active: None,
            generation: 0,
        }
    }

    pub(crate) const fn deadline_ns(self) -> Option<u64> {
        match self.active {
            Some(active) => Some(active.next_deadline_ns),
            None => None,
        }
    }

    pub(crate) const fn active(self) -> Option<ActiveKeyboardRepeat> {
        self.active
    }

    pub(crate) const fn due(self, now_ns: u64) -> bool {
        matches!(self.active, Some(active) if active.next_deadline_ns <= now_ns)
    }

    pub(crate) const fn generation(self) -> u64 {
        self.generation
    }

    pub(crate) fn arm(
        &mut self,
        binding: BindingId,
        code: u16,
        modifiers: ModifierMask,
        inhibition: InhibitionPolicy,
        now_ns: u64,
    ) {
        let Some(config) = self.config else {
            self.cancel();
            return;
        };
        if config.rate == 0 {
            self.cancel();
            return;
        }
        self.generation = self.generation.saturating_add(1);
        self.active = Some(ActiveKeyboardRepeat {
            binding,
            code,
            modifiers,
            inhibition,
            repeat: RepeatPolicy::Enabled,
            phase: KeyboardRepeatPhase::InitialDelay,
            next_deadline_ns: now_ns.saturating_add(config.delay_ns()),
        });
    }

    pub(crate) fn configured(config: KeyboardRepeatConfig) -> Self {
        Self::with_config(config)
    }

    pub(crate) fn cancel(&mut self) -> bool {
        if self.active.take().is_some() {
            self.generation = self.generation.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub(crate) fn cancel_if_modifiers_changed(&mut self, modifiers: ModifierMask) -> bool {
        if self
            .active
            .is_some_and(|active| active.modifiers != modifiers)
        {
            self.cancel()
        } else {
            false
        }
    }

    pub(crate) fn cancel_if_respected_inhibition(&mut self, inhibited: bool) -> bool {
        if inhibited
            && self
                .active
                .is_some_and(|active| active.inhibition == InhibitionPolicy::Respect)
        {
            self.cancel()
        } else {
            false
        }
    }

    pub(crate) fn update_config(&mut self, config: KeyboardRepeatConfig, now_ns: u64) {
        self.config = Some(config);
        if config.rate == 0 {
            self.cancel();
            return;
        }
        let Some(mut active) = self.active else {
            return;
        };
        active.next_deadline_ns = match active.phase {
            KeyboardRepeatPhase::InitialDelay => now_ns.saturating_add(config.delay_ns()),
            KeyboardRepeatPhase::Repeating => {
                now_ns.saturating_add(config.interval_ns().expect("positive rate has an interval"))
            }
        };
        self.generation = self.generation.saturating_add(1);
        self.active = Some(active);
    }

    pub(crate) fn schedule_after_fire(&mut self, now_ns: u64) {
        let Some(config) = self.config else {
            self.cancel();
            return;
        };
        let Some(interval_ns) = config.interval_ns() else {
            self.cancel();
            return;
        };
        let Some(mut active) = self.active else {
            return;
        };
        active.phase = KeyboardRepeatPhase::Repeating;
        active.next_deadline_ns = now_ns.saturating_add(interval_ns);
        self.generation = self.generation.saturating_add(1);
        self.active = Some(active);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> KeyboardRepeatState {
        let mut state = KeyboardRepeatState::with_config(KeyboardRepeatConfig::new(25, 600));
        state.arm(
            BindingId::from_index(0).expect("test binding ID"),
            44,
            ModifierMask::EMPTY,
            InhibitionPolicy::Respect,
            1_000_000,
        );
        state
    }

    #[test]
    fn initial_deadline_uses_configured_delay_and_is_due_at_the_boundary() {
        let state = target();
        assert_eq!(state.deadline_ns(), Some(601_000_000));
        assert!(!state.due(600_999_999));
        assert!(state.due(601_000_000));
    }

    #[test]
    fn late_service_schedules_one_interval_from_actual_service_time() {
        let mut state = KeyboardRepeatState::with_config(KeyboardRepeatConfig::new(25, 100));
        state.arm(
            BindingId::from_index(0).expect("test binding ID"),
            44,
            ModifierMask::EMPTY,
            InhibitionPolicy::Respect,
            0,
        );
        assert!(state.due(100_000_000));
        state.schedule_after_fire(260_000_000);
        assert_eq!(state.deadline_ns(), Some(300_000_000));
        assert!(!state.due(299_999_999));
    }

    #[test]
    fn zero_rate_never_arms_and_zero_delay_is_due_without_inline_service() {
        let mut disabled = KeyboardRepeatState::with_config(KeyboardRepeatConfig::new(0, 0));
        disabled.arm(
            BindingId::from_index(0).expect("test binding ID"),
            44,
            ModifierMask::EMPTY,
            InhibitionPolicy::Respect,
            10,
        );
        assert_eq!(disabled.deadline_ns(), None);

        let mut immediate = KeyboardRepeatState::with_config(KeyboardRepeatConfig::new(25, 0));
        immediate.arm(
            BindingId::from_index(0).expect("test binding ID"),
            44,
            ModifierMask::EMPTY,
            InhibitionPolicy::Respect,
            10,
        );
        assert_eq!(immediate.deadline_ns(), Some(10));
        assert!(immediate.active().is_some());
    }

    #[test]
    fn config_changes_rebase_the_active_phase_and_rate_zero_cancels() {
        let mut state = target();
        state.update_config(KeyboardRepeatConfig::new(50, 300), 2_000_000);
        assert_eq!(state.deadline_ns(), Some(302_000_000));
        state.schedule_after_fire(302_000_000);
        state.update_config(KeyboardRepeatConfig::new(20, 100), 400_000_000);
        assert_eq!(state.deadline_ns(), Some(450_000_000));
        state.update_config(KeyboardRepeatConfig::new(0, 100), 500_000_000);
        assert_eq!(state.deadline_ns(), None);
    }
}
