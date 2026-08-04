mod support {
    pub use oblivion_one::astreactl::*;
}

use oblivion_one::cursor_theme::{validate_cursor_size, validate_cursor_theme};
use std::{path::PathBuf, process::ExitCode, time::Duration};
use support::{
    client::{self, AstreactlError},
    discovery, output,
};

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("astreactl: {error}");
            ExitCode::from(exit_code(&error))
        }
    }
}

fn run(args: Vec<String>) -> Result<u8, AstreactlError> {
    let mut json = false;
    let mut instance = None;
    let mut socket = None;
    let mut timeout = Duration::from_secs(2);
    let mut timeout_supplied = false;
    let mut positionals = Vec::new();
    let mut cursor_theme = None;
    let mut cursor_size = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => {
                if json {
                    return Err(AstreactlError::Usage("duplicate --json".to_string()));
                }
                json = true;
            }
            "--instance" => {
                index += 1;
                if instance.is_some() {
                    return Err(AstreactlError::Usage("duplicate --instance".to_string()));
                }
                let value = args.get(index).ok_or_else(|| {
                    AstreactlError::Usage("missing value for --instance".to_string())
                })?;
                if value.starts_with('-') {
                    return Err(AstreactlError::Usage(
                        "missing value for --instance".to_string(),
                    ));
                }
                instance = Some(value.clone());
            }
            "--socket" => {
                index += 1;
                if socket.is_some() {
                    return Err(AstreactlError::Usage("duplicate --socket".to_string()));
                }
                let value = args.get(index).ok_or_else(|| {
                    AstreactlError::Usage("missing value for --socket".to_string())
                })?;
                if value.starts_with('-') {
                    return Err(AstreactlError::Usage(
                        "missing value for --socket".to_string(),
                    ));
                }
                socket = Some(PathBuf::from(value));
            }
            "--timeout" => {
                index += 1;
                if timeout_supplied {
                    return Err(AstreactlError::Usage("duplicate --timeout".to_string()));
                }
                let value = args.get(index).ok_or_else(|| {
                    AstreactlError::Usage("missing value for --timeout".to_string())
                })?;
                if value.starts_with('-') {
                    return Err(AstreactlError::Usage(
                        "missing value for --timeout".to_string(),
                    ));
                }
                timeout = parse_timeout(value)?;
                timeout_supplied = true;
            }
            "-h" | "--help" => {
                println!(
                    "astreactl [global options] <version|status|doctor|outputs|windows|activewindow|cursor ...>"
                );
                return Ok(0);
            }
            "-V" | "--version" => {
                if args.len() != 1 {
                    return Err(AstreactlError::Usage(
                        "--version cannot be combined with a command".to_string(),
                    ));
                }
                println!("astreactl {}", env!("CARGO_PKG_VERSION"));
                return Ok(0);
            }
            value if value.starts_with('-') => {
                if value == "--theme" || value == "--size" {
                    index += 1;
                    let argument = args.get(index).ok_or_else(|| {
                        AstreactlError::Usage(format!("missing value for {value}"))
                    })?;
                    if argument.starts_with('-') {
                        return Err(AstreactlError::Usage(format!("missing value for {value}")));
                    }
                    if value == "--theme" {
                        if cursor_theme.is_some() {
                            return Err(AstreactlError::Usage(
                                "duplicate cursor --theme".to_string(),
                            ));
                        }
                        cursor_theme = Some(argument.clone());
                    } else {
                        if cursor_size.is_some() {
                            return Err(AstreactlError::Usage(
                                "duplicate cursor --size".to_string(),
                            ));
                        }
                        cursor_size = Some(argument.clone());
                    }
                } else {
                    return Err(AstreactlError::Usage(format!("unknown option {value}")));
                }
            }
            value => positionals.push(value.to_string()),
        }
        index += 1;
    }
    if instance.is_some() && socket.is_some() {
        return Err(AstreactlError::Usage(
            "--instance and --socket cannot be combined".to_string(),
        ));
    }
    let command = positionals
        .first()
        .ok_or_else(|| AstreactlError::Usage("missing command".to_string()))?;
    let (display_command, wire_command, request_args) = if command == "cursor" {
        parse_cursor_command(&positionals[1..], cursor_theme, cursor_size)?
    } else {
        if positionals.len() != 1 || cursor_theme.is_some() || cursor_size.is_some() {
            return Err(AstreactlError::Usage(
                "multiple commands are not allowed".to_string(),
            ));
        }
        let wire_command = match command.as_str() {
            "activewindow" => "active-window",
            "version" | "status" | "doctor" | "outputs" | "windows" => command.as_str(),
            _ => return Err(AstreactlError::Usage(format!("unknown command {command}"))),
        };
        (command.as_str(), wire_command, serde_json::json!({}))
    };
    let path = discovery::discover_socket(instance.as_deref(), socket.as_deref())?;
    let result = client::request_with_args(&path, wire_command, request_args, timeout)?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&result).map_err(|_| AstreactlError::MalformedResponse)?
        );
    } else {
        println!("{}", output::human(&result));
    }
    if display_command == "doctor"
        && matches!(&result, oblivion_one::control_snapshots::AstreactlResult::Doctor(snapshot) if !snapshot.healthy)
    {
        return Ok(7);
    }
    Ok(0)
}

fn parse_cursor_command(
    positionals: &[String],
    theme: Option<String>,
    size: Option<String>,
) -> Result<(&'static str, &'static str, serde_json::Value), AstreactlError> {
    let subcommand = positionals
        .first()
        .ok_or_else(|| AstreactlError::Usage("missing cursor subcommand".to_string()))?;
    match subcommand.as_str() {
        "get" | "reload" => {
            if positionals.len() != 1 || theme.is_some() || size.is_some() {
                return Err(AstreactlError::Usage(
                    "cursor command does not accept these arguments".to_string(),
                ));
            }
            if subcommand == "get" {
                Ok(("cursor", "cursor.get", serde_json::json!({})))
            } else {
                Ok(("cursor", "cursor.reload", serde_json::json!({})))
            }
        }
        "set-theme" => {
            if positionals.len() != 2 || theme.is_some() || size.is_some() {
                return Err(AstreactlError::Usage(
                    "cursor set-theme requires exactly one theme".to_string(),
                ));
            }
            validate_cursor_theme(&positionals[1])
                .map_err(|_| AstreactlError::Usage("invalid cursor theme".to_string()))?;
            Ok((
                "cursor",
                "cursor.set-theme",
                serde_json::json!({"theme": positionals[1]}),
            ))
        }
        "set-size" => {
            if positionals.len() != 2 || theme.is_some() || size.is_some() {
                return Err(AstreactlError::Usage(
                    "cursor set-size requires exactly one size".to_string(),
                ));
            }
            let size_px = parse_cursor_size(&positionals[1])?;
            Ok((
                "cursor",
                "cursor.set-size",
                serde_json::json!({"sizePx": size_px}),
            ))
        }
        "set" => {
            if positionals.len() != 1 {
                return Err(AstreactlError::Usage(
                    "cursor set accepts only --theme and --size".to_string(),
                ));
            }
            let theme = theme
                .ok_or_else(|| AstreactlError::Usage("cursor set requires --theme".to_string()))?;
            let size = size
                .ok_or_else(|| AstreactlError::Usage("cursor set requires --size".to_string()))?;
            validate_cursor_theme(&theme)
                .map_err(|_| AstreactlError::Usage("invalid cursor theme".to_string()))?;
            let size_px = parse_cursor_size(&size)?;
            Ok((
                "cursor",
                "cursor.set",
                serde_json::json!({"theme": theme, "sizePx": size_px}),
            ))
        }
        _ => Err(AstreactlError::Usage(format!(
            "unknown cursor subcommand {subcommand}"
        ))),
    }
}

fn parse_cursor_size(value: &str) -> Result<u32, AstreactlError> {
    let size = value
        .parse::<u32>()
        .map_err(|_| AstreactlError::Usage("invalid cursor size".to_string()))?;
    validate_cursor_size(size)
        .map_err(|_| AstreactlError::Usage("invalid cursor size".to_string()))?;
    Ok(size)
}

fn parse_timeout(value: &str) -> Result<Duration, AstreactlError> {
    let value = value.trim();
    let (number, unit) = if let Some(number) = value.strip_suffix("ms") {
        (number, "ms")
    } else if let Some(number) = value.strip_suffix('s') {
        (number, "s")
    } else {
        return Err(AstreactlError::Usage(
            "timeout must use ms or s".to_string(),
        ));
    };
    let amount: u64 = number
        .parse()
        .map_err(|_| AstreactlError::Usage("invalid timeout".to_string()))?;
    let duration = if unit == "ms" {
        Duration::from_millis(amount)
    } else {
        Duration::from_secs(amount)
    };
    if duration.is_zero() || duration > Duration::from_secs(60) {
        return Err(AstreactlError::Usage(
            "timeout must be between 1ms and 60s".to_string(),
        ));
    }
    Ok(duration)
}

fn exit_code(error: &AstreactlError) -> u8 {
    match error {
        AstreactlError::Usage(_) => 2,
        AstreactlError::EndpointNotFound(_) => 3,
        AstreactlError::Transport(_) => 4,
        AstreactlError::Timeout => 5,
        AstreactlError::ResponseTooLarge
        | AstreactlError::MalformedResponse
        | AstreactlError::ProtocolMismatch
        | AstreactlError::ResponseIdMismatch { .. } => 6,
        AstreactlError::Server(_) => 1,
    }
}
