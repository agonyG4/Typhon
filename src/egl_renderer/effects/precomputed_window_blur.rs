use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const PRECOMPUTED_WINDOW_BLUR_ENV: &str = "TYPHON_EFFECT_PRECOMPUTED_WINDOW_BLUR";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PrecomputedWindowBlurPolicy {
    #[default]
    Off,
    On,
}

impl PrecomputedWindowBlurPolicy {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
        }
    }

    pub(crate) fn from_environment() -> Self {
        let (policy, unknown) = match std::env::var_os(PRECOMPUTED_WINDOW_BLUR_ENV) {
            None => (Self::Off, false),
            Some(value) => match value.to_str() {
                Some(value) => parse_precomputed_window_blur_policy(Some(value)),
                None => (Self::Off, true),
            },
        };
        if unknown {
            warn_unknown_policy_once();
        }
        policy
    }
}

pub(crate) fn parse_precomputed_window_blur_policy(
    value: Option<&str>,
) -> (PrecomputedWindowBlurPolicy, bool) {
    match value.map(str::trim) {
        None => (PrecomputedWindowBlurPolicy::Off, false),
        Some(value) if value.eq_ignore_ascii_case("off") => {
            (PrecomputedWindowBlurPolicy::Off, false)
        }
        Some(value) if value.eq_ignore_ascii_case("on") => (PrecomputedWindowBlurPolicy::On, false),
        Some(_) => (PrecomputedWindowBlurPolicy::Off, true),
    }
}

fn warn_unknown_policy_once() {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("warning: unknown {PRECOMPUTED_WINDOW_BLUR_ENV}; expected off or on, using off");
    }
}

#[cfg(test)]
mod tests {
    use super::super::precomputed_window_blur::{
        PrecomputedWindowBlurPolicy, parse_precomputed_window_blur_policy,
    };

    #[test]
    fn policy_parser_accepts_only_off_and_on() {
        assert_eq!(
            parse_precomputed_window_blur_policy(None),
            (PrecomputedWindowBlurPolicy::Off, false)
        );
        assert_eq!(
            parse_precomputed_window_blur_policy(Some("off")),
            (PrecomputedWindowBlurPolicy::Off, false)
        );
        assert_eq!(
            parse_precomputed_window_blur_policy(Some("OFF")),
            (PrecomputedWindowBlurPolicy::Off, false)
        );
        assert_eq!(
            parse_precomputed_window_blur_policy(Some(" on ")),
            (PrecomputedWindowBlurPolicy::On, false)
        );
        assert_eq!(
            parse_precomputed_window_blur_policy(Some("aggressive")),
            (PrecomputedWindowBlurPolicy::Off, true)
        );
    }

    #[test]
    fn policy_environment_wrapper_reads_the_process_setting() {
        use std::ffi::OsString;

        let _environment_lock = crate::native_output::ASTREA_ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os(super::PRECOMPUTED_WINDOW_BLUR_ENV);
        struct RestoreEnvironment(Option<OsString>);
        impl Drop for RestoreEnvironment {
            fn drop(&mut self) {
                // SAFETY: the test owns ASTREA_ENV_LOCK until this guard drops.
                unsafe {
                    if let Some(value) = self.0.take() {
                        std::env::set_var(super::PRECOMPUTED_WINDOW_BLUR_ENV, value);
                    } else {
                        std::env::remove_var(super::PRECOMPUTED_WINDOW_BLUR_ENV);
                    }
                }
            }
        }
        let _restore = RestoreEnvironment(previous);
        // SAFETY: ASTREA_ENV_LOCK serializes this environment mutation.
        unsafe { std::env::set_var(super::PRECOMPUTED_WINDOW_BLUR_ENV, " ON ") };

        assert_eq!(
            PrecomputedWindowBlurPolicy::from_environment(),
            PrecomputedWindowBlurPolicy::On
        );
    }
}
