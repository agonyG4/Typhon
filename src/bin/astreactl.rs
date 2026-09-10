mod support {
    pub use oblivion_one::astreactl::*;
}

use oblivion_one::cursor_theme::{validate_cursor_size, validate_cursor_theme};
use std::{path::PathBuf, process::ExitCode, time::Duration};
use support::{
    client::{self, AstreactlError},
    discovery, output, wallpaper,
};

#[derive(Debug, Default)]
struct KeyboardConfigureOptions {
    rules: Option<String>,
    model: Option<String>,
    layout: Option<String>,
    variant: Option<String>,
    options: Option<String>,
    repeat_rate: Option<String>,
    repeat_delay: Option<String>,
    default_layout_index: Option<String>,
    clear_rules: bool,
    clear_model: bool,
    clear_variant: bool,
    clear_options: bool,
}

impl KeyboardConfigureOptions {
    fn has_any(&self) -> bool {
        self.rules.is_some()
            || self.model.is_some()
            || self.layout.is_some()
            || self.variant.is_some()
            || self.options.is_some()
            || self.repeat_rate.is_some()
            || self.repeat_delay.is_some()
            || self.default_layout_index.is_some()
            || self.clear_rules
            || self.clear_model
            || self.clear_variant
            || self.clear_options
    }
}

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
    let mut wallpaper_fit = None;
    let mut keyboard_configure = KeyboardConfigureOptions::default();
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
                    "astreactl [global options] <version|status|doctor|performance|outputs|windows|activewindow|keyboard ...|cursor ...|decoration ...|effects reload|blur ...|animation get|animation set JSON|wallpaper ...>"
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
                if matches!(
                    value,
                    "--theme"
                        | "--size"
                        | "--fit"
                        | "--rules"
                        | "--model"
                        | "--layout"
                        | "--variant"
                        | "--options"
                        | "--repeat-rate"
                        | "--repeat-delay"
                        | "--default-layout-index"
                        | "--default-layout"
                ) {
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
                    } else if value == "--size" {
                        if cursor_size.is_some() {
                            return Err(AstreactlError::Usage(
                                "duplicate cursor --size".to_string(),
                            ));
                        }
                        cursor_size = Some(argument.clone());
                    } else if value == "--fit" {
                        if wallpaper_fit.is_some() {
                            return Err(AstreactlError::Usage(
                                "duplicate wallpaper --fit".to_string(),
                            ));
                        }
                        wallpaper_fit = Some(argument.clone());
                    } else {
                        let destination = match value {
                            "--rules" => &mut keyboard_configure.rules,
                            "--model" => &mut keyboard_configure.model,
                            "--layout" => &mut keyboard_configure.layout,
                            "--variant" => &mut keyboard_configure.variant,
                            "--options" => &mut keyboard_configure.options,
                            "--repeat-rate" => &mut keyboard_configure.repeat_rate,
                            "--repeat-delay" => &mut keyboard_configure.repeat_delay,
                            "--default-layout-index" | "--default-layout" => {
                                &mut keyboard_configure.default_layout_index
                            }
                            _ => unreachable!("matched keyboard configuration option"),
                        };
                        if destination.is_some() {
                            return Err(AstreactlError::Usage(format!(
                                "duplicate keyboard option {value}"
                            )));
                        }
                        *destination = Some(argument.clone());
                    }
                } else if matches!(
                    value,
                    "--clear-rules" | "--clear-model" | "--clear-variant" | "--clear-options"
                ) {
                    let destination = match value {
                        "--clear-rules" => &mut keyboard_configure.clear_rules,
                        "--clear-model" => &mut keyboard_configure.clear_model,
                        "--clear-variant" => &mut keyboard_configure.clear_variant,
                        "--clear-options" => &mut keyboard_configure.clear_options,
                        _ => unreachable!("matched keyboard clear option"),
                    };
                    if *destination {
                        return Err(AstreactlError::Usage(format!(
                            "duplicate keyboard option {value}"
                        )));
                    }
                    *destination = true;
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
    if command == "wallpaper" {
        if keyboard_configure.has_any() {
            return Err(AstreactlError::Usage(
                "keyboard configuration options require a keyboard command".to_string(),
            ));
        }
        if instance.is_some() || socket.is_some() {
            return Err(AstreactlError::Usage(
                "wallpaper commands use the secure Paper endpoint and do not accept Typhon socket options".to_string(),
            ));
        }
        if cursor_theme.is_some() || cursor_size.is_some() {
            return Err(AstreactlError::Usage(
                "wallpaper commands do not accept cursor options".to_string(),
            ));
        }
        let (action, arguments) = parse_wallpaper_command(&positionals[1..], wallpaper_fit)?;
        let wallpaper_timeout = if timeout_supplied {
            timeout
        } else {
            wallpaper::DEFAULT_WALLPAPER_TIMEOUT
        };
        let result = wallpaper::request(action, arguments, wallpaper_timeout)?;
        if json {
            println!(
                "{}",
                serde_json::to_string(&result).map_err(|_| AstreactlError::MalformedResponse)?
            );
        } else {
            println!("{}", output::human(&result));
        }
        return Ok(0);
    }
    let (display_command, wire_command, request_args) = if command == "cursor" {
        if keyboard_configure.has_any() {
            return Err(AstreactlError::Usage(
                "keyboard configuration options require a keyboard command".to_string(),
            ));
        }
        parse_cursor_command(&positionals[1..], cursor_theme, cursor_size)?
    } else if command == "decoration" {
        if keyboard_configure.has_any() {
            return Err(AstreactlError::Usage(
                "keyboard configuration options require a keyboard command".to_string(),
            ));
        }
        if cursor_theme.is_some() || cursor_size.is_some() {
            return Err(AstreactlError::Usage(
                "decoration command does not accept cursor options".to_string(),
            ));
        }
        parse_decoration_command(&positionals[1..])?
    } else if command == "effects" {
        if keyboard_configure.has_any() {
            return Err(AstreactlError::Usage(
                "keyboard configuration options require a keyboard command".to_string(),
            ));
        }
        if cursor_theme.is_some() || cursor_size.is_some() || wallpaper_fit.is_some() {
            return Err(AstreactlError::Usage(
                "effects commands do not accept cursor or wallpaper options".to_string(),
            ));
        }
        parse_effects_command(&positionals[1..])?
    } else if command == "blur" {
        if keyboard_configure.has_any() {
            return Err(AstreactlError::Usage(
                "keyboard configuration options require a keyboard command".to_string(),
            ));
        }
        if cursor_theme.is_some() || cursor_size.is_some() || wallpaper_fit.is_some() {
            return Err(AstreactlError::Usage(
                "blur commands do not accept cursor or wallpaper options".to_string(),
            ));
        }
        parse_blur_command(&positionals[1..])?
    } else if command == "animation" {
        if keyboard_configure.has_any()
            || cursor_theme.is_some()
            || cursor_size.is_some()
            || wallpaper_fit.is_some()
        {
            return Err(AstreactlError::Usage(
                "animation commands do not accept unrelated options".to_string(),
            ));
        }
        parse_animation_command(&positionals[1..])?
    } else if command == "keyboard" {
        if cursor_theme.is_some() || cursor_size.is_some() || wallpaper_fit.is_some() {
            return Err(AstreactlError::Usage(
                "keyboard commands do not accept cursor or wallpaper options".to_string(),
            ));
        }
        if positionals
            .get(1)
            .is_some_and(|subcommand| subcommand == "configure")
        {
            parse_keyboard_configure_command(&positionals[1..], keyboard_configure)?
        } else {
            if keyboard_configure.has_any() {
                return Err(AstreactlError::Usage(
                    "keyboard configuration options require keyboard configure".to_string(),
                ));
            }
            parse_keyboard_command(&positionals[1..])?
        }
    } else {
        if positionals.len() != 1
            || cursor_theme.is_some()
            || cursor_size.is_some()
            || wallpaper_fit.is_some()
            || keyboard_configure.has_any()
        {
            return Err(AstreactlError::Usage(
                "multiple commands are not allowed".to_string(),
            ));
        }
        let wire_command = match command.as_str() {
            "activewindow" => "active-window",
            "version" | "status" | "doctor" | "performance" | "outputs" | "windows" => {
                command.as_str()
            }
            _ => return Err(AstreactlError::Usage(format!("unknown command {command}"))),
        };
        (command.as_str(), wire_command, serde_json::json!({}))
    };
    let path = discovery::discover_socket(instance.as_deref(), socket.as_deref())?;
    let request_args = if command == "keyboard"
        && positionals
            .get(1)
            .is_some_and(|subcommand| subcommand == "configure")
    {
        merge_keyboard_configuration(&path, request_args, timeout)?
    } else {
        request_args
    };
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

fn parse_animation_command(
    positionals: &[String],
) -> Result<(&'static str, &'static str, serde_json::Value), AstreactlError> {
    let subcommand = positionals
        .first()
        .ok_or_else(|| AstreactlError::Usage("missing animation subcommand".to_string()))?;
    match subcommand.as_str() {
        "get" if positionals.len() == 1 => {
            Ok(("animation", "animation.config.get", serde_json::json!({})))
        }
        "set" if positionals.len() == 2 => {
            let value: serde_json::Value = serde_json::from_str(&positionals[1]).map_err(|_| {
                AstreactlError::Usage(
                    "animation set requires a JSON configuration object".to_string(),
                )
            })?;
            if !value.is_object() {
                return Err(AstreactlError::Usage(
                    "animation set requires a JSON configuration object".to_string(),
                ));
            }
            Ok(("animation", "animation.config.set", value))
        }
        "get" => Err(AstreactlError::Usage(
            "animation get takes no arguments".to_string(),
        )),
        "set" => Err(AstreactlError::Usage(
            "animation set requires exactly one JSON configuration object".to_string(),
        )),
        _ => Err(AstreactlError::Usage(format!(
            "unknown animation subcommand {subcommand}"
        ))),
    }
}

fn parse_keyboard_command(
    positionals: &[String],
) -> Result<(&'static str, &'static str, serde_json::Value), AstreactlError> {
    let subcommand = positionals
        .first()
        .ok_or_else(|| AstreactlError::Usage("missing keyboard subcommand".to_string()))?;
    match subcommand.as_str() {
        "config" => {
            if positionals.len() != 1 {
                return Err(AstreactlError::Usage(
                    "keyboard config takes no extra arguments".to_string(),
                ));
            }
            Ok(("keyboard", "keyboard.config.get", serde_json::json!({})))
        }
        "layout" => {
            if positionals.len() != 1 {
                return Err(AstreactlError::Usage(
                    "keyboard layout takes no extra arguments".to_string(),
                ));
            }
            Ok(("keyboard", "keyboard.layout.get", serde_json::json!({})))
        }
        "next" | "previous" => {
            if positionals.len() != 1 {
                return Err(AstreactlError::Usage(
                    "keyboard next/previous takes no extra arguments".to_string(),
                ));
            }
            let wire = if subcommand == "next" {
                "keyboard.layout.next"
            } else {
                "keyboard.layout.previous"
            };
            Ok(("keyboard", wire, serde_json::json!({})))
        }
        "set" => {
            if positionals.len() != 2 {
                return Err(AstreactlError::Usage(
                    "keyboard set requires exactly one non-negative index".to_string(),
                ));
            }
            let index = positionals[1].parse::<u32>().map_err(|_| {
                AstreactlError::Usage(
                    "keyboard layout index must be a non-negative integer".to_string(),
                )
            })?;
            Ok((
                "keyboard",
                "keyboard.layout.set",
                serde_json::json!({"index": index}),
            ))
        }
        _ => Err(AstreactlError::Usage(format!(
            "unknown keyboard subcommand {subcommand}"
        ))),
    }
}

fn parse_keyboard_configure_command(
    positionals: &[String],
    options: KeyboardConfigureOptions,
) -> Result<(&'static str, &'static str, serde_json::Value), AstreactlError> {
    if positionals.len() != 1 {
        return Err(AstreactlError::Usage(
            "keyboard configure accepts typed configuration options only".to_string(),
        ));
    }
    if options.clear_rules && options.rules.is_some()
        || options.clear_model && options.model.is_some()
        || options.clear_variant && options.variant.is_some()
        || options.clear_options && options.options.is_some()
    {
        return Err(AstreactlError::Usage(
            "a keyboard field cannot be set and cleared together".to_string(),
        ));
    }
    let mut args = serde_json::Map::new();
    for (name, value) in [
        ("rules", options.rules),
        ("model", options.model),
        ("layout", options.layout),
        ("variant", options.variant),
        ("options", options.options),
        ("repeatRate", options.repeat_rate),
        ("repeatDelay", options.repeat_delay),
        ("defaultLayoutIndex", options.default_layout_index),
    ] {
        if let Some(value) = value {
            let json_value = match name {
                "repeatRate" | "repeatDelay" => serde_json::Value::Number(
                    value
                        .parse::<i32>()
                        .map_err(|_| AstreactlError::Usage(format!("invalid keyboard {name}")))?
                        .into(),
                ),
                "defaultLayoutIndex" => serde_json::Value::Number(
                    value
                        .parse::<u32>()
                        .map_err(|_| {
                            AstreactlError::Usage(
                                "invalid keyboard default layout index".to_string(),
                            )
                        })?
                        .into(),
                ),
                _ => serde_json::Value::String(value),
            };
            args.insert(name.to_string(), json_value);
        }
    }
    for name in ["rules", "model", "variant", "options"] {
        let clear = match name {
            "rules" => options.clear_rules,
            "model" => options.clear_model,
            "variant" => options.clear_variant,
            "options" => options.clear_options,
            _ => false,
        };
        if clear {
            args.insert(name.to_string(), serde_json::Value::Null);
        }
    }
    Ok((
        "keyboard",
        "keyboard.config.set",
        serde_json::Value::Object(args),
    ))
}

fn merge_keyboard_configuration(
    path: &std::path::Path,
    partial: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, AstreactlError> {
    let current =
        client::request_with_args(path, "keyboard.config.get", serde_json::json!({}), timeout)?;
    let snapshot = match current {
        oblivion_one::control_snapshots::AstreactlResult::KeyboardConfiguration(snapshot) => {
            snapshot
        }
        _ => return Err(AstreactlError::MalformedResponse),
    };
    let mut merged = serde_json::to_value(snapshot.configuration)
        .map_err(|_| AstreactlError::MalformedResponse)?;
    let Some(partial) = partial.as_object() else {
        return Err(AstreactlError::MalformedResponse);
    };
    let Some(merged) = merged.as_object_mut() else {
        return Err(AstreactlError::MalformedResponse);
    };
    for (name, value) in partial {
        merged.insert(name.clone(), value.clone());
    }
    Ok(serde_json::Value::Object(merged.clone()))
}

fn parse_wallpaper_command(
    positionals: &[String],
    fit: Option<String>,
) -> Result<(&'static str, serde_json::Value), AstreactlError> {
    let subcommand = positionals
        .first()
        .ok_or_else(|| AstreactlError::Usage("missing wallpaper subcommand".to_string()))?;
    match subcommand.as_str() {
        "get" | "list" | "reset" | "default" => {
            if positionals.len() != 1 || fit.is_some() {
                return Err(AstreactlError::Usage(
                    "wallpaper get/list/reset/default take no extra arguments".to_string(),
                ));
            }
            let action = match subcommand.as_str() {
                "get" => "get",
                "list" => "list",
                "reset" => "reset",
                _ => "default",
            };
            Ok((action, serde_json::json!({})))
        }
        "set" => {
            if positionals.len() != 2 {
                return Err(AstreactlError::Usage(
                    "wallpaper set requires exactly one path or ID".to_string(),
                ));
            }
            let fit = fit.unwrap_or_else(|| "cover".to_string()).to_lowercase();
            if !matches!(
                fit.as_str(),
                "cover" | "contain" | "stretch" | "center" | "tile"
            ) {
                return Err(AstreactlError::Usage("invalid wallpaper fit".to_string()));
            }
            let target = &positionals[1];
            let mut arguments = if target.starts_with("astrea://wallpaper/") {
                serde_json::json!({"id": target})
            } else {
                serde_json::json!({"source": target})
            };
            arguments["fit"] = serde_json::json!(fit);
            arguments["kind"] = serde_json::json!("image");
            arguments["scope"] = serde_json::json!("global");
            Ok(("set", arguments))
        }
        "import" => {
            if positionals.len() != 2 {
                return Err(AstreactlError::Usage(
                    "wallpaper import requires exactly one path".to_string(),
                ));
            }
            let fit = fit.unwrap_or_else(|| "cover".to_string()).to_lowercase();
            if !matches!(
                fit.as_str(),
                "cover" | "contain" | "stretch" | "center" | "tile"
            ) {
                return Err(AstreactlError::Usage("invalid wallpaper fit".to_string()));
            }
            Ok((
                "import",
                serde_json::json!({"path": positionals[1], "fit": fit}),
            ))
        }
        _ => Err(AstreactlError::Usage(format!(
            "unknown wallpaper subcommand {subcommand}"
        ))),
    }
}

fn parse_decoration_command(
    positionals: &[String],
) -> Result<(&'static str, &'static str, serde_json::Value), AstreactlError> {
    let subcommand = positionals
        .first()
        .ok_or_else(|| AstreactlError::Usage("missing decoration subcommand".to_string()))?;
    match subcommand.as_str() {
        "status" | "reload" | "list" => {
            if positionals.len() != 1 {
                return Err(AstreactlError::Usage(
                    "decoration command takes no extra arguments".to_string(),
                ));
            }
            let wire = match subcommand.as_str() {
                "status" => "decoration.status",
                "reload" => "decoration.reload",
                _ => "decoration.list",
            };
            Ok(("decoration", wire, serde_json::json!({})))
        }
        "set-theme" => {
            if positionals.len() != 2 {
                return Err(AstreactlError::Usage(
                    "decoration set-theme requires exactly one theme".to_string(),
                ));
            }
            Ok((
                "decoration",
                "decoration.set-theme",
                serde_json::json!({"theme": positionals[1]}),
            ))
        }
        _ => Err(AstreactlError::Usage(format!(
            "unknown decoration command {subcommand}"
        ))),
    }
}

fn parse_effects_command(
    positionals: &[String],
) -> Result<(&'static str, &'static str, serde_json::Value), AstreactlError> {
    if positionals.len() == 1 && positionals[0] == "reload" {
        return Ok(("effects", "effects.reload", serde_json::json!({})));
    }
    Err(AstreactlError::Usage(
        "effects command requires reload".to_string(),
    ))
}

fn parse_blur_command(
    positionals: &[String],
) -> Result<(&'static str, &'static str, serde_json::Value), AstreactlError> {
    if positionals.len() != 1 {
        return Err(AstreactlError::Usage(
            "blur command requires status or reload".to_string(),
        ));
    }
    match positionals[0].as_str() {
        "status" => Ok(("blur", "blur.status", serde_json::json!({}))),
        "reload" => Ok(("blur", "blur.reload", serde_json::json!({}))),
        _ => Err(AstreactlError::Usage(
            "blur command requires status or reload".to_string(),
        )),
    }
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
        AstreactlError::Server(_) | AstreactlError::Paper { .. } => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        KeyboardConfigureOptions, parse_keyboard_command, parse_keyboard_configure_command,
        parse_wallpaper_command,
    };
    use oblivion_one::astreactl::wallpaper::DEFAULT_WALLPAPER_TIMEOUT;
    use oblivion_one::control_snapshots::AstreactlResult;

    #[test]
    fn wallpaper_parser_preserves_source_and_fit() {
        let source = "/tmp/snow & café.png".to_string();
        let (action, args) = parse_wallpaper_command(
            &["set".to_string(), source.clone()],
            Some("contain".to_string()),
        )
        .unwrap();
        assert_eq!(action, "set");
        assert_eq!(args["source"], source);
        assert_eq!(args["fit"], "contain");
        assert_eq!(args["kind"], "image");
        assert_eq!(args["scope"], "global");
    }

    #[test]
    fn wallpaper_parser_rejects_invalid_fit_and_typhon_socket_is_not_in_parser() {
        assert!(
            parse_wallpaper_command(
                &["set".to_string(), "/tmp/wallpaper.png".to_string()],
                Some("invalid".to_string()),
            )
            .is_err()
        );
        let (action, args) = parse_wallpaper_command(&["default".to_string()], None).unwrap();
        assert_eq!(action, "default");
        assert!(args.is_object());
        let _ = std::mem::size_of::<AstreactlResult>();
        assert!(DEFAULT_WALLPAPER_TIMEOUT >= std::time::Duration::from_secs(6));
    }

    #[test]
    fn wallpaper_parser_supports_catalog_ids_and_imports() {
        let (action, args) = parse_wallpaper_command(
            &["set".to_string(), "astrea://wallpaper/user/abc".to_string()],
            Some("center".to_string()),
        )
        .unwrap();
        assert_eq!(action, "set");
        assert_eq!(args["id"], "astrea://wallpaper/user/abc");
        assert!(args.get("source").is_none());
        let (action, args) =
            parse_wallpaper_command(&["import".to_string(), "/tmp/source.png".to_string()], None)
                .unwrap();
        assert_eq!(action, "import");
        assert_eq!(args["path"], "/tmp/source.png");
    }

    #[test]
    fn keyboard_parser_maps_runtime_layout_commands() {
        let (display, wire, args) = parse_keyboard_command(&["layout".to_string()]).unwrap();
        assert_eq!((display, wire), ("keyboard", "keyboard.layout.get"));
        assert_eq!(args, serde_json::json!({}));

        let (_, wire, args) = parse_keyboard_command(&["next".to_string()]).unwrap();
        assert_eq!(wire, "keyboard.layout.next");
        assert_eq!(args, serde_json::json!({}));

        let (_, wire, args) = parse_keyboard_command(&["previous".to_string()]).unwrap();
        assert_eq!(wire, "keyboard.layout.previous");
        assert_eq!(args, serde_json::json!({}));

        let (_, wire, args) =
            parse_keyboard_command(&["set".to_string(), "1".to_string()]).unwrap();
        assert_eq!(wire, "keyboard.layout.set");
        assert_eq!(args, serde_json::json!({"index": 1}));

        let (_, wire, args) = parse_keyboard_command(&["config".to_string()]).unwrap();
        assert_eq!(wire, "keyboard.config.get");
        assert_eq!(args, serde_json::json!({}));
    }

    #[test]
    fn keyboard_configure_parser_emits_only_typed_overrides_and_clears() {
        let options = KeyboardConfigureOptions {
            layout: Some("us,br".to_string()),
            repeat_rate: Some("30".to_string()),
            clear_variant: true,
            ..KeyboardConfigureOptions::default()
        };
        let (_, wire, args) =
            parse_keyboard_configure_command(&["configure".to_string()], options).unwrap();
        assert_eq!(wire, "keyboard.config.set");
        assert_eq!(
            args,
            serde_json::json!({
                "layout": "us,br",
                "repeatRate": 30,
                "variant": null
            })
        );
    }

    #[test]
    fn keyboard_parser_rejects_invalid_arity_and_indices() {
        for args in [
            vec!["layout".to_string(), "extra".to_string()],
            vec!["next".to_string(), "extra".to_string()],
            vec!["set".to_string()],
            vec!["set".to_string(), "-1".to_string()],
            vec!["set".to_string(), "one".to_string()],
        ] {
            assert!(parse_keyboard_command(&args).is_err(), "args={args:?}");
        }
    }
}
