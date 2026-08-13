use super::OwnCompositorServer;
use crate::compositor::{KeyboardShortcutInhibitionMetrics, KeyboardShortcutInhibitionSnapshot};

impl OwnCompositorServer {
    pub fn keyboard_shortcut_inhibition_snapshot(&self) -> KeyboardShortcutInhibitionSnapshot {
        self.state.keyboard_shortcut_inhibition_snapshot()
    }

    pub fn keyboard_shortcut_inhibition_metrics(&self) -> KeyboardShortcutInhibitionMetrics {
        self.state.keyboard_shortcut_inhibition_metrics()
    }
}
