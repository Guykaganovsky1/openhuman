//! Tunnel client for the device pairing domain.
//!
//! Reuses the existing `SocketManager` (global singleton) to emit and receive
//! `tunnel:*` Socket.IO events without opening a second WebSocket connection to
//! the backend. Incoming `tunnel:peer-status` and `tunnel:frame` events arrive
//! via the event bus (published by `socket::event_handlers` after this module
//! adds them to the dispatch table) and are handled by `devices::bus`.
//!
//! Frame cap: 64 KB. Rate limit: callers are expected to stay ≤ 100 frames/s.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::openhuman::platform::socket::global_socket_manager;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// Payload emitted as `tunnel:register` to the backend.
#[derive(Debug, Serialize)]
pub struct TunnelRegisterPayload {
    pub role: String, // always "core"
}

/// Response from the `tunnel:register` ACK callback.
///
/// Shape per `gitbooks/developing/architecture.md` and this domain's README:
/// `{channelId, pairingToken, pairingExpiresAt}`. The SDK
/// (`vendor/tinyhumans-sdk`) declares the *event names* for the tunnel surface
/// (`socket::events::outbound::TUNNEL_REGISTER`) but no ack type, so those two
/// documents are the only source for the field names and they are kept as-is.
#[derive(Debug, Clone, Deserialize)]
pub struct TunnelRegisterResponse {
    #[serde(rename = "channelId")]
    pub channel_id: String,
    #[serde(rename = "pairingToken")]
    pub pairing_token: String,
    /// ISO 8601 expiry. Accepts an epoch-millis integer too — see
    /// [`deserialize_pairing_expires_at`].
    #[serde(
        rename = "pairingExpiresAt",
        deserialize_with = "deserialize_pairing_expires_at"
    )]
    pub pairing_expires_at: String,
}

/// Deserialize `pairingExpiresAt` from either a string or an epoch-millis
/// integer, always yielding the ISO 8601 string the rest of the flow expects.
///
/// The live backend has been observed sending the integer form: the ack failed
/// to decode with `invalid type: integer 1788441492159, expected a string`,
/// which killed the whole pairing. Everything downstream treats this as an ISO
/// string — `types::PairingSession::expires_at` documents it as one, and
/// `PairPhoneModal.tsx` does `new Date(session.expires_at)` to build the QR's
/// `exp` field — so the integer is normalised here rather than widening the
/// type through four structs and the RPC surface.
///
/// An integer is read as **milliseconds**, which is the only form observed
/// (13 digits). A value outside the representable range is a decode error
/// rather than a silently wrong timestamp.
fn deserialize_pairing_expires_at<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawExpiry {
        Text(String),
        EpochMillis(i64),
    }

    match RawExpiry::deserialize(deserializer)? {
        RawExpiry::Text(text) => Ok(text),
        RawExpiry::EpochMillis(ms) => chrono::DateTime::from_timestamp_millis(ms)
            .map(|dt| dt.to_rfc3339())
            .ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "pairingExpiresAt epoch-millis value out of range: {ms}"
                ))
            }),
    }
}

/// Names the shape of an ack for a log line **without disclosing any value** —
/// the ack carries a single-use pairing token, so only top-level keys (or the
/// JSON type, when it is not an object) may be logged.
fn describe_ack_shape(ack: &serde_json::Value) -> String {
    match ack {
        serde_json::Value::Object(map) => {
            let keys: Vec<&str> = map.keys().map(String::as_str).collect();
            format!("object keys=[{}]", keys.join(", "))
        }
        serde_json::Value::Array(items) => format!("array len={}", items.len()),
        serde_json::Value::String(_) => "string".to_string(),
        serde_json::Value::Number(_) => "number".to_string(),
        serde_json::Value::Bool(_) => "bool".to_string(),
        serde_json::Value::Null => "null".to_string(),
    }
}

/// Payload emitted as `tunnel:connect` to join a channel.
#[derive(Debug, Serialize)]
pub struct TunnelConnectPayload {
    #[serde(rename = "channelId")]
    pub channel_id: String,
    pub role: String, // "core" or "client"
}

/// Inbound `tunnel:peer-status` event payload.
#[derive(Debug, Clone, Deserialize)]
pub struct TunnelPeerStatus {
    #[serde(rename = "channelId")]
    pub channel_id: String,
    pub online: bool,
}

/// Inbound `tunnel:frame` event payload.
#[derive(Debug, Clone, Deserialize)]
pub struct TunnelFrame {
    #[serde(rename = "channelId")]
    pub channel_id: String,
    /// Base64url-encoded encrypted frame bytes.
    pub payload: String,
}

/// Outbound `tunnel:frame` emit payload.
#[derive(Debug, Serialize)]
struct TunnelFrameEmit<'a> {
    #[serde(rename = "channelId")]
    channel_id: &'a str,
    payload: &'a str,
}

// ---------------------------------------------------------------------------
// Tunnel operations
// ---------------------------------------------------------------------------

/// Emit `tunnel:register` on the shared socket and parse the ACK response.
pub async fn emit_register() -> Result<TunnelRegisterResponse, String> {
    log::debug!("[devices/tunnel] emit_register: sending tunnel:register");
    let mgr = global_socket_manager()
        .ok_or_else(|| "[devices/tunnel] SocketManager not initialized".to_string())?;

    let payload = json!({ "role": "core" });
    let ack = mgr
        .emit_with_ack(
            "tunnel:register",
            payload,
            std::time::Duration::from_secs(10),
        )
        .await
        .map_err(|e| format!("[devices/tunnel] emit tunnel:register failed: {e}"))?;

    // Diagnose a decode failure with the ack's *shape* only. This ack has no
    // HTTP status (it is a Socket.IO acknowledgement, not a response), and its
    // body carries the single-use pairing token, so the keys and the encoded
    // length are everything that may be logged. Both field-shape failures seen
    // in the field — an integer `pairingExpiresAt` and a `channelId` that was
    // absent entirely — were undiagnosable from the bare serde message alone.
    let shape = describe_ack_shape(&ack);
    let encoded_len = ack.to_string().len();
    serde_json::from_value::<TunnelRegisterResponse>(ack).map_err(|e| {
        log::warn!(
            "[devices/tunnel] tunnel:register ack did not decode: {e} ({shape}, encoded_bytes={encoded_len})"
        );
        format!("[devices/tunnel] parse tunnel:register ack failed: {e} ({shape})")
    })
}

/// Emit `tunnel:connect` to start listening on a channel as `role:"core"`.
pub async fn emit_connect(channel_id: &str) -> Result<(), String> {
    log::debug!("[devices/tunnel] emit_connect channel_id={channel_id}");
    let mgr = global_socket_manager()
        .ok_or_else(|| "[devices/tunnel] SocketManager not initialized".to_string())?;

    let payload = build_core_connect_payload(channel_id);

    mgr.emit("tunnel:connect", payload)
        .await
        .map_err(|e| format!("[devices/tunnel] emit tunnel:connect failed: {e}"))
}

fn build_core_connect_payload(channel_id: &str) -> serde_json::Value {
    json!({
        "channelId": channel_id,
        "role": "core",
    })
}

/// Emit a `tunnel:frame` carrying an encrypted payload for the peer.
///
/// `payload_b64` is the base64url-encoded sealed frame from `TunnelCipher::seal`.
pub async fn emit_frame(channel_id: &str, payload_b64: &str) -> Result<(), String> {
    if payload_b64.len() > 64 * 1024 {
        return Err(format!(
            "[devices/tunnel] frame too large: {} bytes (max 64 KB)",
            payload_b64.len()
        ));
    }
    let mgr = global_socket_manager()
        .ok_or_else(|| "[devices/tunnel] SocketManager not initialized".to_string())?;

    let payload = json!({
        "channelId": channel_id,
        "payload": payload_b64,
    });

    mgr.emit("tunnel:frame", payload)
        .await
        .map_err(|e| format!("[devices/tunnel] emit tunnel:frame failed: {e}"))
}

#[cfg(test)]
#[path = "tunnel_client_tests.rs"]
mod tests;
