mod support {
    pub use oblivion_one::astreactl::*;
}

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
    let mut command = None;
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
                    "astreactl [--json] [--instance NAME] [--socket PATH] [--timeout DURATION] <version|status|doctor|outputs|windows|activewindow>"
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
                return Err(AstreactlError::Usage(format!("unknown option {value}")));
            }
            value => {
                if command.is_some() {
                    return Err(AstreactlError::Usage(
                        "multiple commands are not allowed".to_string(),
                    ));
                }
                command = Some(value.to_string());
            }
        }
        index += 1;
    }
    if instance.is_some() && socket.is_some() {
        return Err(AstreactlError::Usage(
            "--instance and --socket cannot be combined".to_string(),
        ));
    }
    let command = command.ok_or_else(|| AstreactlError::Usage("missing command".to_string()))?;
    let wire_command = match command.as_str() {
        "activewindow" => "active-window",
        "version" | "status" | "doctor" | "outputs" | "windows" => command.as_str(),
        _ => return Err(AstreactlError::Usage(format!("unknown command {command}"))),
    };
    let path = discovery::discover_socket(instance.as_deref(), socket.as_deref())?;
    let result = client::request(&path, wire_command, timeout)?;
    if json {
        println!(
            "{}",
            serde_json::to_string(&result).map_err(|_| AstreactlError::MalformedResponse)?
        );
    } else {
        println!("{}", output::human(&result));
    }
    if command == "doctor"
        && matches!(&result, oblivion_one::control_snapshots::AstreactlResult::Doctor(snapshot) if !snapshot.healthy)
    {
        return Ok(7);
    }
    Ok(0)
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
