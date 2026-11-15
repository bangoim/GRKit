//! Non-iOS placeholder: Bluetooth is not available in this build.

use crate::domain::{CameraStatus, LinkError, NearbyCamera, PackageInfo, RememberedCamera};

pub fn load_remembered() -> Option<RememberedCamera> {
    None
}

#[allow(dead_code)]
pub fn save_remembered(_camera: &RememberedCamera) -> Result<(), String> {
    Ok(())
}

pub fn start_scanning(tx: async_channel::Sender<Vec<NearbyCamera>>, _remembered_id: Option<&str>) {
    let _ = tx.try_send(Vec::new());
}

#[allow(dead_code)]
pub fn stop_scanning() {}

pub fn watch_disconnects(_tx: async_channel::Sender<String>) {}

pub async fn connect_camera() -> Result<(CameraStatus, PackageInfo, RememberedCamera), LinkError> {
    Err(LinkError::BluetoothUnavailable)
}
