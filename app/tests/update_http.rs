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
use nsh_core::platform::Os;
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
}

fn json_response(body: serde_json::Value) -> StubResponse {
    let mut response = StubResponse::default();
    response.status = 200;
    response.body = serde_json::to_vec(&body).unwrap();
    response
}

fn start_phone(environment: &TestEnvironment) -> (Headless, Arc<Mutex<Camera>>, Arc<StubNetwork>) {
    let camera = Arc::new(Mutex::new(Camera {
        version: "1.7.0",
        journal: "none",
        rollback_available: true,
    }));
    let handler_camera = camera.clone();
    let handler = StubRoute::Handler(Arc::new(move |request, reply| {
        let mut camera = handler_camera.lock().unwrap();
        let body = match (request.method.as_str(), request.url.as_str()) {
            ("GET", "http://camera.test/v1/status") => json!({
                "protocol": 1, "version": camera.version, "uptime": 60,
                "boot": [], "update": {"state": camera.journal, "rollback": null},
            }),
            ("GET", "http://camera.test/v1/package") => json!({
                "version": camera.version, "modules": [], "effects": [], "left_out": [],
                "camera": {"model": "RICOH GR III", "firmware": "2.21"},
            }),
            ("GET", "http://camera.test/v1/update") => json!({
                "state": camera.journal,
                "rollback": if camera.rollback_available && camera.journal == "none" {
                    json!({"version": "1.6.1"})
                } else { json!(null) },
            }),
            ("PUT", "http://camera.test/v1/rollback") => {
                assert!(
                    request.body.is_empty(),
                    "rollback must not upload an archive"
                );
                assert_eq!(camera.journal, "none");
                camera.journal = "waiting";
                json!({"state": "waiting", "version": "1.6.1", "install": 3, "remove": 0})
            }
            ("DELETE", "http://camera.test/v1/update") => {
                camera.journal = "none";
                json!({"state": "none", "cancelled": true})
            }
            _ => panic!(
                "unexpected camera request: {} {}",
                request.method, request.url
            ),
        };
        reply.send(json_response(body));
    }));
    let mut release_missing = StubResponse::default();
    release_missing.status = 404;
    release_missing.body = b"not published".to_vec();
    let network = Arc::new(StubNetwork::new([
        ("GET http://camera.test/*".into(), handler.clone()),
        ("PUT http://camera.test/*".into(), handler.clone()),
        ("DELETE http://camera.test/*".into(), handler),
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
            .filter(|r| r.method == "PUT")
            .count(),
        1
    );
    assert!(!network.requests().iter().any(|r| r.method == "DELETE"));
}

#[test]
fn cancelling_rollback_deletes_and_verifies_the_camera_journal() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let environment = TestEnvironment::new();
    let (mut app, camera, network) = start_phone(&environment);
    begin_rollback(&mut app);
    app.press("Cancel").unwrap();
    app.wait_for_gone("Confirm Roll Back on the Camera", WAIT)
        .unwrap();
    let requests = network.requests();
    let deletion = requests
        .iter()
        .position(|r| r.method == "DELETE")
        .expect("cancel must reach the camera");
    assert!(
        requests[deletion + 1..]
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
    camera.lock().unwrap().journal = "none";
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
    assert!(!network.requests().iter().any(|r| r.method == "PUT"));
}
