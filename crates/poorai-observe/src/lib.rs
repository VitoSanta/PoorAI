//! Structured, secret-safe observability events.
use poorai_domain::hash_bytes;
use serde::Serialize;
pub fn emit<T: Serialize>(event_type: &str, payload: &T) {
    let value = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
    tracing::info!(event_type, payload_hash = %hash_bytes(serde_json::to_vec(&value).unwrap_or_default()), "poorai_event");
}
