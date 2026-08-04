use crate::control_snapshots::{AstreactlResult, CursorSnapshot, DoctorCheck, WindowSnapshot};

pub fn human(result: &AstreactlResult) -> String {
    match result {
        AstreactlResult::Version(snapshot) => format!(
            "{} {}\nProtocol: {}",
            sanitize_terminal_text(&snapshot.compositor_name),
            sanitize_terminal_text(&snapshot.compositor_version),
            snapshot.protocol_version
        ),
        AstreactlResult::Status(snapshot) => format!(
            "Instance: {}\nSession: {}\nWindows: {} mapped, {} minimized",
            sanitize_terminal_text(&snapshot.instance),
            snapshot.session_state.as_str(),
            snapshot.mapped_window_count,
            snapshot.minimized_window_count
        ),
        AstreactlResult::Doctor(snapshot) => snapshot
            .checks
            .iter()
            .map(format_doctor_check)
            .collect::<Vec<_>>()
            .join("\n"),
        AstreactlResult::Outputs(snapshot) => {
            if snapshot.outputs.is_empty() {
                "No outputs".to_string()
            } else {
                snapshot
                    .outputs
                    .iter()
                    .map(|output| {
                        let mode = output
                            .current_mode
                            .as_ref()
                            .map(|mode| {
                                format!(
                                    "{}x{} @ {:.3} Hz",
                                    mode.width,
                                    mode.height,
                                    f64::from(mode.refresh_millihz) / 1000.0
                                )
                            })
                            .unwrap_or_else(|| "unknown mode".to_string());
                        format!(
                            "{}\t{}\t{}\t{}",
                            sanitize_terminal_text(&output.name),
                            sanitize_terminal_text(&output.backend),
                            mode,
                            if output.enabled {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        AstreactlResult::Windows(snapshot) => window_table(&snapshot.windows),
        AstreactlResult::ActiveWindow(snapshot) => snapshot
            .window
            .as_ref()
            .map(|window| {
                format!(
                    "{} {} - {}",
                    window.id.0,
                    sanitize_optional_text(window.app_id.as_deref()),
                    sanitize_terminal_text(&window.title)
                )
            })
            .unwrap_or_else(|| "No active window".to_string()),
        AstreactlResult::Cursor(snapshot) => format_cursor(snapshot),
    }
}

fn format_cursor(snapshot: &CursorSnapshot) -> String {
    format!(
        "Theme: {}\nSize: {} px\nActive: {} at {} px\nGeneration: {}\nBackend: {}\nSource: {}\nPersistence: {}\nAsset: {}",
        sanitize_terminal_text(&snapshot.desired_theme),
        snapshot.desired_size_px,
        sanitize_terminal_text(&snapshot.active_theme),
        snapshot.active_size_px,
        snapshot.generation,
        snapshot.backend.as_str(),
        snapshot.source.as_str(),
        snapshot.persistence.as_str(),
        snapshot.asset_source.as_str(),
    )
}

fn format_doctor_check(check: &DoctorCheck) -> String {
    format!(
        "[{}] {} - {}",
        check.severity.as_str().to_uppercase(),
        sanitize_terminal_text(&check.id),
        sanitize_terminal_text(&check.summary)
    )
}

fn window_table(windows: &[WindowSnapshot]) -> String {
    if windows.is_empty() {
        return "No windows".to_string();
    }
    let mut rows = vec!["ID\tSTATE\tAPP\tTITLE".to_string()];
    rows.extend(windows.iter().map(|window| {
        let state = if window.minimized {
            "minimized"
        } else if window.mapped {
            "mapped"
        } else {
            "unmapped"
        };
        format!(
            "{}\t{}\t{}\t{}",
            window.id.0,
            state,
            sanitize_optional_text(window.app_id.as_deref()),
            sanitize_terminal_text(&window.title)
        )
    }));
    rows.join("\n")
}

fn sanitize_optional_text(value: Option<&str>) -> String {
    value.map(sanitize_terminal_text).unwrap_or_default()
}

pub fn sanitize_terminal_text(value: &str) -> String {
    value
        .chars()
        .filter_map(|character| {
            if character == '\u{1b}' || (0x80..=0x9f).contains(&(character as u32)) {
                None
            } else if character.is_control() {
                Some(' ')
            } else {
                Some(character)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::human;
    use crate::control_snapshots::{
        AstreactlResult, ControlWindowId, VersionSnapshot, WindowKindSnapshot, WindowListSnapshot,
        WindowSnapshot,
    };

    #[test]
    fn version_does_not_print_json_quotes() {
        let value = AstreactlResult::Version(VersionSnapshot {
            protocol_version: 1,
            compositor_name: "Typhon".to_string(),
            compositor_version: "0.1.0".to_string(),
            git_commit: None,
            build_profile: "debug".to_string(),
            rustc_version: None,
        });
        assert_eq!(human(&value), "Typhon 0.1.0\nProtocol: 1");
    }

    #[test]
    fn windows_are_rendered_as_a_compact_table() {
        let value = AstreactlResult::Windows(crate::control_snapshots::WindowListSnapshot {
            windows: vec![crate::control_snapshots::WindowSnapshot {
                id: crate::control_snapshots::ControlWindowId(7),
                app_id: Some("app".to_string()),
                title: "Title".to_string(),
                pid: None,
                kind: crate::control_snapshots::WindowKindSnapshot::XdgToplevel,
                mapped: true,
                active: false,
                minimized: false,
                maximized: false,
                fullscreen: false,
                urgent: None,
                skip_taskbar: false,
                workspace: None,
                output: None,
                geometry: None,
                focus_serial: None,
            }],
            total: 1,
            truncated: false,
        });
        assert_eq!(
            human(&value),
            "ID\tSTATE\tAPP\tTITLE\n7\tmapped\tapp\tTitle"
        );
    }

    fn window_result(title: &str, app_id: &str) -> AstreactlResult {
        AstreactlResult::Windows(WindowListSnapshot {
            windows: vec![WindowSnapshot {
                id: ControlWindowId(7),
                app_id: Some(app_id.to_string()),
                title: title.to_string(),
                pid: None,
                kind: WindowKindSnapshot::XdgToplevel,
                mapped: true,
                active: false,
                minimized: false,
                maximized: false,
                fullscreen: false,
                urgent: None,
                skip_taskbar: false,
                workspace: None,
                output: None,
                geometry: None,
                focus_serial: None,
            }],
            total: 1,
            truncated: false,
        })
    }

    #[test]
    fn human_output_replaces_terminal_controls_without_mutating_json_values() {
        assert_eq!(
            human(&window_result("line1\nline2", "app")),
            "ID\tSTATE\tAPP\tTITLE\n7\tmapped\tapp\tline1 line2"
        );
        assert_eq!(
            human(&window_result("title", "app\tid")),
            "ID\tSTATE\tAPP\tTITLE\n7\tmapped\tapp id\ttitle"
        );
        assert_eq!(
            human(&window_result("line1\rline2", "app")),
            "ID\tSTATE\tAPP\tTITLE\n7\tmapped\tapp\tline1 line2"
        );
        assert_eq!(
            human(&window_result("\u{1b}[31mred\u{1b}[0m", "app")),
            "ID\tSTATE\tAPP\tTITLE\n7\tmapped\tapp\t[31mred[0m"
        );
        assert_eq!(
            human(&window_result("\u{1b}]0;owned\u{7}Title", "app")),
            "ID\tSTATE\tAPP\tTITLE\n7\tmapped\tapp\t]0;owned Title"
        );
        let unicode = "λ".repeat(300);
        assert_eq!(
            human(&window_result(&unicode, "app")),
            format!("ID\tSTATE\tAPP\tTITLE\n7\tmapped\tapp\t{unicode}")
        );
    }

    #[test]
    fn normal_human_text_is_unchanged() {
        assert_eq!(
            human(&window_result("Normal Unicode — title", "org.example.App")),
            "ID\tSTATE\tAPP\tTITLE\n7\tmapped\torg.example.App\tNormal Unicode — title"
        );
    }
}
