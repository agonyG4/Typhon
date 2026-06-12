use std::{
    env,
    path::{Path, PathBuf},
};

use crate::STATE_DIR_NAME;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStatus {
    pub name: String,
    pub path: Option<PathBuf>,
}

impl ToolStatus {
    pub fn is_available(&self) -> bool {
        self.path.is_some()
    }
}

pub fn default_state_dir() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    default_state_dir_from_home(home)
}

pub fn default_state_dir_from_home(home: impl AsRef<Path>) -> PathBuf {
    home.as_ref()
        .join(".local")
        .join("state")
        .join(STATE_DIR_NAME)
}

pub fn discover_tools(names: &[&str]) -> Vec<ToolStatus> {
    names
        .iter()
        .map(|name| ToolStatus {
            name: (*name).to_string(),
            path: find_in_path(name),
        })
        .collect()
}

pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_' | b':' | b'@')
    }) {
        return value.to_string();
    }

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}
