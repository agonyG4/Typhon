#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeDirectScanoutPreference {
    Auto,
    Off,
}

impl NativeDirectScanoutPreference {
    pub(crate) fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_DIRECT_SCANOUT") {
            Ok(value) => {
                let preference = Self::parse(&value);
                if preference == Self::Auto && value != "auto" {
                    eprintln!(
                        "native scanout: unknown OBLIVION_ONE_DIRECT_SCANOUT={value:?}; using auto"
                    );
                }
                preference
            }
            Err(_) => Self::Auto,
        }
    }

    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "auto" => Self::Auto,
            "off" => Self::Off,
            _ => Self::Auto,
        }
    }

    pub(crate) const fn enabled(self) -> bool {
        matches!(self, Self::Auto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_scanout_policy_defaults_to_auto_and_has_no_force_mode() {
        assert_eq!(
            NativeDirectScanoutPreference::parse("auto"),
            NativeDirectScanoutPreference::Auto
        );
        assert_eq!(
            NativeDirectScanoutPreference::parse("off"),
            NativeDirectScanoutPreference::Off
        );
        assert_eq!(
            NativeDirectScanoutPreference::parse("force"),
            NativeDirectScanoutPreference::Auto
        );
        assert!(NativeDirectScanoutPreference::Auto.enabled());
        assert!(!NativeDirectScanoutPreference::Off.enabled());
    }
}
