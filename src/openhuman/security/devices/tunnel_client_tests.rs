use super::*;
use serde_json::json;

#[test]
fn tunnel_register_response_accepts_backend_ack_shape_without_session_token() {
    let response: TunnelRegisterResponse = serde_json::from_value(json!({
        "channelId": "ch_123",
        "pairingToken": "pt_123",
        "pairingExpiresAt": "2026-06-30T15:00:00Z"
    }))
    .expect("backend register ack shape should parse");

    assert_eq!(response.channel_id, "ch_123");
    assert_eq!(response.pairing_token, "pt_123");
    assert_eq!(response.pairing_expires_at, "2026-06-30T15:00:00Z");
}

#[test]
fn build_core_connect_payload_omits_session_token_for_core_role() {
    let payload = build_core_connect_payload("ch_123");

    assert_eq!(payload["channelId"], "ch_123");
    assert_eq!(payload["role"], "core");
    assert!(payload.get("sessionToken").is_none());
    assert!(payload.get("pairingToken").is_none());
}

/// Observed live (20:17): the backend sent `pairingExpiresAt` as an
/// epoch-millis integer and the whole ack failed with
/// `invalid type: integer 1788441492159, expected a string`, killing the
/// pairing. It must decode, normalised to the ISO 8601 string every consumer
/// downstream parses with `new Date(...)`.
#[test]
fn tunnel_register_response_accepts_epoch_millis_expiry() {
    let response: TunnelRegisterResponse = serde_json::from_value(json!({
        "channelId": "ch_123",
        "pairingToken": "pt_123",
        "pairingExpiresAt": 1_788_441_492_159i64
    }))
    .expect("an epoch-millis expiry must decode");

    assert_eq!(response.channel_id, "ch_123");
    assert_eq!(
        response.pairing_expires_at,
        chrono::DateTime::from_timestamp_millis(1_788_441_492_159)
            .expect("in range")
            .to_rfc3339(),
        "the integer must be normalised to an ISO 8601 string"
    );
    assert!(
        chrono::DateTime::parse_from_rfc3339(&response.pairing_expires_at).is_ok(),
        "the normalised value must parse as a timestamp: {}",
        response.pairing_expires_at
    );
}

/// An epoch value no calendar date can represent is a decode error, not a
/// silently wrong expiry.
#[test]
fn tunnel_register_response_rejects_out_of_range_epoch_millis() {
    let err = serde_json::from_value::<TunnelRegisterResponse>(json!({
        "channelId": "ch_123",
        "pairingToken": "pt_123",
        "pairingExpiresAt": i64::MAX
    }))
    .expect_err("an unrepresentable epoch must not decode");
    assert!(
        err.to_string().contains("out of range"),
        "unexpected error: {err}"
    );
}

/// The second shape seen live the same day (22:39): `channelId` absent entirely. Nothing in
/// the SDK or the architecture docs describes an ack without it, so it stays a
/// decode failure — but the failure must now be diagnosable from the log.
#[test]
fn ack_shape_description_names_keys_and_never_values() {
    let ack = json!({
        "pairingToken": "pt_secret_value",
        "pairingExpiresAt": 1_788_441_492_159i64
    });
    let described = describe_ack_shape(&ack);

    assert!(described.contains("pairingToken"), "{described}");
    assert!(described.contains("pairingExpiresAt"), "{described}");
    assert!(
        !described.contains("pt_secret_value"),
        "the pairing token must never reach a log line: {described}"
    );
    assert!(
        !described.contains("1788441492159"),
        "values must not be logged: {described}"
    );

    assert!(serde_json::from_value::<TunnelRegisterResponse>(ack).is_err());

    assert_eq!(describe_ack_shape(&json!(null)), "null");
    assert_eq!(describe_ack_shape(&json!("oops")), "string");
    assert_eq!(describe_ack_shape(&json!([1, 2, 3])), "array len=3");
}
