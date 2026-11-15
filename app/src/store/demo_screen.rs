//! Demo screen ids for screenshots: `GRKIT_DEMO_SCREEN` / `--demo-screen`.
//!
//! When demo mode is on and an id is set, [`apply`] jumps the FakeCamera and UI
//! to that design screen (sheet, push, or alert) and freezes the ticker so the
//! frame stays still for a screenshot. Unknown ids leave the normal demo.

use std::{
    env,
    time::{Duration, Instant},
};

use nsh_base::alert_dialog;
use nsh_core::prelude::*;

use crate::domain::{
    CameraLink, FakeCamera, FakeConfig, UpdateError, UpdateKind, UpdatePhase, block_on_ready,
};
use crate::store::{AppState, Route, Sheet, Tab};
use crate::strings::tr;

/// Design-file screen ids used for demo screenshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemoScreen {
    /// A1 — first launch (empty Camera tab).
    A1,
    /// A2 — camera nearby sheet.
    A2,
    /// A3 — add a camera sheet.
    A3,
    /// A4 — connecting checklist.
    A4,
    /// A5 — connected, update available.
    A5,
    /// A6 — connected, up to date.
    A6,
    /// A7 — Settings tab.
    A7,
    /// B1 — Software Update (update available).
    B1,
    /// B2 — sending (~62% progress).
    B2,
    /// B3 — confirm on the camera (countdown).
    B3,
    /// B4 — restart the camera.
    B4,
    /// B5 — update installed.
    B5,
    /// C1 — Software Update with roll-back row (on 1.7.0).
    C1,
    /// C2 — roll-back confirmation alert.
    C2,
    /// C3 — roll-back confirm on the camera.
    C3,
    /// C4 — card full error.
    C4,
    /// C5 — not confirmed error.
    C5,
}

/// Every id string the env / CLI accept, in design order.
pub const DEMO_SCREEN_IDS: &[(&str, DemoScreen)] = &[
    ("A1", DemoScreen::A1),
    ("A2", DemoScreen::A2),
    ("A3", DemoScreen::A3),
    ("A4", DemoScreen::A4),
    ("A5", DemoScreen::A5),
    ("A6", DemoScreen::A6),
    ("A7", DemoScreen::A7),
    ("B1", DemoScreen::B1),
    ("B2", DemoScreen::B2),
    ("B3", DemoScreen::B3),
    ("B4", DemoScreen::B4),
    ("B5", DemoScreen::B5),
    ("C1", DemoScreen::C1),
    ("C2", DemoScreen::C2),
    ("C3", DemoScreen::C3),
    ("C4", DemoScreen::C4),
    ("C5", DemoScreen::C5),
];

/// Parse a design id (`A5`, `a5`, …). Unknown → `None` (normal demo).
pub fn parse_demo_screen(id: &str) -> Option<DemoScreen> {
    let id = id.trim();
    DEMO_SCREEN_IDS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(id))
        .map(|(_, screen)| *screen)
}

/// Screen from `GRKIT_DEMO_SCREEN` or `--demo-screen <id>`.
pub fn demo_screen_from_env() -> Option<DemoScreen> {
    if let Ok(id) = env::var("GRKIT_DEMO_SCREEN")
        && let Some(screen) = parse_demo_screen(&id)
    {
        return Some(screen);
    }
    let mut args = env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        if arg == "--demo-screen" {
            return args.next().as_deref().and_then(parse_demo_screen);
        }
        if let Some(id) = arg.strip_prefix("--demo-screen=") {
            return parse_demo_screen(id);
        }
    }
    None
}

/// Jump `app` to `screen` and freeze timed advances for a stable screenshot.
pub fn apply(app: AppState, screen: DemoScreen) {
    app.set_frozen(true);
    match screen {
        DemoScreen::A1 => {
            // Empty first launch: nearby fake stays for later Add Camera.
            app.status.set(None);
            app.package.set(None);
            app.set_sheet(None);
            app.set_tab(Tab::Camera);
        }
        DemoScreen::A2 => {
            app.status.set(None);
            app.package.set(None);
            app.set_tab(Tab::Camera);
            app.set_sheet(Some(Sheet::Nearby));
        }
        DemoScreen::A3 => {
            app.status.set(None);
            app.package.set(None);
            app.set_tab(Tab::Camera);
            app.set_sheet(Some(Sheet::Scan));
        }
        DemoScreen::A4 => {
            app.status.set(None);
            app.package.set(None);
            app.set_tab(Tab::Camera);
            app.set_sheet(Some(Sheet::Connecting { step: 1 }));
        }
        DemoScreen::A5 => {
            seed_connected_available(app);
            app.set_tab(Tab::Camera);
            app.set_sheet(None);
        }
        DemoScreen::A6 => {
            seed_connected_up_to_date(app);
            app.set_tab(Tab::Camera);
            app.set_sheet(None);
        }
        DemoScreen::A7 => {
            seed_connected_available(app);
            app.set_tab(Tab::Settings);
            app.set_sheet(None);
        }
        DemoScreen::B1 => {
            seed_connected_available(app);
            app.set_tab(Tab::Camera);
            app.set_sheet(None);
            app.push(Route::SoftwareUpdate);
        }
        DemoScreen::B2 => {
            seed_connected_available(app);
            seed_sending(app, UpdateKind::Install, 0.62);
            app.set_tab(Tab::Camera);
            app.set_sheet(Some(Sheet::Sending));
        }
        DemoScreen::B3 => {
            seed_connected_available(app);
            seed_waiting(app, UpdateKind::Install);
            app.set_tab(Tab::Camera);
            app.confirm_deadline
                .set(Some(Instant::now() + Duration::from_secs(4 * 60 + 32)));
            app.set_sheet(Some(Sheet::Confirm));
        }
        DemoScreen::B4 => {
            seed_connected_available(app);
            seed_restart(app, UpdateKind::Install);
            app.set_tab(Tab::Camera);
            app.set_sheet(Some(Sheet::Restart));
        }
        DemoScreen::B5 => {
            seed_connected_up_to_date(app);
            app.baseline.set(Some("1.6.1".into()));
            app.phase.set(UpdatePhase::Installed {
                kind: UpdateKind::Install,
                version: "1.7.0".into(),
            });
            app.set_tab(Tab::Camera);
            app.set_sheet(Some(Sheet::Installed {
                version: "1.7.0".into(),
                previous: "1.6.1".into(),
            }));
        }
        DemoScreen::C1 => {
            seed_connected_up_to_date(app);
            app.set_tab(Tab::Camera);
            app.set_sheet(None);
            app.push(Route::SoftwareUpdate);
        }
        DemoScreen::C2 => {
            seed_connected_up_to_date(app);
            app.set_tab(Tab::Camera);
            app.set_sheet(None);
            app.push(Route::SoftwareUpdate);
            spawn(async move {
                next_frame().await;
                next_frame().await;
                let _ = alert_dialog()
                    .title(tr("roll.alert_title", &[("version", "1.6.1")]))
                    .description(tr("roll.alert_body", &[]))
                    .cancel("cancel", tr("roll.cancel", &[]))
                    .action("rollback", tr("roll.confirm", &[]))
                    .show()
                    .await;
            });
        }
        DemoScreen::C3 => {
            seed_connected_up_to_date(app);
            seed_waiting(app, UpdateKind::Rollback);
            app.set_tab(Tab::Camera);
            app.confirm_deadline
                .set(Some(Instant::now() + Duration::from_secs(4 * 60 + 32)));
            app.set_sheet(Some(Sheet::Confirm));
        }
        DemoScreen::C4 => {
            seed_connected_available(app);
            app.phase.set(UpdatePhase::Failed(UpdateError::CardFull {
                bytes_needed: 12 * 1024 * 1024,
            }));
            app.set_tab(Tab::Camera);
            app.set_sheet(Some(Sheet::Error(UpdateError::CardFull {
                bytes_needed: 12 * 1024 * 1024,
            })));
        }
        DemoScreen::C5 => {
            seed_connected_available(app);
            app.phase
                .set(UpdatePhase::Failed(UpdateError::NotConfirmed));
            app.set_tab(Tab::Camera);
            app.set_sheet(Some(Sheet::Error(UpdateError::NotConfirmed)));
        }
    }
}

fn seed_connected_available(app: AppState) {
    app.camera.with(|slot| {
        let mut slot = slot.borrow_mut();
        let cam = slot.get_or_insert_with(|| FakeCamera::new(FakeConfig::default()));
        let _ = block_on_ready(cam.connect());
        if let Ok(status) = block_on_ready(cam.read_status()) {
            app.status.set(Some(status));
        }
        if let Ok(package) = block_on_ready(cam.read_package()) {
            app.package.set(Some(package));
        }
    });
}

fn seed_connected_up_to_date(app: AppState) {
    app.camera.with(|slot| {
        let mut slot = slot.borrow_mut();
        let cfg = FakeConfig {
            installed: "1.7.0".into(),
            available: "1.7.0".into(),
            rollback: "1.6.1".into(),
            ..FakeConfig::default()
        };
        let mut cam = FakeCamera::new(cfg);
        let _ = block_on_ready(cam.connect());
        // Mark package as current with a roll-back target.
        cam.set_up_to_date("1.7.0", "1.6.1");
        if let Ok(status) = block_on_ready(cam.read_status()) {
            app.status.set(Some(status));
        }
        if let Ok(package) = block_on_ready(cam.read_package()) {
            app.package.set(Some(package));
        }
        *slot = Some(cam);
    });
}

fn seed_sending(app: AppState, kind: UpdateKind, progress: f32) {
    app.camera.with(|slot| {
        if let Some(cam) = slot.borrow_mut().as_mut() {
            cam.seed_sending(kind, progress);
            app.phase.set(cam.flow().phase().clone());
            app.baseline
                .set(cam.flow().baseline_version().map(str::to_owned));
        }
    });
}

fn seed_waiting(app: AppState, kind: UpdateKind) {
    app.camera.with(|slot| {
        if let Some(cam) = slot.borrow_mut().as_mut() {
            cam.seed_waiting(kind);
            app.phase.set(cam.flow().phase().clone());
            app.baseline
                .set(cam.flow().baseline_version().map(str::to_owned));
        }
    });
}

fn seed_restart(app: AppState, kind: UpdateKind) {
    app.camera.with(|slot| {
        if let Some(cam) = slot.borrow_mut().as_mut() {
            cam.seed_restart(kind);
            app.phase.set(cam.flow().phase().clone());
            app.baseline
                .set(cam.flow().baseline_version().map(str::to_owned));
        }
    });
}

/// Key English text a headless test looks for on each screen.
pub fn key_text(screen: DemoScreen) -> &'static str {
    match screen {
        DemoScreen::A1 => "Add Your Camera",
        DemoScreen::A2 => "Connect",
        DemoScreen::A3 => "Add a Camera",
        DemoScreen::A4 => "Connecting to RICOH GR III",
        DemoScreen::A5 => "What's New",
        DemoScreen::A6 => "GRKit Is Up to Date",
        DemoScreen::A7 => "More from the Maker",
        DemoScreen::B1 => "Update Now",
        DemoScreen::B2 => "Sending to your camera…",
        DemoScreen::B3 => "Confirm on the Camera",
        DemoScreen::B4 => "Restart the Camera",
        DemoScreen::B5 => "GRKit 1.7.0 Is Installed",
        DemoScreen::C1 => "Roll Back to 1.6.1",
        DemoScreen::C2 => "Roll Back to GRKit 1.6.1?",
        DemoScreen::C3 => "Confirm Roll Back on the Camera",
        DemoScreen::C4 => "Card Is Full",
        DemoScreen::C5 => "Not Confirmed",
    }
}
