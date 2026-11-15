//! Scripted camera used by demo mode and tests.

use std::time::{Duration, Instant};

use crate::domain::{
    Camera, CameraLink, CameraModel, CameraStatus, ConnectionState, LinkError, PackageInfo,
    UpdateError, UpdateFlow, UpdateKind, UpdatePhase, UpdatePoll,
};

/// How a demo or test should fail, if at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DemoFail {
    #[default]
    None,
    CardFull,
    Refused,
    NotConfirmed,
    ConnectionLost,
}

/// Tunables for the fake timeline (tests use short durations).
#[derive(Clone, Debug)]
pub struct FakeConfig {
    pub model: CameraModel,
    pub firmware: String,
    pub installed: String,
    pub available: String,
    pub rollback: String,
    pub release_notes: String,
    pub package_bytes: u64,
    pub fail: DemoFail,
    pub card_full_bytes: u64,
    /// How many [`FakeCamera::tick`] calls to finish a send.
    pub send_ticks: u32,
    /// How many ticks after waiting before the camera confirms.
    pub confirm_ticks: u32,
    /// Confirm deadline used when failing with [`DemoFail::NotConfirmed`].
    pub confirm_deadline: Duration,
}

impl Default for FakeConfig {
    fn default() -> Self {
        Self {
            model: CameraModel::Gr3,
            firmware: "2.21".into(),
            installed: "1.6.1".into(),
            available: "1.7.0".into(),
            rollback: "1.6.1".into(),
            release_notes: "Faster Wi-Fi updates and clearer confirm steps on the camera.".into(),
            package_bytes: 4_700_000,
            fail: DemoFail::None,
            card_full_bytes: 12 * 1024 * 1024,
            send_ticks: 5,
            confirm_ticks: 3,
            confirm_deadline: Duration::from_secs(5 * 60),
        }
    }
}

/// In-memory camera that advances on [`tick`](Self::tick).
#[derive(Debug)]
pub struct FakeCamera {
    config: FakeConfig,
    camera: Camera,
    package: PackageInfo,
    flow: UpdateFlow,
    kind: Option<UpdateKind>,
    send_tick: u32,
    wait_tick: u32,
    /// Version after a successful install or roll-back; unchanged on errors.
    live_version: String,
    previous_version: Option<String>,
    started_at: Instant,
}

impl FakeCamera {
    pub fn new(config: FakeConfig) -> Self {
        let camera = Camera {
            model: config.model,
            firmware: config.firmware.clone(),
            installed_version: config.installed.clone(),
            connection: ConnectionState::Nearby,
        };
        let package = PackageInfo {
            installed: config.installed.clone(),
            available: Some(config.available.clone()),
            rollback: Some(config.rollback.clone()),
            firmware: config.firmware.clone(),
            release_notes: config.release_notes.clone(),
            package_bytes: config.package_bytes,
        };
        let live_version = config.installed.clone();
        Self {
            config,
            camera,
            package,
            flow: UpdateFlow::new(),
            kind: None,
            send_tick: 0,
            wait_tick: 0,
            live_version,
            previous_version: None,
            started_at: Instant::now(),
        }
    }

    pub fn demo() -> Self {
        Self::new(FakeConfig::default())
    }

    pub fn with_fail(fail: DemoFail) -> Self {
        let config = if fail == DemoFail::NotConfirmed {
            FakeConfig {
                fail,
                confirm_deadline: Duration::from_millis(50),
                confirm_ticks: 100,
                ..FakeConfig::default()
            }
        } else {
            FakeConfig {
                fail,
                ..FakeConfig::default()
            }
        };
        Self::new(config)
    }

    pub fn flow(&self) -> &UpdateFlow {
        &self.flow
    }

    pub fn live_version(&self) -> &str {
        &self.live_version
    }

    pub fn config(&self) -> &FakeConfig {
        &self.config
    }

    /// Demo screenshot: installed is current, with an optional roll-back target.
    pub fn set_up_to_date(&mut self, version: &str, rollback: &str) {
        self.live_version = version.to_owned();
        self.camera.installed_version = version.to_owned();
        self.camera.connection = ConnectionState::Connected;
        self.package.installed = version.to_owned();
        self.package.available = None;
        self.package.rollback = Some(rollback.to_owned());
        self.previous_version = Some(rollback.to_owned());
        self.flow.reset();
    }

    /// Demo screenshot: mid-send at `progress` (0..=1).
    pub fn seed_sending(&mut self, kind: UpdateKind, progress: f32) {
        self.flow.reset();
        self.flow.begin_sending(kind, self.live_version.clone());
        self.flow.set_progress(progress.clamp(0.0, 1.0));
        self.kind = Some(kind);
        self.send_tick = ((progress.clamp(0.0, 1.0)) * self.config.send_ticks as f32) as u32;
        self.wait_tick = 0;
    }

    /// Demo screenshot: waiting for confirmation on the camera.
    pub fn seed_waiting(&mut self, kind: UpdateKind) {
        self.seed_sending(kind, 1.0);
        self.flow.mark_waiting(Instant::now());
        self.wait_tick = 0;
    }

    /// Demo screenshot: restart step after confirmation.
    pub fn seed_restart(&mut self, kind: UpdateKind) {
        self.seed_waiting(kind);
        self.flow.mark_confirmed();
        self.flow.mark_restart();
    }

    /// Advance the scripted timeline by one step.
    pub fn tick(&mut self) {
        match self.flow.phase().clone() {
            UpdatePhase::Sending { kind, .. } => {
                if self.config.fail == DemoFail::CardFull && self.send_tick >= 1 {
                    self.flow.fail(UpdateError::CardFull {
                        bytes_needed: self.config.card_full_bytes,
                    });
                    return;
                }
                if self.config.fail == DemoFail::Refused && self.send_tick >= 1 {
                    self.flow.fail(UpdateError::Refused);
                    return;
                }
                if self.config.fail == DemoFail::ConnectionLost && self.send_tick >= 1 {
                    self.flow.fail(UpdateError::ConnectionLost);
                    self.camera.connection = ConnectionState::Disconnected;
                    return;
                }
                self.send_tick += 1;
                let progress =
                    (self.send_tick as f32 / self.config.send_ticks.max(1) as f32).min(1.0);
                self.flow.set_progress(progress);
                if self.send_tick >= self.config.send_ticks {
                    self.flow.mark_waiting(self.started_at);
                }
                let _ = kind;
            }
            UpdatePhase::WaitingConfirm { .. } => {
                if self.config.fail == DemoFail::NotConfirmed {
                    self.wait_tick += 1;
                    if self.wait_tick >= 2 {
                        self.flow.fail(UpdateError::NotConfirmed);
                    }
                    return;
                }
                if self.config.fail == DemoFail::ConnectionLost {
                    self.flow.fail(UpdateError::ConnectionLost);
                    self.camera.connection = ConnectionState::Disconnected;
                    return;
                }
                self.wait_tick += 1;
                if self.wait_tick >= self.config.confirm_ticks {
                    self.flow.mark_confirmed();
                }
            }
            UpdatePhase::Confirmed { .. } => {
                self.flow.mark_restart();
            }
            UpdatePhase::Restart { kind } => {
                let version = match kind {
                    UpdateKind::Install => {
                        self.previous_version = Some(self.live_version.clone());
                        self.config.available.clone()
                    }
                    UpdateKind::Rollback => self
                        .previous_version
                        .clone()
                        .unwrap_or_else(|| self.config.rollback.clone()),
                };
                self.live_version = version.clone();
                self.camera.installed_version = version.clone();
                self.package.installed = version.clone();
                if kind == UpdateKind::Install {
                    self.package.available = None;
                    self.package.rollback = Some(self.config.installed.clone());
                } else {
                    self.package.available = Some(self.config.available.clone());
                    self.package.rollback = None;
                }
                self.flow.mark_installed(version);
                self.kind = None;
            }
            _ => {}
        }
    }

    fn sync_package_from_live(&mut self) {
        self.package.installed = self.live_version.clone();
        self.camera.installed_version = self.live_version.clone();
    }
}

impl CameraLink for FakeCamera {
    fn nearby(&self) -> Option<CameraModel> {
        if matches!(
            self.camera.connection,
            ConnectionState::Nearby | ConnectionState::Connecting | ConnectionState::Connected
        ) {
            Some(self.camera.model)
        } else if self.camera.connection == ConnectionState::Disconnected
            && !matches!(
                self.flow.phase(),
                UpdatePhase::Failed(UpdateError::ConnectionLost)
            )
        {
            // Demo starts nearby; after a lost connection it stays away until reset.
            Some(self.camera.model)
        } else {
            None
        }
    }

    fn connect(&mut self) -> Result<CameraStatus, LinkError> {
        if self.camera.connection == ConnectionState::Disconnected
            && matches!(
                self.flow.phase(),
                UpdatePhase::Failed(UpdateError::ConnectionLost)
            )
        {
            return Err(LinkError::ConnectionLost);
        }
        self.camera.connection = ConnectionState::Connected;
        self.sync_package_from_live();
        Ok(CameraStatus {
            camera: self.camera.clone(),
            powered_on: true,
        })
    }

    fn read_status(&self) -> Result<CameraStatus, LinkError> {
        if self.camera.connection == ConnectionState::Disconnected {
            return Err(LinkError::ConnectionLost);
        }
        Ok(CameraStatus {
            camera: self.camera.clone(),
            powered_on: true,
        })
    }

    fn read_package(&self) -> Result<PackageInfo, LinkError> {
        if self.camera.connection != ConnectionState::Connected {
            return Err(LinkError::NotNearby);
        }
        Ok(self.package.clone())
    }

    fn send_update(&mut self, kind: UpdateKind) -> Result<(), LinkError> {
        if self.camera.connection != ConnectionState::Connected {
            return Err(LinkError::NotNearby);
        }
        if self.flow.is_busy() {
            return Err(LinkError::Busy);
        }
        match kind {
            UpdateKind::Install if self.package.available.is_none() => {
                return Err(LinkError::Busy);
            }
            UpdateKind::Rollback if self.package.rollback.is_none() => {
                return Err(LinkError::Busy);
            }
            _ => {}
        }
        self.kind = Some(kind);
        self.send_tick = 0;
        self.wait_tick = 0;
        self.flow.reset();
        self.flow.begin_sending(kind, self.live_version.clone());
        Ok(())
    }

    fn poll_update(&mut self) -> Result<UpdatePoll, LinkError> {
        self.tick();
        Ok(match self.flow.phase().clone() {
            UpdatePhase::Sending { progress, .. } => UpdatePoll::Sending { progress },
            UpdatePhase::WaitingConfirm { .. } => UpdatePoll::WaitingConfirm,
            UpdatePhase::Confirmed { .. } | UpdatePhase::Restart { .. } => UpdatePoll::Confirmed,
            UpdatePhase::Installed { version, .. } => UpdatePoll::Installed { version },
            UpdatePhase::Failed(error) => UpdatePoll::Failed(error),
            UpdatePhase::Idle => UpdatePoll::Sending { progress: 0.0 },
        })
    }

    fn cancel(&mut self) -> Result<(), LinkError> {
        if matches!(
            self.flow.phase(),
            UpdatePhase::Installed { .. } | UpdatePhase::Idle
        ) {
            return Ok(());
        }
        // Cancel leaves the live version alone.
        self.flow.reset();
        self.kind = None;
        self.send_tick = 0;
        self.wait_tick = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected_demo() -> FakeCamera {
        let mut cam = FakeCamera::demo();
        cam.connect().unwrap();
        cam
    }

    #[test]
    fn happy_path_install() {
        let mut cam = connected_demo();
        let before = cam.live_version().to_owned();
        cam.send_update(UpdateKind::Install).unwrap();
        let mut saw_waiting = false;
        let mut installed = None;
        for _ in 0..40 {
            match cam.poll_update().unwrap() {
                UpdatePoll::WaitingConfirm => saw_waiting = true,
                UpdatePoll::Installed { version } => {
                    installed = Some(version);
                    break;
                }
                UpdatePoll::Failed(e) => panic!("unexpected {e:?}"),
                _ => {}
            }
        }
        assert!(saw_waiting);
        assert_eq!(installed.as_deref(), Some("1.7.0"));
        assert_eq!(cam.live_version(), "1.7.0");
        assert_ne!(cam.live_version(), before);
    }

    #[test]
    fn rollback_after_install() {
        let mut cam = connected_demo();
        cam.send_update(UpdateKind::Install).unwrap();
        for _ in 0..40 {
            if matches!(cam.poll_update().unwrap(), UpdatePoll::Installed { .. }) {
                break;
            }
        }
        cam.flow.reset();
        cam.send_update(UpdateKind::Rollback).unwrap();
        let mut version = None;
        for _ in 0..40 {
            if let UpdatePoll::Installed { version: v } = cam.poll_update().unwrap() {
                version = Some(v);
                break;
            }
        }
        assert_eq!(version.as_deref(), Some("1.6.1"));
        assert_eq!(cam.live_version(), "1.6.1");
    }

    #[test]
    fn card_full_leaves_package_unchanged() {
        let mut cam = FakeCamera::with_fail(DemoFail::CardFull);
        cam.connect().unwrap();
        let before = cam.live_version().to_owned();
        cam.send_update(UpdateKind::Install).unwrap();
        let mut err = None;
        for _ in 0..10 {
            if let UpdatePoll::Failed(e) = cam.poll_update().unwrap() {
                err = Some(e);
                break;
            }
        }
        assert_eq!(
            err,
            Some(UpdateError::CardFull {
                bytes_needed: 12 * 1024 * 1024
            })
        );
        assert_eq!(cam.live_version(), before);
    }

    #[test]
    fn refused_leaves_package_unchanged() {
        let mut cam = FakeCamera::with_fail(DemoFail::Refused);
        cam.connect().unwrap();
        let before = cam.live_version().to_owned();
        cam.send_update(UpdateKind::Install).unwrap();
        let mut err = None;
        for _ in 0..10 {
            if let UpdatePoll::Failed(e) = cam.poll_update().unwrap() {
                err = Some(e);
                break;
            }
        }
        assert_eq!(err, Some(UpdateError::Refused));
        assert_eq!(cam.live_version(), before);
    }

    #[test]
    fn not_confirmed_leaves_package_unchanged() {
        let mut cam = FakeCamera::with_fail(DemoFail::NotConfirmed);
        cam.connect().unwrap();
        let before = cam.live_version().to_owned();
        cam.send_update(UpdateKind::Install).unwrap();
        let mut err = None;
        for _ in 0..20 {
            if let UpdatePoll::Failed(e) = cam.poll_update().unwrap() {
                err = Some(e);
                break;
            }
        }
        assert_eq!(err, Some(UpdateError::NotConfirmed));
        assert_eq!(cam.live_version(), before);
    }

    #[test]
    fn connection_lost_leaves_package_unchanged() {
        let mut cam = FakeCamera::with_fail(DemoFail::ConnectionLost);
        cam.connect().unwrap();
        let before = cam.live_version().to_owned();
        cam.send_update(UpdateKind::Install).unwrap();
        let mut err = None;
        for _ in 0..10 {
            if let UpdatePoll::Failed(e) = cam.poll_update().unwrap() {
                err = Some(e);
                break;
            }
        }
        assert_eq!(err, Some(UpdateError::ConnectionLost));
        assert_eq!(cam.live_version(), before);
    }
}
