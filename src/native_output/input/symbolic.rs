use super::{BindingKeySym, ModifierMask};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyboardSymbolicIdentity {
    pub(crate) keysym: BindingKeySym,
    pub(crate) modifiers: ModifierMask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct KeyboardSymbolicSnapshot {
    pub(crate) raw: Option<KeyboardSymbolicIdentity>,
    pub(crate) translated: Option<KeyboardSymbolicIdentity>,
}

#[derive(Debug, Clone)]
pub(crate) struct KeyboardSymbolicPressLedger {
    keys: [Option<KeyboardSymbolicSnapshot>; super::LINUX_KEY_CODE_COUNT],
}

impl Default for KeyboardSymbolicPressLedger {
    fn default() -> Self {
        Self {
            keys: [None; super::LINUX_KEY_CODE_COUNT],
        }
    }
}

impl KeyboardSymbolicPressLedger {
    pub(crate) fn capture(&mut self, code: u16, snapshot: KeyboardSymbolicSnapshot) {
        if let Some(slot) = self.keys.get_mut(usize::from(code)) {
            *slot = Some(snapshot);
        }
    }

    pub(crate) fn take(&mut self, code: u16) -> Option<KeyboardSymbolicSnapshot> {
        self.keys.get_mut(usize::from(code)).and_then(Option::take)
    }

    pub(crate) fn clear(&mut self) {
        self.keys.fill(None);
    }
}
