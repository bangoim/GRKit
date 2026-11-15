//! Real update orchestration. The camera journal is authoritative across reconnects.
use super::*;
use crate::domain::{
    JournalState, UpdateIntent, UpdateObservation, observe_update, package_to_models,
};

impl AppState {
    pub fn release_checking(self) -> bool {
        self.release_busy.get()
    }
    pub fn release_error(self) -> Option<String> {
        self.release_error.get()
    }
    pub fn has_pending_update(self) -> bool {
        platform::update::load_intent()
            .is_some_and(|i| self.camera_id().as_deref() == Some(&i.camera_id))
    }
    fn camera_id(self) -> Option<String> {
        self.selected_id
            .get()
            .or_else(|| self.remembered.get().map(|r| r.peripheral_id))
    }

    pub fn refresh_update_info(self) {
        if self.demo() || self.release_busy.get() || self.update_busy.get() {
            return;
        }
        let Some(status) = self.status.get() else {
            return;
        };
        let generation = self.connect_generation.get();
        self.release_busy.set(true);
        self.release_error.set(None);
        self.root.run(|| {
            spawn(async move {
                let snapshot = platform::update::snapshot().await;
                if self.connect_generation.get() == generation
                    && let Ok(snapshot) = snapshot
                {
                    self.package.update(|p| {
                        if let Some(p) = p {
                            p.rollback = snapshot.rollback.and_then(|r| r.version);
                        }
                    });
                }
                let release =
                    platform::release::latest(status.camera.model, &status.camera.firmware).await;
                if self.connect_generation.get() == generation {
                    match release {
                        Ok(release) => {
                            log::info!(
                                "Release {} transfer estimate {} bytes",
                                release.version,
                                release.bytes
                            );
                            self.package.update(|p| {
                                if let Some(p) = p {
                                    p.available = Some(release.version.clone());
                                    // Transfer estimates are not the final archive's size.
                                    p.package_bytes = 0;
                                }
                            });
                            self.release.with(|slot| *slot.borrow_mut() = Some(release));
                        }
                        Err(message) => self.release_error.set(Some(message)),
                    }
                }
                self.release_busy.set(false);
            })
        });
    }

    fn fail_update(self, error: UpdateError) {
        self.phase.set(UpdatePhase::Failed(error.clone()));
        self.sheet.set(Some(Sheet::Error(error)));
    }

    pub(super) fn begin_real_update(self, kind: UpdateKind) {
        let Some(package) = self.package.get() else {
            return;
        };
        let Some(camera_id) = self.camera_id() else {
            return;
        };
        if self
            .status
            .get()
            .is_none_or(|s| s.camera.connection != ConnectionState::Connected)
        {
            return;
        }
        if platform::update::load_intent().is_some() {
            self.fail_update(UpdateError::Service("A previous update needs to be checked before starting another one. Reconnect to that camera first.".into()));
            return;
        }
        let target = match kind {
            UpdateKind::Install => package.available,
            UpdateKind::Rollback => package.rollback,
        };
        let Some(target) = target else {
            self.fail_update(UpdateError::Download("Update information is unavailable. Check for updates while connected to the internet.".into()));
            return;
        };
        let intent = UpdateIntent {
            camera_id,
            kind,
            baseline: package.installed,
            target,
        };
        self.update_busy.set(true);
        self.update_cancel.set(false);
        self.baseline.set(Some(intent.baseline.clone()));
        self.phase.set(UpdatePhase::Sending {
            progress: -1.0,
            kind,
        });
        self.sheet.set(Some(Sheet::Sending));
        self.root.run(|| {
            spawn(async move {
                let result = self.prepare_and_stage(&intent).await;
                match result {
                    Ok(()) => self.monitor_update(intent).await,
                    Err(_) if self.update_cancel.get() && !self.has_pending_update() => {
                        self.dismiss_error()
                    }
                    Err(error) => self.fail_update(error),
                }
                self.update_busy.set(false);
            })
        });
    }

    async fn prepare_and_stage(self, intent: &UpdateIntent) -> Result<(), UpdateError> {
        let state = platform::update::snapshot().await?;
        if state.state != JournalState::None {
            return Err(UpdateError::Service("The camera already has a pending update. Confirm or cancel it on the camera first.".into()));
        }
        if intent.kind == UpdateKind::Rollback
            && state.rollback.and_then(|r| r.version).as_deref() != Some(&intent.target)
        {
            return Err(UpdateError::Service(
                "The rollback version changed. Refresh before trying again.".into(),
            ));
        }
        let before = platform::update::package().await?;
        if before.version.as_deref() != Some(&intent.baseline) {
            return Err(UpdateError::Service(
                "The camera version changed. Reconnect before updating.".into(),
            ));
        }
        let body = if intent.kind == UpdateKind::Install {
            let release = self
                .release
                .with(|slot| slot.borrow().clone())
                .ok_or_else(|| {
                    UpdateError::Download(
                        "Check for updates while connected to the internet, then try again.".into(),
                    )
                })?;
            if release.version != intent.target {
                return Err(UpdateError::Download(
                    "The available release changed. Check for updates and try again.".into(),
                ));
            }
            let cancelled = async {
                while !self.update_cancel.get() {
                    timer(Duration::from_millis(100)).await;
                }
            };
            match futures_util::future::select(
                Box::pin(platform::release::download(&release, &before, |_| {})),
                Box::pin(cancelled),
            )
            .await
            {
                futures_util::future::Either::Left((result, _)) => {
                    result.map_err(UpdateError::Download)?
                }
                futures_util::future::Either::Right(_) => {
                    self.dismiss_error();
                    return Ok(());
                }
            }
        } else {
            Vec::new()
        };
        if self.update_cancel.get() {
            self.dismiss_error();
            return Ok(());
        }
        // Recheck after downloading. Do not overwrite changes made by another client.
        let fresh = platform::update::snapshot().await?;
        if platform::update::package().await? != before
            || fresh.state != JournalState::None
            || (intent.kind == UpdateKind::Rollback
                && fresh.rollback.and_then(|r| r.version).as_deref() != Some(&intent.target))
        {
            return Err(UpdateError::Service(
                "The camera package changed while preparing the update. Reconnect and try again."
                    .into(),
            ));
        }
        if self.update_cancel.get() {
            self.dismiss_error();
            return Ok(());
        }
        platform::update::save_intent(intent)?;
        // Do not abort an in-flight PUT: cancellation must run after it settles.
        let staged = platform::update::stage(intent.kind, body).await;
        if self.update_cancel.get() {
            platform::update::cancel().await?;
            platform::update::clear_intent();
            self.dismiss_error();
            return Ok(());
        }
        if let Err(error) = staged {
            // Only a fresh camera response can resolve an uncertain mutation.
            if platform::update::snapshot()
                .await
                .is_ok_and(|s| s.state == JournalState::None)
            {
                platform::update::clear_intent();
            }
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn resume_update(self) {
        if self.demo() || self.update_busy.get() {
            return;
        }
        let Some(intent) = platform::update::load_intent() else {
            return;
        };
        if self.camera_id().as_deref() != Some(&intent.camera_id) {
            return;
        }
        self.update_kind.set(intent.kind);
        self.baseline.set(Some(intent.baseline.clone()));
        self.update_busy.set(true);
        self.update_cancel.set(false);
        self.root.run(|| {
            spawn(async move {
                self.monitor_update(intent).await;
                self.update_busy.set(false);
            })
        });
    }

    async fn monitor_update(self, intent: UpdateIntent) {
        if platform::update::load_intent().is_none() {
            return;
        }
        let started = Instant::now();
        let mut failures = 0;
        loop {
            if self.update_cancel.get() {
                match platform::update::cancel().await {
                    Ok(()) => {
                        platform::update::clear_intent();
                        self.dismiss_error();
                        self.refresh_camera_package().await;
                    }
                    Err(error) => self.fail_update(error),
                }
                return;
            }
            if self.update_reconnect.get() {
                self.update_reconnect.set(false);
                let model = self
                    .status
                    .get()
                    .map(|s| s.camera.model)
                    .unwrap_or(CameraModel::Gr3);
                match platform::connect_camera(&intent.camera_id, model, |_| {}).await {
                    Ok((status, _, _)) => self.status.set(Some(status)),
                    Err(_) => {
                        self.fail_update(UpdateError::ConnectionLost);
                        return;
                    }
                }
            }
            let result = async {
                let state = platform::update::snapshot().await?;
                let package = platform::update::package().await?;
                Ok::<_, UpdateError>((state, package))
            }
            .await;
            match result {
                Ok((state, package)) => {
                    failures = 0;
                    let version = package.version.as_deref().unwrap_or_default();
                    match observe_update(state.state, version, &intent.target) {
                        UpdateObservation::Waiting => {
                            let deadline = *self
                                .confirm_deadline
                                .get()
                                .get_or_insert_with(|| Instant::now() + Duration::from_secs(300));
                            self.confirm_deadline.set(Some(deadline));
                            self.phase.set(UpdatePhase::WaitingConfirm {
                                kind: intent.kind,
                                deadline,
                            });
                            self.sheet.set(Some(Sheet::Confirm));
                        }
                        UpdateObservation::Restart => {
                            self.phase.set(UpdatePhase::Restart { kind: intent.kind });
                            self.sheet.set(Some(Sheet::Restart));
                        }
                        UpdateObservation::Installed => {
                            self.refresh_camera_package().await;
                            self.phase.set(UpdatePhase::Installed {
                                kind: intent.kind,
                                version: intent.target.clone(),
                            });
                            self.sheet.set(Some(Sheet::Installed {
                                version: intent.target.clone(),
                                previous: intent.baseline.clone(),
                            }));
                            self.confirm_deadline.set(None);
                            platform::update::clear_intent();
                            return;
                        }
                        UpdateObservation::Stopped => {
                            platform::update::clear_intent();
                            self.fail_update(UpdateError::NotConfirmed);
                            return;
                        }
                    }
                }
                Err(_) => {
                    failures += 1;
                    if failures >= 3 && !matches!(self.flow_phase(), UpdatePhase::Restart { .. }) {
                        self.fail_update(UpdateError::ConnectionLost);
                        return;
                    }
                }
            }
            // Leave the intent for a later reconnect; never infer success or delete on timeout.
            if started.elapsed() > Duration::from_secs(600) {
                self.fail_update(UpdateError::ConnectionLost);
                return;
            }
            timer(Duration::from_secs(1)).await;
        }
    }

    async fn refresh_camera_package(self) {
        let model = self
            .status
            .get()
            .map(|s| s.camera.model)
            .unwrap_or(CameraModel::Gr3);
        if let Ok(raw) = platform::update::package().await
            && let Ok((camera, mut package)) = package_to_models(&raw, model)
        {
            package.available = self
                .release
                .with(|slot| slot.borrow().as_ref().map(|r| r.version.clone()));
            if let Ok(state) = platform::update::snapshot().await {
                package.rollback = state.rollback.and_then(|r| r.version);
            }
            self.status.set(Some(CameraStatus {
                camera,
                powered_on: true,
            }));
            self.package.set(Some(package));
        }
    }

    pub(super) fn cancel_real_update(self) {
        self.update_cancel.set(true);
        if !self.update_busy.get() {
            self.resume_update();
            self.update_cancel.set(true);
        }
    }
}
