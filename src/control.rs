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
    Windows,
    ActiveWindow,
    Performance,
    CursorGet,
    CursorSetTheme,
    CursorSetSize,
    CursorSet,
    CursorReload,
    DecorationStatus,
    DecorationSetTheme,
    DecorationReload,
    DecorationList,
    WindowActivate,
    WindowMinimize,
    WindowRestore,
    WindowClose,
}

impl ControlCommand {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Version => "version",
            Self::Doctor => "doctor",
            Self::Outputs => "outputs",
            Self::Windows => "windows",
            Self::ActiveWindow => "active-window",
            Self::Performance => "performance",
            Self::CursorGet => "cursor.get",
            Self::CursorSetTheme => "cursor.set-theme",
            Self::CursorSetSize => "cursor.set-size",
            Self::CursorSet => "cursor.set",
            Self::CursorReload => "cursor.reload",
            Self::DecorationStatus => "decoration.status",
            Self::DecorationSetTheme => "decoration.set-theme",
            Self::DecorationReload => "decoration.reload",
            Self::DecorationList => "decoration.list",
            Self::WindowActivate => "window.activate",
            Self::WindowMinimize => "window.minimize",
            Self::WindowRestore => "window.restore",
            Self::WindowClose => "window.close",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "status" => Self::Status,
            "version" => Self::Version,
            "doctor" => Self::Doctor,
            "outputs" => Self::Outputs,
            "windows" => Self::Windows,
            "active-window" => Self::ActiveWindow,
            "performance" => Self::Performance,
            "cursor.get" => Self::CursorGet,
            "cursor.set-theme" => Self::CursorSetTheme,
            "cursor.set-size" => Self::CursorSetSize,
            "cursor.set" => Self::CursorSet,
            "cursor.reload" => Self::CursorReload,
            "decoration.status" => Self::DecorationStatus,
            "decoration.set-theme" => Self::DecorationSetTheme,
            "decoration.reload" => Self::DecorationReload,
            "decoration.list" => Self::DecorationList,
            "window.activate" => Self::WindowActivate,
            "window.minimize" => Self::WindowMinimize,
            "window.restore" => Self::WindowRestore,
            "window.close" => Self::WindowClose,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlErrorCode {
    InvalidArgument,
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
    use super::ControlCommand;

    #[test]
    fn performance_command_is_part_of_the_bounded_control_codec() {
        assert_eq!(
            ControlCommand::parse("performance"),
            Some(ControlCommand::Performance)
        );
        assert_eq!(ControlCommand::Performance.as_str(), "performance");
    }
}
