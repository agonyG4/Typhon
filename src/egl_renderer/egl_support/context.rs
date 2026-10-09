use std::{error::Error, io};

use khronos_egl as egl;

use super::super::{EglInstance, GlEglImageTargetTexture2DOes, RendererResult};

// Attribute and value tokens defined by the EGL_IMG_context_priority extension.
pub(in crate::egl_renderer) const EGL_CONTEXT_PRIORITY_LEVEL_IMG: egl::Int = 0x3100;
pub(in crate::egl_renderer) const EGL_CONTEXT_PRIORITY_HIGH_IMG: egl::Int = 0x3101;
pub(in crate::egl_renderer) const EGL_CONTEXT_PRIORITY_MEDIUM_IMG: egl::Int = 0x3102;
pub(in crate::egl_renderer) const EGL_CONTEXT_PRIORITY_LOW_IMG: egl::Int = 0x3103;
const EGL_CONTEXT_PRIORITY_EXTENSION: &str = "EGL_IMG_context_priority";
const EGL_CONTEXT_PRIORITY_ENV: &str = "TYPHON_EGL_CONTEXT_PRIORITY";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EglContextPriorityPolicy {
    Auto,
    High,
    Medium,
}

impl EglContextPriorityPolicy {
    pub(in crate::egl_renderer) fn parse(value: Option<&str>) -> ParsedEglContextPriorityPolicy {
        let policy = match value.map(str::trim) {
            None => return ParsedEglContextPriorityPolicy::valid(Self::Auto),
            Some(value) if value.eq_ignore_ascii_case("auto") => Self::Auto,
            Some(value) if value.eq_ignore_ascii_case("high") => Self::High,
            Some(value) if value.eq_ignore_ascii_case("medium") => Self::Medium,
            Some(_) => {
                return ParsedEglContextPriorityPolicy {
                    policy: Self::Auto,
                    unknown_value: true,
                };
            }
        };
        ParsedEglContextPriorityPolicy::valid(policy)
    }

    pub(crate) fn from_environment() -> Self {
        let parsed = match std::env::var_os(EGL_CONTEXT_PRIORITY_ENV) {
            None => Self::parse(None),
            Some(value) => match value.to_str() {
                Some(value) => Self::parse(Some(value)),
                None => ParsedEglContextPriorityPolicy {
                    policy: Self::Auto,
                    unknown_value: true,
                },
            },
        };
        if parsed.unknown_value {
            eprintln!(
                "typhon egl: unrecognized {EGL_CONTEXT_PRIORITY_ENV}; using auto context priority"
            );
        }
        parsed.policy
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::High => "high",
            Self::Medium => "medium",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::egl_renderer) struct ParsedEglContextPriorityPolicy {
    pub(in crate::egl_renderer) policy: EglContextPriorityPolicy,
    pub(in crate::egl_renderer) unknown_value: bool,
}

impl ParsedEglContextPriorityPolicy {
    fn valid(policy: EglContextPriorityPolicy) -> Self {
        Self {
            policy,
            unknown_value: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::egl_renderer) enum EglPriorityExtensionSupport {
    Available,
    Unsupported,
    QueryFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::egl_renderer) enum EglContextCreationCandidate {
    High,
    Default,
}

impl EglContextCreationCandidate {
    pub(in crate::egl_renderer) fn attributes(self) -> &'static [egl::Int] {
        const HIGH_ATTRIBUTES: &[egl::Int] = &[
            egl::CONTEXT_CLIENT_VERSION,
            3,
            EGL_CONTEXT_PRIORITY_LEVEL_IMG,
            EGL_CONTEXT_PRIORITY_HIGH_IMG,
            egl::NONE,
        ];

        match self {
            Self::High => HIGH_ATTRIBUTES,
            Self::Default => gles_context_attributes(),
        }
    }
}

pub(in crate::egl_renderer) fn has_egl_img_context_priority(extensions: &str) -> bool {
    extensions
        .split_ascii_whitespace()
        .any(|extension| extension == EGL_CONTEXT_PRIORITY_EXTENSION)
}

pub(in crate::egl_renderer) fn plan_gles_context_candidates(
    policy: EglContextPriorityPolicy,
    extension_support: EglPriorityExtensionSupport,
) -> Vec<EglContextCreationCandidate> {
    let request_high = matches!(
        policy,
        EglContextPriorityPolicy::Auto | EglContextPriorityPolicy::High
    ) && extension_support == EglPriorityExtensionSupport::Available;
    if request_high {
        vec![
            EglContextCreationCandidate::High,
            EglContextCreationCandidate::Default,
        ]
    } else {
        vec![EglContextCreationCandidate::Default]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EglContextPriorityLevel {
    High,
    Medium,
    Low,
    Unknown,
}

impl EglContextPriorityLevel {
    pub(in crate::egl_renderer) fn from_img_value(value: egl::Int) -> Self {
        match value {
            EGL_CONTEXT_PRIORITY_HIGH_IMG => Self::High,
            EGL_CONTEXT_PRIORITY_MEDIUM_IMG => Self::Medium,
            EGL_CONTEXT_PRIORITY_LOW_IMG => Self::Low,
            _ => Self::Unknown,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::egl_renderer) struct ContextCreationAttempts {
    pub(in crate::egl_renderer) high_attempted: bool,
    pub(in crate::egl_renderer) high_creation_failed: bool,
    pub(in crate::egl_renderer) fallback_used: bool,
    pub(in crate::egl_renderer) requested_priority: EglContextPriorityLevel,
}

pub(in crate::egl_renderer) fn create_context_from_candidates<T, E>(
    candidates: &[EglContextCreationCandidate],
    mut create: impl FnMut(EglContextCreationCandidate) -> Result<T, E>,
) -> Result<(T, ContextCreationAttempts), Vec<E>> {
    let high_attempted = candidates.contains(&EglContextCreationCandidate::High);
    let mut high_creation_failed = false;
    let mut failures = Vec::new();

    for candidate in candidates {
        match create(*candidate) {
            Ok(context) => {
                return Ok((
                    context,
                    ContextCreationAttempts {
                        high_attempted,
                        high_creation_failed,
                        fallback_used: *candidate == EglContextCreationCandidate::Default
                            && high_creation_failed,
                        requested_priority: if high_attempted {
                            EglContextPriorityLevel::High
                        } else {
                            EglContextPriorityLevel::Medium
                        },
                    },
                ));
            }
            Err(error) => {
                if *candidate == EglContextCreationCandidate::High {
                    high_creation_failed = true;
                }
                failures.push(error);
            }
        }
    }

    Err(failures)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AssignedEglContextPriority {
    pub(crate) priority: EglContextPriorityLevel,
    pub(crate) assigned_known: bool,
    pub(crate) query_failed: bool,
}

pub(in crate::egl_renderer) fn retain_context_when_priority_query_fails<T, E>(
    context: T,
    query: impl FnOnce() -> Result<egl::Int, E>,
) -> (T, AssignedEglContextPriority) {
    let assignment = match query() {
        Ok(value) => {
            let priority = EglContextPriorityLevel::from_img_value(value);
            AssignedEglContextPriority {
                priority,
                assigned_known: priority != EglContextPriorityLevel::Unknown,
                query_failed: false,
            }
        }
        Err(_) => AssignedEglContextPriority {
            priority: EglContextPriorityLevel::Unknown,
            assigned_known: false,
            query_failed: true,
        },
    };
    (context, assignment)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EglContextPriorityReport {
    pub(crate) policy: EglContextPriorityPolicy,
    pub(crate) extension_available: bool,
    pub(crate) extension_query_failed: bool,
    pub(crate) high_attempted: bool,
    pub(crate) high_creation_failed: bool,
    pub(crate) fallback_used: bool,
    pub(crate) requested_priority: EglContextPriorityLevel,
    pub(crate) assigned_priority: EglContextPriorityLevel,
    pub(crate) assigned_priority_known: bool,
    pub(crate) priority_query_failed: bool,
}

impl EglContextPriorityReport {
    pub(crate) fn log_startup(self, backend: &str) {
        println!(
            "typhon egl: event=context_priority backend={backend} policy={} extension_available={} extension_query_failed={} requested={} high_attempted={} high_creation_failed={} fallback_used={} assigned={} assigned_known={} priority_query_failed={}",
            self.policy.as_str(),
            self.extension_available,
            self.extension_query_failed,
            self.requested_priority.as_str(),
            self.high_attempted,
            self.high_creation_failed,
            self.fallback_used,
            self.assigned_priority.as_str(),
            self.assigned_priority_known,
            self.priority_query_failed,
        );
    }
}

pub(crate) struct CreatedGlesContext {
    pub(crate) context: egl::Context,
    pub(crate) priority: EglContextPriorityReport,
}

#[allow(dead_code)] // Retained for the default-priority GLES test contexts.
pub(crate) fn create_gles_context(
    egl: &EglInstance,
    display: egl::Display,
    config: egl::Config,
) -> RendererResult<egl::Context> {
    egl.create_context(display, config, None, gles_context_attributes())
        .map_err(|error| io::Error::other(format_gles3_context_error(&error)).into())
}

pub(crate) fn create_compositor_gles_context(
    egl: &EglInstance,
    display: egl::Display,
    config: egl::Config,
    policy: EglContextPriorityPolicy,
) -> RendererResult<CreatedGlesContext> {
    let extension_support = match egl.query_string(Some(display), egl::EXTENSIONS) {
        Ok(extensions) if has_egl_img_context_priority(&extensions.to_string_lossy()) => {
            EglPriorityExtensionSupport::Available
        }
        Ok(_) => EglPriorityExtensionSupport::Unsupported,
        Err(_) => EglPriorityExtensionSupport::QueryFailed,
    };
    let candidates = plan_gles_context_candidates(policy, extension_support);
    let (context, attempts) = create_context_from_candidates(&candidates, |candidate| {
        egl.create_context(display, config, None, candidate.attributes())
    })
    .map_err(|errors| io::Error::other(format_gles3_context_attempt_errors(&errors)))?;

    let (context, assignment) = if extension_support == EglPriorityExtensionSupport::Available {
        retain_context_when_priority_query_fails(context, || {
            egl.query_context(display, context, EGL_CONTEXT_PRIORITY_LEVEL_IMG)
        })
    } else {
        (
            context,
            AssignedEglContextPriority {
                priority: EglContextPriorityLevel::Unknown,
                assigned_known: false,
                query_failed: false,
            },
        )
    };

    Ok(CreatedGlesContext {
        context,
        priority: EglContextPriorityReport {
            policy,
            extension_available: extension_support == EglPriorityExtensionSupport::Available,
            extension_query_failed: extension_support == EglPriorityExtensionSupport::QueryFailed,
            high_attempted: attempts.high_attempted,
            high_creation_failed: attempts.high_creation_failed,
            fallback_used: attempts.fallback_used,
            requested_priority: attempts.requested_priority,
            assigned_priority: assignment.priority,
            assigned_priority_known: assignment.assigned_known,
            priority_query_failed: assignment.query_failed,
        },
    })
}

fn format_gles3_context_attempt_errors(errors: &[egl::Error]) -> String {
    match errors {
        [error] => format_gles3_context_error(error),
        [high_error, default_error] => format!(
            "failed to create required GLES3 context; HIGH priority attempt failed: {high_error}; default priority attempt failed: {default_error}"
        ),
        _ => format!(
            "failed to create required GLES3 context after {} attempts",
            errors.len()
        ),
    }
}

pub(in crate::egl_renderer) fn gles_context_attributes() -> &'static [egl::Int] {
    &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE]
}

pub(in crate::egl_renderer) fn format_gles3_context_error(error: &dyn Error) -> String {
    format!("failed to create required GLES3 context: {error}")
}

pub(crate) fn load_egl_image_target_texture_2d(
    egl: &EglInstance,
) -> Option<GlEglImageTargetTexture2DOes> {
    let symbol = egl.get_proc_address("glEGLImageTargetTexture2DOES")?;
    Some(unsafe {
        std::mem::transmute::<extern "system" fn(), GlEglImageTargetTexture2DOes>(symbol)
    })
}
