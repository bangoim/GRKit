//! The v1 router. HTTP representations and snapshot watermarks share the
//! coordinator lock; transport work never holds this lock while receiving.
use crate::protocol::{check_precondition, Admission, Intent, RecordedResult, RequestCache};
use crate::update::{SoftwareDomain, SoftwareStatus};
use crate::{
    changes::{Changes, SharedChanges},
    protocol::{strong_etag, ChangeHub, Problem},
    Env, Files, Request, Response,
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    headers: Vec<(String, String)>,
}
impl Reply {
    pub fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            body: serde_json::to_vec(&body).expect("JSON value"),
            headers: vec![
                (
                    "Content-Type".into(),
                    if status >= 400 {
                        "application/problem+json"
                    } else {
                        "application/json"
                    }
                    .into(),
                ),
                ("Cache-Control".into(), "no-store".into()),
            ],
        }
    }
    pub fn binary(status: u16, body: Vec<u8>, content_type: &str) -> Self {
        Self {
            status,
            body,
            headers: vec![("Cache-Control".into(), "no-cache".into())],
        }
        .header("Content-Type", content_type.to_owned())
    }
    pub fn header(mut self, name: &str, value: String) -> Self {
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.bytes().any(|b| b < 32 || b == 127)
        {
            return Self::problem(Problem::new(
                500,
                "invalid_response_header",
                "The server could not construct a response header.",
            ));
        }
        self.headers
            .retain(|(key, _)| !key.eq_ignore_ascii_case(name));
        self.headers.push((name.to_owned(), value));
        self
    }
    fn recorded(result: RecordedResult) -> Self {
        let mut reply = Self::json(result.status, result.body.clone());
        if result.status == 202 {
            if let (Some(domain @ ("update" | "rollback")), Some(id)) = (
                result.body["domain"].as_str(),
                result.body["requestId"].as_str(),
            ) {
                if crate::protocol::valid_request_id(id) {
                    reply
                        .headers
                        .push(("Location".into(), format!("/v1/{domain}?requestId={id}")));
                }
            }
        }
        reply
    }
    pub fn problem(problem: Problem) -> Self {
        Self::json(
            problem.status,
            serde_json::to_value(problem).expect("problem JSON"),
        )
    }
    pub fn versioned(mut self) -> Self {
        self.headers.retain(|(name, _)| name != "Cache-Control");
        self.headers
            .push(("Cache-Control".into(), "no-cache".into()));
        self.headers.push(("ETag".into(), strong_etag(&self.body)));
        self
    }
    pub fn snapshot_headers(mut self, watermark: &crate::protocol::Watermark) -> Self {
        self.headers.extend([
            ("X-GRKit-Boot-Id".into(), watermark.boot_id.clone()),
            ("X-GRKit-Card-Id".into(), watermark.card_id.clone()),
            (
                "X-GRKit-Snapshot-Seq".into(),
                watermark.snapshot_seq.to_string(),
            ),
        ]);
        self
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.status,
            crate::reason(self.status),
            self.body.len()
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}
impl From<Response> for Reply {
    fn from(response: Response) -> Self {
        Self::json(response.status, response.body)
    }
}

/// This is an unresolved transaction, not evidence of removal or a new card.
#[derive(Debug)]
struct PendingRuntimeIdentity;
impl std::fmt::Display for PendingRuntimeIdentity {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Runtime identity is quarantined until its commit is reconciled")
    }
}
impl std::error::Error for PendingRuntimeIdentity {}

pub struct Service {
    pub changes: Changes,
    pub hub: SharedChanges,
    pub boot_id: String,
    requests: Mutex<RequestCache>,
    verified_card: Mutex<Option<String>>,
    card_identity: fn(&std::path::Path) -> anyhow::Result<String>,
    release_keys: Vec<[u8; 32]>,
    looks: crate::look_api::LookApi,
}
impl Service {
    pub fn new(boot_id: String) -> Self {
        Self::with_runtime_backend(
            boot_id,
            Box::new(crate::runtime_backend::UnavailableBackend),
        )
    }
    pub fn with_runtime_backend(
        boot_id: String,
        backend: Box<dyn crate::runtime_backend::RuntimeBackend>,
    ) -> Self {
        let hub = Arc::new(Mutex::new(ChangeHub::new(
            Value::Null,
            boot_id.clone(),
            "unavailable".into(),
            2,
            32,
            8192,
        )));
        Self {
            changes: Changes::new(Arc::clone(&hub)),
            hub,
            requests: Mutex::new(RequestCache::new(
                boot_id.clone(),
                64,
                8192,
                Duration::from_secs(900),
            )),
            verified_card: Mutex::new(None),
            card_identity: |_| {
                anyhow::bail!(
                    "Linux mounts the card through the firmware and has no identity of its own"
                )
            },
            release_keys: grkit_core::release::OFFICIAL_KEYS.to_vec(),
            looks: crate::look_api::LookApi::new(backend),
            boot_id,
        }
    }
    /// The runtime backend identifies the card (`identity::public_card_id`).
    /// When another source also identifies it, both must agree.
    pub fn current_card_id(&self, root: &std::path::Path) -> anyhow::Result<String> {
        let kernel = (self.card_identity)(root);
        match self.looks.card_identity() {
            Ok(owner) => {
                if let Ok(kernel) = kernel {
                    anyhow::ensure!(
                        kernel == owner,
                        "The runtime and Linux identify different cards"
                    );
                }
                Ok(owner)
            }
            Err(error)
                if matches!(
                    error.code,
                    "runtime_unavailable" | "runtime_protocol_unsupported"
                ) =>
            {
                kernel.map_err(|kernel| anyhow::anyhow!("{kernel:#}; runtime: {}", error.detail))
            }
            Err(error)
                if matches!(
                    error.code,
                    "recovery_required" | "runtime_busy" | "startup_reconciliation_unsupported"
                ) =>
            {
                Err(PendingRuntimeIdentity.into())
            }
            Err(error) => anyhow::bail!("Runtime identity is uncertain: {}", error.detail),
        }
    }

    /// Preserve the request namespace during an uncertain commit, while still
    /// returning no verified identity to callers. Only a fresh backend receipt
    /// can authorize recovery; cached identity never enables resource writes.
    pub fn poll_card_identity(&self, root: &std::path::Path) -> Result<Option<String>, Problem> {
        let identity = self.current_card_id(root);
        if identity
            .as_ref()
            .is_err_and(|error| error.is::<PendingRuntimeIdentity>())
        {
            return Ok(None);
        }
        let identity = identity.ok();
        self.observe_card(identity.clone())?;
        Ok(identity)
    }

    /// Records the card a request found; a change resets the Looks session.
    pub fn observe_card(&self, card_id: Option<String>) -> Result<(), Problem> {
        let mut verified = self.verified_card.lock().map_err(|_| {
            Problem::new(
                503,
                "coordinator_unavailable",
                "The coordinator is unavailable.",
            )
        })?;
        let different_medium = card_id.as_ref().is_some_and(|current| {
            verified
                .as_ref()
                .is_some_and(|previous| previous != current)
        });
        if let Some(current) = &card_id {
            *verified = Some(current.clone());
        }
        let mut hub = self.hub.lock().map_err(|_| {
            Problem::new(
                503,
                "coordinator_unavailable",
                "The coordinator is unavailable.",
            )
        })?;
        let (_, watermark) = hub.snapshot();
        let card_id = card_id.unwrap_or_else(|| "unavailable".into());
        let changed = watermark.card_id != card_id;
        if changed {
            hub.replace_card(card_id, Value::Null)?;
        }
        drop(hub);
        if changed {
            if different_medium {
                self.looks.accept_new_card()?;
            } else {
                self.looks.invalidate_session()?;
            }
        }
        Ok(())
    }
    /// Called under the card transaction lock of the HTTP request.
    pub fn observe_software(&self, card: &crate::update::Card) -> anyhow::Result<()> {
        let Some(request) = card.software_request()? else {
            return Ok(());
        };
        let mut hub = self
            .hub
            .lock()
            .map_err(|_| anyhow::anyhow!("Coordinator unavailable"))?;
        let (mut state, watermark) = hub.snapshot();
        anyhow::ensure!(
            request.card_id == watermark.card_id && watermark.card_id != "unavailable",
            "The persisted software request belongs to a different card"
        );
        let summary = software_summary(&request);
        if request.boot_id == self.boot_id {
            if let Ok(mut cache) = self.requests.lock() {
                let running = request.status == SoftwareStatus::Running;
                let _ = cache.update(
                    &request.request_id,
                    RecordedResult {
                        status: if running { 202 } else { 200 },
                        body: summary.clone(),
                    },
                    !running,
                    Instant::now(),
                );
            }
        }
        if state["softwareRequest"] == summary {
            return Ok(());
        }
        if !state.is_object() {
            state = json!({});
        }
        state["softwareRequest"] = summary.clone();
        let domain = if request.domain == SoftwareDomain::Update {
            "update"
        } else {
            "rollback"
        };
        let event = json!({"type":"requestChanged","domain":domain,"requestId":request.request_id,"queryUri":format!("/v1/{domain}?requestId={}",request.request_id),"request":summary});
        hub.publish(state, event.as_object().expect("event").clone())
            .map_err(|error| anyhow::anyhow!(error.detail))?;
        Ok(())
    }
    pub fn handle(&self, request: &Request, body: &[u8], env: &Env<impl Files>) -> Reply {
        if request.method == "GET" && request.path == "/v1/ping" {
            return Reply::json(200, json!({"protocol":1,"bootId":self.boot_id}));
        }
        // A persisted software UUID remains a recovery handle across boots.
        // It cannot be rebound to a new mutation body or another domain.
        if request.method != "GET" {
            if let Some(id) = request.headers.get("x-grkit-request-id") {
                if let Ok(Some(previous)) = env.card.software_request() {
                    if previous.boot_id != self.boot_id
                        && previous.request_id.eq_ignore_ascii_case(id)
                        && self.current_card_id(env.card.root()).ok().as_deref()
                            == Some(&previous.card_id)
                    {
                        return Reply::problem(Problem::new(
                            409,
                            "request_id_conflict",
                            "This persisted request ID is query-only after a service restart. Use a new ID for a new mutation.",
                        ));
                    }
                }
            }
        }
        let route = request.path.strip_prefix("/v1/").unwrap_or("");
        if route == "looks" || route.starts_with("looks/") || route.starts_with("params/looks/") {
            let card_id = match self.poll_card_identity(env.card.root()) {
                Ok(identity) => identity,
                Err(error) => return Reply::problem(error),
            };
            let query = request.method == "GET" && request.path.contains("/requests/");
            let query_card = card_id.clone().or_else(|| {
                if query && self.looks.recovery_required() {
                    self.hub
                        .lock()
                        .ok()
                        .map(|hub| hub.snapshot().1.card_id)
                        .filter(|id| id != "unavailable")
                } else {
                    None
                }
            });
            let device = if request.method == "GET" && !query {
                None
            } else {
                (env.device)().ok()
            };
            let device_id = device
                .as_ref()
                .and_then(|camera| camera.serial_number.as_ref())
                .map(|serial| grkit_core::packaging::sha256(serial.as_bytes()));
            if request.method != "GET" && (card_id.is_none() || device_id.is_none()) {
                return unavailable(
                    "identity_unavailable",
                    "Read verified camera and card identities before writing.",
                );
            }
            let pending = env.card.state().map_or(true, |state| state != "none")
                || env.card.software_request().map_or(true, |request| {
                    request.is_some_and(|request| request.status == SoftwareStatus::Running)
                });
            let context = crate::look_api::RequestContext {
                device_id: device_id.as_deref().unwrap_or("unavailable"),
                boot_id: &self.boot_id,
                card_id: query_card.as_deref().unwrap_or("unavailable"),
                write_allowed: device_id.is_some() && card_id.is_some(),
                software_pending: pending,
            };
            if let Some(reply) =
                self.looks
                    .handle(request, body, &context, &self.hub, &self.requests)
            {
                return reply;
            }
        }

        match (request.method.as_str(), route) {
            ("GET", "props") => self.props(env),
            ("GET", "debug/revisions") => Reply::json(
                200,
                json!({"protocol":1,"service":env!("CARGO_PKG_VERSION"),"runtime":null}),
            ),
            ("GET", "debug/diagnostics") => crate::respond(request, body, env).into(),
            ("GET", "update" | "rollback") => {
                if let Some(query) = &request.query {
                    self.request_result(route, query, env)
                } else {
                    self.software(route, env)
                }
            }
            ("PUT", "update") | ("POST", "rollback" | "update/cancel" | "rollback/cancel") => {
                self.mutate_software(request, body, env)
            }
            _ => Reply::problem(Problem::new(
                404,
                "not_found",
                "The resource does not exist.",
            )),
        }
    }
    /// Another verified identity than the one a request started with. An
    /// identity that becomes known, or that a busy runtime cannot confirm, is
    /// no change of card.
    fn card_changed(&self, env: &Env<impl Files>, card_id: Option<&str>) -> bool {
        self.current_card_id(env.card.root())
            .ok()
            .is_some_and(|current| card_id.is_some_and(|card_id| card_id != current))
    }
    fn props(&self, env: &Env<impl Files>) -> Reply {
        let card_id = match self.poll_card_identity(env.card.root()) {
            Ok(identity) => identity,
            Err(error) => return Reply::problem(error),
        };
        if card_id.is_none() && self.looks.recovery_required() {
            return unavailable(
                "recovery_required",
                "Reconcile the pending runtime request before reading a new resource snapshot.",
            );
        }
        let readable = env.card.ensure_readable().is_ok();
        let runtime = match card_id.as_deref() {
            Some(id) => self.looks.runtime_properties(id).unwrap_or_else(|error| {
                json!({"state":if self.looks.recovery_required() {"recoveryRequired"} else {"unavailable"},"busyReason":error.code})
            }),
            None => json!({"state":"unavailable","busyReason":"card_identity_unavailable"}),
        };
        let looks_capabilities = runtime
            .get("capabilities")
            .cloned()
            .unwrap_or_else(|| json!({"read":false,"write":false,"hotReload":false}));
        let camera = (env.device)().ok();
        if self.card_changed(env, card_id.as_deref()) {
            return Reply::problem(Problem::new(
                409,
                "context_changed",
                "The card changed while reading properties.",
            ));
        }
        let active = env
            .card
            .software_request()
            .ok()
            .flatten()
            .filter(|request| {
                request.status == SoftwareStatus::Running
                    && card_id.as_deref() == Some(&request.card_id)
            })
            .map(|request| software_summary(&request));
        let props = json!({
            "deviceId":camera.as_ref().and_then(|c| c.serial_number.as_ref()).map(|serial| format!("camera-{}", grkit_core::packaging::sha256(serial.as_bytes()))),"bootId":self.boot_id,"cardId":card_id,"revision":null,
            "camera":camera.as_ref().map(|c| json!({"model":c.model,"firmwareVersion":c.firmware})),
            "grkit":{"version":env.card.package().ok().and_then(|package|package["version"].as_str().map(str::to_owned))},
            "storage":{"state":if readable {"ready"} else {"unavailable"},"writable":readable && card_id.is_some(),"freeBytes":camera.as_ref().and_then(|c| c.free_bytes)},
            "runtime":runtime,
            "writeAccess":{"allowed":readable && card_id.is_some() && camera.as_ref().is_some_and(|c|c.serial_number.is_some()),"reason":if readable && card_id.is_some() && camera.as_ref().is_some_and(|c|c.serial_number.is_some()) {None} else {Some("identity_unavailable")}},
            "capabilities":{"looks":looks_capabilities,"recipes":{"read":false,"write":false},"moduleSettings":{"read":false,"write":false}},
            "limits":{"loadedLooks":40,"listedLooks":64,"lookNameAsciiCharacters":16,"maxLookBytes":8704,"maxPackageBytes":crate::update::MAX_PACKAGE},
            "activeRequests":active.into_iter().collect::<Vec<_>>()
        });
        let mut hub = match self.hub.lock() {
            Ok(hub) => hub,
            Err(_) => {
                return unavailable("coordinator_unavailable", "The coordinator is unavailable.");
            }
        };
        if self.card_changed(env, card_id.as_deref()) {
            return Reply::problem(Problem::new(
                409,
                "context_changed",
                "The card changed before snapshot publication.",
            ));
        }
        let (previous, watermark) = hub.snapshot();
        let mut state = if previous.is_object() {
            previous.clone()
        } else {
            json!({})
        };
        state["props"] = props.clone();
        let current_card = card_id.unwrap_or_else(|| "unavailable".into());
        let published = if watermark.card_id != current_card {
            hub.replace_card(current_card, json!({"props":props}))
        } else if previous["props"] != props {
            hub.publish(
                state,
                json!({"type":"resourceChanged","changed":"props"})
                    .as_object()
                    .expect("object")
                    .clone(),
            )
        } else {
            Ok(watermark.snapshot_seq)
        };
        if let Err(error) = published {
            return Reply::problem(error);
        }
        let (state, watermark) = hub.snapshot();
        let mut snapshot = state["props"].clone();
        snapshot["snapshotSeq"] = json!(watermark.snapshot_seq);
        // This watermark covers the published service view. Runtime fields stay
        // unavailable until the runtime bridge provides a coherent snapshot.
        Reply::json(200, snapshot).snapshot_headers(&watermark)
    }
    fn software(&self, route: &str, env: &Env<impl Files>) -> Reply {
        let card_id = match self.poll_card_identity(env.card.root()) {
            Ok(identity) => identity,
            Err(error) => return Reply::problem(error),
        };
        if card_id.is_none() && self.looks.recovery_required() {
            return unavailable(
                "recovery_required",
                "Reconcile the pending runtime request before reading a new resource snapshot.",
            );
        }
        let configuration_revision = match card_id
            .as_deref()
            .map(|id| self.looks.runtime_properties(id))
        {
            Some(Ok(runtime)) => runtime.get("configurationRevision").cloned(),
            Some(Err(error))
                if error.code != "runtime_unavailable"
                    && error.code != "runtime_protocol_unsupported" =>
            {
                return Reply::problem(error);
            }
            _ => None,
        };
        let inventory = match env.card.inventory() {
            Ok(value) => value,
            Err(error) => return unavailable("card_unavailable", &format!("{error:#}")),
        };
        let request = match env.card.software_request() {
            Ok(value) => value
                .filter(|request| {
                    (request.domain == SoftwareDomain::Update) == (route == "update")
                        && card_id.as_deref() == Some(&request.card_id)
                })
                .map(|request| software_summary(&request)),
            Err(error) => return unavailable("recovery_required", &format!("{error:#}")),
        };
        let value = if route == "rollback" {
            match env.card.rollback_availability_with_keys(&self.release_keys) {
                Ok(backup) => {
                    json!({"backup":backup,"request":request,"configurationRevision":configuration_revision})
                }
                Err(error) => return unavailable("card_unavailable", &format!("{error:#}")),
            }
        } else {
            json!({"inventory":inventory,"request":request,"configurationRevision":configuration_revision})
        };
        let reply = Reply::json(200, value.clone()).versioned();
        let mut hub = match self.hub.lock() {
            Ok(hub) => hub,
            Err(_) => {
                return unavailable("coordinator_unavailable", "The coordinator is unavailable.");
            }
        };
        if self.card_changed(env, card_id.as_deref()) {
            return Reply::problem(Problem::new(
                409,
                "context_changed",
                "The card changed before snapshot publication.",
            ));
        }
        let current_card = card_id.unwrap_or_else(|| "unavailable".into());
        let (mut state, mark) = hub.snapshot();
        if mark.card_id != current_card {
            if let Err(error) = hub.replace_card(current_card, Value::Null) {
                return Reply::problem(error);
            }
            state = Value::Null;
        }
        if state[route] != value {
            if !state.is_object() {
                state = json!({});
            }
            state[route] = value;
            let event = json!({"type":"resourceChanged","changed":route,"revision":strong_etag(&reply.body)});
            if let Err(error) = hub.publish(state, event.as_object().expect("event").clone()) {
                return Reply::problem(error);
            }
        }
        reply.snapshot_headers(&hub.snapshot().1)
    }
    fn request_result(&self, domain: &str, query: &str, env: &Env<impl Files>) -> Reply {
        let Some(id) = query
            .strip_prefix("requestId=")
            .filter(|id| crate::protocol::valid_request_id(id))
        else {
            return Reply::problem(Problem::new(
                400,
                "invalid_query",
                "Use one UUID requestId query.",
            ));
        };
        let card_id = match self.current_card_id(env.card.root()) {
            Ok(card_id) => card_id,
            Err(error) => return unavailable("card_identity_unavailable", &format!("{error:#}")),
        };
        match env.card.software_request() {
            Ok(Some(request))
                if request.card_id == card_id
                    && request.request_id.eq_ignore_ascii_case(id)
                    && ((request.domain == SoftwareDomain::Update && domain == "update")
                        || (request.domain == SoftwareDomain::Rollback
                            && domain == "rollback")) =>
            {
                Reply::json(200, software_summary(&request))
            }
            Ok(_) => {
                if let Ok(cache) = self.requests.lock() {
                    if let Ok(result) = cache.get_for_card(id, &card_id, Instant::now()) {
                        if result.body["domain"] == domain {
                            return if result.status >= 400 {
                                Reply::json(
                                    200,
                                    json!({"requestId":id,"domain":domain,"status":"failed","phase":"completed","cancellable":false,"result":null,"error":result.body}),
                                )
                            } else {
                                Reply::recorded(result.clone())
                            };
                        }
                    }
                }
                Reply::problem(Problem::new(
                    404,
                    "request_result_unknown",
                    "Read the resource to reconcile this request.",
                ))
            }
            Err(error) => unavailable("recovery_required", &format!("{error:#}")),
        }
    }

    /// The checks of a software mutation that need no body. The transport
    /// runs them before it receives a package of several megabytes, and
    /// answers at once when one fails. The request repeats them all after
    /// the body. A known request ID is a replay, which the request answers.
    pub fn precheck(&self, request: &Request, env: &Env<impl Files>) -> Option<Reply> {
        let domain = match (request.method.as_str(), request.path.as_str()) {
            ("PUT", "/v1/update") => "update",
            ("POST", "/v1/rollback") => "rollback",
            _ => return None,
        };
        let header = |name: &str| request.headers.get(name).map(String::as_str);
        let Some(id) =
            header("x-grkit-request-id").filter(|id| crate::protocol::valid_request_id(id))
        else {
            return Some(Reply::problem(Problem::new(
                400,
                "invalid_request_id",
                "Supply a UUID request ID.",
            )));
        };
        if self
            .requests
            .lock()
            .is_ok_and(|cache| cache.get(id, Instant::now()).is_ok())
        {
            return None;
        }
        let Ok(card_id) = self.current_card_id(env.card.root()) else {
            return Some(unavailable(
                "card_identity_unavailable",
                "The card identity is unavailable.",
            ));
        };
        if header("x-grkit-boot-id") != Some(&self.boot_id)
            || header("x-grkit-card-id") != Some(&card_id)
        {
            return Some(Reply::problem(Problem::new(
                409,
                "context_changed",
                "Read current boot and card identities before writing.",
            )));
        }
        match (env.device)() {
            Ok(device) if device.serial_number.is_some() => {}
            Ok(_) => {
                return Some(unavailable(
                    "device_identity_unavailable",
                    "The camera identity is unavailable.",
                ))
            }
            Err(error) => return Some(unavailable("camera_unavailable", &format!("{error:#}"))),
        }
        if self.looks.recovery_required() {
            return Some(unavailable(
                "recovery_required",
                "Recover the Looks transaction before changing software.",
            ));
        }
        let resource = self.software(domain, env);
        if resource.status != 200 {
            return Some(resource);
        }
        if let Err(error) = check_precondition(
            Some(&strong_etag(&resource.body)),
            header("if-match"),
            header("if-none-match"),
        ) {
            return Some(Reply::problem(error));
        }
        software_pending(env.card)
    }
    fn mutate_software(&self, request: &Request, body: &[u8], env: &Env<impl Files>) -> Reply {
        let header = |name: &str| request.headers.get(name).map(String::as_str);
        let Some(id) =
            header("x-grkit-request-id").filter(|id| crate::protocol::valid_request_id(id))
        else {
            return Reply::problem(Problem::new(
                400,
                "invalid_request_id",
                "Supply a UUID request ID.",
            ));
        };
        let card_id = match self.current_card_id(env.card.root()) {
            Ok(id) => id,
            Err(_) => {
                return unavailable(
                    "card_identity_unavailable",
                    "The card identity is unavailable.",
                );
            }
        };
        if header("x-grkit-boot-id") != Some(&self.boot_id)
            || header("x-grkit-card-id") != Some(&card_id)
        {
            return Reply::problem(Problem::new(
                409,
                "context_changed",
                "Read current boot and card identities before writing.",
            ));
        }
        let device = match (env.device)() {
            Ok(device) => device,
            Err(error) => return unavailable("camera_unavailable", &format!("{error:#}")),
        };
        let Some(serial) = &device.serial_number else {
            return unavailable(
                "device_identity_unavailable",
                "The camera identity is unavailable.",
            );
        };
        let domain = if request.path.starts_with("/v1/update") {
            "update"
        } else {
            "rollback"
        };
        let digest = grkit_core::packaging::sha256(body);
        if request.method == "PUT"
            && (header("x-grkit-content-sha256") != Some(&digest)
                || !request.headers.contains_key("content-length"))
        {
            return Reply::problem(Problem::new(
                422,
                "content_digest_mismatch",
                "Supply the complete package length and SHA-256.",
            ));
        }
        let intent = Intent {
            domain: domain.into(),
            device_id: grkit_core::packaging::sha256(serial.as_bytes()),
            boot_id: self.boot_id.clone(),
            card_id: card_id.clone(),
            method: request.method.clone(),
            uri: request.query.as_ref().map_or_else(
                || request.path.clone(),
                |query| format!("{}?{query}", request.path),
            ),
            if_match: header("if-match").map(str::to_owned),
            if_none_match: header("if-none-match").map(str::to_owned),
            content_sha256: digest,
        };
        let initial = RecordedResult {
            status: 202,
            body: json!({"requestId":id,"domain":domain,"status":"running","phase":"validating"}),
        };
        let admission = match self.requests.lock() {
            Ok(mut cache) => cache.admit(id, intent, &card_id, initial, Instant::now()),
            Err(_) => {
                return unavailable("coordinator_unavailable", "The coordinator is unavailable.");
            }
        };
        match admission {
            Ok(Admission::Existing(result)) => return Reply::recorded(result),
            Ok(Admission::New) => {}
            Err(error) => return Reply::problem(error),
        }
        let execute = || -> Reply {
            if request.query.is_some() {
                return Reply::problem(Problem::new(
                    400,
                    "invalid_query",
                    "This mutation accepts no query.",
                ));
            }
            if self.current_card_id(env.card.root()).ok().as_deref() != Some(&card_id) {
                return Reply::problem(Problem::new(
                    409,
                    "context_changed",
                    "The card changed before admission.",
                ));
            }
            if self.looks.recovery_required() && !request.path.ends_with("/cancel") {
                return unavailable(
                    "recovery_required",
                    "Recover the Looks transaction before changing software.",
                );
            }
            let resource = self.software(domain, env);
            if resource.status != 200 {
                return resource;
            }
            if self.looks.recovery_required() && !request.path.ends_with("/cancel") {
                return unavailable(
                    "recovery_required",
                    "Recover the Looks transaction before changing software.",
                );
            }
            if !request.path.ends_with("/cancel") {
                if let Err(error) = check_precondition(
                    Some(&strong_etag(&resource.body)),
                    header("if-match"),
                    header("if-none-match"),
                ) {
                    return Reply::problem(error);
                }
            }
            if !request.path.ends_with("/cancel") {
                if let Some(reply) = software_pending(env.card) {
                    return reply;
                }
            }
            let context = crate::update::SoftwareContext {
                request_id: id.to_ascii_lowercase(),
                boot_id: self.boot_id.clone(),
                card_id: card_id.clone(),
            };
            let outcome = if request.method == "PUT" {
                crate::update::stage_request(env.card, body, &device, &self.release_keys, &context)
            } else {
                let value = match grkit_core::effects::parse_json(body) {
                    Ok(value) => value,
                    Err(_) => {
                        return Reply::problem(Problem::new(
                            422,
                            "invalid_json",
                            "Supply a JSON object.",
                        ));
                    }
                };
                let key = if request.path.ends_with("/cancel") {
                    "targetRequestId"
                } else {
                    "targetIdentity"
                };
                let Some(target) = value
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|_| value.as_object().is_some_and(|map| map.len() == 1))
                else {
                    return Reply::problem(Problem::new(
                        422,
                        "invalid_fields",
                        "Supply only the documented target field.",
                    ));
                };
                if request.path.ends_with("/cancel") {
                    match env.card.software_request() {
                        Ok(Some(target)) if target.card_id != card_id => {
                            return Reply::problem(Problem::new(
                                409,
                                "context_changed",
                                "The target request belongs to a different card.",
                            ));
                        }
                        Err(error) => {
                            return unavailable("recovery_required", &format!("{error:#}"));
                        }
                        _ => {}
                    }
                    env.card.cancel_software_request(
                        target,
                        if domain == "update" {
                            SoftwareDomain::Update
                        } else {
                            SoftwareDomain::Rollback
                        },
                    )
                } else {
                    crate::update::stage_rollback_request_with_keys(
                        env.card,
                        target,
                        &context,
                        &self.release_keys,
                    )
                }
            };
            match outcome {
                Ok(result) => {
                    if request.path.ends_with("/cancel") {
                        if result.boot_id == self.boot_id {
                            if let Ok(mut cache) = self.requests.lock() {
                                let _ = cache.update(
                                    &result.request_id,
                                    RecordedResult {
                                        status: 200,
                                        body: software_summary(&result),
                                    },
                                    true,
                                    Instant::now(),
                                );
                            }
                        }
                        let _ = self.observe_software(env.card);
                        return Reply::json(
                            200,
                            json!({"requestId":id,"domain":domain,"status":"succeeded","phase":"completed","targetRequest":software_summary(&result)}),
                        );
                    }
                    let status = if result.status == SoftwareStatus::Running {
                        202
                    } else {
                        200
                    };
                    let mut reply = Reply::json(status, software_summary(&result));
                    if status == 202 {
                        reply.headers.push((
                            "Location".into(),
                            format!("/v1/{domain}?requestId={}", result.request_id),
                        ));
                    }
                    reply
                }
                Err(error) => {
                    // A failed journal publication can follow a durable request
                    // checkpoint. Report that request instead of declaring failure.
                    match env.card.software_request() {
                        Ok(Some(record))
                            if recovered_cancel_matches(request, body, &card_id, &record) =>
                        {
                            let _ = self.observe_software(env.card);
                            Reply::json(
                                200,
                                json!({"requestId":id,"domain":domain,"status":"succeeded",
                                "phase":"completed","targetRequest":software_summary(&record)}),
                            )
                        }
                        Ok(Some(record)) if record.request_id.eq_ignore_ascii_case(id) => {
                            Reply::recorded(RecordedResult {
                                status: if record.status == SoftwareStatus::Running {
                                    202
                                } else {
                                    200
                                },
                                body: software_summary(&record),
                            })
                        }
                        Err(recovery) => Reply::recorded(RecordedResult {
                            status: 202,
                            body: json!({"requestId":id,"domain":domain,"status":"running","phase":"recovering","cancellable":false,"error":{"code":"recovery_required","detail":format!("{recovery:#}")}}),
                        }),
                        _ => Reply::problem(Problem::new(
                            if domain == "rollback" { 409 } else { 422 },
                            "software_request_rejected",
                            format!("{error:#}"),
                        )),
                    }
                }
            }
        };
        let mut reply = execute();
        // Publish the actual software view before the request event. This also
        // exposes journal state changes without assuming the install completed.
        if reply.status < 400 {
            let _ = self.software(domain, env);
        }
        if reply.status >= 400 {
            let mut problem: Value = serde_json::from_slice(&reply.body).expect("problem JSON");
            problem["requestId"] = json!(id);
            problem["domain"] = json!(domain);
            reply.body = serde_json::to_vec(&problem).expect("problem JSON");
        }
        if let Ok(mut cache) = self.requests.lock() {
            let _ = cache.update(
                id,
                RecordedResult {
                    status: reply.status,
                    body: serde_json::from_slice(&reply.body).expect("JSON reply"),
                },
                reply.status != 202,
                Instant::now(),
            );
        }
        if let Ok(mut hub) = self.hub.lock() {
            let (state, watermark) = hub.snapshot();
            if watermark.card_id != card_id {
                // The original result remains scoped to its original card in
                // the cache. Never attach that result to the replacement card.
                return reply;
            }
            let response: Value = serde_json::from_slice(&reply.body).expect("JSON reply");
            let summary = if reply.status >= 400 {
                json!({"requestId":id,"domain":domain,"status":"failed","phase":"completed","cancellable":false,"result":null,"error":response})
            } else {
                response
            };
            let event = json!({"type":"requestChanged","requestId":id,"domain":domain,"queryUri":format!("/v1/{domain}?requestId={id}"),"request":summary});
            let _ = hub.publish(state, event.as_object().expect("event object").clone());
        }
        reply
    }
}
/// An append/readback error can occur after cancellation is already durable.
/// Reconcile the target identity, not the cancellation request's distinct UUID.
fn recovered_cancel_matches(
    request: &Request,
    body: &[u8],
    card_id: &str,
    target: &crate::update::SoftwareRequest,
) -> bool {
    let domain = match request.path.as_str() {
        "/v1/update/cancel" => SoftwareDomain::Update,
        "/v1/rollback/cancel" => SoftwareDomain::Rollback,
        _ => return false,
    };
    target.status == SoftwareStatus::Cancelled
        && target.domain == domain
        && target.card_id == card_id
        && grkit_core::effects::parse_json(body).ok().and_then(|body| {
            body.get("targetRequestId")
                .and_then(Value::as_str)
                .map(|id| id.eq_ignore_ascii_case(&target.request_id))
        }) == Some(true)
}

fn software_summary(request: &crate::update::SoftwareRequest) -> Value {
    use crate::update::SoftwarePhase;
    json!({"requestId":request.request_id,"domain":request.domain,"bootId":request.boot_id,"cardId":request.card_id,"status":request.status,"phase":request.phase,"progress":null,"cancellable":request.cancellable,
    "requiredAction":match request.phase {SoftwarePhase::AwaitingConfirmation => Some("confirmOnCamera"),SoftwarePhase::AwaitingRestart => Some("restartCamera"),_=>None},
    "result":if request.status==SoftwareStatus::Succeeded {json!({"identity":request.expected.digest})} else {Value::Null},
    "error":request.error.as_deref().map(|code| json!({"code":code,"detail":match code {
        crate::update::CONFIRMATION_EXPIRED => "Nobody confirmed the request on the camera in time.",
        crate::update::STAGING_FAILED => "The update could not be written to the card.",
        _ => "The request failed.",
    }}))})
}

/// A request that waits on the camera, or an earlier one whose outcome is
/// not yet known, blocks a new update or rollback.
fn software_pending(card: &crate::update::Card) -> Option<Reply> {
    let pending = || {
        Some(Reply::problem(Problem::new(
            409,
            "software_pending",
            "Finish or cancel the pending software request first.",
        )))
    };
    match card.state() {
        Ok("none") => {}
        Ok(_) => return pending(),
        Err(error) => return Some(unavailable("card_not_ready", &format!("{error:#}"))),
    }
    match card.software_request() {
        Ok(Some(previous)) if previous.status == SoftwareStatus::Running => pending(),
        Ok(_) => None,
        Err(error) => Some(unavailable("recovery_required", &format!("{error:#}"))),
    }
}

fn unavailable(code: &'static str, detail: &str) -> Reply {
    Reply::problem(Problem::new(503, code, detail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use grkit_core::{camera, packaging, testing};
    const REQUEST: &str = "550e8400-e29b-41d4-a716-446655440000";
    const CANCEL: &str = "650e8400-e29b-41d4-a716-446655440000";
    fn service() -> Service {
        let mut service = Service::new("boot-test".into());
        service.card_identity = |_| Ok("card-test".into());
        service.release_keys = vec![testing::release_key()];
        service
    }
    fn card() -> (tempfile::TempDir, crate::update::Card) {
        let root = tempfile::tempdir().unwrap();
        let zip = testing::signed_package(&camera::GR3, "1.6.2", &["first"]);
        for (path, bytes) in packaging::card_files(&zip).unwrap() {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let card = crate::update::Card::new(root.path());
        (root, card)
    }
    fn call(
        service: &Service,
        card: &crate::update::Card,
        method: &str,
        path: &str,
        body: &[u8],
        id: &str,
        etag: Option<&str>,
    ) -> Reply {
        let mut request = crate::parse(
            format!(
                "{method} {path} HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
        for (key, value) in [
            ("x-grkit-request-id", id.to_owned()),
            ("x-grkit-boot-id", service.boot_id.clone()),
            ("x-grkit-card-id", "card-test".into()),
            ("x-grkit-content-sha256", packaging::sha256(body)),
        ] {
            request.headers.insert(key.into(), value);
        }
        if let Some(etag) = etag {
            request.headers.insert("if-match".into(), etag.into());
        }
        if method == "PATCH" {
            request
                .headers
                .insert("content-type".into(), "application/json".into());
        }
        service.handle(
            &request,
            body,
            &Env {
                files: &crate::System,
                connection: &crate::Connection {
                    local: "127.0.0.1:8080".into(),
                    peer: "127.0.0.1:50000".into(),
                },
                card,
                device: &|| {
                    Ok(crate::update::Device {
                        model: "RICOH GR III".into(),
                        firmware: "2.21".into(),
                        free_bytes: Some(1 << 30),
                        serial_number: Some("test-camera".into()),
                    })
                },
            },
        )
    }
    fn body(reply: &Reply) -> Value {
        serde_json::from_slice(&reply.body).unwrap()
    }
    #[test]
    fn an_update_that_cannot_succeed_is_answered_before_its_package() {
        let (_root, card) = card();
        let service = service();
        let precheck = |id: &str, card_id: &str, etag: Option<&str>| {
            let mut request =
                crate::parse(b"PUT /v1/update HTTP/1.1\r\nContent-Length: 5000000\r\n\r\n")
                    .unwrap();
            for (key, value) in [
                ("x-grkit-request-id", id),
                ("x-grkit-boot-id", service.boot_id.as_str()),
                ("x-grkit-card-id", card_id),
            ] {
                request.headers.insert(key.into(), value.into());
            }
            if let Some(etag) = etag {
                request.headers.insert("if-match".into(), etag.into());
            }
            service
                .precheck(
                    &request,
                    &Env {
                        files: &crate::System,
                        connection: &crate::Connection {
                            local: "127.0.0.1:8080".into(),
                            peer: "127.0.0.1:50000".into(),
                        },
                        card: &card,
                        device: &|| {
                            Ok(crate::update::Device {
                                model: "RICOH GR III".into(),
                                firmware: "2.21".into(),
                                free_bytes: Some(1 << 30),
                                serial_number: Some("test-camera".into()),
                            })
                        },
                    },
                )
                .map(|reply| (reply.status, body(&reply)["code"].clone()))
        };
        let etag =
            strong_etag(&call(&service, &card, "GET", "/v1/update", b"", REQUEST, None).body);
        assert_eq!(precheck(REQUEST, "card-test", Some(&etag)), None);
        assert_eq!(precheck("x", "card-test", Some(&etag)).unwrap().0, 400);
        assert_eq!(
            precheck(REQUEST, "other-card", Some(&etag)).unwrap(),
            (409, json!("context_changed"))
        );
        assert_eq!(
            precheck(REQUEST, "card-test", Some("\"old\"")).unwrap().0,
            412
        );
        // A request that waits on the camera blocks the next one.
        let zip = testing::signed_package(&camera::GR3, "1.7.0", &["second"]);
        let staged = call(
            &service,
            &card,
            "PUT",
            "/v1/update",
            &zip,
            REQUEST,
            Some(&etag),
        );
        assert_eq!(staged.status, 202, "{}", body(&staged));
        let etag = strong_etag(&call(&service, &card, "GET", "/v1/update", b"", CANCEL, None).body);
        assert_eq!(
            precheck(CANCEL, "card-test", Some(&etag)).unwrap(),
            (409, json!("software_pending"))
        );
        // A replay of a known request is answered by the request itself.
        assert_eq!(precheck(REQUEST, "card-test", Some(&etag)), None);
    }
    #[test]
    fn update_retries_and_targeted_cancel_survive_service_restart() {
        let (_root, card) = card();
        let service = service();
        let before = call(&service, &card, "GET", "/v1/update", b"", REQUEST, None);
        assert_eq!(before.status, 200, "{}", body(&before));
        let etag = strong_etag(&before.body);
        let zip = testing::signed_package(&camera::GR3, "1.7.0", &["second"]);
        let staged = call(
            &service,
            &card,
            "PUT",
            "/v1/update",
            &zip,
            REQUEST,
            Some(&etag),
        );
        assert_eq!(staged.status, 202, "{}", body(&staged));
        assert!(String::from_utf8(staged.to_bytes())
            .unwrap()
            .contains("Location: /v1/update?requestId="));
        let retry = call(
            &service,
            &card,
            "PUT",
            "/v1/update",
            &zip,
            REQUEST,
            Some(&etag),
        );
        assert_eq!(retry.status, 202);
        assert_eq!(retry.body, staged.body);
        assert!(String::from_utf8(retry.to_bytes())
            .unwrap()
            .contains("Location: /v1/update?requestId="));
        let (subscriber, _) = service.hub.lock().unwrap().subscribe().unwrap();
        let cancel_body = serde_json::to_vec(&json!({"targetRequestId":REQUEST})).unwrap();
        let cancelled = call(
            &service,
            &card,
            "POST",
            "/v1/update/cancel",
            &cancel_body,
            CANCEL,
            None,
        );
        assert_eq!(cancelled.status, 200, "{}", body(&cancelled));
        assert_eq!(body(&cancelled)["requestId"], CANCEL);
        assert_eq!(body(&cancelled)["targetRequest"]["status"], "cancelled");
        let mut saw_target = false;
        let mut hub = service.hub.lock().unwrap();
        while let crate::protocol::NextChange::Event(event) = hub.next(subscriber) {
            let value = serde_json::to_value(event).unwrap();
            if value["requestId"] == REQUEST && value["request"]["status"] == "cancelled" {
                saw_target = true;
            }
        }
        drop(hub);
        assert!(
            saw_target,
            "Cancellation must publish the target terminal state immediately"
        );
        let mut restarted = Service::new("different-boot".into());
        restarted.card_identity = |_| Ok("card-test".into());
        let result = call(
            &restarted,
            &card,
            "GET",
            &format!("/v1/update?requestId={REQUEST}"),
            b"",
            REQUEST,
            None,
        );
        assert_eq!(result.status, 200, "{}", body(&result));
        assert_eq!(body(&result)["status"], "cancelled");
        assert_eq!(card.state().unwrap(), "none");
    }
    #[test]
    fn missing_precondition_is_retained_and_changed_retry_is_rejected() {
        let (_root, card) = card();
        let service = service();
        let zip = testing::signed_package(&camera::GR3, "1.7.0", &["second"]);
        let rejected = call(&service, &card, "PUT", "/v1/update", &zip, REQUEST, None);
        assert_eq!(rejected.status, 428);
        assert_eq!(card.state().unwrap(), "none");
        let query = call(
            &service,
            &card,
            "GET",
            &format!("/v1/update?requestId={REQUEST}"),
            b"",
            REQUEST,
            None,
        );
        assert_eq!(query.status, 200);
        assert_eq!(body(&query)["error"]["status"], 428);
        let before = call(&service, &card, "GET", "/v1/update", b"", REQUEST, None);
        let changed = call(
            &service,
            &card,
            "PUT",
            "/v1/update",
            &zip,
            REQUEST,
            Some(&strong_etag(&before.body)),
        );
        assert_eq!(changed.status, 409);
        assert_eq!(body(&changed)["code"], "request_id_conflict");
    }
    #[test]
    fn owner_identity_supports_loop_mounts_and_conflicting_sources_fail_closed() {
        struct Backend;
        impl crate::runtime_backend::RuntimeBackend for Backend {
            fn card_identity(&mut self) -> Result<String, Problem> {
                Ok("owner-card".into())
            }
            fn snapshot(&mut self) -> Result<crate::runtime_backend::RuntimeSnapshot, Problem> {
                panic!("Identity must not depend on the Looks manifest");
            }
            fn apply(
                &mut self,
                _: &str,
                _: crate::library::Prepared,
            ) -> Result<crate::runtime_backend::RuntimeSnapshot, Problem> {
                panic!("Read-only identity probe");
            }
        }
        let mut service = Service::with_runtime_backend("boot-test".into(), Box::new(Backend));
        service.card_identity = |_| anyhow::bail!("No MMC sysfs on a loop mount");
        let root = std::path::Path::new("/test-card");
        assert_eq!(service.current_card_id(root).unwrap(), "owner-card");
        service.card_identity = |_| Ok("owner-card".into());
        assert_eq!(service.current_card_id(root).unwrap(), "owner-card");
        service.card_identity = |_| Ok("other-card".into());
        assert!(service.current_card_id(root).is_err());
    }
    #[test]
    fn monitor_preserves_query_context_until_a_real_commit_receipt_reconciles() {
        use crate::runtime_backend::{RuntimeBackend, RuntimeSnapshot};
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Backend {
            state: RuntimeSnapshot,
            uncertain: bool,
            ready: Arc<AtomicUsize>,
            invalidations: Arc<AtomicUsize>,
            applies: Arc<AtomicUsize>,
        }
        impl RuntimeBackend for Backend {
            fn capabilities(&self) -> crate::runtime_backend::RuntimeCapabilities {
                crate::runtime_backend::RuntimeCapabilities {
                    readable: !self.uncertain,
                    hot_reload: !self.uncertain && self.state.context.hot_reload,
                    unsupported: false,
                }
            }
            fn card_identity(&mut self) -> Result<String, Problem> {
                if self.ready.load(Ordering::SeqCst) == 3 {
                    return Err(Problem::new(503, "runtime_unavailable", "Disconnected"));
                }
                if self.ready.load(Ordering::SeqCst) == 4 {
                    return Ok("new-card".into());
                }
                if self.uncertain {
                    Err(Problem::new(
                        503,
                        "recovery_required",
                        "Pending COMMIT ticket",
                    ))
                } else {
                    Ok(self.state.card_id.clone())
                }
            }
            fn invalidate_session(&mut self) {
                self.invalidations.fetch_add(1, Ordering::SeqCst);
            }
            fn snapshot(&mut self) -> Result<RuntimeSnapshot, Problem> {
                Ok(self.state.clone())
            }
            fn apply(
                &mut self,
                _: &str,
                prepared: crate::library::Prepared,
            ) -> Result<RuntimeSnapshot, Problem> {
                self.applies.fetch_add(1, Ordering::SeqCst);
                self.state.manifest = prepared.manifest;
                self.state.generation += 1;
                self.uncertain = true;
                Err(Problem::new(
                    503,
                    "recovery_required",
                    "COMMIT reply timed out",
                ))
            }
            fn reconcile_apply(&mut self) -> Result<Option<RuntimeSnapshot>, Problem> {
                match self.ready.load(Ordering::SeqCst) {
                    1 => {
                        self.uncertain = false;
                        Ok(Some(self.state.clone()))
                    }
                    2 => {
                        let mut wrong = self.state.clone();
                        wrong.card_id = "other-card".into();
                        Ok(Some(wrong))
                    }
                    _ => Ok(None),
                }
            }
        }
        let context = crate::library::Context {
            hot_reload: true,
            dependencies_complete: true,
            supported_features: u32::MAX,
            ..Default::default()
        };
        let bytes = testing::effect("sample");
        let prepared = crate::library::Manifest::default()
            .prepare_put("sample", &bytes, &context)
            .unwrap();
        let hash = packaging::sha256(&bytes);
        let ready = Arc::new(AtomicUsize::new(0));
        let invalidations = Arc::new(AtomicUsize::new(0));
        let applies = Arc::new(AtomicUsize::new(0));
        let backend = Backend {
            state: RuntimeSnapshot {
                manifest: prepared.manifest,
                card_id: "card-test".into(),
                runtime_epoch: [1; 4],
                context,
                loaded_ids: ["sample".into()].into(),
                contents: [(hash, bytes)].into(),
                generation: 0,
            },
            uncertain: false,
            ready: ready.clone(),
            invalidations: invalidations.clone(),
            applies: applies.clone(),
        };
        let mut service = Service::with_runtime_backend("boot-test".into(), Box::new(backend));
        service.card_identity = |_| anyhow::bail!("Linux loop mount has no independent CID");
        let (_root, card) = card();
        let before = call(
            &service,
            &card,
            "GET",
            "/v1/looks/sample",
            b"",
            REQUEST,
            None,
        );
        assert_eq!(before.status, 200);
        let invalidated = invalidations.load(Ordering::SeqCst);
        let patch = call(
            &service,
            &card,
            "PATCH",
            "/v1/looks/sample",
            br#"{"enabled":false}"#,
            REQUEST,
            Some(&strong_etag(&before.body)),
        );
        assert_eq!(patch.status, 503, "{}", body(&patch));
        assert!(service.current_card_id(card.root()).is_err());
        assert_eq!(service.poll_card_identity(card.root()).unwrap(), None);
        assert_eq!(
            service.hub.lock().unwrap().snapshot().1.card_id,
            "card-test"
        );
        assert_eq!(invalidations.load(Ordering::SeqCst), invalidated);
        assert!(service.looks.recovery_required());
        assert_eq!(
            call(&service, &card, "GET", "/v1/looks", b"", REQUEST, None).status,
            503
        );
        // Ordinary props polling also cannot erase the retained query context.
        call(&service, &card, "GET", "/v1/props", b"", REQUEST, None);
        assert_eq!(
            service.hub.lock().unwrap().snapshot().1.card_id,
            "card-test"
        );
        let uri = format!("/v1/looks/sample/requests/{REQUEST}");
        for mode in [0, 2] {
            ready.store(mode, Ordering::SeqCst);
            let result = call(&service, &card, "GET", &uri, b"", REQUEST, None);
            assert_eq!(body(&result)["phase"], "recovering");
            assert!(service.looks.recovery_required());
        }
        ready.store(1, Ordering::SeqCst);
        let result = call(&service, &card, "GET", &uri, b"", REQUEST, None);
        assert_eq!(body(&result)["status"], "succeeded", "{}", body(&result));
        assert!(!service.looks.recovery_required());
        assert_eq!(applies.load(Ordering::SeqCst), 1);
        assert_eq!(invalidations.load(Ordering::SeqCst), invalidated);
        assert_eq!(
            service.poll_card_identity(card.root()).unwrap(),
            Some("card-test".into())
        );
        ready.store(3, Ordering::SeqCst);
        assert_eq!(service.poll_card_identity(card.root()).unwrap(), None);
        assert_eq!(
            service.hub.lock().unwrap().snapshot().1.card_id,
            "unavailable"
        );
        assert_eq!(invalidations.load(Ordering::SeqCst), invalidated + 1);
        ready.store(4, Ordering::SeqCst);
        assert_eq!(
            service.poll_card_identity(card.root()).unwrap(),
            Some("new-card".into())
        );
        assert_eq!(service.hub.lock().unwrap().snapshot().1.card_id, "new-card");
    }
    #[test]
    fn kernel_identity_cannot_hide_an_uncertain_owner_transaction() {
        struct Backend;
        impl crate::runtime_backend::RuntimeBackend for Backend {
            fn card_identity(&mut self) -> Result<String, Problem> {
                Err(Problem::new(
                    503,
                    "recovery_required",
                    "An owner commit is unresolved.",
                ))
            }
            fn snapshot(&mut self) -> Result<crate::runtime_backend::RuntimeSnapshot, Problem> {
                panic!("No snapshot");
            }
            fn apply(
                &mut self,
                _: &str,
                _: crate::library::Prepared,
            ) -> Result<crate::runtime_backend::RuntimeSnapshot, Problem> {
                panic!("No apply");
            }
        }
        let mut service = Service::with_runtime_backend("boot-test".into(), Box::new(Backend));
        service.card_identity = |_| Ok("card-test".into());
        service.observe_card(Some("card-test".into())).unwrap();
        assert!(service
            .current_card_id(std::path::Path::new("/test-card"))
            .is_err());
        assert!(service.looks.recovery_required());
        service.observe_card(None).unwrap();
        assert!(service.looks.recovery_required());
        service.observe_card(Some("card-test".into())).unwrap();
        assert!(
            service.looks.recovery_required(),
            "Reacquiring the same medium is not recovery"
        );
        service.observe_card(Some("different-card".into())).unwrap();
        assert!(!service.looks.recovery_required());
    }
    #[test]
    fn startup_reconciliation_blocks_kernel_fallback_without_false_card_changes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Backend {
            mode: Arc<AtomicUsize>,
            invalidations: Arc<AtomicUsize>,
        }
        impl crate::runtime_backend::RuntimeBackend for Backend {
            fn card_identity(&mut self) -> Result<String, Problem> {
                match self.mode.load(Ordering::SeqCst) {
                    0 => Err(Problem::new(
                        503,
                        "runtime_busy",
                        "Inherited ticket is pending",
                    )),
                    1 => Err(Problem::new(
                        503,
                        "startup_reconciliation_unsupported",
                        "No coherent restart snapshot",
                    )),
                    _ => Ok("new-card".into()),
                }
            }
            fn invalidate_session(&mut self) {
                self.invalidations.fetch_add(1, Ordering::SeqCst);
            }
            fn snapshot(&mut self) -> Result<crate::runtime_backend::RuntimeSnapshot, Problem> {
                panic!("No snapshot authorization")
            }
            fn apply(
                &mut self,
                _: &str,
                _: crate::library::Prepared,
            ) -> Result<crate::runtime_backend::RuntimeSnapshot, Problem> {
                panic!("No mutation authorization")
            }
        }
        let mode = Arc::new(AtomicUsize::new(0));
        let invalidations = Arc::new(AtomicUsize::new(0));
        let mut service = Service::with_runtime_backend(
            "boot-test".into(),
            Box::new(Backend {
                mode: mode.clone(),
                invalidations: invalidations.clone(),
            }),
        );
        service.card_identity = |_| Ok("old-card".into());
        service.observe_card(Some("old-card".into())).unwrap();
        let baseline = invalidations.load(Ordering::SeqCst);
        let watermark = service.hub.lock().unwrap().snapshot().1;
        let root = std::path::Path::new("/test-card");
        let (_temporary, card) = card();
        for state in [0, 1] {
            mode.store(state, Ordering::SeqCst);
            assert!(service.current_card_id(root).is_err());
            assert_eq!(service.poll_card_identity(root).unwrap(), None);
            assert_eq!(invalidations.load(Ordering::SeqCst), baseline);
            assert_eq!(service.hub.lock().unwrap().snapshot().1, watermark);
            for (method, path) in [("PUT", "/v1/update"), ("POST", "/v1/rollback")] {
                let rejected = call(&service, &card, method, path, b"", REQUEST, None);
                assert_eq!(rejected.status, 503);
                assert_eq!(body(&rejected)["code"], "card_identity_unavailable");
                assert!(card.software_request().unwrap().is_none());
            }
            assert!(!service.looks.recovery_required());
        }
        mode.store(2, Ordering::SeqCst);
        service.card_identity = |_| Ok("new-card".into());
        assert_eq!(
            service.poll_card_identity(root).unwrap(),
            Some("new-card".into())
        );
        assert_eq!(invalidations.load(Ordering::SeqCst), baseline + 1);
        assert_eq!(service.hub.lock().unwrap().snapshot().1.card_id, "new-card");
    }

    #[test]
    fn software_monitor_does_not_publish_a_copied_request_on_another_card() {
        let (_root, card) = card();
        let service = service();
        let zip = testing::signed_package(&camera::GR3, "1.7.0", &["second"]);
        let before = call(&service, &card, "GET", "/v1/update", b"", REQUEST, None);
        let staged = call(
            &service,
            &card,
            "PUT",
            "/v1/update",
            &zip,
            REQUEST,
            Some(&strong_etag(&before.body)),
        );
        assert_eq!(staged.status, 202);
        service.observe_card(Some("different-card".into())).unwrap();
        let before = service.hub.lock().unwrap().snapshot();
        assert!(service.observe_software(&card).is_err());
        let after = service.hub.lock().unwrap().snapshot();
        assert_eq!(before, after);
    }
    #[test]
    fn card_change_invalidates_the_injected_runtime_session_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Backend(Arc<AtomicUsize>);
        impl crate::runtime_backend::RuntimeBackend for Backend {
            fn invalidate_session(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
            fn snapshot(&mut self) -> Result<crate::runtime_backend::RuntimeSnapshot, Problem> {
                Err(Problem::new(503, "test_backend", "No runtime fixture."))
            }
            fn apply(
                &mut self,
                _: &str,
                _: crate::library::Prepared,
            ) -> Result<crate::runtime_backend::RuntimeSnapshot, Problem> {
                panic!("A read must not apply a candidate");
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let mut service = Service::with_runtime_backend(
            "boot-test".into(),
            Box::new(Backend(Arc::clone(&calls))),
        );
        service.card_identity = |_| Ok("card-test".into());
        let (_root, card) = card();
        for _ in 0..2 {
            let reply = call(&service, &card, "GET", "/v1/looks", b"", REQUEST, None);
            assert_eq!(reply.status, 503);
            assert_eq!(body(&reply)["code"], "test_backend");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        service.observe_card(None).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn persisted_request_ids_cannot_rebind_or_overwrite_new_boot_cache() {
        let (_root, card) = card();
        let original = service();
        let zip = testing::signed_package(&camera::GR3, "1.7.0", &["second"]);
        let before = call(&original, &card, "GET", "/v1/update", b"", REQUEST, None);
        assert_eq!(
            call(
                &original,
                &card,
                "PUT",
                "/v1/update",
                &zip,
                REQUEST,
                Some(&strong_etag(&before.body))
            )
            .status,
            202
        );
        let mut restarted = Service::new("new-boot".into());
        restarted.card_identity = |_| Ok("card-test".into());
        restarted.observe_card(Some("card-test".into())).unwrap();
        let initial = RecordedResult {
            status: 202,
            body: json!({"domain":"looks","status":"running"}),
        };
        let intent = Intent {
            domain: "looks".into(),
            device_id: "device".into(),
            boot_id: "new-boot".into(),
            card_id: "card-test".into(),
            method: "PUT".into(),
            uri: "/v1/looks/new/content".into(),
            if_match: None,
            if_none_match: Some("*".into()),
            content_sha256: packaging::sha256(b"new"),
        };
        restarted
            .requests
            .lock()
            .unwrap()
            .admit(
                REQUEST,
                intent,
                "card-test",
                initial.clone(),
                Instant::now(),
            )
            .unwrap();
        restarted.observe_software(&card).unwrap();
        assert_eq!(
            restarted
                .requests
                .lock()
                .unwrap()
                .get(REQUEST, Instant::now())
                .unwrap(),
            &initial
        );
        for path in ["/v1/update", "/v1/looks/new/content"] {
            let reply = call(&restarted, &card, "PUT", path, b"different", REQUEST, None);
            assert_eq!(reply.status, 409);
            assert_eq!(body(&reply)["code"], "request_id_conflict");
        }
    }
    #[test]
    fn swap_after_software_staging_does_not_publish_old_request_on_new_card() {
        struct SwappingOwner(std::path::PathBuf);
        impl crate::runtime_backend::RuntimeBackend for SwappingOwner {
            fn card_identity(&mut self) -> Result<String, Problem> {
                let pending = crate::update::Card::new(&self.0).state().unwrap() != "none";
                Ok(if pending {
                    "replacement-card"
                } else {
                    "card-test"
                }
                .into())
            }
            fn snapshot(&mut self) -> Result<crate::runtime_backend::RuntimeSnapshot, Problem> {
                Err(Problem::new(
                    503,
                    "runtime_unavailable",
                    "No Looks fixture.",
                ))
            }
            fn apply(
                &mut self,
                _: &str,
                _: crate::library::Prepared,
            ) -> Result<crate::runtime_backend::RuntimeSnapshot, Problem> {
                panic!("No Looks mutation");
            }
        }
        let (_root, card) = card();
        let mut service = Service::with_runtime_backend(
            "boot-test".into(),
            Box::new(SwappingOwner(card.root().to_path_buf())),
        );
        service.card_identity = |_| anyhow::bail!("IPC filesystem");
        service.release_keys = vec![testing::release_key()];
        let before = call(&service, &card, "GET", "/v1/update", b"", REQUEST, None);
        let (subscriber, _) = service.hub.lock().unwrap().subscribe().unwrap();
        let zip = testing::signed_package(&camera::GR3, "1.7.0", &["second"]);
        assert_eq!(
            call(
                &service,
                &card,
                "PUT",
                "/v1/update",
                &zip,
                REQUEST,
                Some(&strong_etag(&before.body))
            )
            .status,
            202
        );
        let mut hub = service.hub.lock().unwrap();
        let (state, mark) = hub.snapshot();
        assert_eq!(mark.card_id, "replacement-card");
        assert!(state["update"]["request"].is_null());
        while let crate::protocol::NextChange::Event(event) = hub.next(subscriber) {
            let event = serde_json::to_value(event).unwrap();
            assert!(!(event["cardId"] == "replacement-card" && event["type"] == "requestChanged"));
        }
    }
    #[test]
    fn cancellation_recovery_matches_target_identity_instead_of_cancel_uuid() {
        let (_root, card) = card();
        let service = service();
        let zip = testing::signed_package(&camera::GR3, "1.7.0", &["second"]);
        let before = call(&service, &card, "GET", "/v1/update", b"", REQUEST, None);
        assert_eq!(
            call(
                &service,
                &card,
                "PUT",
                "/v1/update",
                &zip,
                REQUEST,
                Some(&strong_etag(&before.body))
            )
            .status,
            202
        );
        let target = card
            .cancel_software_request(REQUEST, SoftwareDomain::Update)
            .unwrap();
        let body = serde_json::to_vec(&json!({"targetRequestId":REQUEST})).unwrap();
        let mut request = crate::parse(b"POST /v1/update/cancel HTTP/1.1\r\n\r\n").unwrap();
        request
            .headers
            .insert("x-grkit-request-id".into(), CANCEL.into());
        assert!(recovered_cancel_matches(
            &request,
            &body,
            "card-test",
            &target
        ));
        assert!(!recovered_cancel_matches(
            &request,
            &body,
            "other-card",
            &target
        ));
        assert!(!recovered_cancel_matches(
            &request,
            b"{\"targetRequestId\":\"wrong\"}",
            "card-test",
            &target
        ));
        request.path = "/v1/rollback/cancel".into();
        assert!(!recovered_cancel_matches(
            &request,
            &body,
            "card-test",
            &target
        ));
    }
    #[test]
    fn software_result_cache_does_not_cross_card_identity() {
        let (_root, card) = card();
        let mut service = service();
        let zip = testing::signed_package(&camera::GR3, "1.7.0", &["second"]);
        assert_eq!(
            call(&service, &card, "PUT", "/v1/update", &zip, REQUEST, None).status,
            428
        );
        service.card_identity = |_| Ok("different-card".into());
        let result = call(
            &service,
            &card,
            "GET",
            &format!("/v1/update?requestId={REQUEST}"),
            b"",
            REQUEST,
            None,
        );
        assert_eq!(result.status, 404);
    }
    #[test]
    fn cancellation_cannot_target_a_request_copied_from_another_card() {
        let (_root, card) = card();
        let service = service();
        let zip = testing::signed_package(&camera::GR3, "1.7.0", &["second"]);
        crate::update::stage_request(
            &card,
            &zip,
            &crate::update::Device {
                model: "RICOH GR III".into(),
                firmware: "2.21".into(),
                free_bytes: Some(1 << 30),
                serial_number: Some("test-camera".into()),
            },
            &[testing::release_key()],
            &crate::update::SoftwareContext {
                request_id: REQUEST.into(),
                boot_id: "previous-boot".into(),
                card_id: "other-card".into(),
            },
        )
        .unwrap();
        let cancelled = call(
            &service,
            &card,
            "POST",
            "/v1/update/cancel",
            &serde_json::to_vec(&json!({"targetRequestId":REQUEST})).unwrap(),
            CANCEL,
            None,
        );
        assert_eq!(cancelled.status, 409, "{}", body(&cancelled));
        assert_eq!(body(&cancelled)["code"], "context_changed");
        assert_eq!(card.state().unwrap(), "waiting");
        assert_eq!(
            card.software_request().unwrap().unwrap().status,
            SoftwareStatus::Running
        );
    }
    #[test]
    fn mutation_query_is_part_of_request_identity() {
        let (_root, card) = card();
        let service = service();
        let zip = testing::signed_package(&camera::GR3, "1.7.0", &["second"]);
        let rejected = call(
            &service,
            &card,
            "PUT",
            "/v1/update?extra=1",
            &zip,
            REQUEST,
            None,
        );
        assert_eq!(rejected.status, 400);
        let changed = call(&service, &card, "PUT", "/v1/update", &zip, REQUEST, None);
        assert_eq!(changed.status, 409);
        assert_eq!(body(&changed)["code"], "request_id_conflict");
    }
    #[test]
    fn card_observer_notifies_existing_websocket_subscribers() {
        let service = service();
        let (subscriber, before) = service.hub.lock().unwrap().subscribe().unwrap();
        service.observe_card(Some("new-card".into())).unwrap();
        let mut hub = service.hub.lock().unwrap();
        let crate::protocol::NextChange::Event(event) = hub.next(subscriber) else {
            panic!("missing storage invalidation")
        };
        assert_eq!(event.card_id, "new-card");
        assert!(event.seq.unwrap() > before.snapshot_seq);
        assert_eq!(hub.snapshot().1.card_id, "new-card");
    }
    #[test]
    fn etag_covers_exact_response_and_errors_use_problem_media_type() {
        let reply = Reply::json(200, json!({"state":"none"})).versioned();
        let text = String::from_utf8(reply.to_bytes()).unwrap();
        assert!(text.contains(&format!("ETag: {}", strong_etag(&reply.body))));
        assert!(text.contains("Cache-Control: no-cache"));
        let error = Reply::problem(Problem::new(503, "runtime_unavailable", "Unavailable"));
        assert!(String::from_utf8(error.to_bytes())
            .unwrap()
            .contains("Content-Type: application/problem+json"));
    }
}
