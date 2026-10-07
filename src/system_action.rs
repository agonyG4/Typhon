/// Typed desktop intents executed by the Astrea session service.
///
/// Typhon only transports these values; it never executes the operations.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum AstreaSystemAction {
    OutputVolumeUp,
    OutputVolumeDown,
    ToggleOutputMute,
    ToggleMicrophoneMute,
    MediaPlayPause,
    #[allow(dead_code)]
    MediaPlay,
    MediaPause,
    MediaStop,
    MediaNext,
    MediaPrevious,
    MediaRewind,
    MediaFastForward,
    DisplayBrightnessUp,
    DisplayBrightnessDown,
    KeyboardBrightnessUp,
    KeyboardBrightnessDown,
    ToggleKeyboardBacklight,
    ToggleTouchpad,
}

pub(crate) const ASTREA_SYSTEM_ACTION_COUNT: usize = 18;
pub(crate) const SYSTEM_ACTION_APPLICATION_CAPACITY: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AstreaSystemActionBatch {
    actions: [Option<AstreaSystemAction>; SYSTEM_ACTION_APPLICATION_CAPACITY],
    len: u8,
    overflowed: bool,
}

impl Default for AstreaSystemActionBatch {
    fn default() -> Self {
        Self {
            actions: [None; SYSTEM_ACTION_APPLICATION_CAPACITY],
            len: 0,
            overflowed: false,
        }
    }
}

impl AstreaSystemActionBatch {
    pub(crate) fn push(&mut self, action: AstreaSystemAction) -> bool {
        let index = usize::from(self.len);
        if index == self.actions.len() {
            self.overflowed = true;
            return false;
        }
        self.actions[index] = Some(action);
        self.len += 1;
        true
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = AstreaSystemAction> + '_ {
        self.actions[..usize::from(self.len)]
            .iter()
            .flatten()
            .copied()
    }

    pub(crate) const fn overflowed(self) -> bool {
        self.overflowed
    }
}

impl AstreaSystemAction {
    pub(crate) const ALL: [Self; ASTREA_SYSTEM_ACTION_COUNT] = [
        Self::OutputVolumeUp,
        Self::OutputVolumeDown,
        Self::ToggleOutputMute,
        Self::ToggleMicrophoneMute,
        Self::MediaPlayPause,
        Self::MediaPlay,
        Self::MediaPause,
        Self::MediaStop,
        Self::MediaNext,
        Self::MediaPrevious,
        Self::MediaRewind,
        Self::MediaFastForward,
        Self::DisplayBrightnessUp,
        Self::DisplayBrightnessDown,
        Self::KeyboardBrightnessUp,
        Self::KeyboardBrightnessDown,
        Self::ToggleKeyboardBacklight,
        Self::ToggleTouchpad,
    ];

    /// Stable protocol-v1 code. These values are intentionally independent of
    /// the Rust enum layout and declaration order.
    pub(crate) const fn wire_code(self) -> u16 {
        match self {
            Self::OutputVolumeUp => 0x0101,
            Self::OutputVolumeDown => 0x0102,
            Self::ToggleOutputMute => 0x0103,
            Self::ToggleMicrophoneMute => 0x0104,
            Self::MediaPlayPause => 0x0201,
            Self::MediaPlay => 0x0202,
            Self::MediaPause => 0x0203,
            Self::MediaStop => 0x0204,
            Self::MediaNext => 0x0205,
            Self::MediaPrevious => 0x0206,
            Self::MediaRewind => 0x0207,
            Self::MediaFastForward => 0x0208,
            Self::DisplayBrightnessUp => 0x0301,
            Self::DisplayBrightnessDown => 0x0302,
            Self::KeyboardBrightnessUp => 0x0401,
            Self::KeyboardBrightnessDown => 0x0402,
            Self::ToggleKeyboardBacklight => 0x0403,
            Self::ToggleTouchpad => 0x0501,
        }
    }

    pub(crate) const fn from_wire_code(code: u16) -> Option<Self> {
        match code {
            0x0101 => Some(Self::OutputVolumeUp),
            0x0102 => Some(Self::OutputVolumeDown),
            0x0103 => Some(Self::ToggleOutputMute),
            0x0104 => Some(Self::ToggleMicrophoneMute),
            0x0201 => Some(Self::MediaPlayPause),
            0x0202 => Some(Self::MediaPlay),
            0x0203 => Some(Self::MediaPause),
            0x0204 => Some(Self::MediaStop),
            0x0205 => Some(Self::MediaNext),
            0x0206 => Some(Self::MediaPrevious),
            0x0207 => Some(Self::MediaRewind),
            0x0208 => Some(Self::MediaFastForward),
            0x0301 => Some(Self::DisplayBrightnessUp),
            0x0302 => Some(Self::DisplayBrightnessDown),
            0x0401 => Some(Self::KeyboardBrightnessUp),
            0x0402 => Some(Self::KeyboardBrightnessDown),
            0x0403 => Some(Self::ToggleKeyboardBacklight),
            0x0501 => Some(Self::ToggleTouchpad),
            _ => None,
        }
    }

    const fn capability_bit(self) -> u64 {
        match self {
            Self::OutputVolumeUp => 1 << 0,
            Self::OutputVolumeDown => 1 << 1,
            Self::ToggleOutputMute => 1 << 2,
            Self::ToggleMicrophoneMute => 1 << 3,
            Self::MediaPlayPause => 1 << 4,
            Self::MediaPlay => 1 << 5,
            Self::MediaPause => 1 << 6,
            Self::MediaStop => 1 << 7,
            Self::MediaNext => 1 << 8,
            Self::MediaPrevious => 1 << 9,
            Self::MediaRewind => 1 << 10,
            Self::MediaFastForward => 1 << 11,
            Self::DisplayBrightnessUp => 1 << 12,
            Self::DisplayBrightnessDown => 1 << 13,
            Self::KeyboardBrightnessUp => 1 << 14,
            Self::KeyboardBrightnessDown => 1 << 15,
            Self::ToggleKeyboardBacklight => 1 << 16,
            Self::ToggleTouchpad => 1 << 17,
        }
    }

    pub(crate) const fn is_coalescible_step(self) -> bool {
        matches!(
            self,
            Self::OutputVolumeUp
                | Self::OutputVolumeDown
                | Self::DisplayBrightnessUp
                | Self::DisplayBrightnessDown
                | Self::KeyboardBrightnessUp
                | Self::KeyboardBrightnessDown
        )
    }
}

const _: () = assert!(ASTREA_SYSTEM_ACTION_COUNT <= u64::BITS as usize);
const _: () = assert!(AstreaSystemAction::ALL.len() == ASTREA_SYSTEM_ACTION_COUNT);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AstreaSystemActionCapabilities(u64);

impl AstreaSystemActionCapabilities {
    pub(crate) const EMPTY: Self = Self(0);

    pub(crate) const fn for_action(action: AstreaSystemAction) -> Self {
        Self(action.capability_bit())
    }

    pub(crate) const fn contains(self, action: AstreaSystemAction) -> bool {
        self.0 & action.capability_bit() != 0
    }

    pub(crate) const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[cfg(test)]
    pub(crate) fn insert(&mut self, action: AstreaSystemAction) {
        self.0 |= action.capability_bit();
    }

    pub(crate) const fn wire_bits(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_wire_bits_truncate(bits: u64) -> Self {
        let mut known = Self::EMPTY;
        let mut index = 0;
        while index < ASTREA_SYSTEM_ACTION_COUNT {
            known = known.union(Self::for_action(Self::action_at(index)));
            index += 1;
        }
        Self(bits & known.0)
    }

    pub(crate) const fn count(self) -> u32 {
        self.0.count_ones()
    }

    const fn action_at(index: usize) -> AstreaSystemAction {
        AstreaSystemAction::ALL[index]
    }

    #[cfg(test)]
    const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

#[cfg(test)]
mod tests {
    use super::{AstreaSystemAction, AstreaSystemActionCapabilities};

    #[test]
    fn capabilities_are_empty_compact_unionable_and_non_aliasing() {
        assert!(
            AstreaSystemActionCapabilities::EMPTY
                .union(AstreaSystemActionCapabilities::EMPTY)
                .is_empty()
        );
        assert_eq!(AstreaSystemAction::ALL.len(), 18);
        assert_eq!(AstreaSystemActionCapabilities::EMPTY.count(), 0);

        let mut occupied = 0_u64;
        let mut union = AstreaSystemActionCapabilities::EMPTY;
        for action in AstreaSystemAction::ALL {
            let single = AstreaSystemActionCapabilities::for_action(action);
            assert_eq!(single.0.count_ones(), 1);
            assert_eq!(occupied & single.0, 0, "capability bit aliases {action:?}");
            assert!(!AstreaSystemActionCapabilities::EMPTY.contains(action));
            occupied |= single.0;
            union = union.union(single);
            assert!(union.contains(action));
        }
        assert_eq!(
            occupied.count_ones() as usize,
            AstreaSystemAction::ALL.len()
        );
        assert_eq!(AstreaSystemActionCapabilities::EMPTY.wire_bits(), 0);
        assert_eq!(
            AstreaSystemActionCapabilities::from_wire_bits_truncate(u64::MAX).wire_bits(),
            occupied
        );

        let mut inserted = AstreaSystemActionCapabilities::EMPTY;
        inserted.insert(AstreaSystemAction::ToggleTouchpad);
        assert!(inserted.contains(AstreaSystemAction::ToggleTouchpad));
        assert!(!inserted.contains(AstreaSystemAction::ToggleKeyboardBacklight));
    }

    #[test]
    fn every_action_has_a_stable_unique_wire_code() {
        let mut codes = [0_u16; AstreaSystemAction::ALL.len()];
        for (index, action) in AstreaSystemAction::ALL.into_iter().enumerate() {
            let code = action.wire_code();
            assert_ne!(code, 0);
            assert_eq!(AstreaSystemAction::from_wire_code(code), Some(action));
            assert!(!codes[..index].contains(&code));
            codes[index] = code;
        }
        assert_eq!(AstreaSystemAction::from_wire_code(u16::MAX), None);
    }
}
