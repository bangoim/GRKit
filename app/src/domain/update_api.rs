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
pub struct RollbackVersion {
    pub version: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct UpdateSnapshot {
    pub state: JournalState,
    pub rollback: Option<RollbackVersion>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateIntent {
    pub camera_id: String,
    pub kind: UpdateKind,
    pub baseline: String,
    pub target: String,
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
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|v| v.as_str()).map(str::to_owned))
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
    }
}
