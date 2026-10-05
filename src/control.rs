//! Bounded version-one codec for the local Astrea control protocol.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CONTROL_PROTOCOL: &str = "astrea.control";
pub const CONTROL_VERSION: u32 = 1;
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub type ControlResult = Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCodecError {
    RequestTooLarge,
    ResponseTooLarge,
    MalformedJson,
    InvalidRequest,
    InvalidResponse,
    UnsupportedVersion(u32),
}

impl fmt::Display for ControlCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestTooLarge => write!(formatter, "control request exceeds 64 KiB"),
            Self::ResponseTooLarge => write!(formatter, "control response exceeds 1 MiB"),
            Self::MalformedJson => write!(formatter, "control message is not valid JSON"),
            Self::InvalidRequest => write!(formatter, "control request is invalid"),
            Self::InvalidResponse => write!(formatter, "control response is invalid"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported control protocol version {version}")
            }
        }
    }
}

impl Error for ControlCodecError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    Status,
    Version,
    Doctor,
    Outputs,
    OutputsConfigure,
    OutputsConfigureConfirm,
    OutputsConfigureRevert,
    Windows,
    ActiveWindow,
    Performance,
    KeyboardLayoutGet,
    KeyboardLayoutNext,
    KeyboardLayoutPrevious,
    KeyboardLayoutSet,
    KeyboardConfigurationGet,
    KeyboardConfigurationSet,
    CursorGet,
    CursorSetTheme,
    CursorSetSize,
    CursorSet,
    CursorReload,
    DecorationStatus,
    DecorationSetTheme,
    DecorationReload,
    DecorationList,
    EffectsReload,
    BlurStatus,
    BlurReload,
    AnimationConfigurationGet,
    AnimationConfigurationSet,
    MaterialConfigurationGet,
    MaterialConfigurationSet,
    MaterialProgramCatalogGet,
    MaterialProgramGet,
    MaterialProgramSet,
    MaterialProgramStateGet,
    MaterialProgramDescribe,
    MaterialProgramParametersSet,
    WindowActivate,
    WindowMinimize,
    WindowRestore,
    WindowClose,
    WindowDecorationPolicySet,
}

impl ControlCommand {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Version => "version",
            Self::Doctor => "doctor",
            Self::Outputs => "outputs",
            Self::OutputsConfigure => "outputs.configure",
            Self::OutputsConfigureConfirm => "outputs.configure.confirm",
            Self::OutputsConfigureRevert => "outputs.configure.revert",
            Self::Windows => "windows",
            Self::ActiveWindow => "active-window",
            Self::Performance => "performance",
            Self::KeyboardLayoutGet => "keyboard.layout.get",
            Self::KeyboardLayoutNext => "keyboard.layout.next",
            Self::KeyboardLayoutPrevious => "keyboard.layout.previous",
            Self::KeyboardLayoutSet => "keyboard.layout.set",
            Self::KeyboardConfigurationGet => "keyboard.config.get",
            Self::KeyboardConfigurationSet => "keyboard.config.set",
            Self::CursorGet => "cursor.get",
            Self::CursorSetTheme => "cursor.set-theme",
            Self::CursorSetSize => "cursor.set-size",
            Self::CursorSet => "cursor.set",
            Self::CursorReload => "cursor.reload",
            Self::DecorationStatus => "decoration.status",
            Self::DecorationSetTheme => "decoration.set-theme",
            Self::DecorationReload => "decoration.reload",
            Self::DecorationList => "decoration.list",
            Self::EffectsReload => "effects.reload",
            Self::BlurStatus => "blur.status",
            Self::BlurReload => "blur.reload",
            Self::AnimationConfigurationGet => "animation.config.get",
            Self::AnimationConfigurationSet => "animation.config.set",
            Self::MaterialConfigurationGet => "material.config.get",
            Self::MaterialConfigurationSet => "material.config.set",
            Self::MaterialProgramCatalogGet => "material.program.catalog.get",
            Self::MaterialProgramGet => "material.program.get",
            Self::MaterialProgramSet => "material.program.set",
            Self::MaterialProgramStateGet => "material.program.state.get",
            Self::MaterialProgramDescribe => "material.program.describe",
            Self::MaterialProgramParametersSet => "material.program.parameters.set",
            Self::WindowActivate => "window.activate",
            Self::WindowMinimize => "window.minimize",
            Self::WindowRestore => "window.restore",
            Self::WindowClose => "window.close",
            Self::WindowDecorationPolicySet => "window.decoration-policy.set",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "status" => Self::Status,
            "version" => Self::Version,
            "doctor" => Self::Doctor,
            "outputs" => Self::Outputs,
            "outputs.configure" => Self::OutputsConfigure,
            "outputs.configure.confirm" => Self::OutputsConfigureConfirm,
            "outputs.configure.revert" => Self::OutputsConfigureRevert,
            "windows" => Self::Windows,
            "active-window" => Self::ActiveWindow,
            "performance" => Self::Performance,
            "keyboard.layout.get" => Self::KeyboardLayoutGet,
            "keyboard.layout.next" => Self::KeyboardLayoutNext,
            "keyboard.layout.previous" => Self::KeyboardLayoutPrevious,
            "keyboard.layout.set" => Self::KeyboardLayoutSet,
            "keyboard.config.get" => Self::KeyboardConfigurationGet,
            "keyboard.config.set" => Self::KeyboardConfigurationSet,
            "cursor.get" => Self::CursorGet,
            "cursor.set-theme" => Self::CursorSetTheme,
            "cursor.set-size" => Self::CursorSetSize,
            "cursor.set" => Self::CursorSet,
            "cursor.reload" => Self::CursorReload,
            "decoration.status" => Self::DecorationStatus,
            "decoration.set-theme" => Self::DecorationSetTheme,
            "decoration.reload" => Self::DecorationReload,
            "decoration.list" => Self::DecorationList,
            "effects.reload" => Self::EffectsReload,
            "blur.status" => Self::BlurStatus,
            "blur.reload" => Self::BlurReload,
            "animation.config.get" => Self::AnimationConfigurationGet,
            "animation.config.set" => Self::AnimationConfigurationSet,
            "material.config.get" => Self::MaterialConfigurationGet,
            "material.config.set" => Self::MaterialConfigurationSet,
            "material.program.catalog.get" => Self::MaterialProgramCatalogGet,
            "material.program.get" => Self::MaterialProgramGet,
            "material.program.set" => Self::MaterialProgramSet,
            "material.program.state.get" => Self::MaterialProgramStateGet,
            "material.program.describe" => Self::MaterialProgramDescribe,
            "material.program.parameters.set" => Self::MaterialProgramParametersSet,
            "window.activate" => Self::WindowActivate,
            "window.minimize" => Self::WindowMinimize,
            "window.restore" => Self::WindowRestore,
            "window.close" => Self::WindowClose,
            "window.decoration-policy.set" => Self::WindowDecorationPolicySet,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlErrorCode {
    InvalidArgument,
    UnknownOutput,
    StaleOutputGeneration,
    UnknownOutputMode,
    UnsupportedOutputMode,
    UnsupportedOutputScale,
    UnsupportedOutputTransform,
    OutputTransactionActive,
    OutputTransactionNotFound,
    OutputTestFailed,
    OutputApplyFailed,
    OutputPersistFailed,
    OutputRollbackFailed,
    InvalidCommand,
    InvalidRequest,
    MalformedJson,
    RequestTooLarge,
    ResponseTooLarge,
    Unauthorized,
    UnsupportedVersion,
    Internal,
}

impl ControlErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::UnknownOutput => "unknown_output",
            Self::StaleOutputGeneration => "stale_output_generation",
            Self::UnknownOutputMode => "unknown_output_mode",
            Self::UnsupportedOutputMode => "unsupported_output_mode",
            Self::UnsupportedOutputScale => "unsupported_output_scale",
            Self::UnsupportedOutputTransform => "unsupported_output_transform",
            Self::OutputTransactionActive => "output_transaction_active",
            Self::OutputTransactionNotFound => "output_transaction_not_found",
            Self::OutputTestFailed => "output_test_failed",
            Self::OutputApplyFailed => "output_apply_failed",
            Self::OutputPersistFailed => "output_persist_failed",
            Self::OutputRollbackFailed => "output_rollback_failed",
            Self::InvalidCommand => "invalid_command",
            Self::InvalidRequest => "invalid_request",
            Self::MalformedJson => "malformed_json",
            Self::RequestTooLarge => "request_too_large",
            Self::ResponseTooLarge => "response_too_large",
            Self::Unauthorized => "unauthorized",
            Self::UnsupportedVersion => "unsupported_version",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlError {
    pub code: ControlErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ControlError {
    pub fn new(code: ControlErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    pub protocol: String,
    pub version: u32,
    pub id: u64,
    pub command: String,
    pub args: Value,
}

impl ControlRequest {
    pub fn new(
        id: u64,
        command: impl Into<String>,
        args: ControlResult,
    ) -> Result<Self, ControlCodecError> {
        if !args.is_object() {
            return Err(ControlCodecError::InvalidRequest);
        }
        let command = command.into();
        if command.trim().is_empty() {
            return Err(ControlCodecError::InvalidRequest);
        }
        Ok(Self {
            protocol: CONTROL_PROTOCOL.to_string(),
            version: CONTROL_VERSION,
            id,
            command,
            args,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlResponse {
    pub protocol: String,
    pub version: u32,
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ControlResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ControlError>,
}

impl ControlResponse {
    pub fn success(id: u64, result: ControlResult) -> Self {
        Self {
            protocol: CONTROL_PROTOCOL.to_string(),
            version: CONTROL_VERSION,
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: u64, error: ControlError) -> Self {
        Self {
            protocol: CONTROL_PROTOCOL.to_string(),
            version: CONTROL_VERSION,
            id,
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

pub fn encode_request(request: &ControlRequest) -> Result<Vec<u8>, ControlCodecError> {
    validate_request(request)?;
    let mut encoded = serde_json::to_vec(request).map_err(|_| ControlCodecError::InvalidRequest)?;
    encoded.push(b'\n');
    if encoded.len() > MAX_REQUEST_BYTES {
        return Err(ControlCodecError::RequestTooLarge);
    }
    Ok(encoded)
}

pub fn decode_request(bytes: &[u8]) -> Result<ControlRequest, ControlCodecError> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(ControlCodecError::RequestTooLarge);
    }
    let request = serde_json::from_slice(bytes).map_err(classify_request_decode_error)?;
    validate_request(&request)?;
    Ok(request)
}

pub fn encode_response(response: &ControlResponse) -> Result<Vec<u8>, ControlCodecError> {
    validate_response(response)?;
    let mut encoded =
        serde_json::to_vec(response).map_err(|_| ControlCodecError::InvalidResponse)?;
    encoded.push(b'\n');
    if encoded.len() > MAX_RESPONSE_BYTES {
        return Err(ControlCodecError::ResponseTooLarge);
    }
    Ok(encoded)
}

pub fn decode_response(bytes: &[u8]) -> Result<ControlResponse, ControlCodecError> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ControlCodecError::ResponseTooLarge);
    }
    let response = serde_json::from_slice(bytes).map_err(classify_response_decode_error)?;
    validate_response(&response)?;
    Ok(response)
}

fn classify_request_decode_error(error: serde_json::Error) -> ControlCodecError {
    if error.is_syntax() || error.is_eof() {
        ControlCodecError::MalformedJson
    } else {
        ControlCodecError::InvalidRequest
    }
}

fn classify_response_decode_error(error: serde_json::Error) -> ControlCodecError {
    if error.is_syntax() || error.is_eof() {
        ControlCodecError::MalformedJson
    } else {
        ControlCodecError::InvalidResponse
    }
}

fn validate_request(request: &ControlRequest) -> Result<(), ControlCodecError> {
    if request.protocol != CONTROL_PROTOCOL {
        return Err(ControlCodecError::InvalidRequest);
    }
    if request.version != CONTROL_VERSION {
        return Err(ControlCodecError::UnsupportedVersion(request.version));
    }
    if request.command.trim().is_empty() || !request.args.is_object() {
        return Err(ControlCodecError::InvalidRequest);
    }
    Ok(())
}

fn validate_response(response: &ControlResponse) -> Result<(), ControlCodecError> {
    if response.protocol != CONTROL_PROTOCOL {
        return Err(ControlCodecError::InvalidResponse);
    }
    if response.version != CONTROL_VERSION {
        return Err(ControlCodecError::UnsupportedVersion(response.version));
    }
    let valid_payload = match response.ok {
        true => response.result.is_some() && response.error.is_none(),
        false => response.result.is_none() && response.error.is_some(),
    };
    valid_payload
        .then_some(())
        .ok_or(ControlCodecError::InvalidResponse)
}

#[cfg(test)]
mod tests {
    use super::{ControlCommand, ControlErrorCode};

    #[test]
    fn performance_command_is_part_of_the_bounded_control_codec() {
        assert_eq!(
            ControlCommand::parse("performance"),
            Some(ControlCommand::Performance)
        );
        assert_eq!(ControlCommand::Performance.as_str(), "performance");
    }

    #[test]
    fn output_configuration_transaction_commands_are_canonical_and_strict() {
        let cases = [
            ("outputs", ControlCommand::Outputs),
            ("outputs.configure", ControlCommand::OutputsConfigure),
            (
                "outputs.configure.confirm",
                ControlCommand::OutputsConfigureConfirm,
            ),
            (
                "outputs.configure.revert",
                ControlCommand::OutputsConfigureRevert,
            ),
        ];
        for (name, command) in cases {
            assert_eq!(ControlCommand::parse(name), Some(command));
            assert_eq!(command.as_str(), name);
        }
        for near_miss in [
            "outputs.config",
            "outputs.configure.keep",
            "outputs.configure.rollback",
        ] {
            assert_eq!(ControlCommand::parse(near_miss), None);
        }
    }

    #[test]
    fn display_mutation_rejections_have_stable_machine_codes() {
        let cases = [
            (ControlErrorCode::UnknownOutput, "unknown_output"),
            (
                ControlErrorCode::StaleOutputGeneration,
                "stale_output_generation",
            ),
            (ControlErrorCode::UnknownOutputMode, "unknown_output_mode"),
            (
                ControlErrorCode::UnsupportedOutputScale,
                "unsupported_output_scale",
            ),
            (
                ControlErrorCode::UnsupportedOutputTransform,
                "unsupported_output_transform",
            ),
            (
                ControlErrorCode::OutputTransactionActive,
                "output_transaction_active",
            ),
            (
                ControlErrorCode::OutputTransactionNotFound,
                "output_transaction_not_found",
            ),
            (ControlErrorCode::OutputTestFailed, "output_test_failed"),
            (ControlErrorCode::OutputApplyFailed, "output_apply_failed"),
            (
                ControlErrorCode::OutputPersistFailed,
                "output_persist_failed",
            ),
            (
                ControlErrorCode::OutputRollbackFailed,
                "output_rollback_failed",
            ),
        ];
        for (code, expected) in cases {
            assert_eq!(code.as_str(), expected);
            assert_eq!(serde_json::to_value(code).unwrap(), expected);
        }
    }

    #[test]
    fn keyboard_configuration_commands_are_strictly_named() {
        assert_eq!(
            ControlCommand::parse("keyboard.config.get"),
            Some(ControlCommand::KeyboardConfigurationGet)
        );
        assert_eq!(
            ControlCommand::parse("keyboard.config.set"),
            Some(ControlCommand::KeyboardConfigurationSet)
        );
        assert_eq!(
            ControlCommand::KeyboardConfigurationGet.as_str(),
            "keyboard.config.get"
        );
        assert_eq!(
            ControlCommand::KeyboardConfigurationSet.as_str(),
            "keyboard.config.set"
        );
        assert_eq!(ControlCommand::parse("keyboard.config"), None);
    }

    #[test]
    fn trusted_effect_reload_command_is_explicitly_named() {
        assert_eq!(
            ControlCommand::parse("effects.reload"),
            Some(ControlCommand::EffectsReload)
        );
        assert_eq!(ControlCommand::EffectsReload.as_str(), "effects.reload");
        assert_eq!(ControlCommand::parse("effects"), None);
    }

    #[test]
    fn blur_commands_are_separate_from_effect_reload() {
        assert_eq!(
            ControlCommand::parse("blur.status"),
            Some(ControlCommand::BlurStatus)
        );
        assert_eq!(
            ControlCommand::parse("blur.reload"),
            Some(ControlCommand::BlurReload)
        );
        assert_eq!(ControlCommand::BlurStatus.as_str(), "blur.status");
        assert_eq!(ControlCommand::BlurReload.as_str(), "blur.reload");
        assert_eq!(ControlCommand::parse("blur.set-enabled"), None);
    }

    #[test]
    fn material_configuration_commands_are_canonical_and_strictly_named() {
        assert_eq!(
            ControlCommand::parse("material.config.get"),
            Some(ControlCommand::MaterialConfigurationGet)
        );
        assert_eq!(
            ControlCommand::parse("material.config.set"),
            Some(ControlCommand::MaterialConfigurationSet)
        );
        assert_eq!(
            ControlCommand::MaterialConfigurationGet.as_str(),
            "material.config.get"
        );
        assert_eq!(
            ControlCommand::MaterialConfigurationSet.as_str(),
            "material.config.set"
        );
        assert_eq!(ControlCommand::parse("material.set"), None);
    }

    #[test]
    fn material_program_commands_have_exact_canonical_names() {
        assert_eq!(
            ControlCommand::parse("material.program.catalog.get"),
            Some(ControlCommand::MaterialProgramCatalogGet)
        );
        assert_eq!(
            ControlCommand::parse("material.program.get"),
            Some(ControlCommand::MaterialProgramGet)
        );
        assert_eq!(
            ControlCommand::parse("material.program.set"),
            Some(ControlCommand::MaterialProgramSet)
        );
        assert_eq!(
            ControlCommand::MaterialProgramCatalogGet.as_str(),
            "material.program.catalog.get"
        );
        assert_eq!(
            ControlCommand::MaterialProgramGet.as_str(),
            "material.program.get"
        );
        assert_eq!(
            ControlCommand::MaterialProgramSet.as_str(),
            "material.program.set"
        );
        assert_eq!(
            ControlCommand::parse("material.program.state.get"),
            Some(ControlCommand::MaterialProgramStateGet)
        );
        assert_eq!(
            ControlCommand::parse("material.program.describe"),
            Some(ControlCommand::MaterialProgramDescribe)
        );
        assert_eq!(
            ControlCommand::parse("material.program.parameters.set"),
            Some(ControlCommand::MaterialProgramParametersSet)
        );
        for near_miss in [
            "material.catalog.get",
            "material.programs.get",
            "material.shader.set",
        ] {
            assert_eq!(ControlCommand::parse(near_miss), None);
        }
    }

    #[test]
    fn window_decoration_policy_command_has_one_canonical_name() {
        assert_eq!(
            ControlCommand::parse("window.decoration-policy.set"),
            Some(ControlCommand::WindowDecorationPolicySet)
        );
        assert_eq!(
            ControlCommand::WindowDecorationPolicySet.as_str(),
            "window.decoration-policy.set"
        );
        assert_eq!(ControlCommand::parse("window.decoration-policy"), None);
    }
}
