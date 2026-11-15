//! App state: tabs, camera session, update sheets, and demo mode.
//!
//! Reactive fields use NSH `Signal`s so the UI updates when they change.

mod demo_screen;

pub use demo_screen::{
    DEMO_SCREEN_IDS, DemoScreen, demo_screen_from_env, key_text, parse_demo_screen,
};

use std::{
    cell::RefCell,
    env,
    time::{Duration, Instant},
};

use nsh_core::prelude::*;

use crate::domain::{
    CameraLink, CameraModel, CameraStatus, DemoFail, FakeCamera, PackageInfo, UpdateError,
    UpdateKind, UpdatePhase, UpdatePoll,
};

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
    Settings,
}

/// Pages pushed on the Camera tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    SoftwareUpdate,
}

/// Modal sheets over the camera tab.
#[derive(Clone, Debug, PartialEq)]
pub enum Sheet {
    Nearby,
    Scan,
    Connecting { step: u8 },
    Sending,
    Confirm,
    Restart,
    Installed { version: String, previous: String },
    Error(UpdateError),
}

/// Shared app state provided at the root (`Copy` handle on signals).
#[derive(Clone, Copy)]
pub struct AppState {
    demo: bool,
    tab: Signal<Tab>,
    stacks: Signal<[Vec<Route>; 2]>,
    sheet: Signal<Option<Sheet>>,
    pub(crate) status: Signal<Option<CameraStatus>>,
    pub(crate) package: Signal<Option<PackageInfo>>,
    pub(crate) phase: Signal<UpdatePhase>,
    pub(crate) confirm_deadline: Signal<Option<Instant>>,
    connect_started: Signal<Option<Instant>>,
    /// When true, the ticker does not advance connecting / update sheets.
    frozen: Signal<bool>,
    /// Fake camera for demo mode; mutated from tick handlers.
    pub(crate) camera: Stored<RefCell<Option<FakeCamera>>>,
    pub(crate) baseline: Signal<Option<String>>,
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
        let app = Self {
            demo,
            tab: signal(Tab::Camera),
            stacks: signal([Vec::new(), Vec::new()]),
            sheet: signal(None),
            status: signal(None),
            package: signal(None),
            phase: signal(UpdatePhase::Idle),
            confirm_deadline: signal(None),
            connect_started: signal(None),
            frozen: signal(false),
            camera: stored(RefCell::new(camera)),
            baseline: signal(None),
        };
        if let Some(screen) = screen {
            demo_screen::apply(app, screen);
        }
        app
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

    pub fn model_name(self) -> String {
        self.status
            .get()
            .map(|s| s.camera.model.display_name().to_owned())
            .or_else(|| {
                self.camera.with(|c| {
                    c.borrow()
                        .as_ref()
                        .map(|cam| cam.config().model.display_name().to_owned())
                })
            })
            .unwrap_or_else(|| CameraModel::Gr3.display_name().to_owned())
    }

    pub fn nearby_model(self) -> Option<CameraModel> {
        self.camera
            .with(|c| c.borrow().as_ref().and_then(|cam| cam.nearby()))
    }

    pub fn open_scan(self) {
        if self.demo() && self.nearby_model().is_some() {
            self.set_sheet(Some(Sheet::Nearby));
        } else {
            self.set_sheet(Some(Sheet::Scan));
        }
    }

    pub fn start_connect(self) {
        self.set_frozen(false);
        self.sheet.set(Some(Sheet::Connecting { step: 0 }));
        self.connect_started.set(Some(Instant::now()));
    }

    /// Advance connecting checklist and finish with a connected camera.
    pub fn tick_connecting(self) {
        if self.frozen() {
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
                match cam.connect() {
                    Ok(status) => {
                        let package = cam.read_package().ok();
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
        self.set_frozen(false);
        let ok = self.camera.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(cam) = slot.as_mut() else {
                return false;
            };
            cam.send_update(kind).is_ok()
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
            let poll = cam.poll_update().ok()?;
            let flow = cam.flow().clone();
            let package = cam.read_package().ok();
            let status = cam.read_status().ok();
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
                let _ = cam.cancel();
                self.package.set(cam.read_package().ok());
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
                let _ = cam.cancel();
            }
        });
        self.phase.set(UpdatePhase::Idle);
        self.sheet.set(None);
        self.confirm_deadline.set(None);
    }

    pub fn cancel_update(self) {
        self.camera.with(|slot| {
            if let Some(cam) = slot.borrow_mut().as_mut() {
                let _ = cam.cancel();
            }
        });
        self.phase.set(UpdatePhase::Idle);
        self.sheet.set(None);
        self.confirm_deadline.set(None);
    }

    pub fn retry_update(self) {
        self.dismiss_error();
        self.begin_update(UpdateKind::Install);
    }

    pub fn confirm_deadline_label(self) -> String {
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
                match self.sheet() {
                    Some(Sheet::Connecting { .. }) => self.tick_connecting(),
                    Some(Sheet::Sending | Sheet::Confirm | Sheet::Restart) => self.tick_update(),
                    _ => {}
                }
            }
        });
    }
}

fn tab_index(tab: Tab) -> usize {
    match tab {
        Tab::Camera => 0,
        Tab::Settings => 1,
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

fn demo_requested() -> bool {
    if env::var("GRKIT_DEMO").as_deref() == Ok("1") {
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

/// Connected and up to date?
pub fn is_up_to_date(package: &PackageInfo) -> bool {
    package.available.is_none()
}

/// Rate URL for the App Store review prompt.
pub fn rate_url() -> String {
    format!("https://apps.apple.com/app/id{APP_STORE_ID}?action=write-review")
}
