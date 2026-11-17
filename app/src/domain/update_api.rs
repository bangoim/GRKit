//! Wire state and persisted intent for an update. A missing journal is not proof of success.
use super::{UpdateError, UpdateKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JournalState {
    None,
    Waiting,
    Confirmed,
}

#[derive(Clone, Debug, Deserialize)]
pub struct InventoryPackage {
    pub version: Option<String>,
    pub modules: Option<serde_json::Value>,
    pub effects: Option<serde_json::Value>,
    pub left_out: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryBody {
    pub package: InventoryPackage,
    pub state: JournalState,
    #[serde(default)]
    pub backup: Option<RollbackVersion>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RollbackVersion {
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoftwareRequestSummary {
    pub request_id: String,
    pub domain: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub phase: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResource {
    pub inventory: InventoryBody,
    #[serde(default)]
    pub request: Option<SoftwareRequestSummary>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoftwareIdentity {
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackBackup {
    pub available: bool,
    pub allowed: bool,
    pub reason: String,
    pub identity: Option<SoftwareIdentity>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackResource {
    pub backup: RollbackBackup,
    #[serde(default)]
    pub request: Option<SoftwareRequestSummary>,
}

#[derive(Clone, Debug)]
pub struct UpdateSnapshot {
    pub state: JournalState,
    pub package: super::PackageResponse,
    pub etag: String,
    pub boot_id: String,
    pub card_id: String,
    pub request_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RollbackSnapshot {
    pub available: bool,
    pub allowed: bool,
    pub reason: String,
    pub version: Option<String>,
    pub target_identity: Option<String>,
    pub etag: String,
    pub boot_id: String,
    pub card_id: String,
    pub request_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateIntent {
    pub camera_id: String,
    pub kind: UpdateKind,
    pub baseline: String,
    pub target: String,
    /// Staged software request id; needed to cancel after reconnect.
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateObservation {
    Waiting,
    Restart,
    Installed,
    Stopped,
}

pub fn observe_update(state: JournalState, installed: &str, target: &str) -> UpdateObservation {
    match state {
        JournalState::Waiting => UpdateObservation::Waiting,
        JournalState::Confirmed => UpdateObservation::Restart,
        JournalState::None if installed == target => UpdateObservation::Installed,
        JournalState::None => UpdateObservation::Stopped,
    }
}

pub fn response_error(status: u16, body: &str) -> UpdateError {
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|v| {
            v.get("detail")
                .and_then(|v| v.as_str())
                .or_else(|| v.get("error").and_then(|v| v.as_str()))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("Camera returned HTTP {status}."));
    if let Some(bytes) = message
        .strip_prefix("The card needs ")
        .and_then(|s| s.strip_suffix(" more bytes for the update"))
        .and_then(|s| s.parse::<u64>().ok())
    {
        UpdateError::CardFull {
            bytes_needed: bytes,
        }
    } else {
        UpdateError::Service(message)
    }
}

/// A UUID shaped for `X-GRKit-Request-Id` (36 hex digits with dashes).
pub fn new_request_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mixed = nanos ^ ((std::process::id() as u128) << 64);
    let bytes = mixed.to_be_bytes();
    let mut hex = String::with_capacity(32);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    // Stretch to 32 hex chars if the platform gives fewer entropy bytes.
    while hex.len() < 32 {
        hex.push('0');
    }
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_missing_journal_only_succeeds_at_the_target_version() {
        assert_eq!(
            observe_update(JournalState::None, "1.7.0", "1.8.0"),
            UpdateObservation::Stopped
        );
        assert_eq!(
            observe_update(JournalState::None, "1.8.0", "1.8.0"),
            UpdateObservation::Installed
        );
        assert_eq!(
            observe_update(JournalState::Confirmed, "1.8.0", "1.8.0"),
            UpdateObservation::Restart
        );
        assert_eq!(
            observe_update(JournalState::Waiting, "1.8.0", "1.8.0"),
            UpdateObservation::Waiting
        );
    }
    #[test]
    fn rollback_requires_its_own_target() {
        assert_eq!(
            observe_update(JournalState::None, "1.8.0", "1.7.0"),
            UpdateObservation::Stopped
        );
        assert_eq!(
            observe_update(JournalState::None, "1.7.0", "1.7.0"),
            UpdateObservation::Installed
        );
    }
    #[test]
    fn maps_service_errors_without_inventing_free_space() {
        assert_eq!(
            response_error(
                400,
                r#"{"error":"The card needs 123 more bytes for the update"}"#
            ),
            UpdateError::CardFull { bytes_needed: 123 }
        );
        assert!(
            matches!(response_error(409, r#"{"error":"No backup"}"#), UpdateError::Service(s) if s == "No backup")
        );
        assert!(
            matches!(
                response_error(
                    409,
                    r#"{"type":"about:blank","title":"Conflict","status":409,"code":"data_format_incompatible","detail":"The backup cannot be restored."}"#
                ),
                UpdateError::Service(s) if s == "The backup cannot be restored."
            )
        );
    }
    #[test]
    fn request_id_matches_serve_shape() {
        let id = new_request_id();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[8..9], "-");
        assert_eq!(&id[13..14], "-");
        assert_eq!(&id[18..19], "-");
        assert_eq!(&id[23..24], "-");
        assert!(id.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        }));
    }
}
