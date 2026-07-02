use std::{
    env,
    process::{Command, ExitCode, Stdio},
};

const MAX_JSON_BYTES: usize = 64 * 1024;
const MAX_DESKTOP_ID_BYTES: usize = 1024;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("astrea-shell-control-bridge: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let op = args.next().ok_or_else(usage)?;
    let payload = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }

    let launch_args = match op.as_str() {
        "launch-desktop" => {
            validate_desktop_id(&payload)?;
            vec!["--desktop".to_string(), payload]
        }
        "launch-argv-json" => {
            validate_argv_json(&payload)?;
            vec!["--argv-json".to_string(), payload]
        }
        _ => return Err(usage()),
    };

    let mut command = Command::new(find_astrea_launch());
    command
        .args(launch_args)
        .env("ASTREA_LAUNCH_BYPASS_DAEMON", "1")
        .env("ASTREA_LAUNCH_FORCE_DIRECT", "1")
        .env("ASTREA_COMPOSITOR", "TYPHON")
        .env("XDG_CURRENT_DESKTOP", "Astrea")
        .env("XDG_SESSION_DESKTOP", "Astrea")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("DESKTOP_SESSION", "Astrea")
        .env_remove("DISPLAY")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .stdin(Stdio::null());
    let status = command
        .status()
        .map_err(|err| format!("failed to run astrea-launch: {err}"))?;
    if !status.success() {
        return Err(format!("astrea-launch exited with {status}"));
    }
    Ok(())
}

fn usage() -> String {
    "usage: astrea-shell-control-bridge launch-desktop <desktop-id> | launch-argv-json <json-array>"
        .to_string()
}

fn validate_desktop_id(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("desktop id is empty".to_string());
    }
    if value.len() > MAX_DESKTOP_ID_BYTES {
        return Err("desktop id is too large".to_string());
    }
    if value.bytes().any(|byte| byte == 0) {
        return Err("desktop id contains NUL".to_string());
    }
    if value.contains('/') || value.contains('\\') {
        return Err("desktop id must not contain path separators".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err("desktop id contains unsupported characters".to_string());
    }
    Ok(())
}

fn validate_argv_json(value: &str) -> Result<(), String> {
    if value.len() > MAX_JSON_BYTES {
        return Err("argv JSON is too large".to_string());
    }
    let argv: Vec<String> =
        serde_json::from_str(value).map_err(|err| format!("malformed argv JSON: {err}"))?;
    let Some(program) = argv.first() else {
        return Err("argv is empty".to_string());
    };
    if program.is_empty() {
        return Err("argv program is empty".to_string());
    }
    if argv.iter().any(|arg| arg.bytes().any(|byte| byte == 0)) {
        return Err("argv contains NUL".to_string());
    }
    Ok(())
}

fn find_astrea_launch() -> String {
    if let Ok(root) = env::var("ASTREA_ROOT") {
        let candidate = std::path::Path::new(&root)
            .join("bin")
            .join("astrea-launch");
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    "astrea-launch".to_string()
}
