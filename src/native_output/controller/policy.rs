use std::{env, io};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ControllerPolicy {
    #[default]
    Off,
    Observe,
}

impl ControllerPolicy {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Observe => "observe",
        }
    }

    pub(crate) fn from_env() -> io::Result<Self> {
        match env::var("OBLIVION_ONE_CONTROLLER") {
            Ok(value) => parse_controller_policy(Some(&value))
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error)),
            Err(env::VarError::NotPresent) => Ok(Self::Off),
            Err(error @ env::VarError::NotUnicode(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("OBLIVION_ONE_CONTROLLER must be UTF-8: {error}"),
            )),
        }
    }
}

pub(crate) fn parse_controller_policy(value: Option<&str>) -> Result<ControllerPolicy, String> {
    match value {
        None => Ok(ControllerPolicy::Off),
        Some("off") => Ok(ControllerPolicy::Off),
        Some("observe") => Ok(ControllerPolicy::Observe),
        Some(value) => Err(format!(
            "invalid OBLIVION_ONE_CONTROLLER value {value:?}; expected off or observe"
        )),
    }
}
