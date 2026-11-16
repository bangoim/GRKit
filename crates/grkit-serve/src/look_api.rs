//! Film Look HTTP resources. Mutation preparation is pure; only the runtime
//! backend can acknowledge durable application. All domains share RequestCache.
use crate::{
    api::Reply,
    changes::SharedChanges,
    library::{Patch, Prepared, MAX_LISTED, MAX_LOADED},
    protocol::{
        check_precondition, valid_request_id, Admission, Intent, Problem, RecordedResult,
        RequestCache, Watermark,
    },
    runtime_backend::{RuntimeBackend, RuntimeSnapshot},
    Request,
};
use grkit_core::{effects, packaging};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, TryLockError,
    },
    time::Instant,
};

type Result<T> = std::result::Result<T, Problem>;

pub struct RequestContext<'a> {
    pub device_id: &'a str,
    pub boot_id: &'a str,
    pub card_id: &'a str,
    pub write_allowed: bool,
    pub software_pending: bool,
}
pub struct LookApi {
    backend: Mutex<Box<dyn RuntimeBackend>>,
    recovery: AtomicBool,
    pending: Mutex<Option<PendingApply>>,
}
struct PendingApply {
    id: String,
    scope: Scope,
    device_id: String,
    boot_id: String,
    card_id: String,
    before: RuntimeSnapshot,
    prepared: Prepared,
    created: bool,
    receipt: Option<RuntimeSnapshot>,
}
impl LookApi {
    pub fn new(backend: Box<dyn RuntimeBackend>) -> Self {
        Self {
            backend: Mutex::new(backend),
            recovery: AtomicBool::new(false),
            pending: Mutex::new(None),
        }
    }

    pub fn card_identity(&self) -> Result<String> {
        self.backend
            .try_lock()
            .map_err(lock_problem)?
            .card_identity()
            .inspect_err(|error| {
                if error.code == "recovery_required" {
                    self.recovery.store(true, Ordering::Release);
                }
            })
    }

    pub fn invalidate_session(&self) -> Result<()> {
        let mut backend = self.backend.lock().map_err(|_| coordinator())?;
        backend.invalidate_session();
        // Losing identity or rebuilding a session is not proof that an
        // uncertain commit was recovered. Keep quarantine on the same medium.
        Ok(())
    }

    pub fn accept_new_card(&self) -> Result<()> {
        let mut backend = self.backend.lock().map_err(|_| coordinator())?;
        backend.invalidate_session();
        *self.pending.lock().map_err(|_| coordinator())? = None;
        self.recovery.store(false, Ordering::Release);
        Ok(())
    }

    /// Probe the same trusted snapshot path used by resource reads. Capability
    /// flags come from the backend, not from merely finding a mailbox.
    pub fn runtime_properties(&self, card_id: &str) -> Result<Value> {
        self.require_ready()?;
        let mut backend = self.backend.try_lock().map_err(lock_problem)?;
        let snapshot = backend.snapshot().inspect_err(|error| {
            if error.code == "recovery_required" {
                self.recovery.store(true, Ordering::Release);
            }
        })?;
        let context = RequestContext {
            device_id: "",
            boot_id: "",
            card_id,
            write_allowed: false,
            software_pending: false,
        };
        self.validate_snapshot(&snapshot, &context)?;
        let capabilities = backend.capabilities();
        Ok(
            json!({"state":"ready","busyReason":null,"epoch":snapshot.runtime_epoch,
            "generation":snapshot.generation,"configurationRevision":snapshot.revision()?,
            "capabilities":{"read":capabilities.readable,"write":capabilities.hot_reload,"hotReload":capabilities.hot_reload}}),
        )
    }

    pub fn recovery_required(&self) -> bool {
        self.recovery.load(Ordering::Acquire)
    }
    fn require_ready(&self) -> Result<()> {
        if self.recovery_required() {
            Err(recovery_problem())
        } else {
            Ok(())
        }
    }
    fn validate_snapshot(
        &self,
        snapshot: &RuntimeSnapshot,
        context: &RequestContext<'_>,
    ) -> Result<()> {
        if let Err(error) = snapshot.validate() {
            self.recovery.store(true, Ordering::Release);
            return Err(error);
        }
        if snapshot.card_id != context.card_id {
            return Err(problem(
                409,
                "context_changed",
                "The runtime snapshot belongs to a different card.",
            ));
        }
        Ok(())
    }

    /// None means this path belongs to another router. Request queries remain
    /// available after deletion and while another worker applies a candidate.
    pub fn handle(
        &self,
        request: &Request,
        body: &[u8],
        context: &RequestContext<'_>,
        hub: &SharedChanges,
        requests: &Mutex<RequestCache>,
    ) -> Option<Reply> {
        let route = Route::parse(&request.path)?;
        let result = match (&route, request.method.as_str()) {
            (
                Route::Query {
                    scope,
                    id,
                    cancel: false,
                },
                "GET",
            ) => {
                // Querying an uncertain request only reconciles its original
                // ticket; it never authorizes a second apply.
                self.reconcile(scope, id, context, hub, requests);
                query(requests, scope, id, context).map(|record| Reply::json(200, record.body))
            }
            (_, "GET") if route.readable() => self.read(&route, context, hub),
            _ if route.accepts(&request.method) => {
                self.mutate(&route, request, body, context, hub, requests)
            }
            _ => Err(problem(
                405,
                "method_not_allowed",
                "This method is not supported for the resource.",
            )),
        };
        Some(result.unwrap_or_else(Reply::problem))
    }

    fn reconcile(
        &self,
        scope: &Scope,
        id: &str,
        context: &RequestContext<'_>,
        hub: &SharedChanges,
        requests: &Mutex<RequestCache>,
    ) {
        if !self.recovery_required() {
            return;
        }
        let Ok(mut backend) = self.backend.try_lock() else {
            return;
        };
        let Ok(mut pending) = self.pending.try_lock() else {
            return;
        };
        let Some(p) = pending.as_mut() else { return };
        if !p.id.eq_ignore_ascii_case(id)
            || p.scope.resource() != scope.resource()
            || p.device_id != context.device_id
            || p.boot_id != context.boot_id
            || p.card_id != context.card_id
        {
            return;
        }
        // Validate the service context before asking the backend to consume
        // its retained acknowledgement; unrelated request queries do nothing.
        if !query(requests, scope, id, context)
            .is_ok_and(|record| record.body["phase"] == "recovering")
        {
            return;
        }
        if !hub
            .lock()
            .is_ok_and(|events| check_context(context, &events.snapshot().1).is_ok())
        {
            return;
        }
        let retained_receipt = p.receipt.is_some();
        if p.receipt.is_none() {
            let Ok(Some(receipt)) = backend.reconcile_apply() else {
                return;
            };
            if receipt.verify_receipt(&p.before, &p.prepared).is_err() {
                return;
            }
            p.receipt = Some(receipt);
        }
        let receipt = p.receipt.as_ref().expect("verified receipt retained");
        if receipt.verify_receipt(&p.before, &p.prepared).is_err() || receipt.card_id != p.card_id {
            return;
        }
        // The retained receipt proves this request's original result. Camera
        // menu edits since that receipt must not be overwritten in the hub.
        let fresh;
        let current = if retained_receipt {
            let Ok(snapshot) = backend.snapshot() else {
                return;
            };
            if snapshot.validate().is_err()
                || snapshot.card_id != receipt.card_id
                || snapshot.runtime_epoch != receipt.runtime_epoch
                || snapshot.generation < receipt.generation
            {
                return;
            }
            fresh = snapshot;
            &fresh
        } else {
            receipt
        };
        if observe(current, context, hub, Some(id)).is_err() {
            return;
        }
        let Ok(result) = mutation_result(receipt, scope) else {
            return;
        };
        let record = RecordedResult {
            status: if p.created { 201 } else { 200 },
            body: summary(
                id,
                scope,
                context,
                "succeeded",
                "completed",
                Some(result),
                None,
            ),
        };
        if publish_record(requests, hub, id, scope, context, &record, true).is_err() {
            return;
        }
        *pending = None;
        self.recovery.store(false, Ordering::Release);
    }

    fn read(
        &self,
        route: &Route,
        context: &RequestContext<'_>,
        hub: &SharedChanges,
    ) -> Result<Reply> {
        self.require_ready()?;
        let mut backend = self.backend.try_lock().map_err(lock_problem)?;
        let snapshot = backend.snapshot().inspect_err(|error| {
            if error.code == "recovery_required" {
                self.recovery.store(true, Ordering::Release);
            }
        })?;
        self.validate_snapshot(&snapshot, context)?;
        let watermark = observe(&snapshot, context, hub, None)?;
        let reply = match route {
            Route::Collection => {
                let mut body = snapshot_value(&snapshot)?;
                body["bootId"] = json!(watermark.boot_id);
                body["cardId"] = json!(watermark.card_id);
                body["snapshotSeq"] = json!(watermark.snapshot_seq);
                Reply::json(200, body)
            }
            Route::Configuration(id) => Reply::binary(
                200,
                snapshot.manifest.configuration_bytes(id)?,
                "application/json",
            )
            .versioned(),
            Route::Info(id) => Reply::json(200, info(&snapshot, id)?),
            Route::Content(id) => Reply::binary(
                200,
                snapshot.content(id)?.to_vec(),
                "application/octet-stream",
            )
            .versioned()
            .header(
                "X-GRKit-Content-SHA256",
                packaging::sha256(snapshot.content(id)?),
            ),
            Route::Order => {
                Reply::binary(200, snapshot.manifest.order_bytes(), "application/json").versioned()
            }
            _ => return Err(problem(404, "not_found", "The resource does not exist.")),
        };
        Ok(reply.snapshot_headers(&watermark))
    }

    fn mutate(
        &self,
        route: &Route,
        request: &Request,
        body: &[u8],
        context: &RequestContext<'_>,
        hub: &SharedChanges,
        requests: &Mutex<RequestCache>,
    ) -> Result<Reply> {
        let id = header(request, "x-grkit-request-id")?;
        let boot_id = header(request, "x-grkit-boot-id")?;
        let card_id = header(request, "x-grkit-card-id")?;
        let scope = route.scope();
        let domain = scope.domain();
        let digest = packaging::sha256(body);
        // Framing and digest checks precede admission. An incomplete or corrupt
        // transfer does not consume a request record or authorize any mutation.
        validate_body(route, request, body, &digest)?;
        let uri = match &request.query {
            Some(query) => format!("{}?{query}", request.path),
            None => request.path.clone(),
        };
        let intent = Intent {
            domain: domain.into(),
            device_id: context.device_id.into(),
            boot_id: boot_id.into(),
            card_id: card_id.into(),
            method: request.method.clone(),
            uri,
            if_match: request.headers.get("if-match").cloned(),
            if_none_match: request.headers.get("if-none-match").cloned(),
            content_sha256: digest,
        };
        let initial = RecordedResult {
            status: 202,
            body: summary(id, &scope, context, "running", "validating", None, None),
        };
        match requests.lock().map_err(|_| coordinator())?.admit(
            id,
            intent,
            context.card_id,
            initial,
            Instant::now(),
        )? {
            Admission::Existing(record) => return Ok(record_reply(record, &scope, id)),
            Admission::New => {}
        }
        let mut recovering = false;
        let outcome = (|| -> Result<(u16, Value)> {
            if context.software_pending {
                return Err(problem(
                    409,
                    "software_pending",
                    "Finish or cancel the pending software operation before changing Looks.",
                ));
            }
            if !context.write_allowed {
                return Err(problem(
                    403,
                    "writes_disabled",
                    "Phone changes are disabled.",
                ));
            }
            if let Route::Query {
                scope: target_scope,
                id: target,
                cancel: true,
            } = route
            {
                let record = query(requests, target_scope, target, context)?;
                return Err(if record.body["status"] == "running" {
                    problem(
                        409,
                        "commit_in_progress",
                        "The synchronous runtime commit cannot be cancelled.",
                    )
                } else {
                    problem(
                        409,
                        "request_finished",
                        "The target request has already finished.",
                    )
                });
            }
            self.require_ready()?;
            let mut backend = self.backend.try_lock().map_err(lock_problem)?;
            if self.pending.lock().map_err(|_| coordinator())?.is_some() {
                return Err(problem(
                    409,
                    "commit_in_progress",
                    "An earlier runtime result is awaiting publication.",
                ));
            }
            let before = backend.snapshot().inspect_err(|error| {
                if error.code == "recovery_required" {
                    self.recovery.store(true, Ordering::Release);
                }
            })?;
            self.validate_snapshot(&before, context)?;
            if !before.context.hot_reload || !backend.capabilities().hot_reload {
                return Err(problem(
                    503,
                    "hot_reload_unavailable",
                    "This runtime has not verified safe live Look changes.",
                ));
            }
            // Observe menu-side changes before evaluating resource preconditions.
            observe(&before, context, hub, None)?;
            let (prepared, created) = prepare(route, request, body, &before)?;
            let revision = before.revision()?;
            let running = RecordedResult {
                status: 202,
                body: summary(id, &scope, context, "running", "applying", None, None),
            };
            requests.lock().map_err(|_| coordinator())?.update(
                id,
                running,
                false,
                Instant::now(),
            )?;
            *self.pending.lock().map_err(|_| coordinator())? = Some(PendingApply {
                id: id.to_ascii_lowercase(),
                scope: scope.clone(),
                device_id: context.device_id.into(),
                boot_id: context.boot_id.into(),
                card_id: context.card_id.into(),
                before: before.clone(),
                prepared: prepared.clone(),
                created,
                receipt: None,
            });
            let receipt = match backend.apply(&revision, prepared.clone()) {
                Ok(receipt) => receipt,
                Err(error) => {
                    if error.code == "recovery_required" {
                        recovering = true;
                        self.recovery.store(true, Ordering::Release);
                    }
                    if !recovering {
                        *self.pending.lock().map_err(|_| coordinator())? = None;
                    }
                    return Err(error);
                }
            };
            // Every subsequent failure follows a potentially durable apply,
            // including publication/context errors after a valid receipt.
            recovering = true;
            if let Err(error) = receipt.verify_receipt(&before, &prepared) {
                recovering = true;
                self.recovery.store(true, Ordering::Release);
                return Err(error);
            }
            self.pending
                .lock()
                .map_err(|_| coordinator())?
                .as_mut()
                .ok_or_else(coordinator)?
                .receipt = Some(receipt.clone());
            observe(&receipt, context, hub, Some(id))?;
            let result = mutation_result(&receipt, &scope)?;
            recovering = false;
            Ok((if created { 201 } else { 200 }, result))
        })();
        if outcome.is_err() && recovering {
            self.recovery.store(true, Ordering::Release);
        }
        let record = match outcome {
            Ok((status, result)) => RecordedResult {
                status,
                body: summary(
                    id,
                    &scope,
                    context,
                    "succeeded",
                    "completed",
                    Some(result),
                    None,
                ),
            },
            Err(error) => RecordedResult {
                status: error.status,
                body: summary(
                    id,
                    &scope,
                    context,
                    if recovering { "running" } else { "failed" },
                    if recovering {
                        "recovering"
                    } else {
                        "completed"
                    },
                    None,
                    Some(error),
                ),
            },
        };
        // Publish a terminal event and cache result together. A valid runtime
        // receipt remains retained if local publication cannot finish.
        if let Err(error) = publish_record(requests, hub, id, &scope, context, &record, !recovering)
        {
            if self
                .pending
                .lock()
                .map_err(|_| coordinator())?
                .as_ref()
                .is_some_and(|p| p.id.eq_ignore_ascii_case(id))
            {
                self.recovery.store(true, Ordering::Release);
                let waiting = RecordedResult {
                    status: error.status,
                    body: summary(
                        id,
                        &scope,
                        context,
                        "running",
                        "recovering",
                        None,
                        Some(error.clone()),
                    ),
                };
                requests.lock().map_err(|_| coordinator())?.update(
                    id,
                    waiting,
                    false,
                    Instant::now(),
                )?;
            }
            return Err(error);
        }
        if !recovering {
            let mut pending = self.pending.lock().map_err(|_| coordinator())?;
            if pending
                .as_ref()
                .is_some_and(|p| p.id.eq_ignore_ascii_case(id))
            {
                *pending = None;
            }
        }
        Ok(record_reply(record, &scope, id))
    }
}

#[derive(Clone)]
enum Scope {
    Look(String),
    Order,
}
impl Scope {
    fn resource(&self) -> String {
        match self {
            Self::Look(id) => format!("/v1/looks/{id}"),
            Self::Order => "/v1/params/looks/order".into(),
        }
    }
    fn domain(&self) -> &'static str {
        if matches!(self, Self::Order) {
            "params"
        } else {
            "looks"
        }
    }
    fn query_uri(&self, id: &str) -> String {
        match self {
            Self::Look(look) => format!("/v1/looks/{look}/requests/{id}"),
            Self::Order => format!("/v1/params/looks/requests/{id}"),
        }
    }
}
enum Route {
    Collection,
    Configuration(String),
    Info(String),
    Content(String),
    Order,
    Query {
        scope: Scope,
        id: String,
        cancel: bool,
    },
}
impl Route {
    fn parse(path: &str) -> Option<Self> {
        let parts: Vec<_> = path.split('/').collect();
        match parts.as_slice() {
            ["", "v1", "looks"] => Some(Self::Collection),
            ["", "v1", "looks", id] if effects::valid_id(id) => {
                Some(Self::Configuration((*id).into()))
            }
            ["", "v1", "looks", id, "info"] if effects::valid_id(id) => {
                Some(Self::Info((*id).into()))
            }
            ["", "v1", "looks", id, "content"] if effects::valid_id(id) => {
                Some(Self::Content((*id).into()))
            }
            ["", "v1", "looks", look, "requests", id]
                if effects::valid_id(look) && valid_request_id(id) =>
            {
                Some(Self::Query {
                    scope: Scope::Look((*look).into()),
                    id: (*id).into(),
                    cancel: false,
                })
            }
            ["", "v1", "looks", look, "requests", id, "cancel"]
                if effects::valid_id(look) && valid_request_id(id) =>
            {
                Some(Self::Query {
                    scope: Scope::Look((*look).into()),
                    id: (*id).into(),
                    cancel: true,
                })
            }
            ["", "v1", "params", "looks", "order"] => Some(Self::Order),
            ["", "v1", "params", "looks", "requests", id] if valid_request_id(id) => {
                Some(Self::Query {
                    scope: Scope::Order,
                    id: (*id).into(),
                    cancel: false,
                })
            }
            ["", "v1", "params", "looks", "requests", id, "cancel"] if valid_request_id(id) => {
                Some(Self::Query {
                    scope: Scope::Order,
                    id: (*id).into(),
                    cancel: true,
                })
            }
            _ => None,
        }
    }
    fn scope(&self) -> Scope {
        match self {
            Self::Configuration(id) | Self::Info(id) | Self::Content(id) => Scope::Look(id.clone()),
            Self::Query { scope, .. } => scope.clone(),
            _ => Scope::Order,
        }
    }
    fn readable(&self) -> bool {
        !matches!(self, Self::Query { .. })
    }
    fn accepts(&self, method: &str) -> bool {
        matches!(
            (self, method),
            (Self::Content(_), "PUT")
                | (Self::Configuration(_), "PATCH" | "DELETE")
                | (Self::Order, "PUT")
                | (Self::Query { cancel: true, .. }, "POST")
        )
    }
}

fn prepare(
    route: &Route,
    request: &Request,
    body: &[u8],
    snapshot: &RuntimeSnapshot,
) -> Result<(Prepared, bool)> {
    let if_match = request.headers.get("if-match").map(String::as_str);
    let if_none_match = request.headers.get("if-none-match").map(String::as_str);
    if request.query.is_some()
        && !(request.method == "DELETE" && request.query.as_deref() == Some("active=off"))
    {
        return Err(problem(
            422,
            "invalid_query",
            "This mutation query is not supported.",
        ));
    }
    match route {
        Route::Content(id) => {
            let current = snapshot
                .manifest
                .entries
                .get(id)
                .filter(|entry| !entry.removed)
                .map(|entry| format!("\"sha256:{}\"", entry.content_hash));
            check_precondition(current.as_deref(), if_match, if_none_match)?;
            Ok((
                snapshot.manifest.prepare_put(id, body, &snapshot.context)?,
                current.is_none(),
            ))
        }
        Route::Configuration(id) => {
            let etag = snapshot.manifest.configuration_etag(id)?;
            check_precondition(Some(&etag), if_match, if_none_match)?;
            let prepared = if request.method == "PATCH" {
                let patch = Patch::decode(body)?;
                snapshot.manifest.prepare_patch(
                    id,
                    &patch,
                    snapshot.content(id)?,
                    &snapshot.context,
                )?
            } else {
                snapshot.manifest.prepare_delete(
                    id,
                    request.query.as_deref() == Some("active=off"),
                    &snapshot.context,
                )?
            };
            Ok((prepared, false))
        }
        Route::Order => {
            check_precondition(
                Some(&snapshot.manifest.order_etag()),
                if_match,
                if_none_match,
            )?;
            let value = effects::parse_json(body)
                .map_err(|_| problem(422, "invalid_order", "The order JSON is invalid."))?;
            let object = value
                .as_object()
                .filter(|object| object.len() == 1 && object.contains_key("ids"))
                .ok_or_else(|| {
                    problem(422, "invalid_order", "Supply only the complete ids array.")
                })?;
            let ids: Vec<String> = serde_json::from_value(object["ids"].clone())
                .map_err(|_| problem(422, "invalid_order", "The order must contain string IDs."))?;
            Ok((
                snapshot.manifest.prepare_order(&ids, &snapshot.context)?,
                false,
            ))
        }
        _ => Err(problem(
            405,
            "method_not_allowed",
            "This resource is not mutable.",
        )),
    }
}
fn validate_body(route: &Route, request: &Request, body: &[u8], digest: &str) -> Result<()> {
    if request.length != body.len() {
        return Err(problem(
            400,
            "incomplete_body",
            "The complete request body is required.",
        ));
    }
    let expected_type = match route {
        Route::Content(_) => Some("application/octet-stream"),
        Route::Order => Some("application/json"),
        Route::Configuration(_) if request.method == "PATCH" => Some("application/json"),
        _ => None,
    };
    if let Some(expected) = expected_type {
        if request
            .headers
            .get("content-type")
            .is_none_or(|value| value.split(';').next().unwrap_or("").trim() != expected)
        {
            return Err(problem(
                415,
                "unsupported_media_type",
                "The request content type is not supported.",
            ));
        }
    }
    match route {
        Route::Content(_) => {
            header(request, "content-length")?;
            if !matches!(body.len(), effects::SIZE | effects::SIZE_WITH_LEVELS) {
                return Err(problem(
                    413,
                    "invalid_content_length",
                    "A GRF must contain 8192 or 8704 bytes.",
                ));
            }
            if header(request, "x-grkit-content-sha256")? != digest {
                return Err(problem(
                    422,
                    "content_digest_mismatch",
                    "The complete body does not match its SHA-256.",
                ));
            }
        }
        Route::Order if body.len() > 4096 => {
            return Err(problem(
                413,
                "order_too_large",
                "The order body exceeds its size limit.",
            ));
        }
        Route::Configuration(_) if request.method == "PATCH" && body.len() > 1024 => {
            return Err(problem(
                413,
                "patch_too_large",
                "The Look patch exceeds its size limit.",
            ));
        }
        _ if expected_type.is_none() && !body.is_empty() => {
            return Err(problem(
                400,
                "unexpected_body",
                "This mutation does not accept a body.",
            ));
        }
        _ => {}
    }
    Ok(())
}
fn info(snapshot: &RuntimeSnapshot, id: &str) -> Result<Value> {
    snapshot.manifest.configuration(id)?;
    Ok(
        json!({"id":id,"source":snapshot.manifest.origin(id, &snapshot.context)?,"loaded":snapshot.loaded_ids.contains(id),"selected":snapshot.context.selected.as_deref()==Some(id),"dependencies":snapshot.context.dependencies.get(id).cloned().unwrap_or_default(),"dependenciesComplete":snapshot.context.dependencies_complete,"blocked":snapshot.context.blocked_ids.contains(id)}),
    )
}
fn snapshot_value(snapshot: &RuntimeSnapshot) -> Result<Value> {
    let mut looks = Vec::new();
    for (position, id) in snapshot.manifest.order.iter().enumerate() {
        let mut look =
            serde_json::to_value(snapshot.manifest.configuration(id)?).expect("configuration JSON");
        look["position"] = json!(position);
        look["runtime"] = info(snapshot, id)?;
        looks.push(look);
    }
    Ok(
        json!({"revision":snapshot.revision()?,"generation":snapshot.generation,"looks":looks,"counts":{"listed":snapshot.manifest.order.len(),"loaded":snapshot.loaded_ids.len()},"limits":{"listedLooks":MAX_LISTED,"loadedLooks":MAX_LOADED}}),
    )
}
fn observe(
    snapshot: &RuntimeSnapshot,
    context: &RequestContext<'_>,
    hub: &SharedChanges,
    request_id: Option<&str>,
) -> Result<Watermark> {
    let view = snapshot_value(snapshot)?;
    let mut events = hub.lock().map_err(|_| coordinator())?;
    let (mut state, watermark) = events.snapshot();
    check_context(context, &watermark)?;
    if state["looks"] != view {
        if !state.is_object() {
            state = json!({});
        }
        state["looks"] = view;
        let mut event =
            json!({"type":"resourceChanged","changed":"looks","revision":snapshot.revision()?});
        if let Some(id) = request_id {
            event["requestId"] = json!(id.to_ascii_lowercase());
        }
        events.publish(state, event.as_object().expect("event").clone())?;
    }
    Ok(events.snapshot().1)
}
fn check_context(context: &RequestContext<'_>, watermark: &Watermark) -> Result<()> {
    if context.boot_id != watermark.boot_id || context.card_id != watermark.card_id {
        Err(problem(
            409,
            "context_changed",
            "Read the current boot and card context before accessing Looks.",
        ))
    } else {
        Ok(())
    }
}
fn mutation_result(receipt: &RuntimeSnapshot, scope: &Scope) -> Result<Value> {
    Ok(match scope {
        Scope::Look(look) => {
            if receipt
                .manifest
                .entries
                .get(look)
                .is_some_and(|entry| !entry.removed)
            {
                serde_json::to_value(receipt.manifest.configuration(look)?)
                    .expect("configuration JSON")
            } else {
                json!({"id":look,"removed":true})
            }
        }
        Scope::Order => json!({"ids":receipt.manifest.order}),
    })
}
fn publish_record(
    requests: &Mutex<RequestCache>,
    hub: &SharedChanges,
    id: &str,
    scope: &Scope,
    context: &RequestContext<'_>,
    record: &RecordedResult,
    terminal: bool,
) -> Result<()> {
    let mut events = hub.lock().map_err(|_| coordinator())?;
    let (state, watermark) = events.snapshot();
    check_context(context, &watermark)?;
    let mut cache = requests.lock().map_err(|_| coordinator())?;
    let previous = cache.get(id, Instant::now())?.clone();
    cache.update(id, record.clone(), false, Instant::now())?;
    if let Err(error) = events.publish(
        state,
        json!({"type":"requestChanged","domain":scope.domain(),
        "requestId":id.to_ascii_lowercase(),"queryUri":scope.query_uri(id),"request":record.body})
        .as_object()
        .expect("event")
        .clone(),
    ) {
        cache.update(id, previous, false, Instant::now())?;
        return Err(error);
    }
    if terminal {
        cache.update(id, record.clone(), true, Instant::now())?;
    }
    Ok(())
}
fn query(
    requests: &Mutex<RequestCache>,
    scope: &Scope,
    id: &str,
    context: &RequestContext<'_>,
) -> Result<RecordedResult> {
    let records = requests.lock().map_err(|_| coordinator())?;
    let record = records.get(id, Instant::now())?;
    if record.body["resource"] != scope.resource()
        || record.body["domain"] != scope.domain()
        || record.body["bootId"] != context.boot_id
        || record.body["cardId"] != context.card_id
    {
        return Err(problem(
            404,
            "request_result_unknown",
            "No request result belongs to this resource and context.",
        ));
    }
    Ok(record.clone())
}
fn summary(
    id: &str,
    scope: &Scope,
    context: &RequestContext<'_>,
    state: &str,
    phase: &str,
    result: Option<Value>,
    error: Option<Problem>,
) -> Value {
    json!({"requestId":id.to_ascii_lowercase(),"domain":scope.domain(),"resource":scope.resource(),"bootId":context.boot_id,"cardId":context.card_id,"status":state,"phase":phase,"cancellable":false,"progress":null,"result":result,"error":error})
}
fn record_reply(record: RecordedResult, scope: &Scope, id: &str) -> Reply {
    if record.status >= 400 {
        let mut error = record.body["error"].clone();
        for field in ["requestId", "domain", "resource", "bootId", "cardId"] {
            error[field] = record.body[field].clone();
        }
        return Reply::json(record.status, error);
    }
    let reply = Reply::json(record.status, record.body);
    if record.status == 202 {
        reply.header("Location", scope.query_uri(id))
    } else {
        reply
    }
}
fn header<'a>(request: &'a Request, name: &str) -> Result<&'a str> {
    request
        .headers
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| {
            Problem::new(
                400,
                "missing_header",
                format!("The {name} header is required."),
            )
        })
}
fn lock_problem<T>(error: TryLockError<T>) -> Problem {
    match error {
        TryLockError::WouldBlock => problem(
            409,
            "camera_busy",
            "The runtime coordinator is applying another request.",
        ),
        TryLockError::Poisoned(_) => coordinator(),
    }
}
fn recovery_problem() -> Problem {
    problem(
        503,
        "recovery_required",
        "The runtime outcome is uncertain. Verified recovery is required before further access.",
    )
}
fn coordinator() -> Problem {
    problem(
        503,
        "coordinator_unavailable",
        "The runtime coordinator is unavailable.",
    )
}
fn problem(status: u16, code: &'static str, detail: &'static str) -> Problem {
    Problem::new(status, code, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        library::{Context, Manifest, Origin},
        protocol::{strong_etag, ChangeHub},
        runtime_backend::{RuntimeCapabilities, UnavailableBackend},
    };
    use grkit_core::testing;
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    // Test-only backend: models an acknowledged commit. It is never selectable
    // by the production service and does not establish physical camera safety.
    struct MemoryBackend {
        state: Arc<Mutex<RuntimeSnapshot>>,
        applies: Arc<AtomicUsize>,
        fault: Arc<AtomicUsize>,
    }
    impl RuntimeBackend for MemoryBackend {
        fn capabilities(&self) -> RuntimeCapabilities {
            RuntimeCapabilities {
                readable: true,
                hot_reload: self.state.lock().unwrap().context.hot_reload
                    && self.fault.load(Ordering::SeqCst) != 7,
            }
        }
        fn snapshot(&mut self) -> Result<RuntimeSnapshot> {
            Ok(self.state.lock().unwrap().clone())
        }
        fn apply(&mut self, expected: &str, prepared: Prepared) -> Result<RuntimeSnapshot> {
            self.applies.fetch_add(1, Ordering::SeqCst);
            if self.fault.load(Ordering::SeqCst) == 1 {
                return Err(problem(
                    503,
                    "runtime_unavailable",
                    "Injected apply failure.",
                ));
            }
            let mut state = self.state.lock().unwrap();
            if state.revision()? != expected {
                return Err(problem(
                    412,
                    "precondition_failed",
                    "The runtime changed before commit.",
                ));
            }
            state.manifest = prepared.manifest;
            if let Some(content) = prepared.content {
                state.contents.insert(content.hash, content.bytes);
            }
            let live: BTreeSet<_> = state
                .manifest
                .entries
                .values()
                .filter(|entry| !entry.removed)
                .map(|entry| entry.content_hash.clone())
                .collect();
            state.contents.retain(|hash, _| live.contains(hash));
            state.loaded_ids = state
                .manifest
                .entries
                .iter()
                .filter(|(_, entry)| !entry.removed && entry.enabled)
                .map(|(id, _)| id.clone())
                .collect();
            if prepared.select_off {
                state.context.selected = None;
            }
            state.generation += 1;
            let mut receipt = state.clone();
            if self.fault.load(Ordering::SeqCst) == 2 {
                receipt.generation -= 1;
            }
            if self.fault.load(Ordering::SeqCst) == 3 {
                receipt.contents.clear();
            }
            if self.fault.load(Ordering::SeqCst) == 4 {
                receipt.card_id = "other-card".into();
            }
            if self.fault.load(Ordering::SeqCst) == 5 {
                receipt.runtime_epoch = [9; 4];
            }
            if self.fault.load(Ordering::SeqCst) == 6 {
                return Err(recovery_problem());
            }
            Ok(receipt)
        }
    }
    struct Harness {
        api: LookApi,
        hub: SharedChanges,
        requests: Mutex<RequestCache>,
        state: Arc<Mutex<RuntimeSnapshot>>,
        applies: Arc<AtomicUsize>,
        fault: Arc<AtomicUsize>,
    }
    impl Harness {
        fn new() -> Self {
            let state = Arc::new(Mutex::new(RuntimeSnapshot {
                manifest: Manifest::default(),
                card_id: "card-test".into(),
                runtime_epoch: [1, 2, 3, 4],
                context: Context {
                    hot_reload: true,
                    dependencies_complete: true,
                    supported_features: u32::MAX,
                    ..Context::default()
                },
                loaded_ids: BTreeSet::new(),
                contents: BTreeMap::new(),
                generation: 0,
            }));
            let applies = Arc::new(AtomicUsize::new(0));
            let fault = Arc::new(AtomicUsize::new(0));
            Self {
                api: LookApi::new(Box::new(MemoryBackend {
                    state: Arc::clone(&state),
                    applies: Arc::clone(&applies),
                    fault: Arc::clone(&fault),
                })),
                hub: Arc::new(Mutex::new(ChangeHub::new(
                    json!({"props":{"kept":true}}),
                    "boot-test".into(),
                    "card-test".into(),
                    2,
                    32,
                    8192,
                ))),
                requests: Mutex::new(RequestCache::new(
                    "boot-test".into(),
                    64,
                    16384,
                    Duration::from_secs(60),
                )),
                state,
                applies,
                fault,
            }
        }
        fn context() -> RequestContext<'static> {
            RequestContext {
                device_id: "device-test",
                boot_id: "boot-test",
                card_id: "card-test",
                write_allowed: true,
                software_pending: false,
            }
        }
        fn call(
            &self,
            method: &str,
            path: &str,
            body: &[u8],
            request_id: Option<u32>,
            headers: &[(&str, String)],
        ) -> Reply {
            let request = request(method, path, body, request_id, headers);
            self.api
                .handle(&request, body, &Self::context(), &self.hub, &self.requests)
                .unwrap()
        }
        fn put(&self, id: &str, request_id: u32) -> Reply {
            self.call(
                "PUT",
                &format!("/v1/looks/{id}/content"),
                &testing::effect(id),
                Some(request_id),
                &[("If-None-Match", "*".into())],
            )
        }
        fn config_tag(&self, id: &str) -> String {
            header_value(
                &self.call("GET", &format!("/v1/looks/{id}"), b"", None, &[]),
                "ETag",
            )
        }
    }
    fn uuid(id: u32) -> String {
        format!("00000000-0000-4000-8000-{id:012x}")
    }
    fn request(
        method: &str,
        path: &str,
        body: &[u8],
        id: Option<u32>,
        headers: &[(&str, String)],
    ) -> Request {
        let media = if path.ends_with("/content") {
            "application/octet-stream"
        } else {
            "application/json"
        };
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nContent-Length: {}\r\nContent-Type: {media}\r\nX-GRKit-Content-SHA256: {}\r\n",
            body.len(),
            packaging::sha256(body)
        );
        if let Some(id) = id {
            head.push_str(&format!("X-GRKit-Request-Id: {}\r\nX-GRKit-Boot-Id: boot-test\r\nX-GRKit-Card-Id: card-test\r\n",uuid(id)));
        }
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        crate::parse(head.as_bytes()).unwrap()
    }
    fn body(reply: &Reply) -> Value {
        serde_json::from_slice(&reply.body).unwrap()
    }
    fn header_value(reply: &Reply, name: &str) -> String {
        let bytes = reply.to_bytes();
        let end = crate::head_end(&bytes).unwrap();
        std::str::from_utf8(&bytes[..end])
            .unwrap()
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(key, _)| *key == name)
                    .map(|(_, value)| value.trim().to_owned())
            })
            .unwrap()
    }
    #[test]
    fn content_configuration_and_order_are_exact_versioned_representations() {
        let h = Harness::new();
        assert_eq!(h.put("sample", 1).status, 201);
        let content = h.call("GET", "/v1/looks/sample/content", b"", None, &[]);
        assert_eq!(content.body, testing::effect("sample"));
        assert_eq!(header_value(&content, "ETag"), strong_etag(&content.body));
        let config = h.call("GET", "/v1/looks/sample", b"", None, &[]);
        assert_eq!(header_value(&config, "ETag"), strong_etag(&config.body));
        assert!(body(&config).get("position").is_none());
        assert!(body(&config).get("runtime").is_none());
        assert_eq!(header_value(&config, "X-GRKit-Card-Id"), "card-test");
        let list = h.call("GET", "/v1/looks", b"", None, &[]);
        assert_eq!(body(&list)["counts"]["loaded"], 1);
        assert_eq!(body(&list)["cardId"], "card-test");
        assert_eq!(h.hub.lock().unwrap().snapshot().0["props"]["kept"], true);
        let order = h.call("GET", "/v1/params/looks/order", b"", None, &[]);
        assert_eq!(header_value(&order, "ETag"), strong_etag(&order.body));
    }
    #[test]
    fn same_intent_replays_success_and_failure_without_reapplying() {
        let h = Harness::new();
        let first = h.put("sample", 1);
        let retry = h.put("sample", 1);
        assert_eq!(retry.status, first.status);
        assert_eq!(retry.body, first.body);
        assert_eq!(h.applies.load(Ordering::SeqCst), 1);
        let patch = b"{\"name\":\"Renamed\"}";
        let stale = [("If-Match", "\"stale\"".into())];
        let failed = h.call("PATCH", "/v1/looks/sample", patch, Some(2), &stale);
        assert_eq!(failed.status, 412);
        assert_eq!(
            h.call("PATCH", "/v1/looks/sample", patch, Some(2), &stale)
                .body,
            failed.body
        );
        let corrected = [("If-Match", h.config_tag("sample"))];
        assert_eq!(
            h.call("PATCH", "/v1/looks/sample", patch, Some(2), &corrected)
                .status,
            409
        );
        assert_eq!(
            h.call("PATCH", "/v1/looks/sample", patch, Some(3), &corrected)
                .status,
            200
        );
        assert_eq!(h.applies.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn unverified_hot_reload_keeps_reads_and_request_queries_but_never_applies() {
        let h = Harness::new();
        assert_eq!(h.put("sample", 1).status, 201);
        h.state.lock().unwrap().context.hot_reload = false;
        let before = h.state.lock().unwrap().clone();
        assert_eq!(h.call("GET", "/v1/looks", b"", None, &[]).status, 200);
        assert_eq!(
            h.call("GET", "/v1/looks/sample/content", b"", None, &[])
                .status,
            200
        );
        let tag = h.config_tag("sample");
        let order_tag = header_value(
            &h.call("GET", "/v1/params/looks/order", b"", None, &[]),
            "ETag",
        );
        let rejected = h.put("other", 2);
        assert_eq!(rejected.status, 503);
        assert_eq!(body(&rejected)["code"], "hot_reload_unavailable");
        assert_eq!(h.put("other", 2).body, rejected.body);
        assert_eq!(
            h.call(
                "PATCH",
                "/v1/looks/sample",
                br#"{"name":"Changed"}"#,
                Some(3),
                &[("If-Match", tag.clone())]
            )
            .status,
            503
        );
        assert_eq!(
            h.call(
                "DELETE",
                "/v1/looks/sample",
                b"",
                Some(4),
                &[("If-Match", tag)]
            )
            .status,
            503
        );
        assert_eq!(
            h.call(
                "PUT",
                "/v1/params/looks/order",
                br#"{"ids":["sample"]}"#,
                Some(5),
                &[("If-Match", order_tag)]
            )
            .status,
            503
        );
        let query = format!("/v1/looks/other/requests/{}", uuid(2));
        assert_eq!(
            body(&h.call("GET", &query, b"", None, &[]))["status"],
            "failed"
        );
        let cancel = h.call("POST", &format!("{query}/cancel"), b"", Some(6), &[]);
        assert_eq!(cancel.status, 409);
        assert_eq!(body(&cancel)["code"], "request_finished");
        assert_eq!(h.applies.load(Ordering::SeqCst), 1);
        let after = h.state.lock().unwrap();
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.revision().unwrap(), before.revision().unwrap());
        assert!(h.api.pending.lock().unwrap().is_none());

        // A readable owner snapshot cannot override a transport-level refusal.
        let h = Harness::new();
        h.fault.store(7, Ordering::SeqCst);
        assert!(h.state.lock().unwrap().context.hot_reload);
        assert_eq!(h.put("sample", 1).status, 503);
        assert_eq!(h.applies.load(Ordering::SeqCst), 0);
        assert_eq!(h.call("GET", "/v1/looks", b"", None, &[]).status, 200);
    }

    #[test]
    fn reserved_look_ids_remain_addressable_and_order_is_a_separate_resource() {
        let h = Harness::new();
        assert_eq!(h.put("order", 1).status, 201);
        assert_eq!(h.put("requests", 2).status, 201);
        assert_eq!(h.call("GET", "/v1/looks/order", b"", None, &[]).status, 200);
        let tag = header_value(
            &h.call("GET", "/v1/params/looks/order", b"", None, &[]),
            "ETag",
        );
        assert_eq!(
            h.call(
                "PUT",
                "/v1/params/looks/order",
                b"{\"ids\":[\"requests\",\"order\"]}",
                Some(3),
                &[("If-Match", tag)]
            )
            .status,
            200
        );
        assert_eq!(
            body(&h.call("GET", "/v1/params/looks/order", b"", None, &[]))["ids"],
            json!(["requests", "order"])
        );
    }
    #[test]
    fn deleted_look_results_remain_queryable_only_at_the_original_scope() {
        let h = Harness::new();
        h.put("sample", 1);
        let tag = h.config_tag("sample");
        assert_eq!(
            h.call(
                "DELETE",
                "/v1/looks/sample",
                b"",
                Some(2),
                &[("If-Match", tag)]
            )
            .status,
            200
        );
        assert_eq!(
            h.call("GET", "/v1/looks/sample", b"", None, &[]).status,
            404
        );
        let query = format!("/v1/looks/sample/requests/{}", uuid(2));
        assert_eq!(
            body(&h.call("GET", &query, b"", None, &[]))["status"],
            "succeeded"
        );
        assert_eq!(
            h.call(
                "GET",
                &format!("/v1/looks/other/requests/{}", uuid(2)),
                b"",
                None,
                &[]
            )
            .status,
            404
        );
        assert_eq!(
            h.call(
                "GET",
                &format!("/v1/params/looks/requests/{}", uuid(2)),
                b"",
                None,
                &[]
            )
            .status,
            404
        );
        assert_eq!(h.put("other", 2).status, 409);
    }
    #[test]
    fn official_edits_dependencies_and_selected_deletion_are_enforced() {
        let h = Harness::new();
        h.put("sample", 1);
        {
            let mut state = h.state.lock().unwrap();
            state.manifest.entries.get_mut("sample").unwrap().origin = Origin::Official;
            state.context.selected = Some("sample".into());
        }
        let tag = h.config_tag("sample");
        assert_eq!(
            h.call(
                "PATCH",
                "/v1/looks/sample",
                b"{\"name\":\"Edited\"}",
                Some(2),
                &[("If-Match", tag.clone())]
            )
            .status,
            409
        );
        assert_eq!(
            h.call(
                "DELETE",
                "/v1/looks/sample",
                b"",
                Some(3),
                &[("If-Match", tag.clone())]
            )
            .status,
            409
        );
        h.state
            .lock()
            .unwrap()
            .context
            .dependencies
            .insert("sample".into(), vec!["recipe:1".into()]);
        assert_eq!(
            h.call(
                "DELETE",
                "/v1/looks/sample?active=off",
                b"",
                Some(4),
                &[("If-Match", tag.clone())]
            )
            .status,
            409
        );
        h.state.lock().unwrap().context.dependencies.clear();
        assert_eq!(
            h.call(
                "DELETE",
                "/v1/looks/sample?active=off",
                b"",
                Some(5),
                &[("If-Match", tag)]
            )
            .status,
            200
        );
        assert!(h.state.lock().unwrap().context.selected.is_none());
        assert!(h.state.lock().unwrap().manifest.entries["sample"].removed);
    }
    #[test]
    fn bad_patch_digest_and_id_never_reach_apply() {
        let h = Harness::new();
        h.put("sample", 1);
        let tag = h.config_tag("sample");
        for (index, patch) in [
            b"{\"hidden\":true}".as_slice(),
            b"{\"name\":null}",
            b"{\"enabled\":true,\"enabled\":false}",
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(
                h.call(
                    "PATCH",
                    "/v1/looks/sample",
                    patch,
                    Some(index as u32 + 2),
                    &[("If-Match", tag.clone())]
                )
                .status,
                422
            );
        }
        let bytes = testing::effect("other");
        assert_eq!(
            h.call(
                "PUT",
                "/v1/looks/mismatch/content",
                &bytes,
                Some(8),
                &[("If-None-Match", "*".into())]
            )
            .status,
            422
        );
        let mut req = request(
            "PUT",
            "/v1/looks/other/content",
            &bytes,
            Some(9),
            &[("If-None-Match", "*".into())],
        );
        req.headers
            .insert("x-grkit-content-sha256".into(), "0".repeat(64));
        assert_eq!(
            h.api
                .handle(&req, &bytes, &Harness::context(), &h.hub, &h.requests)
                .unwrap()
                .status,
            422
        );
        assert_eq!(h.applies.load(Ordering::SeqCst), 1);
    }
    fn reconciler(h: &Harness) -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
        struct AcknowledgedBackend {
            inner: Box<dyn RuntimeBackend>,
            ready: Arc<AtomicUsize>,
            polls: Arc<AtomicUsize>,
            receipt: Option<RuntimeSnapshot>,
        }
        impl RuntimeBackend for AcknowledgedBackend {
            fn capabilities(&self) -> RuntimeCapabilities {
                self.inner.capabilities()
            }
            fn snapshot(&mut self) -> Result<RuntimeSnapshot> {
                self.inner.snapshot()
            }
            fn apply(&mut self, revision: &str, prepared: Prepared) -> Result<RuntimeSnapshot> {
                let result = self.inner.apply(revision, prepared);
                if result
                    .as_ref()
                    .is_err_and(|e| e.code == "recovery_required")
                {
                    self.receipt = Some(self.inner.snapshot()?);
                }
                result
            }
            fn reconcile_apply(&mut self) -> Result<Option<RuntimeSnapshot>> {
                self.polls.fetch_add(1, Ordering::SeqCst);
                match self.ready.load(Ordering::SeqCst) {
                    0 => Ok(None),
                    1 => Ok(self.receipt.take()),
                    2 => {
                        let mut receipt = self.receipt.clone();
                        if let Some(r) = receipt.as_mut() {
                            r.card_id = "wrong-card".into();
                        }
                        Ok(receipt)
                    }
                    _ => Err(recovery_problem()),
                }
            }
        }
        let ready = Arc::new(AtomicUsize::new(0));
        let polls = Arc::new(AtomicUsize::new(0));
        let mut backend = h.api.backend.lock().unwrap();
        let inner = std::mem::replace(&mut *backend, Box::new(UnavailableBackend));
        *backend = Box::new(AcknowledgedBackend {
            inner,
            ready: ready.clone(),
            polls: polls.clone(),
            receipt: None,
        });
        (ready, polls)
    }
    #[test]
    fn request_poll_reconciles_only_its_original_acknowledged_apply() {
        let h = Harness::new();
        let (ready, polls) = reconciler(&h);
        h.fault.store(6, Ordering::SeqCst);
        assert_eq!(h.put("sample", 1).status, 503);
        let uri = format!("/v1/looks/sample/requests/{}", uuid(1));
        assert_eq!(
            body(&h.call("GET", &uri, b"", None, &[]))["phase"],
            "recovering"
        );
        for mode in [2, 3] {
            ready.store(mode, Ordering::SeqCst);
            assert_eq!(
                body(&h.call("GET", &uri, b"", None, &[]))["phase"],
                "recovering"
            );
            assert!(h.api.recovery_required());
        }
        ready.store(1, Ordering::SeqCst);
        let before = polls.load(Ordering::SeqCst);
        assert_eq!(
            h.call(
                "GET",
                &format!("/v1/looks/other/requests/{}", uuid(1)),
                b"",
                None,
                &[]
            )
            .status,
            404
        );
        let mut changed = Harness::context();
        changed.device_id = "other-device";
        let req = request("GET", &uri, b"", None, &[]);
        h.api
            .handle(&req, b"", &changed, &h.hub, &h.requests)
            .unwrap();
        assert_eq!(polls.load(Ordering::SeqCst), before);
        {
            let _lock = h.api.backend.lock().unwrap();
            assert_eq!(
                body(&h.call("GET", &uri, b"", None, &[]))["phase"],
                "recovering"
            );
        }
        let done = h.call("GET", &uri, b"", None, &[]);
        assert_eq!(body(&done)["status"], "succeeded");
        assert!(!h.api.recovery_required());
        assert_eq!(h.applies.load(Ordering::SeqCst), 1);
        let seq = h.hub.lock().unwrap().snapshot().1.snapshot_seq;
        h.call("GET", &uri, b"", None, &[]);
        assert_eq!(h.hub.lock().unwrap().snapshot().1.snapshot_seq, seq);
        assert_eq!(h.applies.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn accepting_another_card_discards_the_old_pending_apply() {
        let h = Harness::new();
        let (ready, polls) = reconciler(&h);
        h.fault.store(6, Ordering::SeqCst);
        h.put("sample", 1);
        h.api.accept_new_card().unwrap();
        ready.store(1, Ordering::SeqCst);
        let uri = format!("/v1/looks/sample/requests/{}", uuid(1));
        assert_eq!(
            body(&h.call("GET", &uri, b"", None, &[]))["phase"],
            "recovering"
        );
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert!(h.api.pending.lock().unwrap().is_none());
    }
    #[test]
    fn backend_failure_or_invalid_receipt_never_returns_success() {
        for fault in 1..=6 {
            let h = Harness::new();
            h.fault.store(fault, Ordering::SeqCst);
            let reply = h.put("sample", 1);
            assert_eq!(reply.status, 503);
            assert_eq!(body(&reply)["status"], 503);
            let query = h.call(
                "GET",
                &format!("/v1/looks/sample/requests/{}", uuid(1)),
                b"",
                None,
                &[],
            );
            assert_eq!(
                body(&query)["status"],
                if fault == 1 { "failed" } else { "running" }
            );
            if fault != 1 {
                assert_eq!(body(&query)["phase"], "recovering");
                assert!(h.api.recovery_required());
                let future = Instant::now() + Duration::from_secs(3600);
                assert!(
                    h.requests.lock().unwrap().get(&uuid(1), future).is_ok(),
                    "uncertain requests must remain active"
                );
                assert_eq!(h.put("other", 2).status, 503);
                assert_eq!(h.applies.load(Ordering::SeqCst), 1);
            }
        }
    }
    #[test]
    fn publication_failure_after_commit_never_becomes_terminal_failure() {
        struct ChangedCard {
            inner: Box<dyn RuntimeBackend>,
            hub: SharedChanges,
        }
        impl RuntimeBackend for ChangedCard {
            fn capabilities(&self) -> RuntimeCapabilities {
                self.inner.capabilities()
            }
            fn snapshot(&mut self) -> Result<RuntimeSnapshot> {
                self.inner.snapshot()
            }
            fn apply(&mut self, revision: &str, prepared: Prepared) -> Result<RuntimeSnapshot> {
                let receipt = self.inner.apply(revision, prepared)?;
                self.hub
                    .lock()
                    .unwrap()
                    .replace_card("other-card".into(), Value::Null)?;
                Ok(receipt)
            }
        }
        let h = Harness::new();
        {
            let mut backend = h.api.backend.lock().unwrap();
            let inner = std::mem::replace(
                &mut *backend,
                Box::new(crate::runtime_backend::UnavailableBackend),
            );
            *backend = Box::new(ChangedCard {
                inner,
                hub: Arc::clone(&h.hub),
            });
        }
        assert_eq!(h.put("sample", 1).status, 409);
        assert_eq!(h.applies.load(Ordering::SeqCst), 1);
        assert!(h
            .state
            .lock()
            .unwrap()
            .manifest
            .entries
            .contains_key("sample"));
        let cache = h.requests.lock().unwrap();
        let record = cache
            .get(&uuid(1), Instant::now() + Duration::from_secs(3600))
            .unwrap();
        assert_eq!(record.body["status"], "running");
        assert_eq!(record.body["phase"], "recovering");
        assert!(h.api.recovery_required());
        drop(cache);
        {
            let mut state = h.state.lock().unwrap();
            state.manifest.entries.get_mut("sample").unwrap().enabled = false;
            state.generation += 1;
        }
        h.hub
            .lock()
            .unwrap()
            .replace_card("card-test".into(), json!({}))
            .unwrap();
        let result = h.call(
            "GET",
            &format!("/v1/looks/sample/requests/{}", uuid(1)),
            b"",
            None,
            &[],
        );
        assert_eq!(body(&result)["status"], "succeeded");
        assert_eq!(body(&result)["result"]["enabled"], true);
        let view = h.hub.lock().unwrap().snapshot().0;
        assert_eq!(view["looks"]["generation"], 2);
        assert_eq!(view["looks"]["looks"][0]["enabled"], false);
        assert!(!h.api.recovery_required());
        assert_eq!(h.applies.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn busy_backend_does_not_block_result_queries_and_unavailable_is_explicit() {
        let h = Harness::new();
        h.put("sample", 1);
        let _guard = h.api.backend.lock().unwrap();
        assert_eq!(h.call("GET", "/v1/looks", b"", None, &[]).status, 409);
        assert_eq!(
            h.call(
                "GET",
                &format!("/v1/looks/sample/requests/{}", uuid(1)),
                b"",
                None,
                &[]
            )
            .status,
            200
        );
        assert_eq!(h.put("other", 2).status, 409);
        let api = LookApi::new(Box::new(UnavailableBackend));
        assert_eq!(
            api.handle(
                &request("GET", "/v1/looks", b"", None, &[]),
                b"",
                &Harness::context(),
                &h.hub,
                &h.requests
            )
            .unwrap()
            .status,
            503
        );
    }
    #[test]
    fn request_ids_are_global_across_software_looks_and_order_domains() {
        let h = Harness::new();
        let now = Instant::now();
        h.requests
            .lock()
            .unwrap()
            .admit(
                &uuid(1),
                Intent {
                    domain: "update".into(),
                    device_id: "device-test".into(),
                    boot_id: "boot-test".into(),
                    card_id: "card-test".into(),
                    method: "PUT".into(),
                    uri: "/v1/update".into(),
                    if_match: None,
                    if_none_match: None,
                    content_sha256: packaging::sha256(b"package"),
                },
                "card-test",
                RecordedResult {
                    status: 202,
                    body: json!({"domain":"update","status":"running"}),
                },
                now,
            )
            .unwrap();
        assert_eq!(h.put("sample", 1).status, 409);
        assert_eq!(h.applies.load(Ordering::SeqCst), 0);
        assert_eq!(
            h.call(
                "GET",
                &format!("/v1/looks/sample/requests/{}", uuid(1)),
                b"",
                None,
                &[]
            )
            .status,
            404
        );
        h.put("sample", 2);
        let order = h.call("GET", "/v1/params/looks/order", b"", None, &[]);
        assert_eq!(
            h.call(
                "PUT",
                "/v1/params/looks/order",
                b"{\"ids\":[\"sample\"]}",
                Some(2),
                &[("If-Match", header_value(&order, "ETag"))]
            )
            .status,
            409
        );
        assert_eq!(
            h.requests
                .lock()
                .unwrap()
                .get(&uuid(1), Instant::now())
                .unwrap()
                .body["domain"],
            "update"
        );
    }

    #[test]
    fn cancellation_has_its_own_request_id_and_cannot_rewrite_a_terminal_result() {
        let h = Harness::new();
        h.put("sample", 1);
        let target = format!("/v1/looks/sample/requests/{}/cancel", uuid(1));
        let reused = h.call("POST", &target, b"", Some(1), &[]);
        assert_eq!(body(&reused)["code"], "request_id_conflict");
        let cancelled = h.call("POST", &target, b"", Some(2), &[]);
        assert_eq!(body(&cancelled)["code"], "request_finished");
        let own = h.call(
            "GET",
            &format!("/v1/looks/sample/requests/{}", uuid(2)),
            b"",
            None,
            &[],
        );
        assert_eq!(body(&own)["status"], "failed");
        let original = h.call(
            "GET",
            &format!("/v1/looks/sample/requests/{}", uuid(1)),
            b"",
            None,
            &[],
        );
        assert_eq!(body(&original)["status"], "succeeded");
        assert_eq!(h.applies.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn disabled_writes_and_pending_software_have_distinct_cached_errors() {
        for (pending, expected) in [(true, 409), (false, 403)] {
            let h = Harness::new();
            let bytes = testing::effect("sample");
            let req = request(
                "PUT",
                "/v1/looks/sample/content",
                &bytes,
                Some(1),
                &[("If-None-Match", "*".into())],
            );
            let mut context = Harness::context();
            context.write_allowed = false;
            context.software_pending = pending;
            let reply = h
                .api
                .handle(&req, &bytes, &context, &h.hub, &h.requests)
                .unwrap();
            assert_eq!(reply.status, expected);
            assert_eq!(h.applies.load(Ordering::SeqCst), 0);
            let query = h.call(
                "GET",
                &format!("/v1/looks/sample/requests/{}", uuid(1)),
                b"",
                None,
                &[],
            );
            assert_eq!(body(&query)["status"], "failed");
        }
    }

    #[test]
    fn loaded_capacity_and_incomplete_dependencies_fail_before_apply() {
        let h = Harness::new();
        {
            let mut state = h.state.lock().unwrap();
            for index in 0..40 {
                let id = format!("look-{index}");
                let bytes = testing::effect(&id);
                let prepared = state
                    .manifest
                    .prepare_put(&id, &bytes, &state.context)
                    .unwrap();
                state.manifest = prepared.manifest;
                state.contents.insert(packaging::sha256(&bytes), bytes);
                state.loaded_ids.insert(id);
            }
        }
        let result = h.put("overflow", 1);
        assert_eq!(result.status, 409);
        assert_eq!(body(&result)["code"], "look_capacity");
        assert_eq!(h.applies.load(Ordering::SeqCst), 0);
        h.state.lock().unwrap().context.dependencies_complete = false;
        let tag = h.config_tag("look-0");
        let result = h.call(
            "DELETE",
            "/v1/looks/look-0",
            b"",
            Some(2),
            &[("If-Match", tag)],
        );
        assert_eq!(result.status, 503);
        assert_eq!(body(&result)["code"], "dependencies_unavailable");
    }

    #[test]
    fn invalid_runtime_receipt_emits_failure_not_success() {
        let h = Harness::new();
        h.fault.store(2, Ordering::SeqCst);
        let subscriber = h.hub.lock().unwrap().subscribe().unwrap().0;
        assert_eq!(h.put("sample", 1).status, 503);
        let mut terminal = Vec::new();
        loop {
            match h.hub.lock().unwrap().next(subscriber) {
                crate::protocol::NextChange::Event(event)
                    if event.fields["type"] == "requestChanged" =>
                {
                    terminal.push(event.fields["request"].clone())
                }
                crate::protocol::NextChange::Event(_) => {}
                crate::protocol::NextChange::Empty => break,
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0]["status"], "running");
        assert_eq!(terminal[0]["phase"], "recovering");
    }
    #[test]
    fn snapshots_from_another_card_are_not_published_under_the_current_context() {
        let h = Harness::new();
        h.state.lock().unwrap().card_id = "another-card".into();
        let reply = h.call("GET", "/v1/looks", b"", None, &[]);
        assert_eq!(reply.status, 409);
        assert!(h.hub.lock().unwrap().snapshot().0.get("looks").is_none());
        assert_eq!(h.put("sample", 1).status, 409);
        assert_eq!(h.applies.load(Ordering::SeqCst), 0);
    }
}
