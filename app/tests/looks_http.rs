//! Looks pages with the real store and HTTP transport, against a camera that
//! the test controls. Each mutation must carry the camera's preconditions.
use std::{
    env,
    ffi::OsString,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use grkit_app::App;
use nsh_core::geometry::Point;
use nsh_core::platform::{HttpBody, HttpRequest, Os};
use nsh_core::prelude::Points;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless, StubNetwork, StubResponse, StubRoute};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);
static ENV_LOCK: Mutex<()> = Mutex::new(());

const PORTRA: &str = "portra-400-endura-v1";
const TRIX: &str = "trix-400-mono-v1";
const CUSTOM: &str = "lut-night";

struct TestEnvironment {
    previous: Vec<(&'static str, Option<OsString>)>,
    directory: PathBuf,
}

impl TestEnvironment {
    fn new() -> Self {
        let changes = [
            ("GRKIT_DEMO", None),
            ("GRKIT_DEMO_SCREEN", None),
            ("GRKIT_DEMO_FAIL", None),
            ("GRKIT_CAMERA_URL", Some("http://camera.test")),
            ("GRKIT_RELEASE_BASE", Some("https://release.test")),
        ];
        let previous = changes
            .iter()
            .map(|(key, _)| (*key, env::var_os(key)))
            .collect();
        for (key, value) in changes {
            unsafe {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
        }
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            env::temp_dir().join(format!("grkit-looks-test-{}-{unique}", std::process::id()));
        Self {
            previous,
            directory,
        }
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            unsafe {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[derive(Clone)]
struct Entry {
    id: &'static str,
    name: String,
    enabled: bool,
    source: &'static str,
    dependencies: Vec<String>,
    revision: u32,
}

struct Camera {
    looks: Vec<Entry>,
    selected: Option<&'static str>,
    /// Answer the next mutation with this code (409).
    refuse: Option<&'static str>,
    /// Answer mutations with 202 and finish them on the result query.
    accept_later: bool,
    pending: Option<String>,
    order_revision: u32,
}

fn grf(id: &str) -> Vec<u8> {
    let definition = match id {
        PORTRA => include_str!("../../effects/portra-400-endura-v1/effect.json"),
        _ => include_str!("../../effects/trix-400-mono-v1/effect.json"),
    };
    grkit_core::effects::compile_definition(
        &grkit_core::effects::parse_json(definition.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn hash(id: &str, revision: u32) -> String {
    grkit_core::packaging::sha256(format!("{id}:{revision}").as_bytes())
}

impl Camera {
    fn new() -> Self {
        let entry = |id, name: &str, source, dependencies: Vec<String>| Entry {
            id,
            name: name.into(),
            enabled: true,
            source,
            dependencies,
            revision: 1,
        };
        Self {
            looks: vec![
                entry(PORTRA, "Portra 400", "official", vec!["recipe:1".into()]),
                entry(TRIX, "Tri-X 400", "official", Vec::new()),
                entry(CUSTOM, "Night", "custom", Vec::new()),
            ],
            selected: Some(PORTRA),
            refuse: None,
            accept_later: false,
            pending: None,
            order_revision: 1,
        }
    }

    fn list(&self) -> Value {
        let looks: Vec<Value> = self
            .looks
            .iter()
            .enumerate()
            .map(|(position, e)| {
                json!({
                    "id": e.id, "name": e.name, "contentHash": hash(e.id, e.revision),
                    "enabled": e.enabled, "position": position,
                    "runtime": {
                        "id": e.id, "source": e.source, "loaded": e.enabled,
                        "selected": self.selected == Some(e.id),
                        "dependencies": e.dependencies, "dependenciesComplete": true,
                        "blocked": false
                    }
                })
            })
            .collect();
        let loaded = self.looks.iter().filter(|e| e.enabled).count();
        json!({
            "revision": "sha256:r", "generation": 1, "looks": looks,
            "counts": {"listed": self.looks.len(), "loaded": loaded},
            "limits": {"listedLooks": 64, "loadedLooks": 40},
            "bootId": "boot-test", "cardId": "card-test", "snapshotSeq": 1
        })
    }

    fn etag(&self, id: &str) -> String {
        let e = self.looks.iter().find(|e| e.id == id).unwrap();
        format!("\"cfg-{}-{}-{}\"", e.id, e.revision, e.enabled)
    }
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn body(request: &HttpRequest) -> &[u8] {
    match &request.body {
        HttpBody::Bytes(bytes) => bytes,
        HttpBody::Empty => &[],
        HttpBody::Stream => panic!("unexpected streamed body"),
    }
}

fn response(status: u16, value: Value, etag: Option<String>) -> StubResponse {
    let mut response = StubResponse::default();
    response.status = status;
    response.body = serde_json::to_vec(&value).unwrap();
    response.headers = vec![
        ("X-GRKit-Boot-Id".into(), "boot-test".into()),
        ("X-GRKit-Card-Id".into(), "card-test".into()),
        (
            "ETag".into(),
            etag.unwrap_or_else(|| "\"sha256:test\"".into()),
        ),
    ];
    response
}

fn record(id: &str, status: &str) -> Value {
    json!({"requestId": id, "domain": "looks", "status": status, "phase": "completed",
           "cancellable": false, "result": null, "error": null})
}

/// Check the context of a mutation and refuse or accept it.
fn admit(camera: &mut Camera, request: &HttpRequest) -> Result<String, StubResponse> {
    let id = header(request, "X-GRKit-Request-Id")
        .expect("request id")
        .to_owned();
    assert_eq!(header(request, "X-GRKit-Boot-Id"), Some("boot-test"));
    assert_eq!(header(request, "X-GRKit-Card-Id"), Some("card-test"));
    if let Some(code) = camera.refuse.take() {
        return Err(response(
            409,
            json!({"status": 409, "code": code, "detail": "Refused."}),
            None,
        ));
    }
    Ok(id)
}

fn finish(camera: &mut Camera, id: String) -> StubResponse {
    if camera.accept_later {
        camera.pending = Some(id.clone());
        return response(202, record(&id, "running"), None);
    }
    response(200, record(&id, "succeeded"), None)
}

fn start_phone(
    environment: &TestEnvironment,
    camera: Camera,
) -> (Headless, Arc<Mutex<Camera>>, Arc<StubNetwork>) {
    let camera = Arc::new(Mutex::new(camera));
    let shared = camera.clone();
    let handler = StubRoute::Handler(Arc::new(move |request, reply| {
        let mut camera = shared.lock().unwrap();
        let url = request
            .url
            .strip_prefix("http://camera.test")
            .unwrap()
            .to_owned();
        let (path, query) = url
            .split_once('?')
            .map_or((url.as_str(), None), |(p, q)| (p, Some(q)));
        let parts: Vec<&str> = path.split('/').collect();
        let answer = match (request.method.as_str(), parts.as_slice()) {
            ("GET", ["", "v1", "props"]) => response(
                200,
                json!({
                    "bootId": "boot-test", "cardId": "card-test",
                    "camera": {"model": "RICOH GR III", "firmwareVersion": "2.21"},
                    "grkit": {"version": "1.7.0"},
                    "storage": {"state": "ready"}
                }),
                None,
            ),
            ("GET", ["", "v1", "update"]) => response(
                200,
                json!({"inventory": {"package": {"version": "1.7.0", "modules": [], "effects": [], "left_out": []},
                       "state": "none", "backup": null}, "request": null}),
                None,
            ),
            ("GET", ["", "v1", "rollback"]) => response(
                200,
                json!({"backup": {"available": false, "allowed": false, "reason": "none",
                       "identity": null, "version": null}, "request": null}),
                None,
            ),
            ("GET", ["", "v1", "looks"]) => response(200, camera.list(), None),
            ("GET", ["", "v1", "looks", id, "content"]) => {
                let mut r = StubResponse::default();
                r.status = 200;
                r.body = grf(id);
                r
            }
            ("GET", ["", "v1", "looks", id]) => {
                let tag = camera.etag(id);
                response(200, json!({}), Some(tag))
            }
            ("GET", ["", "v1", "looks", _, "requests", rid])
            | ("GET", ["", "v1", "params", "looks", "requests", rid]) => {
                assert_eq!(camera.pending.as_deref(), Some(*rid));
                camera.pending = None;
                response(200, record(rid, "succeeded"), None)
            }
            ("GET", ["", "v1", "params", "looks", "order"]) => {
                let tag = format!("\"order-{}\"", camera.order_revision);
                response(200, json!({}), Some(tag))
            }
            ("PATCH", ["", "v1", "looks", id]) => match admit(&mut camera, &request) {
                Err(refused) => refused,
                Ok(rid) => {
                    assert_eq!(header(&request, "If-Match"), Some(camera.etag(id).as_str()));
                    assert_eq!(header(&request, "Content-Type"), Some("application/json"));
                    let patch: Value = serde_json::from_slice(body(&request)).unwrap();
                    let entry = camera.looks.iter_mut().find(|e| e.id == *id).unwrap();
                    if let Some(enabled) = patch["enabled"].as_bool() {
                        entry.enabled = enabled;
                    }
                    if let Some(name) = patch["name"].as_str() {
                        entry.name = name.into();
                        entry.revision += 1;
                    }
                    finish(&mut camera, rid)
                }
            },
            ("DELETE", ["", "v1", "looks", id]) => match admit(&mut camera, &request) {
                Err(refused) => refused,
                Ok(rid) => {
                    assert_eq!(header(&request, "If-Match"), Some(camera.etag(id).as_str()));
                    let selected = camera.selected == Some(*id);
                    if selected && query != Some("active=off") {
                        response(409, json!({"status": 409, "code": "active_look"}), None)
                    } else {
                        camera.looks.retain(|e| e.id != *id);
                        if selected {
                            camera.selected = None;
                        }
                        finish(&mut camera, rid)
                    }
                }
            },
            ("PUT", ["", "v1", "params", "looks", "order"]) => match admit(&mut camera, &request) {
                Err(refused) => refused,
                Ok(rid) => {
                    let expected = format!("\"order-{}\"", camera.order_revision);
                    assert_eq!(header(&request, "If-Match"), Some(expected.as_str()));
                    let order: Value = serde_json::from_slice(body(&request)).unwrap();
                    let ids: Vec<String> = serde_json::from_value(order["ids"].clone()).unwrap();
                    let mut looks = Vec::new();
                    for id in ids {
                        let at = camera.looks.iter().position(|e| e.id == id).unwrap();
                        looks.push(camera.looks.remove(at));
                    }
                    assert!(camera.looks.is_empty(), "order must list every Look");
                    camera.looks = looks;
                    camera.order_revision += 1;
                    finish(&mut camera, rid)
                }
            },
            _ => panic!(
                "unexpected camera request: {} {}",
                request.method, request.url
            ),
        };
        reply.send(answer);
    }));
    let mut missing = StubResponse::default();
    missing.status = 404;
    let network = Arc::new(StubNetwork::new([
        ("GET http://camera.test/*".into(), handler.clone()),
        ("PUT http://camera.test/*".into(), handler.clone()),
        ("PATCH http://camera.test/*".into(), handler.clone()),
        ("DELETE http://camera.test/*".into(), handler.clone()),
        ("POST http://camera.test/*".into(), handler),
        (
            "GET https://release.test/*".into(),
            StubRoute::Response(missing),
        ),
    ]));
    let mut config = EngineConfig::new(Root::of::<App>());
    config.environment.os = Os::Web;
    config.data_dir = Some(environment.directory.clone());
    config.network = Some(network.clone());
    let app = Headless::start(config).unwrap();
    (app, camera, network)
}

fn open_looks(app: &mut Headless) {
    // The release site answers 404, so the camera card offers Software Update.
    app.wait_for_within("Software Update", WAIT).unwrap();
    app.press("role:button[Looks]").unwrap();
    app.wait_for_within("In Camera Menu Order", WAIT).unwrap();
}

/// Let the app run until `done` holds, at most `WAIT`.
fn until(app: &mut Headless, mut done: impl FnMut() -> bool) {
    for _ in 0..100 {
        if done() {
            return;
        }
        app.wait(Duration::from_millis(100)).unwrap();
    }
    panic!("condition not reached within {WAIT:?}");
}

fn count(network: &StubNetwork, method: &str, suffix: &str) -> usize {
    network
        .requests()
        .iter()
        .filter(|r| r.method == method && r.url.ends_with(suffix))
        .count()
}

#[test]
fn turning_a_look_off_patches_it_with_its_etag() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let (mut app, camera, network) = start_phone(&environment, Camera::new());
    open_looks(&mut app);
    app.press("text:Tri-X 400").unwrap();
    app.wait_for_within("Show in Camera Menu", WAIT).unwrap();
    app.press("role:switch[Show in Camera Menu]").unwrap();
    until(&mut app, || !camera.lock().unwrap().looks[1].enabled);
    app.wait_for_within("Official · B&W · Off", WAIT).unwrap();
    assert_eq!(count(&network, "PATCH", "/v1/looks/trix-400-mono-v1"), 1);
}

#[test]
fn accepted_changes_are_followed_to_their_result() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let mut state = Camera::new();
    state.accept_later = true;
    let (mut app, camera, network) = start_phone(&environment, state);
    open_looks(&mut app);
    app.press("text:Tri-X 400").unwrap();
    app.wait_for_within("Show in Camera Menu", WAIT).unwrap();
    app.press("role:switch[Show in Camera Menu]").unwrap();
    until(&mut app, || {
        camera.lock().unwrap().pending.is_none()
            && network
                .requests()
                .iter()
                .any(|r| r.url.contains("/requests/"))
    });
    app.wait_for_within("Official · B&W · Off", WAIT).unwrap();
    assert!(
        network
            .requests()
            .iter()
            .any(|r| r.method == "GET" && r.url.contains("/v1/looks/trix-400-mono-v1/requests/"))
    );
}

#[test]
fn deleting_the_look_in_use_switches_the_camera_to_off() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let mut state = Camera::new();
    state.looks[0].dependencies.clear();
    let (mut app, camera, network) = start_phone(&environment, state);
    open_looks(&mut app);
    app.press("text:Portra 400").unwrap();
    app.wait_for_within("Delete Look", WAIT).unwrap();
    app.on_dialog(|_| DialogAnswer::Press("Delete".into()));
    app.press("Delete Look").unwrap();
    app.wait_for_within("In Camera Menu Order", WAIT).unwrap();
    app.wait_for_gone("text:Portra 400", WAIT).unwrap();
    assert!(camera.lock().unwrap().selected.is_none());
    assert!(
        network.requests().iter().any(|r| r.method == "DELETE"
            && r.url.ends_with("/v1/looks/portra-400-endura-v1?active=off"))
    );
}

#[test]
fn a_look_that_recipes_use_is_not_deleted() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let (mut app, _camera, network) = start_phone(&environment, Camera::new());
    open_looks(&mut app);
    app.press("text:Portra 400").unwrap();
    app.wait_for_within("Delete Look", WAIT).unwrap();
    app.on_dialog(|dialog| {
        assert_eq!(dialog.title.as_deref(), Some("Can’t Delete Portra 400"));
        DialogAnswer::Press("OK".into())
    });
    app.press("Delete Look").unwrap();
    app.wait(Duration::from_millis(300)).unwrap();
    assert_eq!(count(&network, "DELETE", "portra-400-endura-v1"), 0);
}

#[test]
fn a_refused_change_explains_the_pending_update() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let mut state = Camera::new();
    state.refuse = Some("software_pending");
    let (mut app, camera, _network) = start_phone(&environment, state);
    open_looks(&mut app);
    app.press("text:Tri-X 400").unwrap();
    app.wait_for_within("Show in Camera Menu", WAIT).unwrap();
    let titles = Arc::new(Mutex::new(Vec::new()));
    let seen = titles.clone();
    app.on_dialog(move |dialog| {
        seen.lock()
            .unwrap()
            .push(dialog.title.clone().unwrap_or_default());
        DialogAnswer::Press("OK".into())
    });
    app.press("role:switch[Show in Camera Menu]").unwrap();
    until(&mut app, || !titles.lock().unwrap().is_empty());
    assert_eq!(titles.lock().unwrap().as_slice(), ["An Update Is Waiting"]);
    assert!(camera.lock().unwrap().looks[1].enabled);
}

#[test]
fn dragging_a_look_saves_the_complete_order() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let (mut app, camera, _network) = start_phone(&environment, Camera::new());
    open_looks(&mut app);
    app.press("Edit Order").unwrap();
    app.wait_for_within("label:Move Night", WAIT).unwrap();
    app.drag(
        "label:Move Night",
        Point {
            x: Points(0.0),
            y: Points(-116.0),
        },
    )
    .unwrap();
    app.press("role:button[Done]").unwrap();
    app.wait_for_within("In Camera Menu Order", WAIT).unwrap();
    until(&mut app, || camera.lock().unwrap().order_revision > 1);
    let ids: Vec<&str> = camera.lock().unwrap().looks.iter().map(|e| e.id).collect();
    assert_eq!(ids, [CUSTOM, PORTRA, TRIX]);
}
