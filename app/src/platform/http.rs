//! Poll the camera GRKit HTTP service after Wi-Fi join (or against a dev URL).

use std::time::{Duration, Instant};

use nsh_core::prelude::*;

use crate::domain::{
    CameraModel, CameraStatus, LinkError, PackageInfo, PackageMapError, STATUS_POLL_BUDGET_MS,
    StatusAttempt, classify_status_response, package_to_models, parse_package, status_backoff_ms,
    status_poll_timeout_error,
};

use super::camera_http_base;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

async fn camera_get(url: String) -> Result<nsh_core::element::HttpResponse, LinkError> {
    super::camera_request("GET", url, Vec::new(), REQUEST_TIMEOUT)
        .await
        .map_err(|_| LinkError::ConnectionLost)
}

/// Wait until `/v1/status` answers successfully.
///
/// Retries transport failures and HTTP 503 within the poll budget. A body that
/// does not parse ends the connect at once. On timeout: all unreachable →
/// [`LinkError::NeedsGrkit`] only if the camera API is reachable; otherwise
/// [`LinkError::ConnectionLost`]. A last 503 answer means [`LinkError::CardNotReady`].
pub async fn wait_until_ready() -> Result<(), LinkError> {
    let base = camera_http_base();
    app_info!("HTTP wait status at {base}");
    let deadline = Instant::now() + Duration::from_millis(STATUS_POLL_BUDGET_MS);
    let mut attempt = 0u32;
    loop {
        let last_was_503 = match get_status_once(&base).await {
            StatusAttempt::Ready => {
                app_info!("HTTP status ready");
                return Ok(());
            }
            StatusAttempt::Fatal(err) => {
                log::warn!("HTTP status fatal: {err:?}");
                return Err(err);
            }
            StatusAttempt::CardNotReady => true,
            StatusAttempt::Unreachable => false,
        };
        if Instant::now() >= deadline {
            log::warn!("HTTP status timed out (last_503={last_was_503})");
            return Err(if last_was_503 {
                status_poll_timeout_error(true)
            } else if probe_camera_api(&base).await {
                LinkError::NeedsGrkit
            } else {
                LinkError::ConnectionLost
            });
        }
        let delay = status_backoff_ms(attempt);
        attempt = attempt.saturating_add(1);
        timer(Duration::from_millis(delay)).await;
    }
}

/// A valid GRKit status is stronger evidence than iOS association bookkeeping.
#[cfg(target_os = "ios")]
pub(crate) async fn service_ready() -> bool {
    get_status_once(&camera_http_base()).await == StatusAttempt::Ready
}

/// A mounting card still proves the camera link is alive.
pub(crate) async fn service_reachable() -> bool {
    matches!(
        get_status_once(&camera_http_base()).await,
        StatusAttempt::Ready | StatusAttempt::CardNotReady
    )
}

async fn get_status_once(base: &str) -> StatusAttempt {
    let url = format!("{base}/v1/status");
    let response = match camera_get(url).await {
        Ok(response) => response,
        Err(e) => {
            log::warn!("HTTP status unreachable: {e:?}");
            return StatusAttempt::Unreachable;
        }
    };
    let body = response.text();
    let attempt = classify_status_response(response.status(), &body);
    if let StatusAttempt::Fatal(LinkError::Http(ref msg)) = attempt
        && msg.contains("status body")
    {
        log::warn!("HTTP status parse failed len={} detail={msg}", body.len());
    }
    attempt
}

/// GET `/v1/package` and map it into UI models.
pub async fn read_package(
    fallback_model: CameraModel,
) -> Result<(CameraStatus, PackageInfo), LinkError> {
    let base = camera_http_base();
    let url = format!("{base}/v1/package");
    app_info!("HTTP read package at {base}");
    let response = camera_get(url).await?;
    if response.status() == 503 {
        return Err(LinkError::CardNotReady);
    }
    if !response.ok() {
        return Err(LinkError::Http(format!(
            "package HTTP {}",
            response.status()
        )));
    }
    let text = response.text();
    let package = parse_package(&text).map_err(|e| {
        log::warn!("HTTP package parse failed len={} err={e}", text.len());
        LinkError::Http(format!("package body ({} bytes): {e}", text.len()))
    })?;
    let (camera, info) = package_to_models(&package, fallback_model).map_err(|e| match e {
        PackageMapError::NeedsGrkit => LinkError::NeedsGrkit,
    })?;
    Ok((
        CameraStatus {
            camera,
            powered_on: true,
        },
        info,
    ))
}

/// Logs whether the camera's own HTTP API (port 80) answers, to tell a
/// missing GRKit service from a network the phone cannot reach.
async fn probe_camera_api(base: &str) -> bool {
    let Some(host) = base
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .and_then(|authority| authority.split(':').next())
    else {
        return false;
    };
    match camera_get(format!("http://{host}/v1/props")).await {
        Ok(response) if response.ok() => {
            serde_json::from_str::<serde_json::Value>(&response.text())
                .is_ok_and(|body| body.get("model").is_some() || body.get("errCode").is_some())
        }
        _ => false,
    }
}
