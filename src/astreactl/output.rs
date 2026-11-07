use crate::control_snapshots::{
    AstreactlResult, CursorSnapshot, DoctorCheck, ExactParallelDiagnostics,
    KeyboardConfigurationSnapshot, KeyboardLayoutSnapshot, WindowSnapshot,
};

fn format_exact_parallel_diagnostics(diagnostics: &ExactParallelDiagnostics) -> String {
    let render = diagnostics.render_ready;
    let queue = diagnostics.queue_ready;
    format!(
        "Exact R raw p50/p95/p99/max: {}/{}/{}/{} ns\n\
Exact Q raw p50/p95/p99/max: {}/{}/{}/{} ns\n\
Exact R model ewma/dev/central/selected: {}/{}/{}/{} ns ({})\n\
Exact Q model ewma/dev/central/selected: {}/{}/{}/{} ns ({})\n\
Pipeline p95 C/H/GPU-tail/observed-after-fence: {}/{}/{}/{} ns\n\
Deferred O1 component p95 ready->authority/bind/admission: {}/{}/{} ns (valid {}; origin pageflip/render {}/{}; invalid/missing {}/{})\n\
R/Q over 1R/2R: {}/{}, {}/{}\n\
Fence-before-queue / queue-before-fence: {}/{} (equal {})",
        render.window.p50_ns,
        render.window.p95_ns,
        render.window.p99_ns,
        render.window.max_ns,
        queue.window.p50_ns,
        queue.window.p95_ns,
        queue.window.p99_ns,
        queue.window.max_ns,
        render.ewma_ns,
        render.upper_positive_deviation_ns,
        render.central_risk_ns,
        render.selected_risk_ns,
        render.selected_source.as_str(),
        queue.ewma_ns,
        queue.upper_positive_deviation_ns,
        queue.central_risk_ns,
        queue.selected_risk_ns,
        queue.selected_source.as_str(),
        diagnostics.composite_to_rendered_observed.p95_ns,
        diagnostics.post_render_handoff.p95_ns,
        diagnostics.fence_after_rendered_observed.p95_ns,
        diagnostics.rendered_observed_after_fence.p95_ns,
        diagnostics.deferred_ready_to_authority_observed.p95_ns,
        diagnostics.deferred_authority_to_bind_committed.p95_ns,
        diagnostics.deferred_bind_committed_to_worker_queue.p95_ns,
        diagnostics.deferred_o1_decomposition_samples,
        diagnostics.deferred_o1_predecessor_pageflip_bind_samples,
        diagnostics.deferred_o1_render_completion_bind_samples,
        diagnostics.deferred_o1_decomposition_invalid_samples,
        diagnostics.deferred_o1_binding_timestamp_unavailable_samples,
        render.window.over_refresh_samples,
        render.window.over_two_refresh_samples,
        queue.window.over_refresh_samples,
        queue.window.over_two_refresh_samples,
        diagnostics.fence_before_queue_samples,
        diagnostics.queue_before_fence_samples,
        diagnostics.equal_samples,
    )
}

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
        AstreactlResult::Performance(snapshot) => format!(
            "CPU render p50/p95/p99: {}/{}/{} us\nRepaint skip/partial/full: {}/{}/{}\nTriple render-ahead attempts/ready: {}/{}\nO1 estimator/cold wake guard: {}/{}\nExact parallel samples/render/queue/service: {}/{}/{}/{} ns\nO1 wake/dispatch/apply/selected total: {}/{}/{}/{} ns\nKMS worker-submit wake p95: {} ns\nMiss recovery remaining: {}\nO1 desired/effective credit: {}/{}\nCredit release clean/horizon: {}/{}\n{}",
            snapshot.compositor_cpu_render.p50_us,
            snapshot.compositor_cpu_render.p95_us,
            snapshot.compositor_cpu_render.p99_us,
            snapshot.repaint.skip_frames,
            snapshot.repaint.partial_frames,
            snapshot.repaint.full_frames,
            snapshot.buffering.render_ahead_attempts,
            snapshot.buffering.render_ahead_ready,
            snapshot.buffering.predicted_estimator_mode,
            snapshot.buffering.predicted_cold_wake_guard,
            snapshot.buffering.predicted_parallel_samples,
            snapshot.buffering.predicted_exact_render_ready_risk_ns,
            snapshot.buffering.predicted_exact_queue_ready_risk_ns,
            snapshot.buffering.predicted_parallel_service_ns,
            snapshot.buffering.predicted_main_event_loop_wake_guard_ns,
            snapshot.buffering.predicted_kms_dispatch_budget_ns,
            snapshot.buffering.predicted_kms_apply_guard_ns,
            snapshot.buffering.predicted_total_service_ns,
            snapshot.buffering.predicted_worker_submit_wake_lateness_ns,
            snapshot.buffering.predicted_miss_recovery_remaining,
            snapshot.buffering.desired_future_primary_credit,
            snapshot.buffering.future_primary_credit,
            snapshot.buffering.credit_release_clean_opportunities,
            snapshot.buffering.credit_release_horizon,
            format_exact_parallel_diagnostics(&snapshot.buffering.exact_parallel_diagnostics),
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
                            "{}\t{}\t{}\t{}\t{}-bit",
                            sanitize_terminal_text(&output.name),
                            sanitize_terminal_text(&output.backend),
                            mode,
                            if output.enabled {
                                "enabled"
                            } else {
                                "disabled"
                            },
                            output.color_depth_bits
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        AstreactlResult::Windows(snapshot) => window_table(&snapshot.windows),
        AstreactlResult::WindowDecorationPolicy(snapshot) => format!(
            "Window {} decoration policy: {} (effective mode: {})",
            snapshot.id.0,
            sanitize_terminal_text(&snapshot.policy),
            sanitize_terminal_text(&snapshot.effective_mode)
        ),
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
        AstreactlResult::KeyboardLayout(snapshot) => format_keyboard_layout(snapshot),
        AstreactlResult::KeyboardConfiguration(snapshot) => format_keyboard_configuration(snapshot),
        AstreactlResult::Cursor(snapshot) => format_cursor(snapshot),
        AstreactlResult::DecorationTheme(snapshot) => format!(
            "Selected: {}\nActive: {}\nSchema: {}\nGeneration: {}\nSource: {}\nError: {}",
            sanitize_terminal_text(&snapshot.selected_theme),
            sanitize_terminal_text(&snapshot.active_theme),
            snapshot.schema_version,
            snapshot.generation,
            sanitize_terminal_text(&snapshot.source),
            sanitize_optional_text(snapshot.last_error.as_deref()),
        ),
        AstreactlResult::DecorationThemes(snapshot) => snapshot
            .themes
            .iter()
            .map(|theme| sanitize_terminal_text(theme))
            .collect::<Vec<_>>()
            .join("\n"),
        AstreactlResult::TrustedEffects(snapshot) => format!(
            "Manifest: {}\nGeneration: {}\nEffects: {}",
            sanitize_terminal_text(&snapshot.manifest_path),
            snapshot.generation,
            snapshot.effect_count,
        ),
        AstreactlResult::EffectsPlanner(snapshot) => {
            format!("Planner: {}", sanitize_terminal_text(&snapshot.planner))
        }
        AstreactlResult::EffectsPasses(snapshot) => format!(
            "Material blur passes: {} (default {})",
            snapshot.passes, snapshot.default_passes,
        ),
        AstreactlResult::EffectsRetained(snapshot) => format!(
            "Retained outputs: {}\nGraphs: {}\nHits: {}/{}",
            if snapshot.enabled { "on" } else { "off" },
            snapshot.graphs,
            snapshot.hits,
            snapshot.candidates,
        ),
        AstreactlResult::Blur(snapshot) => format!(
            "Enabled: {}\nRenderer: {}\nGeneration: {}\nWayland: {}\nXWayland: {}\nAuto fullscreen: {}\nLayer default: {}\nRules: {}/{}\nActive assignments: {}\nSources: client={} wayland_auto={} window_rule={} layer_rule={}\nConfig: {}\nLast error: {}",
            snapshot.enabled,
            snapshot.renderer_supported,
            snapshot.generation,
            sanitize_terminal_text(&format!("{:?}", snapshot.wayland_mode)),
            sanitize_terminal_text(&format!("{:?}", snapshot.xwayland_mode)),
            snapshot.auto_fullscreen,
            sanitize_terminal_text(&format!("{:?}", snapshot.layer_default)),
            snapshot.window_rule_count,
            snapshot.layer_rule_count,
            snapshot.active_assignment_count,
            snapshot.assignment_counts_by_source.client,
            snapshot.assignment_counts_by_source.wayland_auto,
            snapshot.assignment_counts_by_source.window_rule,
            snapshot.assignment_counts_by_source.layer_rule,
            sanitize_terminal_text(&snapshot.config_path),
            sanitize_optional_text(snapshot.last_reload_error.as_deref()),
        ),
        AstreactlResult::Animation(snapshot) => format_animation(snapshot),
        AstreactlResult::PresentationTrace(snapshot) => format!(
            "Presentation trace: {}{}",
            if snapshot.active { "on" } else { "off" },
            snapshot
                .path
                .as_deref()
                .map(|path| format!(" ({})", sanitize_terminal_text(path)))
                .unwrap_or_default(),
        ),
        AstreactlResult::PresentationPolicy(snapshot) => format!(
            "Triple buffering: {} (mode {})\nKMS worker: {} (transport {})\nHeld commits: {}\nCredit 2 grant: {}\nRender start: {}",
            sanitize_terminal_text(&snapshot.triple_buffering),
            sanitize_terminal_text(&snapshot.buffering_mode),
            sanitize_terminal_text(&snapshot.kms_worker),
            sanitize_terminal_text(&snapshot.kms_worker_transport),
            sanitize_terminal_text(&snapshot.held_commits),
            sanitize_terminal_text(&snapshot.credit_grant),
            sanitize_terminal_text(&snapshot.render_start),
        ),
        AstreactlResult::Wallpaper(snapshot) => format!(
            "State: {}\nEffective: {}\nConfigured: {}\nFallback: {}\nGeneration: {}\nError: {}",
            sanitize_terminal_text(&snapshot.state),
            sanitize_terminal_text(&snapshot.effective.source),
            snapshot
                .configured
                .as_ref()
                .map(|descriptor| sanitize_terminal_text(&descriptor.source))
                .unwrap_or_else(|| "Factory default".to_string()),
            sanitize_terminal_text(&snapshot.fallback),
            snapshot.generation,
            sanitize_terminal_text(&snapshot.last_error),
        ),
        AstreactlResult::WallpaperList(snapshot) => snapshot
            .wallpapers
            .iter()
            .map(|wallpaper| {
                format!(
                    "{}\t{}\t{}",
                    sanitize_terminal_text(&wallpaper.logical_id),
                    sanitize_terminal_text(&wallpaper.display_name),
                    sanitize_terminal_text(&wallpaper.source),
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn format_animation(snapshot: &crate::animation_control::AnimationControlSnapshot) -> String {
    let config = &snapshot.config;
    let mut lines = vec![
        format!("Enabled: {}", config.enabled),
        format!(
            "Animate maximized window open: {}",
            config.animate_maximized_window_open
        ),
        format!("Preset: {}", sanitize_terminal_text(&config.preset)),
        format!("Speed: {:.2}x", config.speed),
        format!("Generation: {}", snapshot.generation),
        format!("Source: {}", sanitize_terminal_text(&snapshot.source)),
        format!("Startup override: {}", snapshot.startup_override),
        String::new(),
    ];
    lines.extend(snapshot.effective.iter().map(|(slot, effect)| {
        let requested = snapshot
            .requested
            .get(slot)
            .map(String::as_str)
            .unwrap_or(effect);
        format!(
            "{}: {} (requested {})",
            sanitize_terminal_text(slot),
            sanitize_terminal_text(effect),
            sanitize_terminal_text(requested)
        )
    }));
    lines.join("\n")
}

fn format_keyboard_layout(snapshot: &KeyboardLayoutSnapshot) -> String {
    let mut lines = vec![
        format!("Effective: {}", snapshot.effective_index),
        format!("Locked: {}", snapshot.locked_index),
        String::new(),
    ];
    lines.extend(snapshot.layouts.iter().map(|layout| {
        let marker = if layout.index == snapshot.effective_index {
            '*'
        } else {
            ' '
        };
        let name = if layout.name.is_empty() {
            "Unnamed".to_string()
        } else {
            sanitize_terminal_text(&layout.name)
        };
        format!("{} {}  {}", marker, layout.index, name)
    }));
    lines.join("\n")
}

fn format_keyboard_configuration(snapshot: &KeyboardConfigurationSnapshot) -> String {
    let config = &snapshot.configuration;
    format!(
        "Keyboard configuration\nSource: {}\nGeneration: {}\nPersistence: {}\nPending: {}\n\nRules: {}\nModel: {}\nLayout: {}\nVariant: {}\nOptions: {}\n\nRepeat: {}/s\nDelay: {} ms\nDefault layout: {}\n\nEffective layout: {}\nLocked layout: {}",
        snapshot.source.as_str(),
        snapshot.generation,
        snapshot.persistence.as_str(),
        snapshot.pending,
        sanitize_optional_text(config.rules.as_deref()),
        sanitize_optional_text(config.model.as_deref()),
        sanitize_terminal_text(&config.layout),
        sanitize_optional_text(config.variant.as_deref()),
        sanitize_optional_text(config.options.as_deref()),
        config.repeat_rate,
        config.repeat_delay,
        config.default_layout_index,
        snapshot.layout.effective_index,
        snapshot.layout.locked_index,
    )
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
    let mut output = format!(
        "[{}] {} - {}",
        check.severity.as_str().to_uppercase(),
        sanitize_terminal_text(&check.id),
        sanitize_terminal_text(&check.summary)
    );
    if let Some(detail) = check.detail.as_deref() {
        output.push_str("\n  ");
        output.push_str(&sanitize_terminal_text(detail));
    }
    output
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
    use super::{format_exact_parallel_diagnostics, human};
    use crate::animation_control::{
        AnimationCatalogSnapshot, AnimationConfiguration, AnimationConfigurationSnapshot,
        AnimationControlSnapshot, AnimationEffect, AnimationPreset, AnimationSlot,
    };
    use crate::control_snapshots::{
        AstreactlResult, ControlWindowId, DoctorCheck, DoctorSeverity, DoctorSnapshot,
        KeyboardLayoutEntrySnapshot, KeyboardLayoutSnapshot, VersionSnapshot, WindowKindSnapshot,
        WindowListSnapshot, WindowSnapshot,
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
    fn window_decoration_policy_output_shows_id_request_and_effective_mode() {
        let value = AstreactlResult::WindowDecorationPolicy(
            crate::control_snapshots::WindowDecorationPolicySnapshot {
                id: ControlWindowId(7),
                policy: "server".to_string(),
                effective_mode: "ServerSide".to_string(),
            },
        );

        assert_eq!(
            human(&value),
            "Window 7 decoration policy: server (effective mode: ServerSide)"
        );
        assert_eq!(
            serde_json::to_value(value).unwrap(),
            serde_json::json!({"id": 7, "policy": "server", "effectiveMode": "ServerSide"})
        );
    }

    #[test]
    fn doctor_detail_is_displayed_and_sanitized() {
        let value = AstreactlResult::Doctor(DoctorSnapshot {
            healthy: false,
            checks: vec![DoctorCheck {
                id: "direct_scanout.state".to_string(),
                severity: DoctorSeverity::Warning,
                summary: "configured".to_string(),
                detail: Some("line1\n\u{1b}[31mline2\u{1b}[0m".to_string()),
            }],
        });

        assert_eq!(
            human(&value),
            "[WARNING] direct_scanout.state - configured\n  line1 [31mline2[0m"
        );
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

    #[test]
    fn output_human_text_remains_concise_when_capability_modes_are_present() {
        let value = AstreactlResult::Outputs(crate::control_snapshots::OutputListSnapshot {
            outputs: vec![crate::control_snapshots::OutputSnapshot {
                id: String::from("output-1"),
                name: String::from("DP-1"),
                make: None,
                model: None,
                serial: None,
                enabled: true,
                current_mode: Some(crate::control_snapshots::ModeSnapshot {
                    id: 1,
                    width: 1920,
                    height: 1080,
                    refresh_millihz: 165_000,
                }),
                physical_size_mm: Some(crate::control_snapshots::PhysicalSizeSnapshot {
                    width_mm: 600,
                    height_mm: 340,
                }),
                configuration_generation: 1,
                scale_milli: 1000,
                transform: crate::control_snapshots::OutputTransformSnapshot::Normal,
                color_depth_bits: 8,
                mutation_capabilities:
                    crate::control_snapshots::OutputMutationCapabilitiesSnapshot {
                        mode_selection_supported: false,
                        scale: crate::control_snapshots::ScaleMutationCapabilitySnapshot {
                            supported: false,
                            min_milli: None,
                            max_milli: None,
                            values_milli: Vec::new(),
                        },
                        transform: crate::control_snapshots::TransformMutationCapabilitySnapshot {
                            supported: false,
                            transforms: Vec::new(),
                        },
                        enable_disable_supported: false,
                        topology_supported: false,
                        vrr_mutation_supported: false,
                        color_depth_bits: vec![8],
                    },
                position: crate::control_snapshots::PositionSnapshot { x: 0, y: 0 },
                focused: true,
                backend: String::from("atomic"),
                vrr: crate::control_snapshots::FeatureStateSnapshot {
                    state: crate::control_snapshots::FeatureState::Available,
                },
                direct_scanout: crate::control_snapshots::FeatureStateSnapshot {
                    state: crate::control_snapshots::FeatureState::Unavailable,
                },
                modes: vec![crate::control_snapshots::OutputModeSnapshot {
                    id: 1,
                    width: 1920,
                    height: 1080,
                    refresh_millihz: 165_000,
                    preferred: true,
                    interlaced: false,
                }],
                modes_truncated: false,
            }],
            total: 1,
            truncated: false,
            transaction: None,
        });

        let output = human(&value);
        assert_eq!(
            output,
            "DP-1\tatomic\t1920x1080 @ 165.000 Hz\tenabled\t8-bit"
        );
        assert!(!output.contains("preferred"));
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

    #[test]
    fn keyboard_layout_human_output_is_stable_and_sanitized() {
        let value = AstreactlResult::KeyboardLayout(KeyboardLayoutSnapshot {
            effective_index: 1,
            locked_index: 1,
            layout_count: 2,
            layouts: vec![
                KeyboardLayoutEntrySnapshot {
                    index: 0,
                    name: "Portuguese\n\t\u{1b}(Brazil)".to_string(),
                },
                KeyboardLayoutEntrySnapshot {
                    index: 1,
                    name: "English (US)".to_string(),
                },
                KeyboardLayoutEntrySnapshot {
                    index: 2,
                    name: String::new(),
                },
            ],
        });
        assert_eq!(
            human(&value),
            "Effective: 1\nLocked: 1\n\n  0  Portuguese  (Brazil)\n* 1  English (US)\n  2  Unnamed"
        );
    }

    #[test]
    fn human_animation_output_reports_maximized_window_open_policy() {
        let configuration = AnimationConfiguration {
            animate_maximized_window_open: false,
            ..AnimationConfiguration::default()
        };
        let effect = configuration.requested_effect(AnimationSlot::WindowOpen).0;
        let value = AstreactlResult::Animation(AnimationControlSnapshot {
            generation: 3,
            source: "persisted".into(),
            startup_override: false,
            config: AnimationConfigurationSnapshot::from(&configuration),
            effective: [("window.open".to_owned(), effect.id().to_owned())]
                .into_iter()
                .collect(),
            requested: Default::default(),
            catalog: AnimationCatalogSnapshot::default(),
        });

        assert!(human(&value).contains("Animate maximized window open: false"));
        assert_eq!(configuration.preset, AnimationPreset::Astrea);
        assert_eq!(effect, AnimationEffect::WindowScale);
    }

    #[test]
    fn exact_parallel_performance_output_is_compact_and_includes_branch_decomposition() {
        use crate::control_snapshots::{
            BranchRiskDiagnostics, BranchRiskSource, ExactParallelDiagnostics,
            SampleWindowDiagnostics,
        };

        let window = SampleWindowDiagnostics {
            count: 120,
            latest_ns: 8_000_000,
            p50_ns: 1_000_000,
            p90_ns: 1_000_000,
            p95_ns: 1_000_000,
            p99_ns: 1_000_000,
            max_ns: 8_000_000,
            over_refresh_samples: 1,
            over_two_refresh_samples: 1,
        };
        let diagnostics = ExactParallelDiagnostics {
            render_ready: BranchRiskDiagnostics {
                window,
                ewma_ns: 2_000_000,
                upper_positive_deviation_ns: 4_000_000,
                central_risk_ns: 10_000_000,
                selected_risk_ns: 8_000_000,
                selected_source: BranchRiskSource::EvidenceCeiling,
            },
            queue_ready: BranchRiskDiagnostics {
                window: SampleWindowDiagnostics {
                    latest_ns: 4_000_000,
                    p50_ns: 1_000_000,
                    p90_ns: 2_000_000,
                    p95_ns: 3_000_000,
                    p99_ns: 4_000_000,
                    max_ns: 4_000_000,
                    ..window
                },
                ewma_ns: 2_000_000,
                upper_positive_deviation_ns: 500_000,
                central_risk_ns: 3_000_000,
                selected_risk_ns: 3_000_000,
                selected_source: BranchRiskSource::RecentP95,
            },
            composite_to_rendered_observed: SampleWindowDiagnostics {
                p95_ns: 5_000_000,
                ..SampleWindowDiagnostics::default()
            },
            post_render_handoff: SampleWindowDiagnostics {
                p95_ns: 1_000_000,
                ..SampleWindowDiagnostics::default()
            },
            fence_after_rendered_observed: SampleWindowDiagnostics {
                p95_ns: 2_000_000,
                ..SampleWindowDiagnostics::default()
            },
            rendered_observed_after_fence: SampleWindowDiagnostics {
                p95_ns: 3_000_000,
                ..SampleWindowDiagnostics::default()
            },
            deferred_ready_to_authority_observed: SampleWindowDiagnostics {
                p95_ns: 2_000_000,
                ..SampleWindowDiagnostics::default()
            },
            deferred_authority_to_bind_committed: SampleWindowDiagnostics {
                p95_ns: 100_000,
                ..SampleWindowDiagnostics::default()
            },
            deferred_bind_committed_to_worker_queue: SampleWindowDiagnostics {
                p95_ns: 200_000,
                ..SampleWindowDiagnostics::default()
            },
            deferred_o1_decomposition_samples: 12,
            deferred_o1_predecessor_pageflip_bind_samples: 10,
            deferred_o1_render_completion_bind_samples: 2,
            deferred_o1_decomposition_invalid_samples: 1,
            deferred_o1_binding_timestamp_unavailable_samples: 3,
            queue_before_fence_samples: 7,
            fence_before_queue_samples: 8,
            equal_samples: 9,
            ..ExactParallelDiagnostics::default()
        };

        let output = format_exact_parallel_diagnostics(&diagnostics);

        assert!(output.contains("Exact R raw p50/p95/p99/max: 1000000/1000000/1000000/8000000 ns"));
        assert!(output.contains("Exact Q raw p50/p95/p99/max: 1000000/3000000/4000000/4000000 ns"));
        assert!(output.contains("Exact R model ewma/dev/central/selected: 2000000/4000000/10000000/8000000 ns (evidence_ceiling)"));
        assert!(output.contains("Exact Q model ewma/dev/central/selected: 2000000/500000/3000000/3000000 ns (recent_p95)"));
        assert!(output.contains(
            "Pipeline p95 C/H/GPU-tail/observed-after-fence: 5000000/1000000/2000000/3000000 ns"
        ));
        assert!(output.contains(
            "Deferred O1 component p95 ready->authority/bind/admission: 2000000/100000/200000 ns (valid 12; origin pageflip/render 10/2; invalid/missing 1/3)"
        ));
        assert!(output.contains("R/Q over 1R/2R: 1/1, 1/1"));
        assert!(output.contains("Fence-before-queue / queue-before-fence: 8/7 (equal 9)"));
    }
}
