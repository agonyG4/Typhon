use serde_json::json;

use crate::control::{
    ControlCodecError, ControlError, ControlErrorCode, ControlRequest, ControlResponse,
    MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, decode_request, decode_response, encode_request,
    encode_response,
};

#[test]
fn request_codec_round_trips_the_v1_envelope() {
    let request = ControlRequest::new(7, "status", json!({})).unwrap();

    let encoded = encode_request(&request).unwrap();
    assert_eq!(
        encoded,
        br#"{"protocol":"astrea.control","version":1,"id":7,"command":"status","args":{}}
"#
    );
    assert_eq!(decode_request(&encoded).unwrap(), request);
}

#[test]
fn request_codec_rejects_wrong_protocol_version_and_argument_shape() {
    for (payload, expected) in [
        (
            br#"{"protocol":"other.control","version":1,"id":1,"command":"status","args":{}}"#
                .as_slice(),
            ControlCodecError::InvalidRequest,
        ),
        (
            br#"{"protocol":"astrea.control","version":2,"id":1,"command":"status","args":{}}"#
                .as_slice(),
            ControlCodecError::UnsupportedVersion(2),
        ),
        (
            br#"{"protocol":"astrea.control","version":1,"id":1,"command":"status","args":null}"#
                .as_slice(),
            ControlCodecError::InvalidRequest,
        ),
    ] {
        assert_eq!(decode_request(payload).unwrap_err(), expected);
    }
}

#[test]
fn request_codec_rejects_unknown_fields_and_oversized_input() {
    let unknown_field =
        br#"{"protocol":"astrea.control","version":1,"id":1,"command":"status","args":{},"extra":true}"#;
    assert_eq!(
        decode_request(unknown_field).unwrap_err(),
        ControlCodecError::InvalidRequest
    );

    assert_eq!(
        decode_request(br#"{"protocol":"astrea.control""#).unwrap_err(),
        ControlCodecError::MalformedJson
    );

    let oversized = vec![b' '; MAX_REQUEST_BYTES + 1];
    assert_eq!(
        decode_request(&oversized).unwrap_err(),
        ControlCodecError::RequestTooLarge
    );
}

#[test]
fn response_codec_round_trips_success_and_error_responses() {
    let success = ControlResponse::success(7, json!({"ready": true}));
    let error = ControlResponse::failure(
        8,
        ControlError::new(ControlErrorCode::InvalidArgument, "bad argument"),
    );

    assert_eq!(
        decode_response(&encode_response(&success).unwrap()).unwrap(),
        success
    );
    assert_eq!(
        decode_response(&encode_response(&error).unwrap()).unwrap(),
        error
    );
}

#[test]
fn response_codec_rejects_oversized_output() {
    let response = ControlResponse::success(1, json!({"data": "x".repeat(MAX_RESPONSE_BYTES)}));

    assert_eq!(
        encode_response(&response).unwrap_err(),
        ControlCodecError::ResponseTooLarge
    );
}
