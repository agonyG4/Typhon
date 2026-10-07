/// Typed intent for hardware and media actions owned by the Astrea session
/// service. Typhon only records this intent; it does not execute it.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum AstreaSystemAction {
    OutputVolumeUp,
    OutputVolumeDown,
    ToggleOutputMute,
    ToggleMicrophoneMute,
    MediaPlayPause,
    #[allow(dead_code)] // Kept distinct for executors that expose Play separately.
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
    #[doc(hidden)]
    #[doc(hidden)]
    Count,
}

impl AstreaSystemAction {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 18] = [
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

    pub(crate) const CAPABILITY_COUNT: usize = Self::Count as usize;

    const fn capability_bit(self) -> u32 {
        let index = self as u32;
        if index >= Self::Count as u32 || index >= u32::BITS {
            0
        } else {
            1_u32 << index
        }
    }
}

// Keep `Count` after every action. This fails at compile time if the action
// vocabulary outgrows the u32 capability mask.
const _: () = assert!(AstreaSystemAction::CAPABILITY_COUNT <= u32::BITS as usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AstreaSystemActionCapabilities(u32);

impl AstreaSystemActionCapabilities {
    pub(crate) const EMPTY: Self = Self(0);

    #[allow(dead_code)] // Used to construct advertised capabilities in K4B.
    pub(crate) const fn for_action(action: AstreaSystemAction) -> Self {
        Self(action.capability_bit())
    }

    pub(crate) const fn contains(self, action: AstreaSystemAction) -> bool {
        let bit = action.capability_bit();
        bit != 0 && self.0 & bit != 0
    }

    #[allow(dead_code)] // Used to combine advertised capabilities in K4B.
    pub(crate) const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[cfg(test)]
    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[allow(dead_code)] // Used to build an advertised capability mask in K4B.
    pub(crate) fn insert(&mut self, action: AstreaSystemAction) {
        self.0 |= action.capability_bit();
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
        assert_eq!(
            AstreaSystemAction::ALL.len(),
            AstreaSystemAction::CAPABILITY_COUNT
        );
        assert!(AstreaSystemAction::CAPABILITY_COUNT <= u32::BITS as usize);

        let mut occupied = 0_u32;
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

        let mut inserted = AstreaSystemActionCapabilities::EMPTY;
        inserted.insert(AstreaSystemAction::ToggleTouchpad);
        assert!(inserted.contains(AstreaSystemAction::ToggleTouchpad));
        assert!(!inserted.contains(AstreaSystemAction::ToggleKeyboardBacklight));
    }
}
