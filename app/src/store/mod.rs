//! App state: tabs, camera session, update sheets, and demo mode.
//!
//! Reactive fields use NSH `Signal`s so the UI updates when they change.

mod demo_screen;
mod liveness;
mod looks;
mod update;
mod warmup;

pub use looks::{CatalogItem, ImportDraft, LookPicture, LooksStore, LooksView};

pub use demo_screen::{
    DEMO_SCREEN_IDS, DemoScreen, demo_screen_from_env, key_text, parse_demo_screen, shows_alert,
};

use std::{
    cell::RefCell,
    env,
    time::{Duration, Instant},
};

use nsh_core::prelude::*;

use crate::domain::nearby_watch::{NearbyWatch, PRUNE_INTERVAL_MS};
use crate::domain::{
    CameraLink, CameraModel, CameraStatus, ConnectStep, ConnectionState, DemoFail, FakeCamera,
    LinkError, NearbyCamera, PackageInfo, RememberedCamera, UpdateError, UpdateKind, UpdatePhase,
    UpdatePoll, block_on_ready,
};
use crate::platform;

/// Placeholder App Store id until the listing exists.
pub const APP_STORE_ID: &str = "0000000000";
pub const PRIVACY_URL: &str = "https://grkit.app/privacy";
pub const GR_SNAP_URL: &str = "https://apps.apple.com/app/gr-snap-ricoh-gr-wireless/id6768040383";
pub const APP_VERSION: &str = "1.0.0";
pub const APP_BUILD: &str = "1";

const MAX_DEPTH: usize = 4;

/// Which tab is selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Camera,
    Looks,
    Settings,
}

/// Pages pushed on a tab's navigation stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    SoftwareUpdate,
    /// One Look on the camera, by ID.
    LookDetail(String),
    /// Rename an imported Look.
    LookRename(String),
    /// Camera menu order.
    LookOrder,
    /// A Look made from a file on the phone.
    LookImport,
}

/// Modal sheets over the camera tab.
#[derive(Clone, Debug, PartialEq)]
pub enum Sheet {
    Nearby,
    Scan,
    Connecting {
        step: u8,
    },
    ConnectError(LinkError),
    Sending,
    Confirm,
    Restart,
    Installed {
        version: String,
        previous: String,
    },
    Error(UpdateError),
    /// Choose what kind of file becomes a new Look.
    ImportLook,
}

/// Shared app state provided at the root (`Copy` handle on signals).
#[derive(Clone, Copy)]
pub struct AppState {
    demo: bool,
    tab: Signal<Tab>,
    stacks: Signal<[Vec<Route>; 3]>,
    pub(crate) looks: LooksStore,
    sheet: Signal<Option<Sheet>>,
    pub(crate) status: Signal<Option<CameraStatus>>,
    pub(crate) package: Signal<Option<PackageInfo>>,
    pub(crate) phase: Signal<UpdatePhase>,
    pub(crate) confirm_deadline: Signal<Option<Instant>>,
    confirm_tick: Signal<u64>,
    connect_started: Signal<Option<Instant>>,
    initial_connect: Signal<bool>,
    /// When true, the ticker does not advance connecting / update sheets.
    frozen: Signal<bool>,
    /// Fake camera for demo mode; mutated from tick handlers.
    pub(crate) camera: Stored<RefCell<Option<FakeCamera>>>,
    pub(crate) baseline: Signal<Option<String>>,
    nearby: Signal<Vec<NearbyCamera>>,
    remembered: Signal<Option<RememberedCamera>>,
    selected_id: Signal<Option<String>>,
    connect_generation: Signal<u64>,
    connect_abort: Stored<RefCell<Option<futures_util::future::AbortHandle>>>,
    /// Suppresses the automatic nearby sheet until the camera powers off.
    nearby_watch: Stored<RefCell<NearbyWatch>>,
    /// The app root's owner. Tasks that must outlive the sheet or page that
    /// starts them (a connect, an update) run under it: a task made with
    /// `spawn` ends with the owner it was made under, and a sheet's owner is
    /// disposed as soon as the sheet changes.
    root: Owner,
    update_busy: Signal<bool>,
    update_cancel: Signal<bool>,
    update_reconnect: Signal<bool>,
    update_kind: Signal<UpdateKind>,
    release_busy: Signal<bool>,
    release_error: Signal<Option<String>>,
    release: Stored<RefCell<Option<platform::release::Release>>>,
}

impl AppState {
    pub fn new() -> Self {
        let demo = demo_requested();
        let fail = demo_fail_from_env();
        let screen = if demo { demo_screen_from_env() } else { None };
        let camera = if demo {
            let base = FakeCamera::with_fail(fail).config().clone();
            Some(FakeCamera::new(crate::domain::FakeConfig {
                send_ticks: 4,
                confirm_ticks: 3,
                fail,
                ..base
            }))
        } else {
            None
        };
        let remembered = if demo {
            None
        } else {
            platform::load_remembered()
        };
        let app = Self {
            demo,
            update_busy: signal(false),
            update_cancel: signal(false),
            update_reconnect: signal(false),
            update_kind: signal(UpdateKind::Install),
            release_busy: signal(false),
            release_error: signal(None),
            release: stored(RefCell::new(None)),
            tab: signal(Tab::Camera),
            stacks: signal([Vec::new(), Vec::new(), Vec::new()]),
            looks: LooksStore::new(demo),
            sheet: signal(None),
            status: signal(None),
            package: signal(None),
            phase: signal(UpdatePhase::Idle),
            confirm_deadline: signal(None),
            confirm_tick: signal(0),
            connect_started: signal(None),
            initial_connect: signal(!demo && platform::has_dev_camera_url()),
            frozen: signal(false),
            camera: stored(RefCell::new(camera)),
            baseline: signal(None),
            nearby: signal(Vec::new()),
            remembered: signal(remembered),
            selected_id: signal(None),
            connect_generation: signal(0),
            connect_abort: stored(RefCell::new(None)),
            root: Owner::current(),
            nearby_watch: stored(RefCell::new(NearbyWatch::new())),
        };
        // Development: open a tab at launch, for simulator runs without taps.
        match platform::config_value("GRKIT_START_TAB").as_deref() {
            Some("looks") => app.tab.set(Tab::Looks),
            Some("looks-all") => {
                app.tab.set(Tab::Looks);
                app.set_looks_view(LooksView::All);
            }
            _ => {}
        }
        if let Some(screen) = screen {
            demo_screen::apply(app, screen);
        } else if !demo && !platform::has_dev_camera_url() {
            app.watch_link();
            app.start_ble_scan();
        } else if !demo {
            app_info!(
                "GRKIT_CAMERA_URL set; skipping BLE scan ({})",
                platform::camera_http_base()
            );
            // The development URL connects at launch, so a simulator run
            // needs no tap to reach the HTTP part.
            spawn(async move {
                next_frame().await;
                app.open_scan();
            });
        }
        app
    }

    pub fn initial_connect(self) -> bool {
        self.initial_connect.get()
    }

    pub fn demo(self) -> bool {
        self.demo
    }

    pub fn set_frozen(self, frozen: bool) {
        self.frozen.set(frozen);
    }

    pub fn frozen(self) -> bool {
        self.frozen.get()
    }

    pub fn tab(self) -> Tab {
        self.tab.get()
    }

    pub fn set_tab(self, tab: Tab) {
        self.tab.set(tab);
        if tab == Tab::Looks {
            self.open_looks();
        }
    }

    pub fn route_at(self, tab: Tab, depth: usize) -> Option<Route> {
        self.stacks
            .get()
            .get(tab_index(tab))
            .and_then(|stack| stack.get(depth).cloned())
    }

    pub fn push(self, route: Route) {
        let tab = self.tab();
        self.stacks.update(|stacks| {
            let stack = &mut stacks[tab_index(tab)];
            if stack.len() < MAX_DEPTH {
                stack.push(route);
            }
        });
    }

    pub fn pop_to(self, tab: Tab, depth: usize) {
        self.stacks.update(|stacks| {
            stacks[tab_index(tab)].truncate(depth);
        });
    }

    pub fn sheet(self) -> Option<Sheet> {
        self.sheet.get()
    }

    pub fn set_sheet(self, sheet: Option<Sheet>) {
        self.sheet.set(sheet);
    }

    pub fn status(self) -> Option<CameraStatus> {
        self.status.get()
    }

    pub fn package(self) -> Option<PackageInfo> {
        self.package.get()
    }

    pub fn flow_phase(self) -> UpdatePhase {
        self.phase.get()
    }

    pub fn nearby_cameras(self) -> Vec<NearbyCamera> {
        if self.demo() {
            return self
                .nearby_model()
                .map(|model| {
                    vec![NearbyCamera {
                        peripheral_id: "demo".into(),
                        model,
                        powered_on: true,
                        rssi: Some(-45),
                    }]
                })
                .unwrap_or_default();
        }
        self.nearby.get()
    }

    pub fn remembered(self) -> Option<RememberedCamera> {
        self.remembered.get()
    }

    pub fn model_name(self) -> String {
        self.status
            .get()
            .map(|s| s.camera.model.display_name().to_owned())
            .or_else(|| self.selected_model().map(|m| m.display_name().to_owned()))
            .or_else(|| {
                self.camera.with(|c| {
                    c.borrow()
                        .as_ref()
                        .map(|cam| cam.config().model.display_name().to_owned())
                })
            })
            .unwrap_or_else(|| CameraModel::Gr3.display_name().to_owned())
    }

    pub fn selected_model(self) -> Option<CameraModel> {
        if let Some(id) = self.selected_id.get() {
            if let Some(cam) = self
                .nearby_cameras()
                .into_iter()
                .find(|c| c.peripheral_id == id)
            {
                return Some(cam.model);
            }
            if let Some(rem) = self.remembered.get()
                && rem.peripheral_id == id
            {
                return Some(rem.model);
            }
        }
        self.nearby_model()
    }

    pub fn nearby_model(self) -> Option<CameraModel> {
        if self.demo() {
            return self
                .camera
                .with(|c| c.borrow().as_ref().and_then(|cam| cam.nearby()));
        }
        self.nearby_cameras()
            .first()
            .map(|c| c.model)
            .or_else(|| self.remembered.get().map(|r| r.model))
    }

    pub fn pairing_hint_key(self) -> &'static str {
        self.selected_model()
            .unwrap_or(CameraModel::Gr3)
            .pairing_hint_key()
    }

    /// Watch for powered-on cameras while the app is in the foreground: a
    /// background scan keeps the radio busy for a prompt nobody can see.
    fn start_ble_scan(self) {
        spawn(async move {
            let mut scanning = false;
            loop {
                let active = lifecycle() == nsh_core::platform::Lifecycle::Active;
                if active && !scanning {
                    self.resume_nearby_watch();
                    scanning = true;
                } else if !active && scanning {
                    platform::stop_scanning();
                    self.nearby_watch.with(|w| w.borrow_mut().clear());
                    self.nearby.set(Vec::new());
                    scanning = false;
                }
                if scanning {
                    let changed = self.nearby_watch.with(|w| w.borrow_mut().prune(now_ms()));
                    if changed {
                        self.publish_nearby();
                    }
                }
                timer(Duration::from_millis(PRUNE_INTERVAL_MS)).await;
            }
        });
    }

    fn resume_nearby_watch(self) {
        let (tx, rx) = async_channel::unbounded();
        let remembered_id = self.remembered.get().map(|r| r.peripheral_id.clone());
        platform::start_scanning(tx, remembered_id.as_deref());
        spawn(async move {
            while let Ok(frame) = rx.recv().await {
                if frame.power == crate::domain::nearby_watch::AdvertisedPower::Standby {
                    self.camera_powered_off(&frame.peripheral_id);
                }
                let changed = self
                    .nearby_watch
                    .with(|w| w.borrow_mut().frame(&frame, now_ms()));
                if changed {
                    self.publish_nearby();
                }
            }
        });
    }

    /// Show the sightings and offer the nearby prompt once per power-on.
    fn publish_nearby(self) {
        let list = self.nearby_watch.with(|w| w.borrow().sightings().to_vec());
        self.nearby.set(list);
        // Only an idle, disconnected app offers a camera.
        let connected = self
            .status()
            .is_some_and(|s| s.camera.connection == ConnectionState::Connected);
        if self.sheet().is_some() || connected || self.connect_started.get().is_some() {
            return;
        }
        let remembered = self.remembered.get().map(|r| r.peripheral_id);
        let candidate = self
            .nearby_watch
            .with(|w| w.borrow().candidate(remembered.as_deref()));
        if let Some(camera) = candidate {
            self.suppress_nearby(&camera.peripheral_id);
            self.selected_id.set(Some(camera.peripheral_id));
            self.set_sheet(Some(Sheet::Nearby));
        }
    }

    /// Keep a camera out of the nearby prompt for its current power-on.
    pub(super) fn suppress_nearby(self, peripheral_id: &str) {
        self.nearby_watch
            .with(|w| w.borrow_mut().suppress(peripheral_id, now_ms()));
    }

    /// Whether a fresh advertisement says this camera is on.
    fn advertised_powered_on(self, peripheral_id: &str) -> bool {
        self.nearby_watch.with(|w| {
            w.borrow()
                .sightings()
                .iter()
                .any(|c| c.peripheral_id.eq_ignore_ascii_case(peripheral_id))
        })
    }

    pub fn dismiss_nearby(self) {
        if let Some(id) = self.selected_id.get().or_else(|| {
            self.nearby_cameras()
                .first()
                .map(|c| c.peripheral_id.clone())
        }) {
            self.suppress_nearby(&id);
        }
        self.set_sheet(None);
    }

    pub fn open_scan(self) {
        if self.demo() && self.nearby_model().is_some() {
            self.set_sheet(Some(Sheet::Nearby));
            return;
        }
        if !self.demo() && platform::has_dev_camera_url() {
            self.selected_id.set(Some("dev-url".into()));
            self.start_connect();
            return;
        }
        if !self.demo() && platform::bluetooth_unavailable() {
            self.set_sheet(Some(Sheet::ConnectError(LinkError::BluetoothUnavailable)));
            return;
        }
        if !self.demo() && self.nearby_cameras().len() == 1 {
            let cam = self.nearby_cameras()[0].clone();
            self.selected_id.set(Some(cam.peripheral_id));
            self.set_sheet(Some(Sheet::Nearby));
        } else {
            self.set_sheet(Some(Sheet::Scan));
        }
    }

    pub fn select_nearby(self, peripheral_id: String) {
        self.selected_id.set(Some(peripheral_id));
        self.start_connect();
    }

    pub fn start_connect(self) {
        if self.update_busy.get() {
            return;
        }
        if self.connect_started.get().is_some()
            || self.connect_abort.with(|slot| slot.borrow().is_some())
        {
            return;
        }
        self.set_frozen(false);
        if !self.initial_connect.get() {
            self.sheet.set(Some(Sheet::Connecting { step: 0 }));
        }
        self.connect_started.set(Some(Instant::now()));
        let generation = self.connect_generation.get().wrapping_add(1);
        self.connect_generation.set(generation);

        if self.demo() {
            return;
        }

        let peripheral_id = self
            .selected_id
            .get()
            .or_else(|| {
                self.nearby_cameras()
                    .first()
                    .map(|c| c.peripheral_id.clone())
            })
            .or_else(|| self.remembered.get().map(|r| r.peripheral_id.clone()));
        let Some(peripheral_id) = peripheral_id else {
            self.initial_connect.set(false);
            self.connect_started.set(None);
            self.sheet
                .set(Some(Sheet::ConnectError(LinkError::NotNearby)));
            return;
        };
        let model_hint = self.selected_model().unwrap_or(CameraModel::Gr3);
        app_info!("connect requested {peripheral_id} model={model_hint:?}");
        let (abort, registration) = futures_util::future::AbortHandle::new_pair();
        self.connect_abort
            .with(|slot| *slot.borrow_mut() = Some(abort));
        self.root.run(|| {
            spawn(async move {
                app_info!("connect task started");
                platform::begin_connect_attempt();
                let options = crate::domain::ConnectOptions {
                    advertised_powered_on: self.advertised_powered_on(&peripheral_id),
                    ..warmup::options_for(&peripheral_id)
                };
                let last_step = std::rc::Rc::new(std::cell::Cell::new(ConnectStep::Bluetooth));
                let step_seen = last_step.clone();
                let operation =
                    platform::connect_camera(&peripheral_id, model_hint, options, |step| {
                        step_seen.set(step);
                        if self.connect_generation.get() != generation {
                            return;
                        }
                        if !self.initial_connect.get() {
                            self.sheet
                                .set(Some(Sheet::Connecting { step: step.as_u8() }));
                        }
                    });
                // One attempt is bounded by foreground time; the step it was
                // on when time ran out names the timeout.
                let bounded = async {
                    use futures_util::future::{Either, select};
                    match select(Box::pin(operation), Box::pin(connect_deadline())).await {
                        Either::Left((result, _)) => result,
                        Either::Right((_, operation)) => {
                            platform::mark_connect_deadline();
                            drop(operation);
                            Err(match last_step.get() {
                                ConnectStep::Wifi => LinkError::WifiJoinTimeout,
                                _ => LinkError::BleTimeout,
                            })
                        }
                    }
                };
                let result = futures_util::future::Abortable::new(bounded, registration).await;
                if self.connect_generation.get() != generation {
                    self.connect_abort.with(|slot| slot.borrow_mut().take());
                    return;
                }
                self.connect_abort.with(|slot| slot.borrow_mut().take());
                let result = result.unwrap_or(Err(LinkError::Cancelled));
                self.initial_connect.set(false);
                match result {
                    Ok(platform::Connected {
                        status,
                        package,
                        remembered,
                        report,
                    }) => {
                        if let Some(report) = &report {
                            warmup::learn(&peripheral_id, status.camera.model, options, report);
                        }
                        // A camera just connected to is not offered again.
                        self.suppress_nearby(&peripheral_id);
                        self.reset_looks();
                        self.status.set(Some(status));
                        self.package.set(Some(package));
                        self.remembered.set(Some(remembered));
                        self.sheet.set(None);
                        self.connect_started.set(None);
                        self.refresh_update_info();
                        self.resume_update();
                        if self.tab() == Tab::Looks {
                            self.refresh_looks();
                        }
                    }
                    Err(LinkError::Cancelled) => {
                        self.dismiss_nearby();
                        self.connect_started.set(None);
                    }
                    Err(err) => {
                        self.sheet.set(Some(Sheet::ConnectError(err)));
                        self.connect_started.set(None);
                    }
                }
            })
        });
    }

    /// Fence progress before aborting so late native callbacks cannot reopen the sheet.
    pub fn cancel_connect(self) {
        self.initial_connect.set(false);
        self.connect_generation
            .update(|generation| *generation = generation.wrapping_add(1));
        self.connect_started.set(None);
        self.connect_abort.with(|slot| {
            if let Some(abort) = slot.borrow().as_ref() {
                abort.abort();
            }
        });
        self.dismiss_nearby();
    }

    /// Advance connecting checklist and finish with a connected camera (demo).
    pub fn tick_connecting(self) {
        if self.frozen() || !self.demo() {
            return;
        }
        let Some(started) = self.connect_started.get() else {
            return;
        };
        let step = (started.elapsed().as_millis() / 250) as u8;
        if !matches!(self.sheet.get(), Some(Sheet::Connecting { .. })) {
            return;
        }
        if step >= 3 {
            let (status, package) = self.camera.with(|slot| {
                let mut slot = slot.borrow_mut();
                let Some(cam) = slot.as_mut() else {
                    return (None, None);
                };
                match block_on_ready(cam.connect()) {
                    Ok(status) => {
                        let package = block_on_ready(cam.read_package()).ok();
                        (Some(status), package)
                    }
                    Err(_) => (None, None),
                }
            });
            if let Some(status) = status {
                self.status.set(Some(status));
            }
            if let Some(package) = package {
                self.package.set(Some(package));
            }
            self.sheet.set(None);
            self.connect_started.set(None);
        } else {
            self.sheet.set(Some(Sheet::Connecting { step }));
        }
    }

    pub fn open_software_update(self) {
        self.push(Route::SoftwareUpdate);
    }

    pub fn begin_update(self, kind: UpdateKind) {
        if self.update_busy.get() {
            return;
        }
        self.update_kind.set(kind);
        if !self.demo() {
            self.begin_real_update(kind);
            return;
        }
        self.set_frozen(false);
        let ok = self.camera.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(cam) = slot.as_mut() else {
                return false;
            };
            block_on_ready(cam.send_update(kind)).is_ok()
        });
        if !ok {
            return;
        }
        self.baseline.set(self.package.get().map(|p| p.installed));
        self.phase.set(UpdatePhase::Sending {
            progress: 0.0,
            kind,
        });
        self.confirm_deadline.set(None);
        self.sheet.set(Some(Sheet::Sending));
    }

    /// Poll the fake camera and mirror phases into sheets.
    pub fn tick_update(self) {
        if !self.demo() {
            return;
        }
        if self.frozen() {
            return;
        }
        let sheet = self.sheet.get();
        let in_flow = matches!(
            sheet,
            Some(Sheet::Sending | Sheet::Confirm | Sheet::Restart)
        );

        let outcome = self.camera.with(|slot| {
            let mut slot = slot.borrow_mut();
            let cam = slot.as_mut()?;
            if !in_flow
                && !cam.flow().is_busy()
                && !matches!(
                    cam.flow().phase(),
                    UpdatePhase::Installed { .. } | UpdatePhase::Failed(_)
                )
            {
                return None;
            }
            let poll = block_on_ready(cam.poll_update()).ok()?;
            let flow = cam.flow().clone();
            let package = block_on_ready(cam.read_package()).ok();
            let status = block_on_ready(cam.read_status()).ok();
            Some((poll, flow, package, status))
        });

        let Some((poll, flow, package, status)) = outcome else {
            return;
        };

        let previous = flow
            .baseline_version()
            .map(str::to_owned)
            .or_else(|| self.baseline.get())
            .unwrap_or_else(|| "1.6.1".into());
        let restarting = matches!(flow.phase(), UpdatePhase::Restart { .. });
        self.phase.set(flow.phase().clone());
        if let Some(package) = package {
            self.package.set(Some(package));
        }
        if let Some(status) = status {
            self.status.set(Some(status));
        }

        match poll {
            UpdatePoll::Sending { .. } => self.sheet.set(Some(Sheet::Sending)),
            UpdatePoll::WaitingConfirm => {
                if self.confirm_deadline.get().is_none() {
                    self.confirm_deadline
                        .set(Some(Instant::now() + Duration::from_secs(5 * 60)));
                }
                self.sheet.set(Some(Sheet::Confirm));
            }
            UpdatePoll::Confirmed => {
                self.sheet.set(Some(if restarting {
                    Sheet::Restart
                } else {
                    Sheet::Confirm
                }));
            }
            UpdatePoll::Installed { version } => {
                self.sheet.set(Some(Sheet::Installed { version, previous }));
            }
            UpdatePoll::Failed(error) => self.sheet.set(Some(Sheet::Error(error))),
        }
    }

    pub fn confirm_remaining(self) -> Duration {
        self.confirm_deadline
            .get()
            .map(|d| d.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(5 * 60))
    }

    pub fn finish_restart(self) {
        if !self.demo() {
            self.update_reconnect.set(true);
            self.resume_update();
            return;
        }
        self.set_frozen(false);
        for _ in 0..8 {
            self.tick_update();
            if matches!(
                self.flow_phase(),
                UpdatePhase::Installed { .. } | UpdatePhase::Failed(_)
            ) || matches!(self.sheet(), Some(Sheet::Installed { .. }))
            {
                break;
            }
        }
    }

    pub fn dismiss_installed(self) {
        self.camera.with(|slot| {
            if let Some(cam) = slot.borrow_mut().as_mut() {
                let _ = block_on_ready(cam.cancel());
                self.package.set(block_on_ready(cam.read_package()).ok());
            }
        });
        self.phase.set(UpdatePhase::Idle);
        self.sheet.set(None);
        self.confirm_deadline.set(None);
        self.stacks
            .update(|stacks| stacks[tab_index(Tab::Camera)].clear());
    }

    pub fn dismiss_error(self) {
        self.camera.with(|slot| {
            if let Some(cam) = slot.borrow_mut().as_mut() {
                let _ = block_on_ready(cam.cancel());
            }
        });
        self.phase.set(UpdatePhase::Idle);
        self.sheet.set(None);
        self.confirm_deadline.set(None);
    }

    pub fn dismiss_connect_error(self) {
        self.sheet.set(None);
        self.connect_started.set(None);
    }

    pub fn retry_connect(self) {
        self.start_connect();
    }

    pub fn cancel_update(self) {
        if !self.demo() {
            self.cancel_real_update();
            return;
        }
        self.camera.with(|slot| {
            if let Some(cam) = slot.borrow_mut().as_mut() {
                let _ = block_on_ready(cam.cancel());
            }
        });
        self.phase.set(UpdatePhase::Idle);
        self.sheet.set(None);
        self.confirm_deadline.set(None);
    }

    pub fn retry_update(self) {
        self.dismiss_error();
        if !self.demo() && platform::update::load_intent().is_some() {
            self.update_reconnect.set(true);
            self.resume_update();
        } else {
            self.begin_update(self.update_kind.get());
        }
    }

    pub fn confirm_deadline_label(self) -> String {
        let _ = self.confirm_tick.get();
        let rem = self.confirm_remaining();
        let total = rem.as_secs();
        format!("{}:{:02}", total / 60, total % 60)
    }

    /// Spawn a light poll loop while connecting or updating.
    pub fn ensure_ticker(self) {
        spawn(async move {
            loop {
                timer(Duration::from_millis(150)).await;
                if self.frozen() {
                    continue;
                }
                if self.sheet() == Some(Sheet::Confirm) {
                    self.confirm_tick.set(self.confirm_remaining().as_secs());
                }
                match self.sheet() {
                    Some(Sheet::Connecting { .. }) => self.tick_connecting(),
                    Some(Sheet::Sending | Sheet::Confirm | Sheet::Restart) => self.tick_update(),
                    Some(Sheet::Scan) if !self.demo() && platform::bluetooth_unavailable() => {
                        self.set_sheet(Some(Sheet::ConnectError(LinkError::BluetoothUnavailable)));
                    }
                    _ => {}
                }
            }
        });
    }
}

fn tab_index(tab: Tab) -> usize {
    match tab {
        Tab::Camera => 0,
        Tab::Looks => 1,
        Tab::Settings => 2,
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

fn demo_requested() -> bool {
    if env::var("GRKIT_DEMO").as_deref() == Ok("1")
        || crate::platform::config_value("GRKIT_DEMO_SCREEN").is_some()
    {
        return true;
    }
    for arg in env::args() {
        if arg == "--demo"
            || arg == "demo"
            || arg == "--demo-screen"
            || arg.starts_with("--demo-screen=")
        {
            return true;
        }
    }
    false
}

fn demo_fail_from_env() -> DemoFail {
    match env::var("GRKIT_DEMO_FAIL").as_deref() {
        Ok("card_full") => DemoFail::CardFull,
        Ok("refused") => DemoFail::Refused,
        Ok("not_confirmed") => DemoFail::NotConfirmed,
        Ok("connection_lost") => DemoFail::ConnectionLost,
        _ => DemoFail::None,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolves after `connect_policy::OUTER_TIMEOUT` of foreground time.
async fn connect_deadline() {
    let start = platform::link_support::foreground_clock();
    loop {
        timer(Duration::from_millis(250)).await;
        if platform::link_support::foreground_clock() - start
            >= crate::domain::connect_policy::OUTER_TIMEOUT
        {
            return;
        }
    }
}

/// Connected and up to date?
pub fn is_up_to_date(package: &PackageInfo) -> bool {
    package
        .available
        .as_ref()
        .is_none_or(|available| crate::domain::version_at_least(&package.installed, available))
}

/// Rate URL for the App Store review prompt.
pub fn rate_url() -> String {
    format!("https://apps.apple.com/app/id{APP_STORE_ID}?action=write-review")
}

/// User-facing copy for a connect-time link error.
pub fn link_error_title(error: &LinkError) -> String {
    crate::strings::tr(error.copy().0, &[])
}

pub fn link_error_body(error: &LinkError) -> String {
    use crate::strings::tr;
    match error {
        LinkError::Http(msg) => format!("{} {msg}", tr(error.copy().1, &[])),
        _ => tr(error.copy().1, &[]),
    }
}
