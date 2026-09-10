use super::model::BlurPolicySnapshot;
use std::{fmt, fs, io, path::Path, path::PathBuf};

pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
pub const MAX_RULES: usize = 256;

#[derive(Debug)]
pub enum BlurPolicyConfigError {
    Io(io::Error),
    Json(serde_json::Error),
    UnsupportedVersion(u32),
    TooLarge,
    TooManyRules,
}

impl fmt::Display for BlurPolicyConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "could not read blur policy: {error}"),
            Self::Json(error) => write!(formatter, "invalid blur policy JSON: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported blur policy version {version}")
            }
            Self::TooLarge => write!(formatter, "blur policy exceeds 64 KiB"),
            Self::TooManyRules => write!(formatter, "blur policy has too many rules"),
        }
    }
}

impl std::error::Error for BlurPolicyConfigError {}

impl From<io::Error> for BlurPolicyConfigError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for BlurPolicyConfigError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("AstreaOS").join("typhon").join("blur.json")
}

pub fn load() -> Result<(PathBuf, BlurPolicySnapshot), BlurPolicyConfigError> {
    let path = config_path();
    let mut snapshot = match fs::read(&path) {
        Ok(bytes) => parse_bytes(&bytes)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => BlurPolicySnapshot::default(),
        Err(error) => return Err(error.into()),
    };
    snapshot.config_path = path.display().to_string();
    Ok((path, snapshot))
}

pub fn load_from_path(path: &Path) -> Result<BlurPolicySnapshot, BlurPolicyConfigError> {
    match fs::read(path) {
        Ok(bytes) => parse_bytes(&bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(BlurPolicySnapshot::default()),
        Err(error) => Err(error.into()),
    }
}

fn parse_bytes(bytes: &[u8]) -> Result<BlurPolicySnapshot, BlurPolicyConfigError> {
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(BlurPolicyConfigError::TooLarge);
    }
    let snapshot = serde_json::from_slice::<BlurPolicySnapshot>(bytes)?;
    if snapshot.version != 1 {
        return Err(BlurPolicyConfigError::UnsupportedVersion(snapshot.version));
    }
    if snapshot.window_rules.len() > MAX_RULES || snapshot.layer_rules.len() > MAX_RULES {
        return Err(BlurPolicyConfigError::TooManyRules);
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn missing_file_uses_the_approved_default() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("astrea-blur-missing-{unique}.json"));
        let snapshot = load_from_path(&path).expect("missing config is not an error");
        assert_eq!(snapshot, BlurPolicySnapshot::default());
    }

    #[test]
    fn approved_json_round_trips_without_runtime_state() {
        let path = std::env::temp_dir().join("astrea-blur-default-test.json");
        let json = br#"{
            "version": 1,
            "enabled": true,
            "applications": {
                "wayland": "auto",
                "xwayland": "rules_only",
                "auto_fullscreen": false
            },
            "layers": { "default": "client_only" },
            "window_rules": [],
            "layer_rules": []
        }"#;
        std::fs::write(&path, json).expect("write test config");
        let snapshot = load_from_path(&path).expect("valid config");
        let _ = std::fs::remove_file(path);
        assert_eq!(snapshot.version, 1);
        assert!(snapshot.enabled);
        assert_eq!(
            snapshot.applications.wayland,
            super::super::model::BlurApplicationMode::Auto
        );
    }
}
