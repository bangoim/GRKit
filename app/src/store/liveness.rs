//! Health of an established camera link: HTTP decides whether the camera is
//! still there, Bluetooth loss is only a hint, and a lost camera never brings
//! up a system Wi-Fi dialog.

use std::time::Duration;

use nsh_core::platform::Lifecycle;
use nsh_core::prelude::*;

use super::AppState;
use crate::domain::{CameraModel, ConnectionState, PropsResponse, UpdatePhase};
use crate::platform::{self, link_support};

const LOOP_INTERVAL: Duration = Duration::from_secs(5);
const WARM_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_WEAK_FAILURES: u32 = 4;
const FOREGROUND_RESUME_PROBE_TIMEOUT: Duration = Duration::from_millis(2_500);
const FOREGROUND_RESUME_RETRY_DELAY: Duration = Duration::from_millis(500);
const FOREGROUND_RESUME_TRIES: u32 = 2;

/// What one probe of the connected camera found.
enum Probe {
    Ok,
    /// Another camera answered at the camera address.
    Mismatch,
    Failed,
}

async fn probe(model: CameraModel, timeout: Duration) -> Probe {
    let Some(props) = link_support::read_props(timeout).await else {
        return Probe::Failed;
    };
    if same_camera(&props, model) {
        Probe::Ok
    } else {
        Probe::Mismatch
    }
}

fn same_camera(props: &PropsResponse, model: CameraModel) -> bool {
    props
        .camera
        .as_ref()
        .and_then(|camera| camera.model.as_deref())
        .and_then(crate::domain::http_api::model_from_name)
        .is_none_or(|answered| answered == model)
}

impl AppState {
    /// Watch the connected camera every few seconds in the foreground, at
    /// once after a Bluetooth disconnect, and right after the app returns to
    /// the foreground.
    pub(super) fn watch_link(self) {
        let (tx, rx) = async_channel::unbounded();
        platform::watch_disconnects(tx);
        spawn(async move {
            use futures_util::future::{Either, select};
            let mut weak_failures = 0;
            let mut was_active = true;
            loop {
                match select(Box::pin(rx.recv()), Box::pin(timer(LOOP_INTERVAL))).await {
                    Either::Left((Err(_), _)) => break,
                    Either::Left((Ok(_), _)) | Either::Right(_) => {}
                }
                let active = lifecycle() == Lifecycle::Active;
                let resumed = active && !was_active;
                was_active = active;
                if !active
                    || self.sheet().is_some()
                    || !matches!(self.flow_phase(), UpdatePhase::Idle)
                {
                    weak_failures = 0;
                    continue;
                }
                let Some(status) = self.status.get() else {
                    continue;
                };
                let model = status.camera.model;
                let generation = self.connect_generation.get();
                if status.camera.connection != ConnectionState::Connected {
                    self.reconnect_on_camera_network(model, generation).await;
                    continue;
                }
                let healthy = if resumed {
                    weak_failures = 0;
                    self.probe_after_resume(model).await
                } else {
                    match probe(model, WARM_PROBE_TIMEOUT).await {
                        Probe::Ok => {
                            weak_failures = 0;
                            true
                        }
                        Probe::Mismatch => false,
                        Probe::Failed => {
                            weak_failures += 1;
                            weak_failures < MAX_WEAK_FAILURES
                                || matches!(probe(model, WARM_PROBE_TIMEOUT).await, Probe::Ok)
                        }
                    }
                };
                // A connect, cancel, or update may have started meanwhile.
                if self.connect_generation.get() != generation || self.sheet().is_some() {
                    weak_failures = 0;
                    continue;
                }
                if !healthy {
                    weak_failures = 0;
                    self.mark_link_down();
                } else if weak_failures >= MAX_WEAK_FAILURES {
                    weak_failures = 0;
                }
            }
        });
    }

    async fn probe_after_resume(self, model: CameraModel) -> bool {
        for attempt in 0..FOREGROUND_RESUME_TRIES {
            match probe(model, FOREGROUND_RESUME_PROBE_TIMEOUT).await {
                Probe::Ok => return true,
                Probe::Mismatch => return false,
                Probe::Failed => {}
            }
            if attempt + 1 < FOREGROUND_RESUME_TRIES {
                timer(FOREGROUND_RESUME_RETRY_DELAY).await;
            }
        }
        false
    }

    /// The connected camera advertised that it powered off: it is gone.
    pub(super) fn camera_powered_off(self, peripheral_id: &str) {
        let ours = self
            .remembered
            .get()
            .is_some_and(|r| r.peripheral_id.eq_ignore_ascii_case(peripheral_id));
        let connected = self
            .status
            .get()
            .is_some_and(|s| s.camera.connection == ConnectionState::Connected);
        if ours && connected && self.sheet().is_none() {
            self.mark_link_down();
        }
    }

    /// The camera is gone. Keep its nearby prompt away for this power-on.
    fn mark_link_down(self) {
        if let Some(mut status) = self.status.get() {
            status.camera.connection = ConnectionState::Disconnected;
            self.status.set(Some(status));
        }
        if let Some(remembered) = self.remembered.get() {
            self.suppress_nearby(&remembered.peripheral_id);
        }
    }

    /// Reconnect without Bluetooth or a join dialog, only when the phone is
    /// already on the camera network and the same camera answers.
    async fn reconnect_on_camera_network(self, model: CameraModel, generation: u64) {
        if !link_support::network_snapshot().on_camera_subnet(model) {
            return;
        }
        if !matches!(probe(model, WARM_PROBE_TIMEOUT).await, Probe::Ok)
            || self.connect_generation.get() != generation
            || self.sheet().is_some()
        {
            return;
        }
        if let Some(mut status) = self.status.get() {
            status.camera.connection = ConnectionState::Connected;
            self.status.set(Some(status));
            app_info!("camera answered on its network again");
            self.refresh_update_info();
        }
    }
}
