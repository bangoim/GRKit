//! Camera Looks over HTTP (`/v1/looks`) and the official Look catalog.
//!
//! Each mutation carries the request, boot and card identities and the
//! resource ETag the camera expects. A mutation is never replayed
//! automatically: after an unknown result the store reads the Looks again.

use std::{fs, path::PathBuf, time::Duration};

use grkit_core::{camera, packaging};
use nsh_core::{
    element::HttpResponse,
    platform::HttpRequest,
    prelude::{data_dir, fetch, timer},
};

use super::camera_http_base;
use crate::domain::{
    CameraModel, Catalog, CatalogLook, LookError, LooksSnapshot, look_error, new_request_id,
    parse_catalog, parse_looks,
};

const READ_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to follow an accepted (202) change before reading the Looks again.
const RESULT_BUDGET: Duration = Duration::from_secs(30);
const CATALOG_LIMIT: usize = 4 * 1024 * 1024;
const GRF_LIMIT: usize = 16 * 1024;

/// The camera's boot and card identities from the last Looks read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraContext {
    pub boot_id: String,
    pub card_id: String,
}

impl From<&LooksSnapshot> for CameraContext {
    fn from(snapshot: &LooksSnapshot) -> Self {
        Self {
            boot_id: snapshot.boot_id.clone(),
            card_id: snapshot.card_id.clone(),
        }
    }
}

async fn request(
    method: &str,
    path: &str,
    body: Vec<u8>,
    budget: Duration,
    headers: &[(&str, String)],
) -> Result<HttpResponse, LookError> {
    super::camera_request(
        method,
        format!("{}{path}", camera_http_base()),
        body,
        budget,
        headers,
    )
    .await
    .map_err(|_| LookError::ConnectionLost)
}

fn failure(response: &HttpResponse) -> LookError {
    look_error(response.status(), &response.text())
}

/// The camera's Looks in menu order.
pub async fn list() -> Result<LooksSnapshot, LookError> {
    let response = request("GET", "/v1/looks", Vec::new(), READ_TIMEOUT, &[]).await?;
    if !response.ok() {
        return Err(failure(&response));
    }
    parse_looks(&response.body).map_err(LookError::Service)
}

fn cache_dir() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("look-cache-v1"))
}

fn checked_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A Look file from the cache, when one with this content hash is there.
pub fn cached(hash: &str) -> Option<Vec<u8>> {
    if !checked_hash(hash) {
        return None;
    }
    let bytes = fs::read(cache_dir()?.join(format!("{hash}.grf"))).ok()?;
    (packaging::sha256(&bytes) == hash).then_some(bytes)
}

fn store(hash: &str, bytes: &[u8]) {
    let Some(dir) = cache_dir() else { return };
    if !checked_hash(hash) {
        return;
    }
    let save = || -> std::io::Result<()> {
        fs::create_dir_all(&dir)?;
        let temporary = dir.join(format!("{hash}.tmp"));
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, dir.join(format!("{hash}.grf")))
    };
    if let Err(error) = save() {
        log::warn!("Could not cache a Look: {error}");
    }
}

/// The GRF bytes of a Look, from the cache when its hash matches.
pub async fn content(id: &str, content_hash: &str) -> Result<Vec<u8>, LookError> {
    if let Some(bytes) = cached(content_hash) {
        return Ok(bytes);
    }
    let response = request(
        "GET",
        &format!("/v1/looks/{id}/content"),
        Vec::new(),
        READ_TIMEOUT,
        &[],
    )
    .await?;
    if !response.ok() {
        return Err(failure(&response));
    }
    let bytes = response.body;
    if bytes.len() > GRF_LIMIT {
        return Err(LookError::Service("The Look file is too large.".into()));
    }
    let hash = packaging::sha256(&bytes);
    if hash != content_hash {
        // The Look changed after the list was read; show the new content.
        log::info!("Look {id} content changed since the list was read");
    }
    store(&hash, &bytes);
    Ok(bytes)
}

async fn etag(path: &str) -> Result<String, LookError> {
    let response = request("GET", path, Vec::new(), READ_TIMEOUT, &[]).await?;
    if !response.ok() {
        return Err(failure(&response));
    }
    response
        .header("ETag")
        .map(str::to_owned)
        .ok_or_else(|| LookError::Service("The camera response omitted an ETag.".into()))
}

/// Send one mutation and follow it to its result.
async fn mutate(
    method: &str,
    path: &str,
    body: Vec<u8>,
    content_type: Option<&str>,
    precondition: (&str, String),
    context: &CameraContext,
    results: &str,
) -> Result<(), LookError> {
    let request_id = new_request_id();
    let mut headers = vec![
        ("X-GRKit-Request-Id", request_id.clone()),
        ("X-GRKit-Boot-Id", context.boot_id.clone()),
        ("X-GRKit-Card-Id", context.card_id.clone()),
        precondition,
    ];
    if let Some(content_type) = content_type {
        headers.push(("Content-Type", content_type.to_owned()));
        headers.push(("X-GRKit-Content-SHA256", packaging::sha256(&body)));
    }
    let response = request(method, path, body, WRITE_TIMEOUT, &headers).await?;
    match response.status() {
        200 | 201 => Ok(()),
        202 => follow(&format!("{results}/{request_id}")).await,
        _ => Err(failure(&response)),
    }
}

/// Poll a request record until it finishes. A record that cannot be read
/// leaves the result unknown, which the caller treats as a changed camera.
async fn follow(path: &str) -> Result<(), LookError> {
    let started = std::time::Instant::now();
    loop {
        timer(Duration::from_millis(400)).await;
        let response = request("GET", path, Vec::new(), READ_TIMEOUT, &[]).await?;
        if !response.ok() {
            return Err(LookError::Changed);
        }
        let record: serde_json::Value =
            serde_json::from_slice(&response.body).map_err(|_| LookError::Changed)?;
        match record["status"].as_str() {
            Some("succeeded") => return Ok(()),
            Some("failed") | Some("cancelled") => {
                let status = record["error"]["status"].as_u64().unwrap_or(409) as u16;
                return Err(look_error(status, &response.text()));
            }
            _ if started.elapsed() > RESULT_BUDGET => return Err(LookError::Busy),
            _ => {}
        }
    }
}

fn json_body(value: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&value).expect("JSON encodes")
}

/// Show or hide a Look in the camera menu.
pub async fn set_enabled(
    context: &CameraContext,
    id: &str,
    enabled: bool,
) -> Result<(), LookError> {
    let path = format!("/v1/looks/{id}");
    let tag = etag(&path).await?;
    mutate(
        "PATCH",
        &path,
        json_body(serde_json::json!({ "enabled": enabled })),
        Some("application/json"),
        ("If-Match", tag),
        context,
        &format!("/v1/looks/{id}/requests"),
    )
    .await
}

/// Rename an imported Look.
pub async fn rename(context: &CameraContext, id: &str, name: &str) -> Result<(), LookError> {
    let path = format!("/v1/looks/{id}");
    let tag = etag(&path).await?;
    mutate(
        "PATCH",
        &path,
        json_body(serde_json::json!({ "name": name })),
        Some("application/json"),
        ("If-Match", tag),
        context,
        &format!("/v1/looks/{id}/requests"),
    )
    .await
}

/// Remove a Look. `active_off` switches the camera to Off when it is in use.
pub async fn delete(context: &CameraContext, id: &str, active_off: bool) -> Result<(), LookError> {
    let path = format!("/v1/looks/{id}");
    let tag = etag(&path).await?;
    let target = if active_off {
        format!("{path}?active=off")
    } else {
        path
    };
    mutate(
        "DELETE",
        &target,
        Vec::new(),
        None,
        ("If-Match", tag),
        context,
        &format!("/v1/looks/{id}/requests"),
    )
    .await
}

/// Replace the camera menu order with the complete list of IDs.
pub async fn set_order(context: &CameraContext, ids: &[String]) -> Result<(), LookError> {
    let path = "/v1/params/looks/order";
    let tag = etag(path).await?;
    mutate(
        "PUT",
        path,
        json_body(serde_json::json!({ "ids": ids })),
        Some("application/json"),
        ("If-Match", tag),
        context,
        "/v1/params/looks/requests",
    )
    .await
}

/// Add a Look, or replace an imported Look whose current content hash is
/// `replacing`.
pub async fn put_content(
    context: &CameraContext,
    id: &str,
    grf: Vec<u8>,
    replacing: Option<&str>,
) -> Result<(), LookError> {
    let precondition = match replacing {
        Some(hash) => ("If-Match", format!("\"sha256:{hash}\"")),
        None => ("If-None-Match", "*".to_owned()),
    };
    mutate(
        "PUT",
        &format!("/v1/looks/{id}/content"),
        grf,
        Some("application/octet-stream"),
        precondition,
        context,
        &format!("/v1/looks/{id}/requests"),
    )
    .await
}

fn site_base() -> String {
    super::config_value("GRKIT_RELEASE_BASE")
        .unwrap_or_else(|| "https://grkit.app".into())
        .trim_end_matches('/')
        .to_owned()
}

async fn site_get(path: &str, limit: usize) -> Result<Vec<u8>, String> {
    let mut request =
        HttpRequest::get(format!("{}/{path}", site_base())).header("Cache-Control", "no-cache");
    request.follow_redirects = false;
    let response = super::with_timeout(fetch(request), Duration::from_secs(15))
        .await
        .ok_or_else(|| format!("Timed out downloading {path}"))??;
    if !response.ok() {
        return Err(format!("HTTP {}: {path}", response.status()));
    }
    if response.body.len() > limit {
        return Err(format!("{path} exceeds its size limit"));
    }
    Ok(response.body)
}

fn camera_for(model: CameraModel) -> &'static camera::Camera {
    match model {
        CameraModel::Gr3 => &camera::GR3,
        CameraModel::Gr3x => &camera::GR3X,
        CameraModel::Gr4 => &camera::GR4,
    }
}

/// The catalog target of a camera firmware.
pub fn target(model: CameraModel, firmware: &str) -> Option<&'static str> {
    let camera = camera_for(model);
    camera
        .releases
        .iter()
        .find(|release| release.firmware == firmware)
        .map(|release| release.target)
        .or_else(|| camera.targets().first().copied())
}

/// The official Look catalog of a firmware target. A cached copy serves
/// when the phone is offline.
pub async fn catalog(target: &str) -> Result<Catalog, String> {
    let name = format!("catalog-{target}.json");
    let cache = cache_dir();
    match site_get(&format!("effects/{target}/catalog.json"), CATALOG_LIMIT).await {
        Ok(bytes) => {
            let catalog = parse_catalog(&bytes)?;
            if let Some(dir) = cache {
                let _ = fs::create_dir_all(&dir).and_then(|_| fs::write(dir.join(&name), &bytes));
            }
            Ok(catalog)
        }
        Err(error) => {
            let bytes = cache
                .and_then(|dir| fs::read(dir.join(&name)).ok())
                .ok_or(error)?;
            parse_catalog(&bytes)
        }
    }
}

/// The GRF of a catalog Look, checked against the catalog digest.
pub async fn catalog_file(target: &str, look: &CatalogLook) -> Result<Vec<u8>, String> {
    if let Some(bytes) = cached(&look.sha256) {
        return Ok(bytes);
    }
    let bytes = site_get(&format!("effects/{target}/{}", look.file), GRF_LIMIT).await?;
    if packaging::sha256(&bytes) != look.sha256 {
        return Err(format!("{} failed its integrity check", look.name));
    }
    store(&look.sha256, &bytes);
    Ok(bytes)
}
