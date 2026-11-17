//! Camera mutations are serialized by the store. Never automatically replay a PUT.
use super::camera_http_base;
use crate::domain::{
    JournalState, PackageResponse, RollbackResource, RollbackSnapshot, UpdateError, UpdateIntent,
    UpdateKind, UpdateResource, UpdateSnapshot, new_request_id, response_error,
};
use grkit_core::packaging;
use nsh_core::prelude::data_dir;
use std::{fs, time::Duration};

fn header_value(response: &nsh_core::element::HttpResponse, name: &str) -> Option<String> {
    response.header(name).map(str::to_owned)
}

fn context_from_response(
    response: &nsh_core::element::HttpResponse,
) -> Result<(String, String, String), UpdateError> {
    let etag = header_value(response, "ETag").ok_or_else(|| {
        UpdateError::Service("The camera response omitted an ETag.".into())
    })?;
    let boot_id = header_value(response, "X-GRKit-Boot-Id").ok_or_else(|| {
        UpdateError::Service("The camera response omitted a boot identity.".into())
    })?;
    let card_id = header_value(response, "X-GRKit-Card-Id").ok_or_else(|| {
        UpdateError::Service("The camera response omitted a card identity.".into())
    })?;
    Ok((etag, boot_id, card_id))
}

async fn raw_request(
    method: &str,
    path: &str,
    body: Vec<u8>,
    budget: Duration,
    headers: &[(&str, String)],
) -> Result<nsh_core::element::HttpResponse, UpdateError> {
    super::camera_request(
        method,
        format!("{}{path}", camera_http_base()),
        body,
        budget,
        headers,
    )
    .await
    .map_err(|_| UpdateError::ConnectionLost)
}

fn decode_json<T: serde::de::DeserializeOwned>(
    response: &nsh_core::element::HttpResponse,
) -> Result<T, UpdateError> {
    if !response.ok() {
        return Err(response_error(response.status(), &response.text()));
    }
    serde_json::from_slice(&response.body)
        .map_err(|_| UpdateError::Service("The camera returned an invalid response.".into()))
}

pub async fn snapshot() -> Result<UpdateSnapshot, UpdateError> {
    let response = raw_request("GET", "/v1/update", Vec::new(), Duration::from_secs(5), &[]).await?;
    if response.status() == 503 {
        return Err(UpdateError::Service(
            "The card is not ready. Try again.".into(),
        ));
    }
    let (etag, boot_id, card_id) = context_from_response(&response)?;
    let resource: UpdateResource = decode_json(&response)?;
    let package = PackageResponse {
        version: resource.inventory.package.version,
        modules: resource.inventory.package.modules,
        effects: resource.inventory.package.effects,
        left_out: resource.inventory.package.left_out,
        camera: None,
    };
    Ok(UpdateSnapshot {
        state: resource.inventory.state,
        package,
        etag,
        boot_id,
        card_id,
        request_id: resource.request.map(|r| r.request_id),
    })
}

pub async fn package() -> Result<PackageResponse, UpdateError> {
    let mut package = snapshot().await?.package;
    if let Ok(response) = raw_request("GET", "/v1/props", Vec::new(), Duration::from_secs(5), &[]).await
        && response.ok()
        && let Ok(props) = serde_json::from_slice::<crate::domain::PropsResponse>(&response.body)
    {
        package.camera = props.camera.map(|camera| crate::domain::PackageCamera {
            model: camera.model,
            firmware: camera.firmware_version,
        });
        if package.version.is_none() {
            package.version = props.grkit.version;
        }
    }
    Ok(package)
}

pub async fn rollback() -> Result<RollbackSnapshot, UpdateError> {
    let response = raw_request(
        "GET",
        "/v1/rollback",
        Vec::new(),
        Duration::from_secs(5),
        &[],
    )
    .await?;
    if response.status() == 503 {
        return Err(UpdateError::Service(
            "The card is not ready. Try again.".into(),
        ));
    }
    let (etag, boot_id, card_id) = context_from_response(&response)?;
    let resource: RollbackResource = decode_json(&response)?;
    Ok(RollbackSnapshot {
        available: resource.backup.available,
        allowed: resource.backup.allowed,
        reason: resource.backup.reason,
        version: resource.backup.version,
        target_identity: resource.backup.identity.map(|id| id.digest),
        etag,
        boot_id,
        card_id,
        request_id: resource.request.map(|r| r.request_id),
    })
}

pub async fn stage(
    kind: UpdateKind,
    body: Vec<u8>,
    etag: &str,
    boot_id: &str,
    card_id: &str,
) -> Result<String, UpdateError> {
    let request_id = new_request_id();
    let digest = packaging::sha256(&body);
    let response = match kind {
        UpdateKind::Install => {
            let headers = [
                ("Content-Type", "application/zip".into()),
                ("If-Match", etag.to_owned()),
                ("X-GRKit-Request-Id", request_id.clone()),
                ("X-GRKit-Boot-Id", boot_id.to_owned()),
                ("X-GRKit-Card-Id", card_id.to_owned()),
                ("X-GRKit-Content-SHA256", digest),
            ];
            // The camera verifies and writes the entire archive before replying.
            raw_request(
                "PUT",
                "/v1/update",
                body,
                Duration::from_secs(180),
                &headers,
            )
            .await?
        }
        UpdateKind::Rollback => {
            return Err(UpdateError::Service(
                "Rollback staging requires a target identity.".into(),
            ));
        }
    };
    if !(200..300).contains(&response.status()) {
        return Err(response_error(response.status(), &response.text()));
    }
    let recorded = serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|v| {
            v.get("requestId")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .unwrap_or(request_id);
    Ok(recorded)
}

pub async fn stage_rollback(
    target_identity: &str,
    etag: &str,
    boot_id: &str,
    card_id: &str,
) -> Result<String, UpdateError> {
    let request_id = new_request_id();
    let body = serde_json::to_vec(&serde_json::json!({ "targetIdentity": target_identity }))
        .map_err(|_| UpdateError::Service("Could not encode the rollback request.".into()))?;
    let headers = [
        ("Content-Type", "application/json".into()),
        ("If-Match", etag.to_owned()),
        ("X-GRKit-Request-Id", request_id.clone()),
        ("X-GRKit-Boot-Id", boot_id.to_owned()),
        ("X-GRKit-Card-Id", card_id.to_owned()),
        ("X-GRKit-Content-SHA256", packaging::sha256(&body)),
    ];
    let response = raw_request(
        "POST",
        "/v1/rollback",
        body,
        Duration::from_secs(30),
        &headers,
    )
    .await?;
    if !(200..300).contains(&response.status()) {
        return Err(response_error(response.status(), &response.text()));
    }
    let recorded = serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|v| {
            v.get("requestId")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .unwrap_or(request_id);
    Ok(recorded)
}

pub async fn cancel(
    kind: UpdateKind,
    target_request_id: &str,
    boot_id: &str,
    card_id: &str,
) -> Result<(), UpdateError> {
    let request_id = new_request_id();
    let path = match kind {
        UpdateKind::Install => "/v1/update/cancel",
        UpdateKind::Rollback => "/v1/rollback/cancel",
    };
    let body = serde_json::to_vec(&serde_json::json!({ "targetRequestId": target_request_id }))
        .map_err(|_| UpdateError::Service("Could not encode the cancel request.".into()))?;
    let headers = [
        ("Content-Type", "application/json".into()),
        ("X-GRKit-Request-Id", request_id),
        ("X-GRKit-Boot-Id", boot_id.to_owned()),
        ("X-GRKit-Card-Id", card_id.to_owned()),
        ("X-GRKit-Content-SHA256", packaging::sha256(&body)),
    ];
    let response = raw_request("POST", path, body, Duration::from_secs(15), &headers).await?;
    if !(200..300).contains(&response.status()) {
        return Err(response_error(response.status(), &response.text()));
    }
    if snapshot().await?.state != JournalState::None {
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
