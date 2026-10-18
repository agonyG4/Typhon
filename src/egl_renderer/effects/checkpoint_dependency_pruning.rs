use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const CHECKPOINT_DEPENDENCY_PRUNING_ENV: &str =
    "TYPHON_EFFECT_CHECKPOINT_DEPENDENCY_PRUNING";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum CheckpointDependencyPruningPolicy {
    #[default]
    Off,
    On,
}

impl CheckpointDependencyPruningPolicy {
    pub(crate) fn from_environment() -> Self {
        let (policy, unknown) = match std::env::var_os(CHECKPOINT_DEPENDENCY_PRUNING_ENV) {
            None => (Self::Off, false),
            Some(value) => match value.to_str() {
                Some(value) => parse_checkpoint_dependency_pruning_policy(Some(value)),
                None => (Self::Off, true),
            },
        };
        if unknown {
            warn_unknown_policy_once();
        }
        policy
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
        }
    }
}

pub(crate) fn parse_checkpoint_dependency_pruning_policy(
    value: Option<&str>,
) -> (CheckpointDependencyPruningPolicy, bool) {
    match value.map(str::trim) {
        None => (CheckpointDependencyPruningPolicy::Off, false),
        Some(value) if value.eq_ignore_ascii_case("off") => {
            (CheckpointDependencyPruningPolicy::Off, false)
        }
        Some(value) if value.eq_ignore_ascii_case("on") => {
            (CheckpointDependencyPruningPolicy::On, false)
        }
        Some(_) => (CheckpointDependencyPruningPolicy::Off, true),
    }
}

fn warn_unknown_policy_once() {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "warning: unknown {CHECKPOINT_DEPENDENCY_PRUNING_ENV}; expected off or on, using off"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_checkpoint_dependency_pruning_policy, CheckpointDependencyPruningPolicy as Policy,
    };

    #[test]
    fn parser_defaults_off_and_accepts_only_off_and_on() {
        assert_eq!(
            parse_checkpoint_dependency_pruning_policy(None),
            (Policy::Off, false)
        );
        assert_eq!(
            parse_checkpoint_dependency_pruning_policy(Some("off")),
            (Policy::Off, false)
        );
        assert_eq!(
            parse_checkpoint_dependency_pruning_policy(Some(" OFF ")),
            (Policy::Off, false)
        );
        assert_eq!(
            parse_checkpoint_dependency_pruning_policy(Some("on")),
            (Policy::On, false)
        );
        assert_eq!(
            parse_checkpoint_dependency_pruning_policy(Some(" On ")),
            (Policy::On, false)
        );
        assert_eq!(
            parse_checkpoint_dependency_pruning_policy(Some("automatic")),
            (Policy::Off, true)
        );
    }
}
