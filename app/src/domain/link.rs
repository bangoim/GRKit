//! The camera link the phone uses. Real Wi-Fi comes later; demo uses FakeCamera.

use crate::domain::{Camera, CameraModel, PackageInfo, UpdateError, UpdateKind};

/// Progress while a package is copied to the camera, 0..=1.
pub type SendProgress = f32;

/// What polling the camera reports during an update.
#[derive(Clone, Debug, PartialEq)]
pub enum UpdatePoll {
    Sending { progress: f32 },
    WaitingConfirm,
    Confirmed,
    Installed { version: String },
    Failed(UpdateError),
}

/// Status returned while connecting or when already connected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraStatus {
    pub camera: Camera,
    pub powered_on: bool,
}

/// Errors from the link itself (before an update flow starts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    NotNearby,
    Busy,
    Cancelled,
    ConnectionLost,
}

/// Talks to a camera: connect, read package state, send or roll back an update.
pub trait CameraLink {
    fn nearby(&self) -> Option<CameraModel>;
    fn connect(&mut self) -> Result<CameraStatus, LinkError>;
    fn read_status(&self) -> Result<CameraStatus, LinkError>;
    fn read_package(&self) -> Result<PackageInfo, LinkError>;
    fn send_update(&mut self, kind: UpdateKind) -> Result<(), LinkError>;
    fn poll_update(&mut self) -> Result<UpdatePoll, LinkError>;
    fn cancel(&mut self) -> Result<(), LinkError>;
}
