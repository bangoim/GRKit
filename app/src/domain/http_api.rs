//! Parse camera HTTP status/package JSON and decide status retries.
//!
//! Shapes match `grkit-serve` (`GET /v1/status`, `GET /v1/package`).

use serde::Deserialize;

use super::PackageInfo;
use super::camera::{Camera, CameraModel, ConnectionState};
use super::link::LinkError;

/// Base URL of the GRKit service on the camera access point.
pub const CAMERA_HTTP_BASE: &str = "http://192.168.0.1:8080";

/// How long to keep polling `/v1/status` after joining Wi-Fi.
pub const STATUS_POLL_BUDGET_MS: u64 = 30_000;

/// `/v1/status` body from the camera service.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct StatusResponse {
    /// Boot log lines for this power-on, or null when the file is missing.
    pub boot: Option<serde_json::Value>,
    pub protocol: u32,
    /// Seconds from `/proc/uptime` (fractional), or null when unread.
    pub uptime: Option<f64>,
    pub version: Option<String>,
    /// Update state object, or null when the package folder is not readable yet.
    #[serde(default)]
    pub update: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PackageCamera {
    pub model: Option<String>,
    pub firmware: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PackageResponse {
    pub version: Option<String>,
    pub modules: Option<serde_json::Value>,
    pub effects: Option<serde_json::Value>,
    pub left_out: Option<serde_json::Value>,
    /// Null when the camera's own API has not answered yet (still a valid package).
    pub camera: Option<PackageCamera>,
}

/// Parse a `/v1/status` body.
pub fn parse_status(body: &str) -> Result<StatusResponse, String> {
    serde_json::from_str(body).map_err(|e| e.to_string())
}

/// Parse a `/v1/package` body.
pub fn parse_package(body: &str) -> Result<PackageResponse, String> {
    serde_json::from_str(body).map_err(|e| e.to_string())
}

/// 503 means the card is not ready yet; keep retrying within the poll budget.
pub fn should_retry_status(status_code: u16) -> bool {
    status_code == 503
}

/// Outcome of one `/v1/status` attempt (for the poll loop).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusAttempt {
    /// Parsed a successful status body.
    Ready,
    /// HTTP 503 — card not ready; retry.
    CardNotReady,
    /// Could not reach the service (connection refused, no route, etc.); retry.
    Unreachable,
    /// Non-retryable failure (bad JSON, unexpected HTTP status).
    Fatal(LinkError),
}

/// Classify an HTTP response to `/v1/status` (not transport failures).
pub fn classify_status_response(status_code: u16, body: &str) -> StatusAttempt {
    if should_retry_status(status_code) {
        return StatusAttempt::CardNotReady;
    }
    if !(200..300).contains(&status_code) {
        return StatusAttempt::Fatal(LinkError::Http(format!("status HTTP {status_code}")));
    }
    match parse_status(body) {
        Ok(_) => StatusAttempt::Ready,
        Err(err) => StatusAttempt::Fatal(LinkError::Http(format!(
            "status body ({} bytes): {err}",
            body.len()
        ))),
    }
}

/// Which error to surface when the status poll budget runs out.
pub fn status_poll_timeout_error(last_was_503: bool) -> LinkError {
    if last_was_503 {
        LinkError::CardNotReady
    } else {
        LinkError::NeedsGrkit
    }
}

/// Compare dotted numeric versions (`1.7.0`). Non-numeric parts sort as 0.
pub fn version_at_least(version: &str, minimum: &str) -> bool {
    let a = parse_version_parts(version);
    let b = parse_version_parts(minimum);
    a >= b
}

fn parse_version_parts(v: &str) -> [u32; 3] {
    let mut parts = [0u32; 3];
    for (i, piece) in v.split('.').take(3).enumerate() {
        let digits: String = piece.chars().take_while(|c| c.is_ascii_digit()).collect();
        parts[i] = digits.parse().unwrap_or(0);
    }
    parts
}

/// Map a package response into UI models. `fallback_model` is used when the
/// package omits camera info or a recognizable model string.
pub fn package_to_models(
    package: &PackageResponse,
    fallback_model: CameraModel,
) -> Result<(Camera, PackageInfo), PackageMapError> {
    let installed = package
        .version
        .clone()
        .filter(|v| !v.is_empty())
        .ok_or(PackageMapError::NeedsGrkit)?;
    // Only GRKit 1.7.0 and later run this service, so an answer is enough:
    // the reported version is not checked again (development builds report
    // the version of their branch).
    let model = package
        .camera
        .as_ref()
        .and_then(|c| c.model.as_deref())
        .and_then(model_from_name)
        .unwrap_or(fallback_model);
    let firmware = package
        .camera
        .as_ref()
        .and_then(|c| c.firmware.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "—".into());
    let camera = Camera {
        model,
        firmware: firmware.clone(),
        installed_version: installed.clone(),
        connection: ConnectionState::Connected,
    };
    let info = PackageInfo {
        installed,
        available: None,
        rollback: None,
        firmware,
        release_notes: String::new(),
        package_bytes: 0,
    };
    Ok((camera, info))
}

/// Why package JSON cannot become a usable camera session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageMapError {
    NeedsGrkit,
}

fn model_from_name(name: &str) -> Option<CameraModel> {
    let upper = name.to_ascii_uppercase();
    if upper.contains("IIIX") || upper.contains("GR3X") {
        Some(CameraModel::Gr3x)
    } else if upper.contains("IV") || upper.contains("GR4") {
        Some(CameraModel::Gr4)
    } else if upper.contains("III") || upper.contains("GR3") {
        Some(CameraModel::Gr3)
    } else {
        None
    }
}

/// Next delay before another status poll (ms), capped for the 30s budget.
pub fn status_backoff_ms(attempt: u32) -> u64 {
    let base = 600u64;
    let delay = base.saturating_mul(1u64 << attempt.min(4));
    delay.min(3_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Bodies shaped like `grkit-serve` status/package tests.
    const STATUS_WITH_UPTIME: &str = r#"{"protocol":1,"version":"1.7.0","uptime":12.5,"boot":["2.0 load","2.1 scan-start","2.2 scan 0"],"update":null}"#;
    const STATUS_WITHOUT_FILES: &str =
        r#"{"protocol":1,"version":"1.7.0","uptime":null,"boot":null,"update":null}"#;
    const PACKAGE_WITH_CAMERA: &str = r#"{"version":"1.7.0","modules":[],"effects":[],"left_out":[],"camera":{"model":"RICOH GR III","firmware":"2.21"}}"#;
    const PACKAGE_CAMERA_NULL: &str = r#"{"version":"1.7.0","modules":[],"effects":[],"left_out":["LEFT_OUT.GRF"],"camera":null}"#;

    #[test]
    fn parses_status_like_service() {
        let s = parse_status(STATUS_WITH_UPTIME).unwrap();
        assert_eq!(s.protocol, 1);
        assert_eq!(s.version.as_deref(), Some("1.7.0"));
        assert_eq!(s.uptime, Some(12.5));
        assert_eq!(
            s.boot,
            Some(json!(["2.0 load", "2.1 scan-start", "2.2 scan 0"]))
        );
        assert!(s.update.as_ref().is_none_or(|v| v.is_null()));
    }

    #[test]
    fn parses_status_with_null_uptime_and_boot() {
        let s = parse_status(STATUS_WITHOUT_FILES).unwrap();
        assert!(s.uptime.is_none());
        assert!(s.boot.is_none());
    }

    #[test]
    fn rejects_wrong_status_types_as_parse_error() {
        // Old app bug: protocol/uptime as string/int-only types.
        let err = parse_status(r#"{"boot":"ok","protocol":"1","uptime":12,"version":"1.7.0"}"#)
            .unwrap_err();
        assert!(err.contains("protocol") || err.contains("invalid type") || !err.is_empty());
    }

    #[test]
    fn parse_failure_is_fatal_not_retry() {
        let body = r#"{"protocol":"1","uptime":12}"#;
        match classify_status_response(200, body) {
            StatusAttempt::Fatal(LinkError::Http(msg)) => {
                assert!(msg.contains("status body"));
                assert!(msg.contains(&format!("{} bytes", body.len())));
            }
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn classifies_503_as_retry_card() {
        assert_eq!(
            classify_status_response(503, r#"{"error":"The card is not ready. Try again."}"#),
            StatusAttempt::CardNotReady
        );
    }

    #[test]
    fn classifies_ok_status_as_ready() {
        assert_eq!(
            classify_status_response(200, STATUS_WITH_UPTIME),
            StatusAttempt::Ready
        );
    }

    #[test]
    fn timeout_all_unreachable_needs_grkit() {
        assert_eq!(status_poll_timeout_error(false), LinkError::NeedsGrkit);
    }

    #[test]
    fn timeout_last_503_is_card_not_ready() {
        assert_eq!(status_poll_timeout_error(true), LinkError::CardNotReady);
    }

    #[test]
    fn parses_package_with_camera() {
        let p = parse_package(PACKAGE_WITH_CAMERA).unwrap();
        let (cam, pkg) = package_to_models(&p, CameraModel::Gr3).unwrap();
        assert_eq!(cam.model, CameraModel::Gr3);
        assert_eq!(pkg.installed, "1.7.0");
        assert_eq!(cam.firmware, "2.21");
    }

    #[test]
    fn null_camera_uses_fallback_model() {
        let p = parse_package(PACKAGE_CAMERA_NULL).unwrap();
        let (cam, pkg) = package_to_models(&p, CameraModel::Gr4).unwrap();
        assert_eq!(cam.model, CameraModel::Gr4);
        assert_eq!(pkg.installed, "1.7.0");
        assert_eq!(cam.firmware, "—");
        assert_eq!(pkg.firmware, "—");
    }

    #[test]
    fn any_answering_service_is_supported() {
        let p =
            parse_package(r#"{"version":"1.6.1","camera":{"model":"GR III","firmware":"2.00"}}"#)
                .unwrap();
        let (cam, pkg) = package_to_models(&p, CameraModel::Gr3).unwrap();
        assert_eq!(cam.installed_version, "1.6.1");
        assert_eq!(pkg.installed, "1.6.1");
    }

    #[test]
    fn null_version_needs_grkit() {
        let p = parse_package(r#"{"version":null,"camera":null}"#).unwrap();
        assert_eq!(
            package_to_models(&p, CameraModel::Gr3),
            Err(PackageMapError::NeedsGrkit)
        );
    }

    #[test]
    fn retries_only_503() {
        assert!(should_retry_status(503));
        assert!(!should_retry_status(200));
        assert!(!should_retry_status(404));
    }

    #[test]
    fn version_compare() {
        assert!(version_at_least("1.7.0", "1.7.0"));
        assert!(version_at_least("1.7.1", "1.7.0"));
        assert!(!version_at_least("1.6.9", "1.7.0"));
    }

    #[test]
    fn backoff_grows() {
        assert_eq!(status_backoff_ms(0), 600);
        assert!(status_backoff_ms(3) >= status_backoff_ms(1));
        assert_eq!(status_backoff_ms(10), 3_000);
    }
}
