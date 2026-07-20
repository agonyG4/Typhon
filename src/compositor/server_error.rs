use std::{fmt, io};

#[derive(Debug)]
pub enum CompositorError {
    DisplayInit(String),
    Bind(String),
    Io(io::Error),
}

impl fmt::Display for CompositorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisplayInit(error) => {
                write!(formatter, "failed to initialize Wayland display: {error}")
            }
            Self::Bind(error) => write!(formatter, "failed to bind Wayland socket: {error}"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CompositorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::DisplayInit(_) | Self::Bind(_) => None,
        }
    }
}

impl From<io::Error> for CompositorError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
