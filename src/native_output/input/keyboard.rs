use super::*;

pub(crate) const LINUX_KEY_CODE_COUNT: usize = 0x300;
const KEY_WORD_COUNT: usize = LINUX_KEY_CODE_COUNT.div_ceil(u64::BITS as usize);
const INITIAL_KEYBOARD_SOURCE_CAPACITY: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct KeyboardDeviceId(u32);

impl KeyboardDeviceId {
    #[cfg(test)]
    pub(crate) const fn from_raw(value: u32) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    #[cfg(test)]
    pub(crate) const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug)]
pub(crate) struct KeyboardDeviceIdAllocator {
    next: u32,
    exhausted: bool,
}

impl Default for KeyboardDeviceIdAllocator {
    fn default() -> Self {
        Self {
            next: 1,
            exhausted: false,
        }
    }
}

impl KeyboardDeviceIdAllocator {
    pub(crate) fn allocate(&mut self) -> Option<KeyboardDeviceId> {
        if self.exhausted {
            return None;
        }
        let id = KeyboardDeviceId(self.next);
        if let Some(next) = self.next.checked_add(1) {
            self.next = next;
        } else {
            self.exhausted = true;
        }
        Some(id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyboardAggregateTransition {
    None,
    Pressed,
    Released,
}

#[derive(Debug, Clone)]
struct KeyboardSourceState {
    device: KeyboardDeviceId,
    pressed: [u64; KEY_WORD_COUNT],
}

impl KeyboardSourceState {
    fn is_pressed(&self, key_index: usize) -> bool {
        self.pressed[key_index / u64::BITS as usize] & (1_u64 << (key_index % u64::BITS as usize))
            != 0
    }

    fn set_pressed(&mut self, key_index: usize, pressed: bool) {
        let word = &mut self.pressed[key_index / u64::BITS as usize];
        let mask = 1_u64 << (key_index % u64::BITS as usize);
        if pressed {
            *word |= mask;
        } else {
            *word &= !mask;
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct KeyboardSourceLedger {
    sources: Vec<KeyboardSourceState>,
    aggregate_counts: [u32; LINUX_KEY_CODE_COUNT],
}

impl Default for KeyboardSourceLedger {
    fn default() -> Self {
        Self {
            sources: Vec::with_capacity(INITIAL_KEYBOARD_SOURCE_CAPACITY),
            aggregate_counts: [0; LINUX_KEY_CODE_COUNT],
        }
    }
}

impl KeyboardSourceLedger {
    pub(crate) fn press(
        &mut self,
        device: KeyboardDeviceId,
        code: u16,
    ) -> KeyboardAggregateTransition {
        let Some(key_index) = key_index(code) else {
            return KeyboardAggregateTransition::None;
        };
        let Some(source_index) = self.find_source(device) else {
            self.sources.push(KeyboardSourceState {
                device,
                pressed: [0; KEY_WORD_COUNT],
            });
            return self.press_from_source(self.sources.len() - 1, key_index);
        };
        self.press_from_source(source_index, key_index)
    }

    pub(crate) fn release(
        &mut self,
        device: KeyboardDeviceId,
        code: u16,
    ) -> KeyboardAggregateTransition {
        let Some(key_index) = key_index(code) else {
            return KeyboardAggregateTransition::None;
        };
        let Some(source_index) = self.find_source(device) else {
            return KeyboardAggregateTransition::None;
        };
        if !self.sources[source_index].is_pressed(key_index) {
            return KeyboardAggregateTransition::None;
        }

        self.sources[source_index].set_pressed(key_index, false);
        let count = &mut self.aggregate_counts[key_index];
        if *count == 0 {
            return KeyboardAggregateTransition::None;
        }
        *count -= 1;
        if *count == 0 {
            KeyboardAggregateTransition::Released
        } else {
            KeyboardAggregateTransition::None
        }
    }

    pub(crate) fn remove_source(&mut self, device: KeyboardDeviceId) -> Vec<u16> {
        let Some(source_index) = self.find_source(device) else {
            return Vec::new();
        };
        let source = self.sources.swap_remove(source_index);
        let mut released = Vec::new();
        for key_index in 0..LINUX_KEY_CODE_COUNT {
            if !source.is_pressed(key_index) {
                continue;
            }
            let count = &mut self.aggregate_counts[key_index];
            if *count == 0 {
                continue;
            }
            *count -= 1;
            if *count == 0 {
                released.push(key_index as u16);
            }
        }
        released.sort_unstable_by_key(|code| (is_modifier_key(*code), *code));
        released
    }

    pub(crate) fn is_logically_pressed(&self, code: u16) -> bool {
        key_index(code).is_some_and(|index| self.aggregate_counts[index] != 0)
    }

    pub(crate) fn logical_pressed_keys(&self) -> Vec<u16> {
        self.aggregate_counts
            .iter()
            .enumerate()
            .filter_map(|(code, count)| (*count != 0).then_some(code as u16))
            .collect()
    }

    pub(crate) fn clear(&mut self) {
        self.sources.clear();
        self.aggregate_counts.fill(0);
    }

    #[cfg(test)]
    fn source_count(&self) -> usize {
        self.sources.len()
    }

    fn find_source(&self, device: KeyboardDeviceId) -> Option<usize> {
        self.sources
            .iter()
            .position(|source| source.device == device)
    }

    fn press_from_source(
        &mut self,
        source_index: usize,
        key_index: usize,
    ) -> KeyboardAggregateTransition {
        if self.sources[source_index].is_pressed(key_index) {
            return KeyboardAggregateTransition::None;
        }
        let Some(next_count) = self.aggregate_counts[key_index].checked_add(1) else {
            return KeyboardAggregateTransition::None;
        };
        let was_unpressed = self.aggregate_counts[key_index] == 0;
        self.aggregate_counts[key_index] = next_count;
        self.sources[source_index].set_pressed(key_index, true);
        if was_unpressed {
            KeyboardAggregateTransition::Pressed
        } else {
            KeyboardAggregateTransition::None
        }
    }
}

fn key_index(code: u16) -> Option<usize> {
    let index = usize::from(code);
    (index < LINUX_KEY_CODE_COUNT).then_some(index)
}

fn is_modifier_key(code: u16) -> bool {
    matches!(
        code,
        KEY_LEFTCTRL
            | KEY_RIGHTCTRL
            | KEY_LEFTSHIFT
            | KEY_RIGHTSHIFT
            | KEY_LEFTALT
            | KEY_RIGHTALT
            | KEY_LEFTMETA
            | KEY_RIGHTMETA
            | 58 // KEY_CAPSLOCK
            | 69 // KEY_NUMLOCK
            | 70 // KEY_SCROLLLOCK
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allocate_ids(count: usize) -> Vec<KeyboardDeviceId> {
        let mut allocator = KeyboardDeviceIdAllocator::default();
        (0..count)
            .map(|_| allocator.allocate().expect("available test device id"))
            .collect()
    }

    #[test]
    fn keyboard_device_ids_are_nonzero_monotonic_and_never_reused() {
        let mut allocator = KeyboardDeviceIdAllocator::default();
        let first = allocator.allocate().unwrap();
        let second = allocator.allocate().unwrap();
        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 2);
        assert!(KeyboardDeviceId::from_raw(0).is_none());
        assert!(KeyboardDeviceId::from_raw(first.get()).is_some());

        allocator.next = u32::MAX;
        let last = allocator.allocate().unwrap();
        assert_eq!(last.get(), u32::MAX);
        assert!(allocator.allocate().is_none());
        assert!(allocator.allocate().is_none());
    }

    #[test]
    fn aggregate_transitions_require_first_press_and_last_release() {
        let [first, second, unknown] = allocate_ids(3).try_into().unwrap();
        let mut ledger = KeyboardSourceLedger::default();

        assert_eq!(
            ledger.press(first, KEY_Q),
            KeyboardAggregateTransition::Pressed
        );
        assert_eq!(
            ledger.press(first, KEY_Q),
            KeyboardAggregateTransition::None
        );
        assert_eq!(
            ledger.release(unknown, KEY_Q),
            KeyboardAggregateTransition::None
        );
        assert!(ledger.is_logically_pressed(KEY_Q));
        assert_eq!(ledger.source_count(), 1);
        assert_eq!(
            ledger.press(second, KEY_Q),
            KeyboardAggregateTransition::None
        );
        assert_eq!(
            ledger.release(first, KEY_Q),
            KeyboardAggregateTransition::None
        );
        assert_eq!(
            ledger.release(second, KEY_Q),
            KeyboardAggregateTransition::Released
        );
        assert_eq!(
            ledger.release(second, KEY_Q),
            KeyboardAggregateTransition::None
        );
        assert_eq!(
            ledger.release(first, KEY_Q),
            KeyboardAggregateTransition::None
        );
    }

    #[test]
    fn source_removal_releases_only_last_owners_and_removes_source_state() {
        let [first, second] = allocate_ids(2).try_into().unwrap();
        let mut ledger = KeyboardSourceLedger::default();
        ledger.press(first, KEY_Q);
        ledger.press(second, KEY_Q);

        assert!(ledger.remove_source(first).is_empty());
        assert!(ledger.is_logically_pressed(KEY_Q));
        assert_eq!(ledger.source_count(), 1);
        assert_eq!(ledger.remove_source(first), Vec::<u16>::new());
        assert_eq!(ledger.remove_source(second), vec![KEY_Q]);
        assert_eq!(ledger.source_count(), 0);
        assert!(!ledger.is_logically_pressed(KEY_Q));
        assert_eq!(ledger.remove_source(second), Vec::<u16>::new());

        ledger.press(first, KEY_Q);
        assert_eq!(ledger.remove_source(first), vec![KEY_Q]);
    }

    #[test]
    fn removed_source_keys_are_ordered_before_modifier_families() {
        let [device] = allocate_ids(1).try_into().unwrap();
        let mut ledger = KeyboardSourceLedger::default();
        ledger.press(device, KEY_LEFTALT);
        ledger.press(device, KEY_TAB);
        ledger.press(device, KEY_Q);

        assert_eq!(
            ledger.remove_source(device),
            vec![KEY_TAB, KEY_Q, KEY_LEFTALT]
        );
        assert!(ledger.logical_pressed_keys().is_empty());
    }

    #[test]
    fn invalid_key_codes_do_not_index_the_fixed_key_tables() {
        let [device] = allocate_ids(1).try_into().unwrap();
        let mut ledger = KeyboardSourceLedger::default();
        assert_eq!(
            ledger.press(device, u16::MAX),
            KeyboardAggregateTransition::None
        );
        assert_eq!(
            ledger.release(device, u16::MAX),
            KeyboardAggregateTransition::None
        );
        assert!(!ledger.is_logically_pressed(u16::MAX));
        assert!(ledger.remove_source(device).is_empty());
    }
}
