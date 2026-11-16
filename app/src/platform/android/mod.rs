//! App-owned JNI bridge. Java owns system callbacks; Rust owns connection/update state.
use crate::domain::*;
use async_channel::Sender;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_channel::oneshot;
use jni::{
    JNIEnv, JavaVM,
    objects::{GlobalRef, JClass, JString, JValue},
    sys::jlong,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{
        Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::Duration,
};

struct Bridge {
    vm: JavaVM,
    class: GlobalRef,
}
static BRIDGE: OnceLock<Bridge> = OnceLock::new();
static CONFIG: OnceLock<[String; 4]> = OnceLock::new();
static NEXT: AtomicI64 = AtomicI64::new(1);
type Reply = oneshot::Sender<Result<Value, String>>;
static PENDING: Mutex<BTreeMap<i64, Reply>> = Mutex::new(BTreeMap::new());
static SCANS: Mutex<Option<Sender<Vec<NearbyCamera>>>> = Mutex::new(None);
static CAMERAS: Mutex<BTreeMap<String, NearbyCamera>> = Mutex::new(BTreeMap::new());
static DISCONNECTS: Mutex<Option<Sender<String>>> = Mutex::new(None);
static UNAVAILABLE: AtomicBool = AtomicBool::new(false);

pub fn config(name: &str) -> Option<String> {
    let index = match name {
        "GRKIT_CAMERA_URL" => 0,
        "GRKIT_RELEASE_BASE" => 1,
        "GRKIT_DEMO_SCREEN" => 2,
        "GRKIT_RELEASE_PUBLIC_KEY" => 3,
        _ => return None,
    };
    CONFIG
        .get()
        .map(|c| c[index].clone())
        .filter(|v| !v.is_empty())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_studio_techflow_grkit_MainActivity_nativeInitialize(
    mut env: JNIEnv,
    _: JClass,
    camera: JString,
    release: JString,
    demo: JString,
    public_key: JString,
) {
    let result = (|| -> jni::errors::Result<()> {
        let config = [
            env.get_string(&camera)?.into(),
            env.get_string(&release)?.into(),
            env.get_string(&demo)?.into(),
            env.get_string(&public_key)?.into(),
        ];
        let class = env.find_class("studio/techflow/grkit/CameraBridge")?;
        let bridge = Bridge {
            vm: env.get_java_vm()?,
            class: env.new_global_ref(class)?,
        };
        let _ = BRIDGE.set(bridge);
        let _ = CONFIG.set(config);
        Ok(())
    })();
    if let Err(error) = result {
        let _ = env.exception_clear();
        UNAVAILABLE.store(true, Ordering::Relaxed);
        log::error!("Android bridge initialization failed: {error}");
    }
}

// Engine threads can stay attached to the VM for their entire lifetime. Every
// outbound call gets a frame so repeated HTTP polls cannot accumulate local refs.
fn with_env<T>(
    f: impl FnOnce(&mut JNIEnv<'_>, &JClass<'_>) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let bridge = BRIDGE.get().ok_or("Android host is unavailable")?;
    let mut env = bridge
        .vm
        .attach_current_thread()
        .map_err(|e| e.to_string())?;
    let class: &JClass = bridge.class.as_obj().into();
    let result = env.with_local_frame(16, |env| f(env, class));
    if result.is_err() {
        // Never leave a Java exception pending on a reused engine thread.
        let _ = env.exception_clear();
    }
    result.map_err(|e| e.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn dispatch(id: i64, operation: &str, data: Value) -> Result<(), String> {
    with_env(|env, class| {
        let operation = env.new_string(operation)?;
        let data = env.new_string(data.to_string())?;
        env.call_static_method(
            class,
            "execute",
            "(JLjava/lang/String;Ljava/lang/String;)V",
            &[
                JValue::Long(id),
                JValue::Object(&operation),
                JValue::Object(&data),
            ],
        )
        .map(|_| ())
    })
}
struct RequestGuard(i64);
impl Drop for RequestGuard {
    fn drop(&mut self) {
        let pending = lock(&PENDING).remove(&self.0).is_some();
        if pending {
            let _ = dispatch(0, "cancelRequest", json!({"id": self.0}));
        }
    }
}
async fn call(operation: &str, data: Value, budget: Duration) -> Result<Value, String> {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    lock(&PENDING).insert(id, tx);
    let _guard = RequestGuard(id);
    dispatch(id, operation, data)?;
    super::with_timeout(rx, budget)
        .await
        .ok_or("ConnectionLost")?
        .map_err(|_| "ConnectionLost".to_owned())?
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_studio_techflow_grkit_CameraBridge_complete(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
    value: JString,
    error: JString,
) {
    let result = if !error.is_null() {
        Err(env
            .get_string(&error)
            .map(String::from)
            .unwrap_or_else(|_| "ConnectionLost".into()))
    } else {
        env.get_string(&value)
            .map(String::from)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    };
    if result.is_err() {
        let _ = env.exception_clear();
    }
    let pending = lock(&PENDING).remove(&id);
    if let Some(tx) = pending {
        let _ = tx.send(result);
    }
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_studio_techflow_grkit_CameraBridge_event(
    mut env: JNIEnv,
    _: JClass,
    kind: JString,
    value: JString,
) {
    let Ok(kind) = env.get_string(&kind).map(String::from) else {
        let _ = env.exception_clear();
        return;
    };
    let Ok(value) = env.get_string(&value).map(String::from) else {
        let _ = env.exception_clear();
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(&value) else {
        return;
    };
    match kind.as_str() {
        "unavailable" => UNAVAILABLE.store(value.as_bool().unwrap_or(true), Ordering::Relaxed),
        "disconnected" => {
            if let (Some(tx), Some(id)) = (lock(&DISCONNECTS).as_ref(), value.as_str()) {
                let _ = tx.try_send(id.into());
            }
        }
        "scan" => {
            let Some(id) = value["id"].as_str() else {
                return;
            };
            let bytes: Vec<u8> = value["manufacturer"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
                        .collect()
                })
                .unwrap_or_default();
            let services: Vec<String> = value["services"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let adv = parse_advertisement(Some(&bytes), &services);
            if !is_supported_camera(&adv) {
                return;
            }
            let Some(model) = adv.model else {
                return;
            };
            let camera = NearbyCamera {
                peripheral_id: id.into(),
                model,
                powered_on: adv.shows_powered_on(),
                rssi: value["rssi"].as_i64().and_then(|v| i32::try_from(v).ok()),
            };
            let cameras = {
                let mut all = lock(&CAMERAS);
                all.insert(id.into(), camera);
                all.values().cloned().collect()
            };
            if let Some(tx) = lock(&SCANS).as_ref() {
                let _ = tx.try_send(cameras);
            }
        }
        _ => {}
    }
}

pub fn bluetooth_unavailable() -> bool {
    UNAVAILABLE.load(Ordering::Relaxed)
}
pub fn start_scanning(tx: Sender<Vec<NearbyCamera>>, remembered: Option<&str>) {
    let known =
        remembered.and_then(|id| load_remembered().filter(|camera| camera.peripheral_id == id));
    let cameras = {
        let mut cameras = lock(&CAMERAS);
        cameras.clear();
        // Android can reconnect by a saved address without a fresh advertisement,
        // including a sleeping camera. Do not infer power or signal strength.
        if let Some(known) = known {
            cameras.insert(
                known.peripheral_id.clone(),
                NearbyCamera {
                    peripheral_id: known.peripheral_id,
                    model: known.model,
                    powered_on: false,
                    rssi: None,
                },
            );
        }
        cameras.values().cloned().collect()
    };
    let _ = tx.try_send(cameras);
    *lock(&SCANS) = Some(tx);
    if dispatch(0, "scan", json!({"remembered_id": remembered})).is_err() {
        UNAVAILABLE.store(true, Ordering::Relaxed);
    }
}
pub fn stop_scanning() {
    *lock(&SCANS) = None;
    let _ = dispatch(0, "stopScan", json!({}));
}
pub fn watch_disconnects(tx: Sender<String>) {
    *lock(&DISCONNECTS) = Some(tx);
}
pub fn load_remembered() -> Option<RememberedCamera> {
    let text = with_env(|env, class| {
        let value = env
            .call_static_method(class, "remembered", "()Ljava/lang/String;", &[])?
            .l()?;
        if value.is_null() {
            return Ok(None);
        }
        let value = JString::from(value);
        env.get_string(&value).map(|s| Some(String::from(s)))
    })
    .ok()??;
    decode_remembered(&text).ok()
}
pub fn save_remembered(camera: &RememberedCamera) -> Result<(), String> {
    let encoded = encode_remembered(camera)?;
    with_env(|env, class| {
        let value = env.new_string(encoded)?;
        env.call_static_method(
            class,
            "saveRemembered",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&value)],
        )
        .map(|_| ())
    })
}
fn link_error(error: String) -> LinkError {
    match error.as_str() {
        "NotNearby" => LinkError::NotNearby,
        "Busy" => LinkError::Busy,
        "Cancelled" => LinkError::Cancelled,
        "BluetoothUnavailable" => LinkError::BluetoothUnavailable,
        "PairingFailed" => LinkError::PairingFailed,
        "NotPaired" => LinkError::NotPaired,
        "WifiJoinFailed" => LinkError::WifiJoinFailed,
        "WifiJoinRejected" => LinkError::WifiJoinRejected,
        _ => LinkError::ConnectionLost,
    }
}
struct Attempt(bool);
impl Drop for Attempt {
    fn drop(&mut self) {
        if !self.0 {
            let _ = dispatch(0, "cancel", json!({}));
        }
    }
}
type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

struct AndroidTransport {
    model_hint: CameraModel,
    package: Option<(CameraStatus, PackageInfo)>,
}

impl ConnectTransport for AndroidTransport {
    fn ble_setup(&mut self, peripheral_id: &str) -> BoxFut<'_, Result<BleSetup, LinkError>> {
        let id = peripheral_id.to_owned();
        let model = self.model_hint;
        Box::pin(async move {
            let setup = call(
                "setup",
                json!({"id":id,"model":model}),
                // Includes connection retries and the system pairing prompt.
                Duration::from_secs(180),
            )
            .await
            .map_err(link_error)?;
            let credentials = &setup["credentials"];
            let ssid = credentials["ssid"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or(LinkError::NotPaired)?;
            let passphrase = credentials["passphrase"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or(LinkError::NotPaired)?;
            Ok(BleSetup {
                peripheral_id: id,
                model,
                powered_on_already: setup["powered_on_already"].as_bool().unwrap_or(false),
                credentials: WifiCredentials {
                    ssid: ssid.to_owned(),
                    passphrase: passphrase.to_owned(),
                },
            })
        })
    }

    fn join_wifi(&mut self, credentials: &WifiCredentials) -> BoxFut<'_, Result<(), LinkError>> {
        let data = json!({"ssid":credentials.ssid,"passphrase":credentials.passphrase});
        Box::pin(async move {
            // Permission and network selection each have their own native deadline.
            call("join", data, Duration::from_secs(130))
                .await
                .map(|_| ())
                .map_err(link_error)
        })
    }

    fn wait_status_ready(&mut self) -> BoxFut<'_, Result<(), LinkError>> {
        Box::pin(super::http::wait_until_ready())
    }

    fn read_version(&mut self, model: CameraModel) -> BoxFut<'_, Result<(), LinkError>> {
        Box::pin(async move {
            self.package = Some(super::http::read_package(model).await?);
            Ok(())
        })
    }
}

pub async fn connect_camera(
    id: &str,
    model: CameraModel,
    on_step: &mut dyn FnMut(ConnectStep),
) -> Result<(CameraStatus, PackageInfo, RememberedCamera), LinkError> {
    let mut attempt = Attempt(false);
    let mut transport = AndroidTransport {
        model_hint: model,
        package: None,
    };
    let setup = run_connect(&mut transport, id, on_step).await?;
    let (status, package) = transport.package.take().ok_or(LinkError::ConnectionLost)?;
    let remembered = RememberedCamera::new(
        setup.peripheral_id,
        status.camera.model,
        setup.credentials.ssid,
    );
    if let Err(error) = save_remembered(&remembered) {
        log::warn!("Could not remember camera: {error}");
    }
    attempt.0 = true;
    Ok((status, package, remembered))
}

pub async fn camera_request(
    method: &str,
    url: String,
    body: Vec<u8>,
    budget: Duration,
) -> Result<nsh_core::element::HttpResponse, String> {
    let value = call("http", json!({"method":method,"url":url,"body":STANDARD.encode(body),"timeout_ms":budget.as_millis(),"headers":{"Cache-Control":"no-cache, no-store","Content-Type":"application/zip"}}), budget).await?;
    let status = value["status"]
        .as_u64()
        .and_then(|v| u16::try_from(v).ok())
        .ok_or("Invalid HTTP status")?;
    let headers = value["headers"]
        .as_object()
        .map(|h| {
            h.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let body = STANDARD
        .decode(value["body"].as_str().ok_or("Invalid HTTP body")?)
        .map_err(|e| e.to_string())?;
    Ok(nsh_core::element::HttpResponse {
        head: nsh_core::platform::HttpHead {
            status,
            status_text: String::new(),
            url,
            redirected: false,
            headers,
        },
        body,
    })
}
