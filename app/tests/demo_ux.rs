//! Headless walks of demo mode: update, roll back, and card-full error.

use std::{
    env,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use grkit_app::App;
use nsh_core::platform::Os;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{DialogAnswer, Headless};

const WAIT: Duration = Duration::from_secs(20);

/// Demo flags are process-wide; hold for the whole test so construction stays stable.
static DEMO_LOCK: Mutex<()> = Mutex::new(());

fn lock_demo() -> MutexGuard<'static, ()> {
    DEMO_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn start_phone() -> Headless {
    let mut config = EngineConfig::new(Root::of::<App>());
    config.environment.os = Os::Web;
    Headless::start(config).unwrap()
}

fn set_demo(fail: Option<&str>) {
    unsafe {
        env::set_var("GRKIT_DEMO", "1");
        match fail {
            Some(fail) => env::set_var("GRKIT_DEMO_FAIL", fail),
            None => env::remove_var("GRKIT_DEMO_FAIL"),
        }
    }
}

#[test]
fn demo_add_camera_update_confirm_restart_installed() {
    let _guard = lock_demo();
    set_demo(None);
    let mut app = start_phone();
    app.wait_for("Add Your Camera").unwrap();
    app.press("Add Camera").unwrap();
    app.wait_for("Connect").unwrap();
    app.press("Connect").unwrap();

    app.wait_for_within("Connected", WAIT).unwrap();
    app.wait_for("Update").unwrap();
    app.press("Update").unwrap();

    app.wait_for_within("Sending to your camera…", WAIT)
        .unwrap();
    app.wait_for_within("Confirm on the Camera", WAIT).unwrap();
    app.wait_for_within("Restart the Camera", WAIT).unwrap();
    app.press("Done").unwrap();
    app.wait_for_within("GRKit 1.7.0 Is Installed", WAIT)
        .unwrap();
    app.press("Done").unwrap();
    app.wait_for_within("GRKit Is Up to Date", WAIT).unwrap();
}

#[test]
fn demo_roll_back() {
    let _guard = lock_demo();
    set_demo(None);
    let mut app = start_phone();
    app.wait_for("Add Your Camera").unwrap();
    app.press("Add Camera").unwrap();
    app.press("Connect").unwrap();
    app.wait_for_within("Update", WAIT).unwrap();
    app.press("Update").unwrap();
    app.wait_for_within("Restart the Camera", WAIT).unwrap();
    app.press("Done").unwrap();
    app.wait_for_within("GRKit 1.7.0 Is Installed", WAIT)
        .unwrap();
    app.press("Done").unwrap();
    app.wait_for_within("GRKit Is Up to Date", WAIT).unwrap();

    app.press("GRKit Is Up to Date").unwrap();
    app.wait_for_within("Roll Back to 1.6.1", WAIT).unwrap();
    app.on_dialog(|dialog| {
        assert!(
            dialog
                .title
                .as_deref()
                .is_some_and(|t| t.contains("Roll Back"))
        );
        DialogAnswer::Press("Roll Back".into())
    });
    app.press("Roll Back to 1.6.1").unwrap();
    app.wait_for_within("Confirm Roll Back on the Camera", WAIT)
        .unwrap();
    app.wait_for_within("Restart the Camera", WAIT).unwrap();
    app.press("Done").unwrap();
    app.wait_for_within("GRKit 1.6.1 Is Installed", WAIT)
        .unwrap();
}

#[test]
fn demo_card_full_error() {
    let _guard = lock_demo();
    set_demo(Some("card_full"));
    let mut app = start_phone();
    app.wait_for("Add Your Camera").unwrap();
    app.press("Add Camera").unwrap();
    app.press("Connect").unwrap();
    app.wait_for_within("Update", WAIT).unwrap();
    app.press("Update").unwrap();
    app.wait_for_within("Card Is Full", WAIT).unwrap();
    app.wait_for("Free 12 MB, then try again.").unwrap();
    app.press("Close").unwrap();
    app.wait_for_gone("Card Is Full", WAIT).unwrap();
}

#[test]
fn cancelling_connect_can_start_a_fresh_attempt() {
    let _guard = lock_demo();
    set_demo(None);
    unsafe {
        env::set_var("GRKIT_DEMO_SCREEN", "A4");
    }
    let mut app = start_phone();
    let shown = app.wait_for("Connecting to RICOH GR III");
    unsafe {
        env::remove_var("GRKIT_DEMO_SCREEN");
    }
    shown.unwrap();
    app.press("Cancel").unwrap();
    app.wait_for_gone("Connecting to RICOH GR III", WAIT)
        .unwrap();
    app.press("Add Camera").unwrap();
    app.press("Connect").unwrap();
    app.wait_for_within("Connected", WAIT).unwrap();
}
