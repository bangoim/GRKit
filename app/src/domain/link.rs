//! The camera link the phone uses. Demo uses FakeCamera; iOS uses the real link.

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
    /// This build or device has no Bluetooth central.
    BluetoothUnavailable,
    /// System pairing failed or was cancelled.
    PairingFailed,
    /// The camera is not in pairing mode / not bonded for encrypted reads.
    NotPaired,
    /// Joining the camera access point failed or was denied.
    WifiJoinFailed,
    /// Authorization or credentials were rejected; do not probe or retry joining.
    WifiJoinRejected,
    /// HTTP 503 while the SD card is still mounting.
    CardNotReady,
    /// Camera has no GRKit service or an older version than 1.7.0.
    NeedsGrkit,
    /// Unexpected HTTP or transport failure.
    Http(String),
}

/// A camera seen while scanning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NearbyCamera {
    pub peripheral_id: String,
    pub model: CameraModel,
    pub powered_on: bool,
    /// Signal strength in dBm; absent when the peripheral was retrieved without an advertisement.
    pub rssi: Option<i32>,
}

/// Talks to a camera: connect, read package state, send or roll back an update.
pub trait CameraLink {
    fn nearby(&self) -> Option<CameraModel>;
    fn connect(&mut self) -> impl std::future::Future<Output = Result<CameraStatus, LinkError>>;
    fn read_status(&self) -> impl std::future::Future<Output = Result<CameraStatus, LinkError>>;
    fn read_package(&self) -> impl std::future::Future<Output = Result<PackageInfo, LinkError>>;
    fn send_update(
        &mut self,
        kind: UpdateKind,
    ) -> impl std::future::Future<Output = Result<(), LinkError>>;
    fn poll_update(&mut self) -> impl std::future::Future<Output = Result<UpdatePoll, LinkError>>;
    fn cancel(&mut self) -> impl std::future::Future<Output = Result<(), LinkError>>;
}

/// Poll a future that is known to be ready (demo / unit tests).
pub fn block_on_ready<T>(fut: impl std::future::Future<Output = T>) -> T {
    use std::task::{Context, Poll, Waker};
    let mut fut = std::pin::pin!(fut);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("future was not ready"),
    }
}
