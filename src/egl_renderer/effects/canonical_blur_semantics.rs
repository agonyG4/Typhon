use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const CANONICAL_BLUR_SEMANTICS_ENV: &str = "TYPHON_EFFECT_CANONICAL_BLUR_SEMANTICS";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum CanonicalBlurSemanticsPolicy {
    Legacy,
    #[default]
    Canonical,
}

impl CanonicalBlurSemanticsPolicy {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Canonical => "canonical",
        }
    }

    pub(crate) fn from_environment() -> Self {
        let (policy, unknown) = match std::env::var_os(CANONICAL_BLUR_SEMANTICS_ENV) {
            None => (Self::Canonical, false),
            Some(value) => match value.to_str() {
                Some(value) => parse_canonical_blur_semantics_policy(Some(value)),
                None => (Self::Legacy, true),
            },
        };
        if unknown {
            warn_unknown_policy_once();
        }
        policy
    }
}

pub(crate) fn parse_canonical_blur_semantics_policy(
    value: Option<&str>,
) -> (CanonicalBlurSemanticsPolicy, bool) {
    match value.map(str::trim) {
        None => (CanonicalBlurSemanticsPolicy::Canonical, false),
        Some(value) if value.eq_ignore_ascii_case("legacy") => {
            (CanonicalBlurSemanticsPolicy::Legacy, false)
        }
        Some(value) if value.eq_ignore_ascii_case("canonical") => {
            (CanonicalBlurSemanticsPolicy::Canonical, false)
        }
        Some(_) => (CanonicalBlurSemanticsPolicy::Legacy, true),
    }
}

pub(crate) const fn upsample_shader_variant(
    policy: CanonicalBlurSemanticsPolicy,
    is_kawase_upsample: bool,
    final_kawase_upsample: bool,
) -> u64 {
    if matches!(policy, CanonicalBlurSemanticsPolicy::Canonical)
        && is_kawase_upsample
        && final_kawase_upsample
    {
        1
    } else {
        0
    }
}

pub(crate) const fn fragment_shader_variant(
    policy: CanonicalBlurSemanticsPolicy,
    builtin_fragment: bool,
    contains_noise: bool,
) -> u64 {
    if matches!(policy, CanonicalBlurSemanticsPolicy::Canonical)
        && builtin_fragment
        && contains_noise
    {
        1
    } else {
        0
    }
}

fn warn_unknown_policy_once() {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "warning: unknown {CANONICAL_BLUR_SEMANTICS_ENV}; expected legacy or canonical, using legacy"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CANONICAL_BLUR_SEMANTICS_ENV, CanonicalBlurSemanticsPolicy, fragment_shader_variant,
        parse_canonical_blur_semantics_policy, upsample_shader_variant,
    };

    #[test]
    fn parser_defaults_to_canonical_and_accepts_trimmed_case_insensitive_values() {
        assert_eq!(
            parse_canonical_blur_semantics_policy(None),
            (CanonicalBlurSemanticsPolicy::Canonical, false)
        );
        assert_eq!(
            parse_canonical_blur_semantics_policy(Some("legacy")),
            (CanonicalBlurSemanticsPolicy::Legacy, false)
        );
        assert_eq!(
            parse_canonical_blur_semantics_policy(Some("LEGACY")),
            (CanonicalBlurSemanticsPolicy::Legacy, false)
        );
        assert_eq!(
            parse_canonical_blur_semantics_policy(Some("canonical")),
            (CanonicalBlurSemanticsPolicy::Canonical, false)
        );
        assert_eq!(
            parse_canonical_blur_semantics_policy(Some(" canonical ")),
            (CanonicalBlurSemanticsPolicy::Canonical, false)
        );
        assert_eq!(
            parse_canonical_blur_semantics_policy(Some(" CANONICAL ")),
            (CanonicalBlurSemanticsPolicy::Canonical, false)
        );
        assert_eq!(
            parse_canonical_blur_semantics_policy(Some("automatic")),
            (CanonicalBlurSemanticsPolicy::Legacy, true)
        );
    }

    #[test]
    fn environment_wrapper_defaults_closed_for_non_utf8_and_unknown_values() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let _environment_lock = crate::native_output::ASTREA_ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os(CANONICAL_BLUR_SEMANTICS_ENV);
        struct RestoreEnvironment(Option<OsString>);
        impl Drop for RestoreEnvironment {
            fn drop(&mut self) {
                // SAFETY: this test owns ASTREA_ENV_LOCK until this guard drops.
                unsafe {
                    if let Some(value) = self.0.take() {
                        std::env::set_var(CANONICAL_BLUR_SEMANTICS_ENV, value);
                    } else {
                        std::env::remove_var(CANONICAL_BLUR_SEMANTICS_ENV);
                    }
                }
            }
        }
        let _restore = RestoreEnvironment(previous);

        // SAFETY: ASTREA_ENV_LOCK serializes these environment mutations.
        unsafe { std::env::remove_var(CANONICAL_BLUR_SEMANTICS_ENV) };
        assert_eq!(
            CanonicalBlurSemanticsPolicy::from_environment(),
            CanonicalBlurSemanticsPolicy::Legacy
        );
        // SAFETY: ASTREA_ENV_LOCK serializes these environment mutations.
        unsafe { std::env::set_var(CANONICAL_BLUR_SEMANTICS_ENV, " canonical ") };
        assert_eq!(
            CanonicalBlurSemanticsPolicy::from_environment(),
            CanonicalBlurSemanticsPolicy::Canonical
        );
        // SAFETY: ASTREA_ENV_LOCK serializes these environment mutations.
        unsafe { std::env::set_var(CANONICAL_BLUR_SEMANTICS_ENV, "unknown") };
        assert_eq!(
            CanonicalBlurSemanticsPolicy::from_environment(),
            CanonicalBlurSemanticsPolicy::Legacy
        );
        // SAFETY: ASTREA_ENV_LOCK serializes these environment mutations.
        unsafe { std::env::set_var(CANONICAL_BLUR_SEMANTICS_ENV, OsString::from_vec(vec![0xff])) };
        assert_eq!(
            CanonicalBlurSemanticsPolicy::from_environment(),
            CanonicalBlurSemanticsPolicy::Legacy
        );
    }

    #[test]
    fn legacy_and_nonterminal_upsamples_keep_variant_zero() {
        assert_eq!(
            upsample_shader_variant(CanonicalBlurSemanticsPolicy::Legacy, true, true),
            0
        );
        assert_eq!(
            upsample_shader_variant(CanonicalBlurSemanticsPolicy::Canonical, false, true),
            0
        );
        assert_eq!(
            upsample_shader_variant(CanonicalBlurSemanticsPolicy::Canonical, true, false),
            0
        );
        assert_eq!(
            upsample_shader_variant(CanonicalBlurSemanticsPolicy::Canonical, true, true),
            1
        );
    }

    #[test]
    fn canonical_fragment_variant_requires_builtin_fragment_with_noise() {
        assert_eq!(
            fragment_shader_variant(CanonicalBlurSemanticsPolicy::Legacy, true, true),
            0
        );
        assert_eq!(
            fragment_shader_variant(CanonicalBlurSemanticsPolicy::Canonical, false, true),
            0
        );
        assert_eq!(
            fragment_shader_variant(CanonicalBlurSemanticsPolicy::Canonical, true, false),
            0
        );
        assert_eq!(
            fragment_shader_variant(CanonicalBlurSemanticsPolicy::Canonical, true, true),
            1
        );
    }
}
