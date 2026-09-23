use std::{collections::HashMap, ffi::OsStr, sync::OnceLock};

#[cfg(test)]
use std::cell::RefCell;

use crate::egl_renderer::damage::{
    DamageComplexityShadow, FullRepaintReason, OutputDamage, PartialRepaintComplexityAction,
    PartialRepaintComplexityPolicy, RepaintMode, RepaintPlan,
};

use oblivion_one::effects::{
    CompiledFrameGraph, CompiledRenderPass, EffectDemandPlanStats, GraphTextureId,
    GraphTextureSource, RenderPassKind,
};

use super::resources::PooledEffectTexture;

const TRACE_ENV: &str = "TYPHON_EFFECT_EXEC_TRACE";
const DEBUG_CAPTURE_MODE_ENV: &str = "TYPHON_EFFECT_DEBUG_CAPTURE_MODE";
const DEBUG_KAWASE_MODE_ENV: &str = "TYPHON_EFFECT_DEBUG_KAWASE_MODE";
const DEBUG_CHECKPOINT_CAPTURE_PATH_ENV: &str = "TYPHON_EFFECT_DEBUG_CHECKPOINT_CAPTURE_PATH";
const MAX_TRACE_INPUTS: usize = 8;

#[cfg(test)]
thread_local! {
    static TEST_EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TraceConfig {
    enabled: bool,
}

impl TraceConfig {
    pub(crate) fn from_env(value: Option<&OsStr>) -> Self {
        Self {
            enabled: value == Some(OsStr::new("1")),
        }
    }

    pub(crate) const fn enabled(self) -> bool {
        self.enabled
    }
}

fn trace_config() -> TraceConfig {
    TraceConfig::from_env(std::env::var_os(TRACE_ENV).as_deref())
}

fn tracing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| trace_config().enabled())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EffectDebugCaptureMode {
    Replay,
    Framebuffer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CheckpointCapturePath {
    FramebufferBlit,
    FramebufferShaderCopy,
}

const DEFAULT_CHECKPOINT_CAPTURE_PATH: CheckpointCapturePath =
    CheckpointCapturePath::FramebufferShaderCopy;

impl CheckpointCapturePath {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::FramebufferBlit => "blit",
            Self::FramebufferShaderCopy => "shader-copy",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CapturePathFallbackReason {
    NoSampleableOutputTexture,
}

impl CapturePathFallbackReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NoSampleableOutputTexture => "no-sampleable-output-texture",
        }
    }
}

impl EffectDebugCaptureMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Replay => "replay",
            Self::Framebuffer => "framebuffer",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EffectDebugKawaseMode {
    Partial,
    Full,
}

impl EffectDebugKawaseMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Partial => "partial",
            Self::Full => "full",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EffectDebugConfig {
    capture_mode: EffectDebugCaptureMode,
    kawase_mode: EffectDebugKawaseMode,
    checkpoint_capture_path: CheckpointCapturePath,
}

impl EffectDebugConfig {
    #[cfg(test)]
    pub(crate) const fn new(
        capture_mode: EffectDebugCaptureMode,
        kawase_mode: EffectDebugKawaseMode,
    ) -> Self {
        Self {
            capture_mode,
            kawase_mode,
            checkpoint_capture_path: CheckpointCapturePath::FramebufferBlit,
        }
    }

    #[cfg(test)]
    pub(crate) const fn new_with_checkpoint_capture_path(
        capture_mode: EffectDebugCaptureMode,
        kawase_mode: EffectDebugKawaseMode,
        checkpoint_capture_path: CheckpointCapturePath,
    ) -> Self {
        Self {
            capture_mode,
            kawase_mode,
            checkpoint_capture_path,
        }
    }

    pub(crate) fn from_env_values(
        capture_mode: Option<&OsStr>,
        kawase_mode: Option<&OsStr>,
    ) -> Self {
        Self::from_env_values_with_checkpoint_capture_path(capture_mode, kawase_mode, None)
    }

    pub(crate) fn from_env_values_with_checkpoint_capture_path(
        capture_mode: Option<&OsStr>,
        kawase_mode: Option<&OsStr>,
        checkpoint_capture_path: Option<&OsStr>,
    ) -> Self {
        Self {
            capture_mode: parse_debug_capture_mode(capture_mode),
            kawase_mode: parse_debug_kawase_mode(kawase_mode),
            checkpoint_capture_path: parse_checkpoint_capture_path(checkpoint_capture_path),
        }
    }

    pub(crate) const fn capture_mode(self) -> EffectDebugCaptureMode {
        self.capture_mode
    }

    pub(crate) const fn kawase_mode(self) -> EffectDebugKawaseMode {
        self.kawase_mode
    }

    pub(crate) const fn checkpoint_capture_path(self) -> CheckpointCapturePath {
        self.checkpoint_capture_path
    }
}

fn parse_checkpoint_capture_path(value: Option<&OsStr>) -> CheckpointCapturePath {
    match value.and_then(OsStr::to_str) {
        None => DEFAULT_CHECKPOINT_CAPTURE_PATH,
        Some("blit") => CheckpointCapturePath::FramebufferBlit,
        Some("shader-copy") => CheckpointCapturePath::FramebufferShaderCopy,
        Some(value) => {
            static WARNED: OnceLock<()> = OnceLock::new();
            if WARNED.set(()).is_ok() {
                eprintln!(
                    "warning: invalid {DEBUG_CHECKPOINT_CAPTURE_PATH_ENV}={value:?}; using shader-copy default"
                );
            }
            DEFAULT_CHECKPOINT_CAPTURE_PATH
        }
    }
}

fn parse_debug_capture_mode(value: Option<&OsStr>) -> EffectDebugCaptureMode {
    match value.and_then(OsStr::to_str) {
        None => EffectDebugCaptureMode::Replay,
        Some("replay") => EffectDebugCaptureMode::Replay,
        Some("framebuffer") => EffectDebugCaptureMode::Framebuffer,
        Some(value) => {
            eprintln!("warning: invalid {DEBUG_CAPTURE_MODE_ENV}={value:?}; using replay");
            EffectDebugCaptureMode::Replay
        }
    }
}

fn parse_debug_kawase_mode(value: Option<&OsStr>) -> EffectDebugKawaseMode {
    match value.and_then(OsStr::to_str) {
        None => EffectDebugKawaseMode::Partial,
        Some("partial") => EffectDebugKawaseMode::Partial,
        Some("full") => EffectDebugKawaseMode::Full,
        Some(value) => {
            eprintln!("warning: invalid {DEBUG_KAWASE_MODE_ENV}={value:?}; using partial");
            EffectDebugKawaseMode::Partial
        }
    }
}

pub(crate) fn effect_debug_config() -> &'static EffectDebugConfig {
    static CONFIG: OnceLock<EffectDebugConfig> = OnceLock::new();
    CONFIG.get_or_init(|| {
        EffectDebugConfig::from_env_values(
            std::env::var_os(DEBUG_CAPTURE_MODE_ENV).as_deref(),
            std::env::var_os(DEBUG_KAWASE_MODE_ENV).as_deref(),
        )
        .with_checkpoint_capture_path_from_env(
            std::env::var_os(DEBUG_CHECKPOINT_CAPTURE_PATH_ENV).as_deref(),
        )
    })
}

impl EffectDebugConfig {
    fn with_checkpoint_capture_path_from_env(mut self, value: Option<&OsStr>) -> Self {
        self.checkpoint_capture_path = parse_checkpoint_capture_path(value);
        self
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FrameTraceSummary {
    pub(crate) render_generation: Option<u64>,
    pub(crate) scene_generation: Option<u64>,
    pub(crate) scene_signature: Option<u64>,
    pub(crate) repaint_mode: Option<&'static str>,
    pub(crate) render_damage_signature: Option<u64>,
    pub(crate) repair_damage_signature: Option<u64>,
    pub(crate) visible_effect_count: Option<usize>,
    pub(crate) selected_effect_count: Option<usize>,
    pub(crate) graph_pass_count: Option<usize>,
    pub(crate) graph_texture_count: Option<usize>,
    pub(crate) peak_live_intermediate_count: Option<usize>,
    pub(crate) demand_plan: Option<EffectDemandPlanStats>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DamageTraceKind {
    None,
    Empty,
    Rects,
    Full,
}

impl DamageTraceKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Empty => "empty",
            Self::Rects => "rects",
            Self::Full => "full",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DamageTraceSnapshot {
    pub(crate) kind: DamageTraceKind,
    pub(crate) rects: usize,
    pub(crate) pixels: u64,
}

impl DamageTraceSnapshot {
    pub(crate) fn from_optional(
        damage: Option<&OutputDamage>,
        output_width: u32,
        output_height: u32,
    ) -> Self {
        damage.map_or(
            Self {
                kind: DamageTraceKind::None,
                rects: 0,
                pixels: 0,
            },
            |damage| Self::from_damage(damage, output_width, output_height),
        )
    }

    pub(crate) fn from_damage(
        damage: &OutputDamage,
        output_width: u32,
        output_height: u32,
    ) -> Self {
        let kind = match damage {
            OutputDamage::Empty => DamageTraceKind::Empty,
            OutputDamage::Rects(_) => DamageTraceKind::Rects,
            OutputDamage::Full => DamageTraceKind::Full,
        };
        Self {
            kind,
            rects: damage.rect_count(),
            pixels: damage
                .pixels(output_width, output_height)
                .unwrap_or(u64::MAX),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RepaintPlanTraceSnapshot {
    pub(crate) mode: RepaintMode,
    pub(crate) fallback_reason: Option<FullRepaintReason>,
    pub(crate) complexity_policy: PartialRepaintComplexityPolicy,
    pub(crate) complexity_action: PartialRepaintComplexityAction,
    pub(crate) buffer_age: Option<u32>,
    pub(crate) render_damage: DamageTraceSnapshot,
    pub(crate) repair_damage: DamageTraceSnapshot,
}

impl RepaintPlanTraceSnapshot {
    pub(crate) fn from_plan(plan: &RepaintPlan, output_width: u32, output_height: u32) -> Self {
        Self {
            mode: plan.mode,
            fallback_reason: plan.fallback_reason,
            complexity_policy: plan.complexity_policy,
            complexity_action: plan.complexity_action,
            buffer_age: plan.buffer_age,
            render_damage: DamageTraceSnapshot::from_damage(
                &plan.render_damage,
                output_width,
                output_height,
            ),
            repair_damage: DamageTraceSnapshot::from_damage(
                &plan.repair_damage,
                output_width,
                output_height,
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FirstFullRepaintStage {
    None,
    InputDamage,
    SceneDamage,
    EffectDamageMerge,
    InitialRepaintPlan,
    EffectExecution,
}

impl FirstFullRepaintStage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::InputDamage => "input_damage",
            Self::SceneDamage => "scene_damage",
            Self::EffectDamageMerge => "effect_damage_merge",
            Self::InitialRepaintPlan => "initial_repaint_plan",
            Self::EffectExecution => "effect_execution",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EffectRepaintProvenanceSnapshot {
    pub(crate) input_damage: DamageTraceSnapshot,
    pub(crate) scene_damage: DamageTraceSnapshot,
    pub(crate) merged_damage: DamageTraceSnapshot,
    pub(crate) initial_plan: RepaintPlanTraceSnapshot,
    pub(crate) final_plan: RepaintPlanTraceSnapshot,
    pub(crate) damage_complexity_shadow: DamageComplexityShadow,
}

impl EffectRepaintProvenanceSnapshot {
    pub(crate) const fn new(
        input_damage: DamageTraceSnapshot,
        scene_damage: DamageTraceSnapshot,
        merged_damage: DamageTraceSnapshot,
        initial_plan: RepaintPlanTraceSnapshot,
        final_plan: RepaintPlanTraceSnapshot,
        damage_complexity_shadow: DamageComplexityShadow,
    ) -> Self {
        Self {
            input_damage,
            scene_damage,
            merged_damage,
            initial_plan,
            final_plan,
            damage_complexity_shadow,
        }
    }

    pub(crate) const fn first_full_stage(self) -> FirstFullRepaintStage {
        if matches!(self.input_damage.kind, DamageTraceKind::Full) {
            FirstFullRepaintStage::InputDamage
        } else if matches!(self.scene_damage.kind, DamageTraceKind::Full) {
            FirstFullRepaintStage::SceneDamage
        } else if matches!(self.merged_damage.kind, DamageTraceKind::Full) {
            FirstFullRepaintStage::EffectDamageMerge
        } else if matches!(self.initial_plan.mode, RepaintMode::Full) {
            FirstFullRepaintStage::InitialRepaintPlan
        } else if matches!(self.final_plan.mode, RepaintMode::Full) {
            FirstFullRepaintStage::EffectExecution
        } else {
            FirstFullRepaintStage::None
        }
    }

    pub(crate) const fn promoted_to_full(self) -> bool {
        !matches!(self.initial_plan.mode, RepaintMode::Full)
            && matches!(self.final_plan.mode, RepaintMode::Full)
    }

    pub(crate) fn format_line(self, frame_id: Option<u64>) -> String {
        let (bbox_x, bbox_y, bbox_width, bbox_height) = self
            .damage_complexity_shadow
            .bbox
            .map_or((0, 0, 0, 0), |bbox| {
                (bbox.x, bbox.y, bbox.width, bbox.height)
            });
        format!(
            "event=effect_repaint_provenance frame_id={} input_damage_kind={} input_damage_rects={} input_damage_pixels={} scene_damage_kind={} scene_damage_rects={} scene_damage_pixels={} merged_damage_kind={} merged_damage_rects={} merged_damage_pixels={} initial_repaint_mode={} initial_repaint_reason={} initial_buffer_age={} initial_render_damage_kind={} initial_render_damage_rects={} initial_render_damage_pixels={} initial_repair_damage_kind={} initial_repair_damage_rects={} initial_repair_damage_pixels={} final_repaint_mode={} final_repaint_reason={} final_buffer_age={} final_render_damage_kind={} final_render_damage_rects={} final_render_damage_pixels={} final_repair_damage_kind={} final_repair_damage_rects={} final_repair_damage_pixels={} first_full_stage={} promoted_to_full={} partial_repaint_complexity_policy={} partial_repaint_complexity_action={} damage_complexity_shadow_applicable={} damage_complexity_original_rects={} damage_complexity_original_pixels={} damage_complexity_bbox_x={} damage_complexity_bbox_y={} damage_complexity_bbox_width={} damage_complexity_bbox_height={} damage_complexity_bbox_pixels={} damage_complexity_bbox_accepted={} damage_complexity_candidate_rects={} damage_complexity_candidate_pixels={} damage_complexity_added_pixels={} damage_complexity_outcome={} damage_complexity_would_avoid_full={}",
            optional_u64(frame_id),
            self.input_damage.kind.as_str(),
            self.input_damage.rects,
            self.input_damage.pixels,
            self.scene_damage.kind.as_str(),
            self.scene_damage.rects,
            self.scene_damage.pixels,
            self.merged_damage.kind.as_str(),
            self.merged_damage.rects,
            self.merged_damage.pixels,
            self.initial_plan.mode.as_str(),
            self.initial_plan
                .fallback_reason
                .map_or("none", FullRepaintReason::as_str),
            optional_u32_none(self.initial_plan.buffer_age),
            self.initial_plan.render_damage.kind.as_str(),
            self.initial_plan.render_damage.rects,
            self.initial_plan.render_damage.pixels,
            self.initial_plan.repair_damage.kind.as_str(),
            self.initial_plan.repair_damage.rects,
            self.initial_plan.repair_damage.pixels,
            self.final_plan.mode.as_str(),
            self.final_plan
                .fallback_reason
                .map_or("none", FullRepaintReason::as_str),
            optional_u32_none(self.final_plan.buffer_age),
            self.final_plan.render_damage.kind.as_str(),
            self.final_plan.render_damage.rects,
            self.final_plan.render_damage.pixels,
            self.final_plan.repair_damage.kind.as_str(),
            self.final_plan.repair_damage.rects,
            self.final_plan.repair_damage.pixels,
            self.first_full_stage().as_str(),
            if self.promoted_to_full() { "1" } else { "0" },
            self.final_plan.complexity_policy.as_str(),
            self.final_plan.complexity_action.as_str(),
            if self.damage_complexity_shadow.applicable {
                "1"
            } else {
                "0"
            },
            self.damage_complexity_shadow.original_rects,
            self.damage_complexity_shadow.original_pixels,
            bbox_x,
            bbox_y,
            bbox_width,
            bbox_height,
            self.damage_complexity_shadow.bbox_pixels,
            if self.damage_complexity_shadow.bbox_accepted {
                "1"
            } else {
                "0"
            },
            self.damage_complexity_shadow.candidate_rects,
            self.damage_complexity_shadow.candidate_pixels,
            self.damage_complexity_shadow.added_pixels,
            self.damage_complexity_shadow.outcome.as_str(),
            if self.damage_complexity_shadow.would_avoid_full {
                "1"
            } else {
                "0"
            },
        )
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct PassTraceFields<'a> {
    pub(crate) boundary: &'static str,
    pub(crate) pass_id: u64,
    pub(crate) instance_id: u64,
    pub(crate) kind: &'static str,
    pub(crate) input_ids: &'a [GraphTextureId],
    pub(crate) output_id: Option<u64>,
    pub(crate) capture_mode: Option<&'static str>,
    pub(crate) capture_commands: Option<usize>,
    pub(crate) backdrop_capture_policy: Option<&'static str>,
    pub(crate) kawase_execution_policy: Option<&'static str>,
    pub(crate) scene_work_damage_rects: usize,
    pub(crate) scene_work_damage_bbox: Option<(i32, i32, u32, u32)>,
}

#[derive(Debug, Default)]
pub(crate) struct PassTraceSummary {
    pub(crate) target_flip_y: bool,
    pub(crate) input_flip_y: bool,
    pub(crate) framebuffer_origin: Option<&'static str>,
    pub(crate) damage_rect_count: usize,
    pub(crate) damage_bounding_box: Option<(i32, i32, u32, u32)>,
    pub(crate) capture_mode: Option<&'static str>,
    pub(crate) requested_capture_path: Option<&'static str>,
    pub(crate) executed_capture_path: Option<&'static str>,
    pub(crate) capture_fallback_reason: Option<&'static str>,
    pub(crate) capture_command_count: Option<usize>,
    pub(crate) read_framebuffer: Option<String>,
    pub(crate) draw_framebuffer: Option<String>,
    pub(crate) scratch_fbo_present: Option<bool>,
    pub(crate) conservative_pass_demand: bool,
    pub(crate) conservative_pass_demand_kind: &'static str,
    pub(crate) backdrop_capture_policy: Option<&'static str>,
    pub(crate) kawase_execution_policy: Option<&'static str>,
    pub(crate) scene_work_damage_rects: usize,
    pub(crate) scene_work_damage_bounding_box: Option<(i32, i32, u32, u32)>,
}

#[cfg(test)]
pub(crate) fn format_pass_trace_line(fields: PassTraceFields<'_>) -> String {
    let input_ids = fields
        .input_ids
        .iter()
        .take(MAX_TRACE_INPUTS)
        .map(|id| id.get().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let output_id = fields
        .output_id
        .map_or_else(|| "none".to_owned(), |id| id.to_string());
    let capture_mode = fields.capture_mode.unwrap_or("none");
    let capture_commands = fields
        .capture_commands
        .map_or_else(|| "none".to_owned(), |count| count.to_string());
    let scene_work_damage_bbox = fields.scene_work_damage_bbox.map_or_else(
        || "none".to_owned(),
        |(x, y, width, height)| format!("{x},{y},{width},{height}"),
    );
    format!(
        "event=effect_pass_{} pass={} instance={} kind={} inputs={} output={} capture_mode={} backdrop_capture_policy={} kawase_execution_policy={} scene_work_damage_rects={} scene_work_damage_bbox={} capture_commands={}",
        fields.boundary,
        fields.pass_id,
        fields.instance_id,
        fields.kind,
        input_ids,
        output_id,
        capture_mode,
        fields.backdrop_capture_policy.unwrap_or("replay"),
        fields.kawase_execution_policy.unwrap_or("partial"),
        fields.scene_work_damage_rects,
        scene_work_damage_bbox,
        capture_commands,
    )
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct EffectExecutionTrace {
    enabled: bool,
    frame_id: Option<u64>,
    render_generation: Option<u64>,
    scene_generation: Option<u64>,
    scene_signature: Option<u64>,
}

impl EffectExecutionTrace {
    pub(crate) fn new(
        frame_id: Option<u64>,
        render_generation: Option<u64>,
        scene_generation: Option<u64>,
        scene_signature: Option<u64>,
    ) -> Self {
        Self {
            enabled: tracing_enabled(),
            frame_id,
            render_generation,
            scene_generation,
            scene_signature,
        }
    }

    #[cfg(test)]
    pub(crate) const fn disabled_for_test() -> Self {
        Self {
            enabled: false,
            frame_id: None,
            render_generation: None,
            scene_generation: None,
            scene_signature: None,
        }
    }

    #[cfg(test)]
    pub(crate) const fn enabled_for_test() -> Self {
        Self {
            enabled: true,
            frame_id: None,
            render_generation: None,
            scene_generation: None,
            scene_signature: None,
        }
    }

    pub(crate) const fn enabled(self) -> bool {
        self.enabled
    }

    pub(crate) const fn frame_id(self) -> Option<u64> {
        self.frame_id
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn capture_gpu_timing(
        &self,
        frame_id: Option<u64>,
        pass_id: Option<u64>,
        instance_id: Option<u64>,
        kind: RenderPassKind,
        capture_path: &'static str,
        checkpoint_count: usize,
        pixels: u64,
        duration_ns: u64,
    ) {
        let line = format!(
            "event=effect_capture_gpu_timing frame_id={} pass={} instance={} kind={} capture_path={} checkpoint_count={} pixels={} duration_ns={}",
            optional_u64(frame_id),
            optional_u64(pass_id),
            optional_u64(instance_id),
            render_pass_kind_name(kind),
            capture_path,
            checkpoint_count,
            pixels,
            duration_ns,
        );
        #[cfg(test)]
        TEST_EVENTS.with(|events| events.borrow_mut().push(line.clone()));
        eprintln!("typhon effect: {line}");
    }

    pub(crate) fn event<F>(&self, make_line: F)
    where
        F: FnOnce() -> String,
    {
        if self.enabled {
            let line = make_line();
            #[cfg(test)]
            TEST_EVENTS.with(|events| events.borrow_mut().push(line.clone()));
            eprintln!("typhon effect: {line}");
        }
    }

    pub(crate) fn effect_repaint_provenance<F>(&self, make_snapshot: F)
    where
        F: FnOnce() -> EffectRepaintProvenanceSnapshot,
    {
        self.event(|| make_snapshot().format_line(self.frame_id));
    }

    pub(crate) fn scene_work_preservation(
        &self,
        phase: &'static str,
        rect_count: usize,
        pixels: u64,
        output_pixels: u64,
    ) {
        self.event(|| {
            format!(
                "event=effect_scene_work_preservation phase={phase} rect_count={rect_count} pixels={pixels} output_pixels={output_pixels}"
            )
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn scene_replay_boundary(
        &self,
        boundary: &'static str,
        pass: &CompiledRenderPass,
        reason: &'static str,
        scene_cursor_start: usize,
        scene_cursor_end: usize,
        active_work_rects: usize,
        active_work_pixels: u64,
        baseline_work_rects: usize,
        baseline_work_pixels: u64,
        pending_checkpoint_requirements: usize,
    ) {
        self.event(|| {
            format!(
                "event=effect_scene_replay_{boundary} frame_id={} pass={} kind={} reason={reason} scene_cursor_start={scene_cursor_start} scene_cursor_end={scene_cursor_end} command_count={} active_work_rects={active_work_rects} active_work_pixels={active_work_pixels} baseline_work_rects={baseline_work_rects} baseline_work_pixels={baseline_work_pixels} saved_pixels={} pending_checkpoint_requirements={pending_checkpoint_requirements}",
                optional_u64(self.frame_id),
                pass.id.get(),
                render_pass_kind_name(pass.kind),
                scene_cursor_end.saturating_sub(scene_cursor_start),
                baseline_work_pixels.saturating_sub(active_work_pixels),
            )
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn final_scene_replay_boundary(
        &self,
        boundary: &'static str,
        scene_cursor_start: usize,
        scene_cursor_end: usize,
        active_work_rects: usize,
        active_work_pixels: u64,
        baseline_work_rects: usize,
        baseline_work_pixels: u64,
        pending_checkpoint_requirements: usize,
    ) {
        self.event(|| {
            format!(
                "event=effect_final_scene_replay_{boundary} frame_id={} scene_cursor_start={scene_cursor_start} scene_cursor_end={scene_cursor_end} command_count={} active_work_rects={active_work_rects} active_work_pixels={active_work_pixels} baseline_work_rects={baseline_work_rects} baseline_work_pixels={baseline_work_pixels} saved_pixels={} pending_checkpoint_requirements={pending_checkpoint_requirements}",
                optional_u64(self.frame_id),
                scene_cursor_end.saturating_sub(scene_cursor_start),
                baseline_work_pixels.saturating_sub(active_work_pixels),
            )
        });
    }

    pub(crate) fn overlay_boundary(&self, boundary: &'static str) {
        self.event(|| {
            format!(
                "event=effect_overlay_draw_{boundary} frame_id={}",
                optional_u64(self.frame_id),
            )
        });
    }

    pub(crate) fn frame_boundary(
        &self,
        phase: &'static str,
        boundary: &'static str,
        summary: FrameTraceSummary,
    ) {
        self.event(|| {
            format!(
                "event={phase}_{boundary} frame_id={} render_generation={} scene_generation={} scene_signature={} repaint_mode={} render_damage={} repair_damage={} visible_effects={} selected_effects={} graph_passes={} graph_textures={} peak_live_intermediates={} repair_rect_count={} dependency_edge_count={} dependency_propagations={} max_instance_region_rect_count={} region_representation_overflows={} visible_clip_fallbacks={} work_region_bbox_coalesces={} conservative_full={} pass_count_selected={} partial_pass_count={} full_domain_pass_count={} pass_dependency_propagations={} max_pass_region_rect_count={} pass_conservative_fallbacks={}",
                optional_u64(self.frame_id),
                optional_u64(summary.render_generation.or(self.render_generation)),
                optional_u64(summary.scene_generation.or(self.scene_generation)),
                optional_u64(summary.scene_signature.or(self.scene_signature)),
                summary.repaint_mode.unwrap_or("unknown"),
                optional_u64(summary.render_damage_signature),
                optional_u64(summary.repair_damage_signature),
                optional_usize(summary.visible_effect_count),
                optional_usize(summary.selected_effect_count),
                optional_usize(summary.graph_pass_count),
                optional_usize(summary.graph_texture_count),
                optional_usize(summary.peak_live_intermediate_count),
                summary
                    .demand_plan
                    .map(|stats| stats.repair_rect_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.dependency_edge_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.dependency_propagations)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.max_instance_region_rect_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.region_representation_overflows)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.visible_clip_fallbacks)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.work_region_bbox_coalesces)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary.demand_plan.map_or_else(
                    || "unknown".to_owned(),
                    |stats| stats.conservative_full.to_string(),
                ),
                summary
                    .demand_plan
                    .map(|stats| stats.pass_count_selected)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.partial_pass_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.full_domain_pass_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.pass_dependency_propagations)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.max_pass_region_rect_count)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
                summary
                    .demand_plan
                    .map(|stats| stats.pass_conservative_fallbacks)
                    .map_or_else(|| "unknown".to_owned(), |count| count.to_string()),
            )
        });
    }

    pub(crate) fn visible_clip_fallback(
        &self,
        pass: &CompiledRenderPass,
        input_rect_count: usize,
        clip_rect_count: usize,
    ) {
        self.event(|| {
            format!(
                "event=region_representation_overflow frame_id={} instance={} pass={} input_rect_count={} clip_rect_count={} fallback=output_influence visible_clip_fallback=true",
                optional_u64(self.frame_id),
                pass.instance.get(),
                pass.id.get(),
                input_rect_count,
                clip_rect_count,
            )
        });
    }

    pub(crate) fn execution_region(
        &self,
        pass: &CompiledRenderPass,
        input_rect_count: usize,
        execution_region_rect_count: usize,
        duplicate_rects_removed: usize,
        overlap_fragments_generated: usize,
        fallback: Option<&'static str>,
    ) {
        self.event(|| {
            format!(
                "event=effect_execution_region frame_id={} instance={} pass={} input_rect_count={} execution_region_rect_count={} duplicate_rects_removed={} overlap_fragments_generated={} fallback={}",
                optional_u64(self.frame_id),
                pass.instance.get(),
                pass.id.get(),
                input_rect_count,
                execution_region_rect_count,
                duplicate_rects_removed,
                overlap_fragments_generated,
                fallback.unwrap_or("none"),
            )
        });
    }

    pub(crate) fn capture_materialization(
        &self,
        pass: &CompiledRenderPass,
        logical_rect_count: usize,
        logical_bounding_box: Option<(i32, i32, u32, u32)>,
        physical_rect_count: usize,
        physical_pixels: u64,
    ) {
        self.event(|| {
            let bbox = logical_bounding_box.map_or_else(
                || "none".to_owned(),
                |(x, y, width, height)| format!("{x},{y},{width},{height}"),
            );
            format!(
                "event=effect_capture_materialization frame_id={} instance={} pass={} kind={} logical_rects={logical_rect_count} logical_bbox={bbox} physical_rects={physical_rect_count} physical_pixels={physical_pixels}",
                optional_u64(self.frame_id),
                pass.instance.get(),
                pass.id.get(),
                render_pass_kind_name(pass.kind),
            )
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn checkpoint_source_validity(
        &self,
        pass: &CompiledRenderPass,
        required_rect_count: usize,
        required_bounding_box: Option<(i32, i32, u32, u32)>,
        valid_rect_count: usize,
        valid_bounding_box: Option<(i32, i32, u32, u32)>,
        missing_rect_count: usize,
        missing_bounding_box: Option<(i32, i32, u32, u32)>,
        missing_pixels: u64,
    ) {
        self.event(|| {
            let format_bbox = |bbox: Option<(i32, i32, u32, u32)>| {
                bbox.map_or_else(|| "none".to_owned(), |(x, y, width, height)| {
                    format!("{x},{y},{width},{height}")
                })
            };
            format!(
                "event=effect_checkpoint_source_validity frame_id={} pass={} instance={} kind={} anchor={:?} checkpoints={} required_rects={} required_bbox={} valid_rects={} valid_bbox={} missing_rects={} missing_bbox={} missing_pixels={}",
                optional_u64(self.frame_id),
                pass.id.get(),
                pass.instance.get(),
                render_pass_kind_name(pass.kind),
                pass.anchor,
                pass.checkpoint_dependencies.len(),
                required_rect_count,
                format_bbox(required_bounding_box),
                valid_rect_count,
                format_bbox(valid_bounding_box),
                missing_rect_count,
                format_bbox(missing_bounding_box),
                missing_pixels,
            )
        });
    }

    pub(crate) fn pass_boundary(
        &self,
        boundary: &'static str,
        pass: &CompiledRenderPass,
        graph: &CompiledFrameGraph,
        resources: &HashMap<GraphTextureId, PooledEffectTexture>,
        summary: PassTraceSummary,
    ) {
        self.event(|| {
            let input_ids = pass
                .inputs
                .iter()
                .take(MAX_TRACE_INPUTS)
                .map(|id| id.get().to_string())
                .collect::<Vec<_>>()
                .join(",");
            let input_details = pass
                .inputs
                .iter()
                .take(MAX_TRACE_INPUTS)
                .filter_map(|id| graph_texture_trace(graph, *id, resources))
                .collect::<Vec<_>>()
                .join(";");
            let output_details = pass
                .output
                .and_then(|id| graph_texture_trace(graph, id, resources))
                .unwrap_or_else(|| "none".to_owned());
            let bbox = summary.damage_bounding_box.map_or_else(
                || "none".to_owned(),
                |(x, y, width, height)| format!("{x},{y},{width},{height}"),
            );
            format!(
                "event=effect_pass_{boundary} frame_id={} pass={} instance={} kind={} anchor={:?} anchor_scope={:?} visual_group={} inputs={} input_details={} output={} framebuffer_origin={} target_flip_y={} input_flip_y={} damage_rects={} damage_bbox={} checkpoints={} capture_mode={} requested_capture_path={} executed_capture_path={} fallback_reason={} backdrop_capture_policy={} kawase_execution_policy={} scene_work_damage_rects={} scene_work_damage_bbox={} conservative_pass_demand={} conservative_pass_demand_kind={} capture_commands={} read_fbo={} draw_fbo={} scratch_fbo_present={}",
                optional_u64(self.frame_id),
                pass.id.get(),
                pass.instance.get(),
                render_pass_kind_name(pass.kind),
                pass.anchor,
                pass.anchor_scope,
                pass.visual_group.map_or(0, |group| group.get()),
                input_ids,
                input_details,
                output_details,
                summary.framebuffer_origin.unwrap_or("unknown"),
                summary.target_flip_y,
                summary.input_flip_y,
                summary.damage_rect_count,
                bbox,
                pass.checkpoint_dependencies.len(),
                summary.capture_mode.unwrap_or("none"),
                summary.requested_capture_path.unwrap_or("none"),
                summary.executed_capture_path.unwrap_or("none"),
                summary.capture_fallback_reason.unwrap_or("none"),
                summary.backdrop_capture_policy.unwrap_or("replay"),
                summary.kawase_execution_policy.unwrap_or("partial"),
                summary.scene_work_damage_rects,
                summary.scene_work_damage_bounding_box.map_or_else(
                    || "none".to_owned(),
                    |(x, y, width, height)| format!("{x},{y},{width},{height}"),
                ),
                summary.conservative_pass_demand,
                summary.conservative_pass_demand_kind,
                summary
                    .capture_command_count
                    .map_or_else(|| "none".to_owned(), |count| count.to_string()),
                summary
                    .read_framebuffer
                    .as_deref()
                    .unwrap_or("none"),
                summary
                    .draw_framebuffer
                    .as_deref()
                    .unwrap_or("none"),
                summary
                    .scratch_fbo_present
                    .map_or_else(|| "unknown".to_owned(), |complete| complete.to_string()),
            )
        });
    }

    pub(crate) fn invariant_failure(&self, error: &dyn std::error::Error) {
        self.event(|| format!("event=effect_invariant_failure invariant={error}"));
    }
}

#[cfg(test)]
pub(crate) fn clear_test_events() {
    TEST_EVENTS.with(|events| events.borrow_mut().clear());
}

#[cfg(test)]
pub(crate) fn take_test_events() -> Vec<String> {
    TEST_EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
}

fn render_pass_kind_name(kind: RenderPassKind) -> &'static str {
    match kind {
        RenderPassKind::SceneCapture => "SceneCapture",
        RenderPassKind::SurfaceCapture => "SurfaceCapture",
        RenderPassKind::NormalizeInput => "NormalizeInput",
        RenderPassKind::DualKawaseDownsample => "DualKawaseDownsample",
        RenderPassKind::DualKawaseUpsample => "DualKawaseUpsample",
        RenderPassKind::Fragment => "Fragment",
        RenderPassKind::Blend => "Blend",
        RenderPassKind::Mask => "Mask",
        RenderPassKind::Composite => "Composite",
        RenderPassKind::OutputPostProcess => "OutputPostProcess",
    }
}

fn graph_texture_trace(
    graph: &CompiledFrameGraph,
    id: GraphTextureId,
    resources: &HashMap<GraphTextureId, PooledEffectTexture>,
) -> Option<String> {
    let texture = graph.textures.iter().find(|texture| texture.id == id)?;
    let source = match texture.source {
        GraphTextureSource::Output => "output",
        GraphTextureSource::CapturedScene => "captured_scene",
        GraphTextureSource::CapturedTarget => "captured_target",
        GraphTextureSource::Intermediate => "intermediate",
        GraphTextureSource::Static(_) => "static",
    };
    let physical = match texture.source {
        GraphTextureSource::Output => "output".to_owned(),
        _ => resources
            .get(&id)
            .map_or_else(|| "unrealized".to_owned(), |texture| texture.id.to_string()),
    };
    let origin = match texture.origin {
        oblivion_one::effects::GraphTextureOrigin::BottomLeft => "bottom_left",
    };
    Some(format!(
        "id={}:physical={physical}:source={source}:domain={},{} {}x{}:dims={}x{}:origin={origin}",
        id.get(),
        texture.domain.x,
        texture.domain.y,
        texture.domain.width,
        texture.domain.height,
        texture.width,
        texture.height,
    ))
}

fn optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
}

fn optional_u32_none(value: Option<u32>) -> String {
    value.map_or_else(|| "none".to_owned(), |value| value.to_string())
}

fn optional_usize(value: Option<usize>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::egl_renderer::damage::{FullRepaintReason, OutputDamage, OutputRect, RepaintMode};
    use oblivion_one::compositor::{EffectAnchor, EffectAnchorScope};
    use oblivion_one::effects::{
        EffectAlphaMode, EffectColorConversion, EffectInstanceId, EffectRegion, EffectWorkingSpace,
        GraphTextureOrigin, GraphTexturePlan, RenderGraphCompileStats, RenderPassKind,
    };

    #[test]
    fn trace_gate_requires_exact_one() {
        assert!(!TraceConfig::from_env(None).enabled());
        assert!(!TraceConfig::from_env(Some(OsStr::new(""))).enabled());
        assert!(!TraceConfig::from_env(Some(OsStr::new("0"))).enabled());
        assert!(!TraceConfig::from_env(Some(OsStr::new("true"))).enabled());
        assert!(TraceConfig::from_env(Some(OsStr::new("1"))).enabled());
    }

    #[test]
    fn debug_effect_modes_have_safe_defaults_and_reject_invalid_values() {
        let defaults = EffectDebugConfig::from_env_values(None, None);
        assert_eq!(defaults.capture_mode(), EffectDebugCaptureMode::Replay);
        assert_eq!(defaults.kawase_mode(), EffectDebugKawaseMode::Partial);
        assert_eq!(
            defaults.checkpoint_capture_path(),
            CheckpointCapturePath::FramebufferShaderCopy
        );

        let test_helper_defaults = EffectDebugConfig::new(
            EffectDebugCaptureMode::Replay,
            EffectDebugKawaseMode::Partial,
        );
        assert_eq!(
            test_helper_defaults.checkpoint_capture_path(),
            CheckpointCapturePath::FramebufferBlit,
            "the two-argument test helper retains historical blit semantics"
        );

        let framebuffer_full = EffectDebugConfig::from_env_values(
            Some(OsStr::new("framebuffer")),
            Some(OsStr::new("full")),
        );
        assert_eq!(
            framebuffer_full.capture_mode(),
            EffectDebugCaptureMode::Framebuffer
        );
        assert_eq!(framebuffer_full.kawase_mode(), EffectDebugKawaseMode::Full);

        let invalid =
            EffectDebugConfig::from_env_values(Some(OsStr::new("blit")), Some(OsStr::new("all")));
        assert_eq!(invalid.capture_mode(), EffectDebugCaptureMode::Replay);
        assert_eq!(invalid.kawase_mode(), EffectDebugKawaseMode::Partial);

        let shader_copy = EffectDebugConfig::from_env_values_with_checkpoint_capture_path(
            None,
            None,
            Some(OsStr::new("shader-copy")),
        );
        assert_eq!(
            shader_copy.checkpoint_capture_path(),
            CheckpointCapturePath::FramebufferShaderCopy
        );

        let blit = EffectDebugConfig::from_env_values_with_checkpoint_capture_path(
            None,
            None,
            Some(OsStr::new("blit")),
        );
        assert_eq!(
            blit.checkpoint_capture_path(),
            CheckpointCapturePath::FramebufferBlit
        );

        let invalid_checkpoint = EffectDebugConfig::from_env_values_with_checkpoint_capture_path(
            None,
            None,
            Some(OsStr::new("auto")),
        );
        assert_eq!(
            invalid_checkpoint.checkpoint_capture_path(),
            CheckpointCapturePath::FramebufferShaderCopy
        );
    }

    #[test]
    fn checkpoint_capture_path_overrides_are_independent_of_production_default() {
        assert_eq!(
            parse_checkpoint_capture_path(None),
            DEFAULT_CHECKPOINT_CAPTURE_PATH,
            "unset selects the production default"
        );
        assert_eq!(
            parse_checkpoint_capture_path(Some(OsStr::new("shader-copy"))),
            CheckpointCapturePath::FramebufferShaderCopy,
            "shader-copy keeps its explicit meaning if the production default changes"
        );
        assert_eq!(
            parse_checkpoint_capture_path(Some(OsStr::new("blit"))),
            CheckpointCapturePath::FramebufferBlit,
            "blit remains an explicit diagnostic override"
        );
        assert_eq!(
            parse_checkpoint_capture_path(Some(OsStr::new("invalid"))),
            DEFAULT_CHECKPOINT_CAPTURE_PATH,
            "invalid values warn and select the production default"
        );
    }

    #[test]
    fn disabled_trace_does_not_evaluate_event_formatter() {
        let trace = EffectExecutionTrace::disabled_for_test();
        let formatted = Cell::new(false);

        trace.event(|| {
            formatted.set(true);
            "must not be built".to_owned()
        });

        assert!(!formatted.get());
    }

    #[test]
    fn capture_gpu_timing_event_is_bounded_and_path_specific() {
        clear_test_events();
        let trace = EffectExecutionTrace::enabled_for_test();
        trace.capture_gpu_timing(
            Some(9),
            Some(7),
            Some(11),
            RenderPassKind::SceneCapture,
            "framebuffer_shader_copy",
            2,
            41_184,
            1_234,
        );
        let line = take_test_events().pop().expect("capture timing event");
        assert!(line.contains("event=effect_capture_gpu_timing"));
        assert!(line.contains("frame_id=9"));
        assert!(line.contains("pass=7"));
        assert!(line.contains("instance=11"));
        assert!(line.contains("kind=SceneCapture"));
        assert!(line.contains("capture_path=framebuffer_shader_copy"));
        assert!(line.contains("checkpoint_count=2"));
        assert!(line.contains("pixels=41184"));
        assert!(line.contains("duration_ns=1234"));
        assert!(!line.contains("region"));
    }

    #[test]
    fn pass_trace_line_is_bounded_and_source_free() {
        let line = format_pass_trace_line(PassTraceFields {
            boundary: "begin",
            pass_id: 7,
            instance_id: 11,
            kind: "DualKawaseDownsample",
            input_ids: &[
                GraphTextureId::new(2).unwrap(),
                GraphTextureId::new(3).unwrap(),
                GraphTextureId::new(4).unwrap(),
                GraphTextureId::new(5).unwrap(),
                GraphTextureId::new(6).unwrap(),
                GraphTextureId::new(7).unwrap(),
                GraphTextureId::new(8).unwrap(),
                GraphTextureId::new(9).unwrap(),
                GraphTextureId::new(10).unwrap(),
            ],
            output_id: Some(12),
            capture_mode: Some("replay"),
            capture_commands: Some(13),
            backdrop_capture_policy: Some("replay"),
            kawase_execution_policy: Some("partial"),
            scene_work_damage_rects: 2,
            scene_work_damage_bbox: Some((1, 2, 30, 40)),
        });

        assert!(line.contains("event=effect_pass_begin"));
        assert!(line.contains("pass=7"));
        assert!(line.contains("inputs=2,3,4,5,6,7,8"));
        assert!(line.contains("backdrop_capture_policy=replay"));
        assert!(line.contains("kawase_execution_policy=partial"));
        assert!(line.contains("scene_work_damage_rects=2"));
        assert!(line.contains("scene_work_damage_bbox=1,2,30,40"));
        assert!(!line.contains("shader"));
        assert!(!line.contains("scene_commands"));
    }

    #[test]
    fn unrealized_graph_textures_are_not_reported_as_output() {
        let output_id = GraphTextureId::new(1).expect("output texture id");
        let capture_id = GraphTextureId::new(2).expect("capture texture id");
        let domain =
            oblivion_one::effects::EffectRect::new(5, 6, 10, 11).expect("trace texture domain");
        let graph = CompiledFrameGraph {
            passes: Vec::new(),
            textures: vec![
                GraphTexturePlan {
                    id: output_id,
                    source: GraphTextureSource::Output,
                    width: 100,
                    height: 80,
                    domain,
                    working_space: EffectWorkingSpace::OutputEncodedSrgb,
                    origin: GraphTextureOrigin::BottomLeft,
                    first_use: None,
                    last_use: None,
                },
                GraphTexturePlan {
                    id: capture_id,
                    source: GraphTextureSource::CapturedScene,
                    width: 10,
                    height: 11,
                    domain,
                    working_space: EffectWorkingSpace::OutputEncodedSrgb,
                    origin: GraphTextureOrigin::BottomLeft,
                    first_use: None,
                    last_use: None,
                },
            ],
            instances: Vec::new(),
            final_damage: EffectRegion::empty(),
            stats: RenderGraphCompileStats::default(),
        };
        let resources = HashMap::new();

        let output_line = graph_texture_trace(&graph, output_id, &resources).unwrap();
        let capture_line = graph_texture_trace(&graph, capture_id, &resources).unwrap();

        assert!(output_line.contains("physical=output:source=output"));
        assert!(capture_line.contains("physical=unrealized:source=captured_scene"));
        assert!(!capture_line.contains("physical=output"));
    }

    #[test]
    fn demand_plan_trace_line_includes_bounded_planner_stats() {
        let trace = EffectExecutionTrace::enabled_for_test();
        let summary = FrameTraceSummary {
            demand_plan: Some(oblivion_one::effects::EffectDemandPlanStats {
                repair_rect_count: 71,
                dependency_edge_count: 6,
                dependency_propagations: 6,
                max_instance_region_rect_count: 72,
                region_representation_overflows: 3,
                visible_clip_fallbacks: 2,
                work_region_bbox_coalesces: 1,
                conservative_full: false,
                ..Default::default()
            }),
            ..FrameTraceSummary::default()
        };

        clear_test_events();
        trace.frame_boundary("effect_demand_plan", "end", summary);
        let line = take_test_events().pop().expect("demand trace event");

        assert!(line.contains("repair_rect_count=71"));
        assert!(line.contains("dependency_edge_count=6"));
        assert!(line.contains("dependency_propagations=6"));
        assert!(line.contains("max_instance_region_rect_count=72"));
        assert!(line.contains("region_representation_overflows=3"));
        assert!(line.contains("visible_clip_fallbacks=2"));
        assert!(line.contains("work_region_bbox_coalesces=1"));
        assert!(line.contains("conservative_full=false"));
    }

    #[test]
    fn visible_clip_fallback_trace_identifies_authoritative_output() {
        let trace = EffectExecutionTrace::enabled_for_test();
        let pass = CompiledRenderPass {
            id: oblivion_one::effects::GraphPassId::new(9).unwrap(),
            kind: RenderPassKind::Composite,
            inputs: Vec::new(),
            output: None,
            damage: EffectRegion::empty(),
            instance: EffectInstanceId::new(4).unwrap(),
            anchor: EffectAnchor::OutputPostProcess,
            blur_radius: None,
            stage: None,
            fused_stages: Vec::new(),
            parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
            alpha_mode: EffectAlphaMode::Preserve,
            encode_output: false,
            color_conversion: EffectColorConversion::None,
            checkpoint_dependencies: Vec::new(),
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            visible_clip_fallback: None,
        };

        clear_test_events();
        trace.visible_clip_fallback(&pass, 970, 33);
        let line = take_test_events()
            .pop()
            .expect("visible clip fallback event");

        assert!(line.contains("event=region_representation_overflow"));
        assert!(line.contains("frame_id=unknown"));
        assert!(line.contains("instance=4"));
        assert!(line.contains("pass=9"));
        assert!(line.contains("input_rect_count=970"));
        assert!(line.contains("clip_rect_count=33"));
        assert!(line.contains("fallback=output_influence"));
        assert!(line.contains("visible_clip_fallback=true"));
    }

    #[test]
    fn execution_region_trace_reports_single_coverage_diagnostics() {
        let trace = EffectExecutionTrace::enabled_for_test();
        let pass = CompiledRenderPass {
            id: oblivion_one::effects::GraphPassId::new(10).unwrap(),
            kind: RenderPassKind::DualKawaseDownsample,
            inputs: Vec::new(),
            output: None,
            damage: EffectRegion::empty(),
            instance: EffectInstanceId::new(5).unwrap(),
            anchor: EffectAnchor::OutputPostProcess,
            blur_radius: None,
            stage: None,
            fused_stages: Vec::new(),
            parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
            alpha_mode: EffectAlphaMode::Preserve,
            encode_output: false,
            color_conversion: EffectColorConversion::None,
            checkpoint_dependencies: Vec::new(),
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            visible_clip_fallback: None,
        };

        clear_test_events();
        trace.execution_region(&pass, 130, 2, 1, 4, Some("work_region_bbox_coalesce"));
        let line = take_test_events()
            .pop()
            .expect("execution region diagnostic event");

        assert!(line.contains("event=effect_execution_region"));
        assert!(line.contains("instance=5"));
        assert!(line.contains("pass=10"));
        assert!(line.contains("input_rect_count=130"));
        assert!(line.contains("execution_region_rect_count=2"));
        assert!(line.contains("duplicate_rects_removed=1"));
        assert!(line.contains("overlap_fragments_generated=4"));
        assert!(line.contains("fallback=work_region_bbox_coalesce"));
    }

    #[test]
    fn capture_materialization_trace_reports_logical_and_physical_work() {
        let trace = EffectExecutionTrace::enabled_for_test();
        let pass = CompiledRenderPass {
            id: oblivion_one::effects::GraphPassId::new(11).unwrap(),
            kind: RenderPassKind::SceneCapture,
            inputs: Vec::new(),
            output: Some(GraphTextureId::new(12).unwrap()),
            damage: EffectRegion::empty(),
            instance: EffectInstanceId::new(6).unwrap(),
            anchor: EffectAnchor::OutputPostProcess,
            blur_radius: None,
            stage: None,
            fused_stages: Vec::new(),
            parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
            alpha_mode: EffectAlphaMode::Preserve,
            encode_output: false,
            color_conversion: EffectColorConversion::None,
            checkpoint_dependencies: Vec::new(),
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            visible_clip_fallback: None,
        };

        clear_test_events();
        trace.capture_materialization(&pass, 2, Some((10, 20, 40, 30)), 2, 1_200);
        let line = take_test_events()
            .pop()
            .expect("capture materialization trace event");

        assert!(line.contains("event=effect_capture_materialization"));
        assert!(line.contains("instance=6"));
        assert!(line.contains("pass=11"));
        assert!(line.contains("kind=SceneCapture"));
        assert!(line.contains("logical_rects=2"));
        assert!(line.contains("logical_bbox=10,20,40,30"));
        assert!(line.contains("physical_rects=2"));
        assert!(line.contains("physical_pixels=1200"));
    }

    #[test]
    fn scene_work_preservation_trace_reports_bounded_transfer_metrics() {
        let trace = EffectExecutionTrace::enabled_for_test();

        clear_test_events();
        trace.scene_work_preservation("capture", 2, 48_984, 2_073_600);
        let line = take_test_events()
            .pop()
            .expect("scene-work preservation trace event");

        assert!(line.contains("event=effect_scene_work_preservation"));
        assert!(line.contains("phase=capture"));
        assert!(line.contains("rect_count=2"));
        assert!(line.contains("pixels=48984"));
        assert!(line.contains("output_pixels=2073600"));
        assert!(!line.contains("rects="));
    }

    #[test]
    fn scene_replay_trace_reports_bounded_work_metrics() {
        let trace = EffectExecutionTrace::enabled_for_test();
        let pass = CompiledRenderPass {
            id: oblivion_one::effects::GraphPassId::new(13).unwrap(),
            kind: RenderPassKind::SceneCapture,
            inputs: Vec::new(),
            output: None,
            damage: EffectRegion::empty(),
            instance: EffectInstanceId::new(7).unwrap(),
            anchor: EffectAnchor::OutputPostProcess,
            blur_radius: None,
            stage: None,
            fused_stages: Vec::new(),
            parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
            alpha_mode: EffectAlphaMode::Preserve,
            encode_output: false,
            color_conversion: EffectColorConversion::None,
            checkpoint_dependencies: Vec::new(),
            visual_group: None,
            anchor_scope: EffectAnchorScope::VisualGroup,
            visible_clip_fallback: None,
        };

        clear_test_events();
        trace.scene_replay_boundary(
            "begin",
            &pass,
            "checkpoint_dependency",
            2,
            5,
            2,
            400,
            4,
            1_000,
            1,
        );
        let line = take_test_events().pop().expect("scene replay trace event");

        assert!(line.contains("active_work_rects=2"));
        assert!(line.contains("active_work_pixels=400"));
        assert!(line.contains("baseline_work_rects=4"));
        assert!(line.contains("baseline_work_pixels=1000"));
        assert!(line.contains("saved_pixels=600"));
        assert!(line.contains("pending_checkpoint_requirements=1"));
        assert!(!line.contains("active_work="));
        assert!(!line.contains("baseline_work="));
    }

    fn damage_snapshot(kind: DamageTraceKind) -> DamageTraceSnapshot {
        DamageTraceSnapshot {
            kind,
            rects: if kind == DamageTraceKind::Full { 1 } else { 0 },
            pixels: 0,
        }
    }

    fn repaint_snapshot(
        mode: RepaintMode,
        fallback_reason: Option<FullRepaintReason>,
    ) -> RepaintPlanTraceSnapshot {
        RepaintPlanTraceSnapshot {
            mode,
            fallback_reason,
            complexity_policy: PartialRepaintComplexityPolicy::Legacy,
            complexity_action: PartialRepaintComplexityAction::NotApplicable,
            buffer_age: None,
            render_damage: damage_snapshot(DamageTraceKind::Rects),
            repair_damage: damage_snapshot(DamageTraceKind::Rects),
        }
    }

    fn provenance(
        input: DamageTraceSnapshot,
        scene: DamageTraceSnapshot,
        merged: DamageTraceSnapshot,
        initial: RepaintPlanTraceSnapshot,
        final_plan: RepaintPlanTraceSnapshot,
    ) -> EffectRepaintProvenanceSnapshot {
        EffectRepaintProvenanceSnapshot::new(
            input,
            scene,
            merged,
            initial,
            final_plan,
            DamageComplexityShadow::not_applicable(),
        )
    }

    #[test]
    fn repaint_provenance_damage_snapshots_use_stable_names_counts_and_pixels() {
        let absent = DamageTraceSnapshot::from_optional(None, 10, 20);
        let empty = DamageTraceSnapshot::from_damage(&OutputDamage::Empty, 10, 20);
        let rects = DamageTraceSnapshot::from_damage(
            &OutputDamage::Rects(vec![OutputRect::new(2, 3, 4, 5)]),
            10,
            20,
        );
        let full = DamageTraceSnapshot::from_damage(&OutputDamage::Full, 10, 20);

        assert_eq!(
            (absent.kind.as_str(), absent.rects, absent.pixels),
            ("none", 0, 0)
        );
        assert_eq!(
            (empty.kind.as_str(), empty.rects, empty.pixels),
            ("empty", 0, 0)
        );
        assert_eq!(
            (rects.kind.as_str(), rects.rects, rects.pixels),
            ("rects", 1, 20)
        );
        assert_eq!(
            (full.kind.as_str(), full.rects, full.pixels),
            ("full", 1, 200)
        );

        let overflowing = OutputDamage::Rects(vec![
            OutputRect::new(0, 0, u32::MAX, u32::MAX),
            OutputRect::new(0, 0, u32::MAX, u32::MAX),
        ]);
        let saturated = DamageTraceSnapshot::from_damage(&overflowing, u32::MAX, u32::MAX);
        assert_eq!(saturated.pixels, u64::MAX);
    }

    #[test]
    fn repaint_provenance_first_full_stage_prefers_input_damage() {
        let full = damage_snapshot(DamageTraceKind::Full);
        let full_plan = repaint_snapshot(RepaintMode::Full, None);
        let evidence = provenance(full, full, full, full_plan, full_plan);

        assert_eq!(evidence.first_full_stage().as_str(), "input_damage");
    }

    #[test]
    fn repaint_provenance_first_full_stage_detects_scene_resolution() {
        let rects = damage_snapshot(DamageTraceKind::Rects);
        let full = damage_snapshot(DamageTraceKind::Full);
        let full_plan = repaint_snapshot(RepaintMode::Full, None);
        let evidence = provenance(rects, full, full, full_plan, full_plan);

        assert_eq!(evidence.first_full_stage().as_str(), "scene_damage");
    }

    #[test]
    fn repaint_provenance_first_full_stage_detects_effect_damage_merge() {
        let rects = damage_snapshot(DamageTraceKind::Rects);
        let full = damage_snapshot(DamageTraceKind::Full);
        let full_plan = repaint_snapshot(RepaintMode::Full, None);
        let evidence = provenance(rects, rects, full, full_plan, full_plan);

        assert_eq!(evidence.first_full_stage().as_str(), "effect_damage_merge");
    }

    #[test]
    fn repaint_provenance_initial_planner_fallback_reason_is_stable() {
        let rects = damage_snapshot(DamageTraceKind::Rects);
        let initial = repaint_snapshot(
            RepaintMode::Full,
            Some(FullRepaintReason::DamageAreaThreshold),
        );
        let final_plan =
            repaint_snapshot(RepaintMode::Full, Some(FullRepaintReason::BufferAgeZero));
        let evidence = provenance(rects, rects, rects, initial, final_plan);

        assert_eq!(evidence.first_full_stage().as_str(), "initial_repaint_plan");
        assert!(!evidence.promoted_to_full());
        let line = evidence.format_line(Some(7));
        assert!(line.contains("initial_repaint_reason=damage_area_threshold"));
        assert!(line.contains("final_repaint_reason=buffer_age_zero"));

        let buffer_age_fallback = provenance(
            rects,
            rects,
            rects,
            repaint_snapshot(RepaintMode::Full, Some(FullRepaintReason::BufferAgeZero)),
            repaint_snapshot(RepaintMode::Full, None),
        );
        assert!(
            buffer_age_fallback
                .format_line(Some(7))
                .contains("initial_repaint_reason=buffer_age_zero")
        );
    }

    #[test]
    fn repaint_provenance_effect_execution_promotes_full_with_stable_reason() {
        let rects = damage_snapshot(DamageTraceKind::Rects);
        let initial = repaint_snapshot(RepaintMode::Partial, None);
        let final_plan = repaint_snapshot(
            RepaintMode::Full,
            Some(FullRepaintReason::EffectExecutionConservative),
        );
        let evidence = provenance(rects, rects, rects, initial, final_plan);

        assert_eq!(evidence.first_full_stage().as_str(), "effect_execution");
        assert!(evidence.promoted_to_full());
        let line = evidence.format_line(Some(8));
        assert!(line.contains("final_repaint_reason=effect_execution_conservative"));

        let threshold = provenance(
            rects,
            rects,
            rects,
            initial,
            repaint_snapshot(
                RepaintMode::Full,
                Some(FullRepaintReason::DamageAreaThreshold),
            ),
        );
        assert_eq!(threshold.first_full_stage().as_str(), "effect_execution");
        assert!(
            threshold
                .format_line(Some(9))
                .contains("final_repaint_reason=damage_area_threshold")
        );
    }

    #[test]
    fn repaint_provenance_remains_regional_when_no_stage_is_full() {
        let rects = damage_snapshot(DamageTraceKind::Rects);
        let plan = repaint_snapshot(RepaintMode::Partial, None);
        let evidence = provenance(rects, rects, rects, plan, plan);

        assert_eq!(evidence.first_full_stage().as_str(), "none");
        assert!(!evidence.promoted_to_full());
    }

    #[test]
    fn repaint_provenance_formatter_emits_each_contract_key_once() {
        let rects = damage_snapshot(DamageTraceKind::Rects);
        let empty = damage_snapshot(DamageTraceKind::Empty);
        let initial = repaint_snapshot(RepaintMode::Partial, None);
        let mut final_plan = repaint_snapshot(
            RepaintMode::Full,
            Some(FullRepaintReason::EffectExecutionConservative),
        );
        final_plan.buffer_age = Some(3);
        final_plan.complexity_policy = PartialRepaintComplexityPolicy::StructuredExperimental;
        final_plan.complexity_action = PartialRepaintComplexityAction::StructuredManyRectangles;
        let evidence = provenance(rects, empty, rects, initial, final_plan);
        let line = evidence.format_line(Some(42));
        let keys = [
            "event",
            "frame_id",
            "input_damage_kind",
            "input_damage_rects",
            "input_damage_pixels",
            "scene_damage_kind",
            "scene_damage_rects",
            "scene_damage_pixels",
            "merged_damage_kind",
            "merged_damage_rects",
            "merged_damage_pixels",
            "initial_repaint_mode",
            "initial_repaint_reason",
            "initial_buffer_age",
            "initial_render_damage_kind",
            "initial_render_damage_rects",
            "initial_render_damage_pixels",
            "initial_repair_damage_kind",
            "initial_repair_damage_rects",
            "initial_repair_damage_pixels",
            "final_repaint_mode",
            "final_repaint_reason",
            "final_buffer_age",
            "final_render_damage_kind",
            "final_render_damage_rects",
            "final_render_damage_pixels",
            "final_repair_damage_kind",
            "final_repair_damage_rects",
            "final_repair_damage_pixels",
            "first_full_stage",
            "promoted_to_full",
            "partial_repaint_complexity_policy",
            "partial_repaint_complexity_action",
            "damage_complexity_shadow_applicable",
            "damage_complexity_original_rects",
            "damage_complexity_original_pixels",
            "damage_complexity_bbox_x",
            "damage_complexity_bbox_y",
            "damage_complexity_bbox_width",
            "damage_complexity_bbox_height",
            "damage_complexity_bbox_pixels",
            "damage_complexity_bbox_accepted",
            "damage_complexity_candidate_rects",
            "damage_complexity_candidate_pixels",
            "damage_complexity_added_pixels",
            "damage_complexity_outcome",
            "damage_complexity_would_avoid_full",
        ];

        for key in keys {
            assert_eq!(
                line.split_whitespace()
                    .filter(|field| field.starts_with(&format!("{key}=")))
                    .count(),
                1,
                "key {key} must appear exactly once in {line}"
            );
        }
        assert!(line.starts_with("event=effect_repaint_provenance frame_id=42 "));
        assert!(line.contains("input_damage_kind=rects"));
        assert!(line.contains("scene_damage_kind=empty"));
        assert!(line.contains("initial_repaint_mode=partial"));
        assert!(line.contains("initial_repaint_reason=none"));
        assert!(line.contains("initial_buffer_age=none"));
        assert!(line.contains("final_repaint_mode=full"));
        assert!(line.contains("final_repaint_reason=effect_execution_conservative"));
        assert!(line.contains("final_buffer_age=3"));
        assert!(line.contains("first_full_stage=effect_execution"));
        assert!(line.contains("promoted_to_full=1"));
        assert!(line.contains("partial_repaint_complexity_policy=structured-experimental"));
        assert!(line.contains("partial_repaint_complexity_action=structured_many_rects"));
        assert!(line.contains("damage_complexity_shadow_applicable=0"));
        assert!(line.contains("damage_complexity_original_rects=0"));
        assert!(line.contains("damage_complexity_original_pixels=0"));
        assert!(line.contains("damage_complexity_bbox_x=0"));
        assert!(line.contains("damage_complexity_bbox_y=0"));
        assert!(line.contains("damage_complexity_bbox_width=0"));
        assert!(line.contains("damage_complexity_bbox_height=0"));
        assert!(line.contains("damage_complexity_bbox_pixels=0"));
        assert!(line.contains("damage_complexity_bbox_accepted=0"));
        assert!(line.contains("damage_complexity_candidate_rects=0"));
        assert!(line.contains("damage_complexity_candidate_pixels=0"));
        assert!(line.contains("damage_complexity_added_pixels=0"));
        assert!(line.contains("damage_complexity_outcome=not_applicable"));
        assert!(line.contains("damage_complexity_would_avoid_full=0"));
        assert!(!line.contains("Some("));
        assert!(!line.contains("FullRepaintReason"));
        assert!(!line.contains("DamageTraceKind"));
        assert!(!line.contains("DamageComplexityShadow"));
    }

    #[test]
    fn repaint_provenance_formatter_emits_shadow_values_with_stable_names() {
        let rects = damage_snapshot(DamageTraceKind::Rects);
        let plan = repaint_snapshot(
            RepaintMode::Full,
            Some(FullRepaintReason::TooManyRectangles),
        );
        let candidate = OutputDamage::Rects(
            (0..9)
                .map(|index| crate::egl_renderer::damage::OutputRect::new(index * 11, 0, 10, 10))
                .collect(),
        );
        let shadow = DamageComplexityShadow::for_candidate(
            &candidate,
            (100, 100),
            Some(FullRepaintReason::TooManyRectangles),
        );
        let mut evidence = provenance(rects, rects, rects, plan, plan);
        evidence.damage_complexity_shadow = shadow;
        let line = evidence.format_line(Some(43));

        assert!(line.contains("damage_complexity_shadow_applicable=1"));
        assert!(line.contains("damage_complexity_original_rects=9"));
        assert!(line.contains("damage_complexity_original_pixels=900"));
        assert!(line.contains("damage_complexity_bbox_x=0"));
        assert!(line.contains("damage_complexity_bbox_y=0"));
        assert!(line.contains("damage_complexity_bbox_width=98"));
        assert!(line.contains("damage_complexity_bbox_height=10"));
        assert!(line.contains("damage_complexity_bbox_pixels=980"));
        assert!(line.contains("damage_complexity_bbox_accepted=1"));
        assert!(line.contains("damage_complexity_candidate_rects=1"));
        assert!(line.contains("damage_complexity_candidate_pixels=980"));
        assert!(line.contains("damage_complexity_added_pixels=80"));
        assert!(line.contains("damage_complexity_outcome=partial_bbox"));
        assert!(line.contains("damage_complexity_would_avoid_full=1"));
        assert!(!line.contains("PartialBoundingBox"));
    }

    #[test]
    fn repaint_provenance_callback_is_not_run_when_trace_is_disabled() {
        let trace = EffectExecutionTrace::disabled_for_test();
        let callback_count = Cell::new(0);

        clear_test_events();
        trace.effect_repaint_provenance(|| {
            callback_count.set(callback_count.get() + 1);
            let damage = damage_snapshot(DamageTraceKind::Rects);
            let plan = repaint_snapshot(RepaintMode::Partial, None);
            provenance(damage, damage, damage, plan, plan)
        });

        assert_eq!(callback_count.get(), 0);
        assert!(take_test_events().is_empty());
    }

    #[test]
    fn repaint_provenance_trace_emits_one_event_for_a_resolved_snapshot() {
        let trace = EffectExecutionTrace::enabled_for_test();
        let damage = damage_snapshot(DamageTraceKind::Rects);
        let plan = repaint_snapshot(RepaintMode::Partial, None);

        clear_test_events();
        trace.effect_repaint_provenance(|| provenance(damage, damage, damage, plan, plan));
        let events = take_test_events();

        assert_eq!(events.len(), 1);
        assert!(events[0].starts_with("event=effect_repaint_provenance frame_id=unknown "));
    }
}
