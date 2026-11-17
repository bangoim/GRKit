//! Real store and HTTP transport with a camera journal controlled by the test.
use std::{
    env,
    ffi::OsString,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use grkit_app::App;
use nsh_core::platform::{HttpBody, HttpRequest, Os};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless, StubNetwork, StubResponse, StubRoute};
use serde_json::json;

const WAIT: Duration = Duration::from_secs(10);
static ENV_LOCK: Mutex<()> = Mutex::new(());

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
            env::temp_dir().join(format!("grkit-update-test-{}-{unique}", std::process::id()));
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

struct Camera {
    version: &'static str,
    journal: &'static str,
    rollback_available: bool,
    rollback_allowed: bool,
    request_id: Option<String>,
}

fn json_response(body: serde_json::Value) -> StubResponse {
    let mut response = StubResponse::default();
    response.status = 200;
    response.body = serde_json::to_vec(&body).unwrap();
    response
}

fn with_context(mut response: StubResponse) -> StubResponse {
    response.headers = vec![
        ("ETag".into(), "\"sha256:test\"".into()),
        ("X-GRKit-Boot-Id".into(), "boot-test".into()),
        ("X-GRKit-Card-Id".into(), "card-test".into()),
    ];
    response
}

fn require_context(request: &HttpRequest) {
    assert!(
        request
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("X-GRKit-Request-Id")),
        "mutation requires X-GRKit-Request-Id"
    );
    assert!(
        request
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("X-GRKit-Boot-Id") && v == "boot-test"),
        "mutation requires matching boot id"
    );
    assert!(
        request
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("X-GRKit-Card-Id") && v == "card-test"),
        "mutation requires matching card id"
    );
}

fn request_body(request: &HttpRequest) -> &[u8] {
    match &request.body {
        HttpBody::Bytes(bytes) => bytes,
        HttpBody::Empty => &[],
        HttpBody::Stream => panic!("unexpected streamed body"),
    }
}

fn start_phone(environment: &TestEnvironment) -> (Headless, Arc<Mutex<Camera>>, Arc<StubNetwork>) {
    start_phone_with_delay(environment, Duration::ZERO)
}

fn start_phone_with_delay(
    environment: &TestEnvironment,
    delay: Duration,
) -> (Headless, Arc<Mutex<Camera>>, Arc<StubNetwork>) {
    let camera = Arc::new(Mutex::new(Camera {
        version: "1.7.0",
        journal: "none",
        rollback_available: true,
        rollback_allowed: true,
        request_id: None,
    }));
    let handler_camera = camera.clone();
    let handler = StubRoute::Handler(Arc::new(move |request, reply| {
        let mut camera = handler_camera.lock().unwrap();
        let body = match (request.method.as_str(), request.url.as_str()) {
            ("GET", "http://camera.test/v1/props") => json!({
                "bootId": "boot-test",
                "cardId": "card-test",
                "camera": {"model": "RICOH GR III", "firmwareVersion": "2.21"},
                "grkit": {"version": camera.version},
                "storage": {"state": "ready", "writable": true, "freeBytes": 1_000_000_000u64},
                "runtime": {"state": "ready", "busyReason": null},
                "writeAccess": {"allowed": true, "reason": null},
                "capabilities": {
                    "looks": {"read": true, "write": true, "hotReload": false},
                    "recipes": {"read": false, "write": false},
                    "moduleSettings": {"read": false, "write": false}
                },
                "limits": {},
                "activeRequests": []
            }),
            ("GET", "http://camera.test/v1/update") => json!({
                "inventory": {
                    "package": {
                        "version": camera.version,
                        "modules": [],
                        "effects": [],
                        "left_out": []
                    },
                    "contentHashes": {},
                    "state": camera.journal,
                    "backup": if camera.rollback_available && camera.journal == "none" {
                        json!({"version": "1.6.1"})
                    } else {
                        json!(null)
                    }
                },
                "request": camera.request_id.as_ref().map(|id| json!({
                    "requestId": id,
                    "domain": "rollback",
                    "status": "running",
                    "phase": if camera.journal == "confirmed" { "awaitingRestart" } else { "awaitingConfirmation" }
                })),
                "configurationRevision": null
            }),
            ("GET", "http://camera.test/v1/rollback") => json!({
                "backup": {
                    "available": camera.rollback_available,
                    "allowed": camera.rollback_allowed,
                    "reason": if camera.rollback_allowed { "ok" } else { "data_format_incompatible" },
                    "identity": if camera.rollback_available {
                        json!({"digest": "rollback-digest", "contentHashes": {}})
                    } else {
                        json!(null)
                    },
                    "version": if camera.rollback_available { json!("1.6.1") } else { json!(null) },
                    "trustedFormatSupport": [1]
                },
                "request": camera.request_id.as_ref().map(|id| json!({
                    "requestId": id,
                    "domain": "rollback",
                    "status": "running",
                    "phase": if camera.journal == "confirmed" { "awaitingRestart" } else { "awaitingConfirmation" }
                })),
                "configurationRevision": null
            }),
            ("POST", "http://camera.test/v1/rollback") => {
                require_context(&request);
                assert!(
                    request
                        .headers
                        .iter()
                        .any(|(k, _)| k.eq_ignore_ascii_case("If-Match")),
                    "rollback requires If-Match"
                );
                let body: serde_json::Value = serde_json::from_slice(request_body(&request)).unwrap();
                assert_eq!(body["targetIdentity"], "rollback-digest");
                assert_eq!(camera.journal, "none");
                let id = request
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("X-GRKit-Request-Id"))
                    .map(|(_, v)| v.clone())
                    .expect("request id");
                camera.journal = "waiting";
                camera.request_id = Some(id.clone());
                json!({
                    "requestId": id,
                    "domain": "rollback",
                    "status": "running",
                    "phase": "awaitingConfirmation",
                    "cancellable": true,
                    "requiredAction": "confirmOnCamera",
                    "result": null,
                    "error": null
                })
            }
            ("POST", "http://camera.test/v1/rollback/cancel")
            | ("POST", "http://camera.test/v1/update/cancel") => {
                require_context(&request);
                let body: serde_json::Value = serde_json::from_slice(request_body(&request)).unwrap();
                assert_eq!(
                    body["targetRequestId"].as_str(),
                    camera.request_id.as_deref()
                );
                camera.journal = "none";
                camera.request_id = None;
                json!({
                    "requestId": body["targetRequestId"],
                    "domain": "rollback",
                    "status": "succeeded",
                    "phase": "completed",
                    "targetRequest": {"status": "cancelled"}
                })
            }
            _ => panic!(
                "unexpected camera request: {} {}",
                request.method, request.url
            ),
        };
        let mut response = with_context(json_response(body));
        response.delay = delay;
        reply.send(response);
    }));
    let mut release_missing = StubResponse::default();
    release_missing.status = 404;
    release_missing.body = b"not published".to_vec();
    let network = Arc::new(StubNetwork::new([
        ("GET http://camera.test/*".into(), handler.clone()),
        ("PUT http://camera.test/*".into(), handler.clone()),
        ("POST http://camera.test/*".into(), handler),
        (
            "GET https://release.test/*".into(),
            StubRoute::Response(release_missing),
        ),
    ]));
    let mut config = EngineConfig::new(Root::of::<App>());
    config.environment.os = Os::Web;
    config.data_dir = Some(environment.directory.clone());
    config.network = Some(network.clone());
    let app = Headless::start(config).unwrap();
    (app, camera, network)
}

fn begin_rollback(app: &mut Headless) {
    app.wait_for_within("Software Update", WAIT).unwrap();
    app.press("Software Update").unwrap();
    app.wait_for_within("Roll Back to 1.6.1", WAIT).unwrap();
    app.on_dialog(|_| DialogAnswer::Press("Roll Back".into()));
    app.press("Roll Back to 1.6.1").unwrap();
    app.wait_for_within("Confirm Roll Back on the Camera", WAIT)
        .unwrap();
}

#[test]
fn rollback_verifies_the_version_after_camera_confirmation_and_restart() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let (mut app, camera, network) = start_phone(&environment);
    begin_rollback(&mut app);
    camera.lock().unwrap().journal = "confirmed";
    app.wait_for_within("Restart the Camera", WAIT).unwrap();
    {
        let mut camera = camera.lock().unwrap();
        camera.journal = "none";
        camera.version = "1.6.1";
        camera.rollback_available = false;
        camera.request_id = None;
    }
    app.press("Check Installed Version").unwrap();
    app.wait_for_within("GRKit 1.6.1 Is Restored", WAIT)
        .unwrap();
    app.press("Done").unwrap();
    app.wait_for_gone("GRKit 1.6.1 Is Restored", WAIT).unwrap();
    assert_eq!(
        network
            .requests()
            .iter()
            .filter(|r| r.method == "POST" && r.url.ends_with("/v1/rollback"))
            .count(),
        1
    );
    assert!(!network.requests().iter().any(|r| r.method == "DELETE"));
}

#[test]
fn cancelling_rollback_posts_cancel_and_verifies_the_camera_journal() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let (mut app, camera, network) = start_phone(&environment);
    begin_rollback(&mut app);
    app.press("Cancel").unwrap();
    app.wait_for_gone("Confirm Roll Back on the Camera", WAIT)
        .unwrap();
    let requests = network.requests();
    let cancellation = requests
        .iter()
        .position(|r| r.method == "POST" && r.url.ends_with("/v1/rollback/cancel"))
        .expect("cancel must reach the camera");
    assert!(
        requests[cancellation + 1..]
            .iter()
            .any(|r| r.method == "GET" && r.url.ends_with("/v1/update")),
        "cancel must verify the resulting journal"
    );
    assert_eq!(camera.lock().unwrap().journal, "none");
    assert_eq!(camera.lock().unwrap().version, "1.7.0");
}

#[test]
fn vanished_journal_with_the_old_version_is_not_reported_as_success() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let (mut app, camera, network) = start_phone(&environment);
    begin_rollback(&mut app);
    {
        let mut camera = camera.lock().unwrap();
        camera.journal = "none";
        camera.request_id = None;
    }
    app.wait_for_within("Not Confirmed", WAIT).unwrap();
    app.wait_for_gone("GRKit 1.6.1 Is Restored", WAIT).unwrap();
    assert_eq!(camera.lock().unwrap().version, "1.7.0");
    assert!(!network.requests().iter().any(|r| r.method == "DELETE"));
}

#[test]
fn stale_rollback_selection_does_not_mutate_the_camera() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let (mut app, camera, network) = start_phone(&environment);
    app.wait_for_within("Software Update", WAIT).unwrap();
    app.press("Software Update").unwrap();
    app.wait_for_within("Roll Back to 1.6.1", WAIT).unwrap();
    camera.lock().unwrap().rollback_available = false;
    app.on_dialog(|_| DialogAnswer::Press("Roll Back".into()));
    app.press("Roll Back to 1.6.1").unwrap();
    app.wait_for_within(
        "The rollback version changed. Refresh before trying again.",
        WAIT,
    )
    .unwrap();
    assert!(!network
        .requests()
        .iter()
        .any(|r| r.method == "POST" && r.url.ends_with("/v1/rollback")));
}

#[test]
fn automatic_connection_starts_with_a_local_placeholder() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let (mut app, _, _) = start_phone_with_delay(&environment, Duration::from_millis(300));
    app.wait_for("Connecting to your camera…").unwrap();
    app.wait_for_gone("Add Your Camera", WAIT).unwrap();
    app.wait_for_gone("Connecting to RICOH GR III", WAIT)
        .unwrap();
    app.wait_for_within("Software Update", WAIT).unwrap();
    app.wait_for_gone("Connecting to your camera…", WAIT)
        .unwrap();
}
