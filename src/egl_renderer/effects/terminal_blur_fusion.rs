use std::sync::atomic::{AtomicBool, Ordering};

use super::trace::EffectDebugKawaseMode;

pub(crate) const TERMINAL_BLUR_FUSION_ENV: &str = "TYPHON_EFFECT_TERMINAL_BLUR_FUSION";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TerminalBlurFusionPolicy {
    #[default]
    Off,
    On,
}

impl TerminalBlurFusionPolicy {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
        }
    }

    pub(crate) fn from_environment() -> Self {
        let (policy, unknown) = match std::env::var_os(TERMINAL_BLUR_FUSION_ENV) {
            None => (Self::Off, false),
            Some(value) => match value.to_str() {
                Some(value) => parse_terminal_blur_fusion_policy(Some(value)),
                None => (Self::Off, true),
            },
        };
        if unknown {
            warn_unknown_policy_once();
        }
        policy
    }
}

pub(crate) fn parse_terminal_blur_fusion_policy(
    value: Option<&str>,
) -> (TerminalBlurFusionPolicy, bool) {
    match value.map(str::trim) {
        None => (TerminalBlurFusionPolicy::Off, false),
        Some(value) if value.eq_ignore_ascii_case("off") => (TerminalBlurFusionPolicy::Off, false),
        Some(value) if value.eq_ignore_ascii_case("on") => (TerminalBlurFusionPolicy::On, false),
        Some(_) => (TerminalBlurFusionPolicy::Off, true),
    }
}

pub(crate) const fn terminal_fusion_enabled(
    policy: TerminalBlurFusionPolicy,
    kawase_debug_mode: EffectDebugKawaseMode,
) -> bool {
    matches!(policy, TerminalBlurFusionPolicy::On)
        && !matches!(kawase_debug_mode, EffectDebugKawaseMode::Full)
}

fn warn_unknown_policy_once() {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("warning: unknown {TERMINAL_BLUR_FUSION_ENV}; expected off or on, using off");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TERMINAL_BLUR_FUSION_ENV, TerminalBlurFusionPolicy, parse_terminal_blur_fusion_policy,
        terminal_fusion_enabled,
    };

    #[test]
    fn policy_parser_defaults_closed_and_accepts_only_off_and_on() {
        assert_eq!(
            parse_terminal_blur_fusion_policy(None),
            (TerminalBlurFusionPolicy::Off, false)
        );
        assert_eq!(
            parse_terminal_blur_fusion_policy(Some("off")),
            (TerminalBlurFusionPolicy::Off, false)
        );
        assert_eq!(
            parse_terminal_blur_fusion_policy(Some("OFF")),
            (TerminalBlurFusionPolicy::Off, false)
        );
        assert_eq!(
            parse_terminal_blur_fusion_policy(Some("on")),
            (TerminalBlurFusionPolicy::On, false)
        );
        assert_eq!(
            parse_terminal_blur_fusion_policy(Some(" on ")),
            (TerminalBlurFusionPolicy::On, false)
        );
        assert_eq!(
            parse_terminal_blur_fusion_policy(Some("automatic")),
            (TerminalBlurFusionPolicy::Off, true)
        );
    }

    #[test]
    fn full_kawase_diagnostic_mode_disables_terminal_fusion() {
        use crate::egl_renderer::effects::EffectDebugKawaseMode;

        assert!(terminal_fusion_enabled(
            TerminalBlurFusionPolicy::On,
            EffectDebugKawaseMode::Partial,
        ));
        assert!(!terminal_fusion_enabled(
            TerminalBlurFusionPolicy::On,
            EffectDebugKawaseMode::Full,
        ));
        assert!(!terminal_fusion_enabled(
            TerminalBlurFusionPolicy::Off,
            EffectDebugKawaseMode::Partial,
        ));
    }

    #[test]
    fn environment_wrapper_reads_the_process_setting() {
        use std::ffi::OsString;

        let _environment_lock = crate::native_output::ASTREA_ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os(TERMINAL_BLUR_FUSION_ENV);
        struct RestoreEnvironment(Option<OsString>);
        impl Drop for RestoreEnvironment {
            fn drop(&mut self) {
                unsafe {
                    // SAFETY: this test holds ASTREA_ENV_LOCK until the restoration guard drops.
                    if let Some(value) = self.0.take() {
                        std::env::set_var(TERMINAL_BLUR_FUSION_ENV, value);
                    } else {
                        std::env::remove_var(TERMINAL_BLUR_FUSION_ENV);
                    }
                }
            }
        }
        let _restore = RestoreEnvironment(previous);

        // SAFETY: this test holds ASTREA_ENV_LOCK for every environment mutation.
        unsafe { std::env::remove_var(TERMINAL_BLUR_FUSION_ENV) };
        assert_eq!(
            TerminalBlurFusionPolicy::from_environment(),
            TerminalBlurFusionPolicy::Off
        );
        // SAFETY: this test holds ASTREA_ENV_LOCK for every environment mutation.
        unsafe {
            std::env::set_var(TERMINAL_BLUR_FUSION_ENV, " on ");
        }
        assert_eq!(
            TerminalBlurFusionPolicy::from_environment(),
            TerminalBlurFusionPolicy::On
        );
    }
}
