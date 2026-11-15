//! Camera mutations are serialized by the store. Never automatically replay a PUT.
use super::{camera_http_base, with_timeout};
use crate::domain::{
    PackageResponse, UpdateError, UpdateIntent, UpdateKind, UpdateSnapshot, response_error,
};
use nsh_core::{
    platform::HttpRequest,
    prelude::{data_dir, fetch},
};
use std::{fs, time::Duration};

async fn request(
    method: &str,
    path: &str,
    body: Vec<u8>,
    budget: Duration,
) -> Result<serde_json::Value, UpdateError> {
    let mut request = HttpRequest::new(method, format!("{}{path}", camera_http_base()))
        .header("Cache-Control", "no-cache, no-store")
        .header("Content-Type", "application/zip")
        .body(body);
    request.follow_redirects = false;
    let response = with_timeout(fetch(request), budget)
        .await
        .ok_or(UpdateError::ConnectionLost)?
        .map_err(|_| UpdateError::ConnectionLost)?;
    if !response.ok() {
        return Err(response_error(response.status(), &response.text()));
    }
    serde_json::from_slice(&response.body)
        .map_err(|_| UpdateError::Service("The camera returned an invalid response.".into()))
}

pub async fn snapshot() -> Result<UpdateSnapshot, UpdateError> {
    serde_json::from_value(request("GET", "/v1/update", Vec::new(), Duration::from_secs(5)).await?)
        .map_err(|_| UpdateError::Service("The camera returned an unknown update state.".into()))
}

pub async fn package() -> Result<PackageResponse, UpdateError> {
    serde_json::from_value(request("GET", "/v1/package", Vec::new(), Duration::from_secs(5)).await?)
        .map_err(|_| UpdateError::Service("The camera package could not be read.".into()))
}

pub async fn stage(kind: UpdateKind, body: Vec<u8>) -> Result<(), UpdateError> {
    let path = if kind == UpdateKind::Install {
        "/v1/update"
    } else {
        "/v1/rollback"
    };
    // The camera verifies and writes the entire archive before replying.
    request("PUT", path, body, Duration::from_secs(180)).await?;
    Ok(())
}

pub async fn cancel() -> Result<(), UpdateError> {
    request("DELETE", "/v1/update", Vec::new(), Duration::from_secs(15)).await?;
    if snapshot().await?.state != crate::domain::JournalState::None {
        return Err(UpdateError::Service(
            "The camera still has a pending update.".into(),
        ));
    }
    Ok(())
}

pub fn load_intent() -> Option<UpdateIntent> {
    serde_json::from_slice(&fs::read(data_dir()?.join("pending_update.json")).ok()?).ok()
}

pub fn save_intent(intent: &UpdateIntent) -> Result<(), UpdateError> {
    let save = || -> Result<(), Box<dyn std::error::Error>> {
        let dir = data_dir().ok_or("App storage is unavailable")?;
        fs::create_dir_all(&dir)?;
        let temporary = dir.join("pending_update.tmp");
        fs::write(&temporary, serde_json::to_vec(intent)?)?;
        fs::rename(temporary, dir.join("pending_update.json"))?;
        Ok(())
    };
    save().map_err(|_| {
        UpdateError::Service("Could not save update recovery information. Try again.".into())
    })
}

pub fn clear_intent() {
    if let Some(dir) = data_dir() {
        let _ = fs::remove_file(dir.join("pending_update.json"));
    }
}
