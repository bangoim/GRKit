//! Native Bluetooth / Wi-Fi bridges for iOS and Android.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod stub;

pub(crate) mod http;
pub(crate) mod release;
pub(crate) mod update;

use crate::domain::{
    CAMERA_HTTP_BASE, CameraModel, CameraStatus, ConnectStep, LinkError, NearbyCamera, PackageInfo,
    RememberedCamera,
};

/// Base URL for camera HTTP. `GRKIT_CAMERA_URL` overrides the on-camera default when set.
pub fn camera_http_base() -> String {
    config_value("GRKIT_CAMERA_URL")
        .map(|s| s.trim().trim_end_matches('/').to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| CAMERA_HTTP_BASE.to_owned())
}

/// True when `GRKIT_CAMERA_URL` is set (HTTP-only connect for simulator / local server).
pub fn has_dev_camera_url() -> bool {
    config_value("GRKIT_CAMERA_URL")
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// True when this build can drive a Bluetooth central in principle (iOS / Android).
#[allow(dead_code)]
pub fn bluetooth_available() -> bool {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        true
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        false
    }
}

/// True when the central has reported that Bluetooth cannot be used (e.g. simulator Unsupported).
pub fn bluetooth_unavailable() -> bool {
    #[cfg(target_os = "ios")]
    {
        ios::ble::bluetooth_unavailable()
    }
    #[cfg(target_os = "android")]
    {
        android::bluetooth_unavailable()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        true
    }
}

/// Load the last successfully connected camera, if any.
pub fn load_remembered() -> Option<RememberedCamera> {
    #[cfg(target_os = "ios")]
    {
        ios::remember::load()
    }
    #[cfg(target_os = "android")]
    {
        android::load_remembered()
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        stub::load_remembered()
    }
}

/// Persist a successful connect for later reconnect.
#[allow(dead_code)]
pub fn save_remembered(camera: &RememberedCamera) -> Result<(), String> {
    #[cfg(target_os = "ios")]
    {
        ios::remember::save(camera)
    }
    #[cfg(target_os = "android")]
    {
        android::save_remembered(camera)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        stub::save_remembered(camera)
    }
}

/// Start scanning for nearby cameras. Discoveries are pushed on `tx`.
/// When `remembered_id` is set, retrieve that peripheral so reconnect works before a fresh advertisement.
pub fn start_scanning(tx: async_channel::Sender<Vec<NearbyCamera>>, remembered_id: Option<&str>) {
    #[cfg(target_os = "ios")]
    {
        ios::ble::start_scanning(tx, remembered_id);
    }
    #[cfg(target_os = "android")]
    {
        android::start_scanning(tx, remembered_id);
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        stub::start_scanning(tx, remembered_id);
    }
}

/// Sends the id of each camera that disconnects on `tx`.
pub fn watch_disconnects(tx: async_channel::Sender<String>) {
    #[cfg(target_os = "ios")]
    {
        ios::ble::watch_disconnects(tx);
    }
    #[cfg(target_os = "android")]
    {
        android::watch_disconnects(tx);
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        stub::watch_disconnects(tx);
    }
}

/// Stop an active scan.
#[allow(dead_code)]
pub fn stop_scanning() {
    #[cfg(target_os = "ios")]
    {
        ios::ble::stop_scanning();
    }
    #[cfg(target_os = "android")]
    {
        android::stop_scanning();
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        stub::stop_scanning();
    }
}

/// Full connect: BLE wake + Wi-Fi join + status/package, or HTTP-only when `GRKIT_CAMERA_URL` is set.
pub async fn connect_camera(
    peripheral_id: &str,
    model_hint: CameraModel,
    mut on_step: impl FnMut(ConnectStep),
) -> Result<(CameraStatus, PackageInfo, RememberedCamera), LinkError> {
    if has_dev_camera_url() {
        return connect_http_only(model_hint, &mut on_step).await;
    }
    #[cfg(target_os = "ios")]
    {
        ios::link::connect(peripheral_id, model_hint, &mut on_step).await
    }
    #[cfg(target_os = "android")]
    {
        android::connect_camera(peripheral_id, model_hint, &mut on_step).await
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = (peripheral_id, model_hint, &mut on_step);
        stub::connect_camera().await
    }
}

/// Skip Bluetooth and Wi-Fi; poll status then package against [`camera_http_base`].
async fn connect_http_only(
    model_hint: CameraModel,
    on_step: &mut dyn FnMut(ConnectStep),
) -> Result<(CameraStatus, PackageInfo, RememberedCamera), LinkError> {
    app_info!("connect HTTP-only against {}", camera_http_base());
    on_step(ConnectStep::Bluetooth);
    on_step(ConnectStep::Wifi);
    http::wait_until_ready().await?;
    on_step(ConnectStep::Version);
    let (status, package) = http::read_package(model_hint).await?;
    let remembered = RememberedCamera::new("dev-url", status.camera.model, "dev".to_owned());
    Ok((status, package, remembered))
}

/// Bound a platform operation even when its native completion never arrives.
pub(crate) async fn with_timeout<T>(
    future: impl std::future::Future<Output = T>,
    duration: std::time::Duration,
) -> Option<T> {
    use futures_util::future::{Either, select};
    match select(
        Box::pin(future),
        Box::pin(nsh_core::prelude::timer(duration)),
    )
    .await
    {
        Either::Left((value, _)) => Some(value),
        Either::Right(_) => None,
    }
}

/// Android debug host injects build settings without mutating process environment.
pub(crate) fn config_value(name: &str) -> Option<String> {
    #[cfg(target_os = "android")]
    if let Some(value) = android::config(name) {
        return Some(value);
    }
    std::env::var(name).ok()
}

pub(crate) async fn camera_request(
    method: &str,
    url: String,
    body: Vec<u8>,
    budget: std::time::Duration,
    headers: &[(&str, String)],
) -> Result<nsh_core::element::HttpResponse, String> {
    #[cfg(target_os = "android")]
    {
        android::camera_request(method, url, body, budget, headers).await
    }
    #[cfg(not(target_os = "android"))]
    {
        let mut request = nsh_core::platform::HttpRequest::new(method, url)
            .header("Cache-Control", "no-cache, no-store");
        for (name, value) in headers {
            request = request.header(*name, value.clone());
        }
        let mut request = request.body(body);
        request.follow_redirects = false;
        with_timeout(nsh_core::prelude::fetch(request), budget)
            .await
            .ok_or_else(|| "ConnectionLost".to_owned())?
    }
}
