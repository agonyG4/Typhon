use super::frame::{ControllerButton, ControllerFrame};

const NAVIGATION_ENTER: f32 = 0.55;
const NAVIGATION_EXIT: f32 = 0.40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum ControllerSemanticAction {
    NavigateUp,
    NavigateDown,
    NavigateLeft,
    NavigateRight,
    Activate,
    Cancel,
    Menu,
    System,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ControllerActionMask(u16);

impl ControllerActionMask {
    pub(crate) const fn empty() -> Self {
        Self(0)
    }

    #[allow(dead_code)] // Consumers inspect masks after C2; the runtime has no consumer yet.
    pub(crate) const fn contains(self, action: ControllerSemanticAction) -> bool {
        self.0 & action.bit() != 0
    }

    pub(crate) fn insert(&mut self, action: ControllerSemanticAction) {
        self.0 |= action.bit();
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) const fn count(self) -> u32 {
        self.0.count_ones()
    }

    pub(crate) const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

impl ControllerSemanticAction {
    const fn bit(self) -> u16 {
        1 << self as u8
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ControllerSemanticFrame {
    pub(crate) held: ControllerActionMask,
    pub(crate) pressed: ControllerActionMask,
    pub(crate) released: ControllerActionMask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ControllerSemanticMapping {
    activate: ControllerButton,
    cancel: ControllerButton,
    menu: ControllerButton,
    system: ControllerButton,
}

impl ControllerSemanticMapping {
    pub(crate) const fn new(
        activate: ControllerButton,
        cancel: ControllerButton,
        menu: ControllerButton,
        system: ControllerButton,
    ) -> Self {
        Self {
            activate,
            cancel,
            menu,
            system,
        }
    }
}

impl Default for ControllerSemanticMapping {
    fn default() -> Self {
        Self::new(
            ControllerButton::South,
            ControllerButton::East,
            ControllerButton::Start,
            ControllerButton::Guide,
        )
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct NavigationState {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ControllerSemanticMapper {
    mapping: ControllerSemanticMapping,
    held: ControllerActionMask,
    navigation: NavigationState,
}

impl Default for ControllerSemanticMapper {
    fn default() -> Self {
        Self::new(ControllerSemanticMapping::default())
    }
}

impl ControllerSemanticMapper {
    pub(crate) const fn new(mapping: ControllerSemanticMapping) -> Self {
        Self {
            mapping,
            held: ControllerActionMask::empty(),
            navigation: NavigationState {
                up: false,
                down: false,
                left: false,
                right: false,
            },
        }
    }

    pub(crate) fn seeded(mapping: ControllerSemanticMapping, frame: &ControllerFrame) -> Self {
        let mut mapper = Self::new(mapping);
        mapper.seed(frame);
        mapper
    }

    /// Seeds current physical state without synthesizing any semantic transition.
    pub(crate) fn seed(&mut self, frame: &ControllerFrame) {
        self.navigation = navigation_state(frame.left_stick, NavigationState::default());
        self.held = self.aggregate_held(frame);
    }

    /// Derives transitions from the full physical state after one logical frame.
    pub(crate) fn map_frame(&mut self, frame: &ControllerFrame) -> Option<ControllerSemanticFrame> {
        self.navigation = navigation_state(frame.left_stick, self.navigation);
        let current = self.aggregate_held(frame);
        let pressed = current.difference(self.held);
        let released = self.held.difference(current);
        self.held = current;
        if pressed.is_empty() && released.is_empty() {
            return None;
        }
        Some(ControllerSemanticFrame {
            held: current,
            pressed,
            released,
        })
    }

    pub(crate) const fn held(&self) -> ControllerActionMask {
        self.held
    }

    pub(crate) fn clear(&mut self) {
        self.held = ControllerActionMask::empty();
        self.navigation = NavigationState::default();
    }

    fn aggregate_held(&self, frame: &ControllerFrame) -> ControllerActionMask {
        let mut held = ControllerActionMask::empty();
        for (button, action) in [
            (self.mapping.activate, ControllerSemanticAction::Activate),
            (self.mapping.cancel, ControllerSemanticAction::Cancel),
            (self.mapping.menu, ControllerSemanticAction::Menu),
            (self.mapping.system, ControllerSemanticAction::System),
        ] {
            if frame.standard_button_pressed(button) {
                held.insert(action);
            }
        }

        if frame.standard_button_pressed(ControllerButton::DpadUp) || self.navigation.up {
            held.insert(ControllerSemanticAction::NavigateUp);
        }
        if frame.standard_button_pressed(ControllerButton::DpadDown) || self.navigation.down {
            held.insert(ControllerSemanticAction::NavigateDown);
        }
        if frame.standard_button_pressed(ControllerButton::DpadLeft) || self.navigation.left {
            held.insert(ControllerSemanticAction::NavigateLeft);
        }
        if frame.standard_button_pressed(ControllerButton::DpadRight) || self.navigation.right {
            held.insert(ControllerSemanticAction::NavigateRight);
        }
        held
    }
}

fn navigation_state(stick: [f32; 2], previous: NavigationState) -> NavigationState {
    NavigationState {
        left: update_direction(previous.left, -stick[0]),
        right: update_direction(previous.right, stick[0]),
        up: update_direction(previous.up, -stick[1]),
        down: update_direction(previous.down, stick[1]),
    }
}

fn update_direction(active: bool, component: f32) -> bool {
    if active {
        component > NAVIGATION_EXIT
    } else {
        component >= NAVIGATION_ENTER
    }
}
