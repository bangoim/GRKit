//! Orchestrate BLE setup, Wi-Fi join, and HTTP status/package reads via `run_connect`.

use std::future::Future;
use std::pin::Pin;

use crate::domain::{
    BleSetup, CameraModel, CameraStatus, ConnectStep, ConnectTransport, LinkError, PackageInfo,
    RememberedCamera, WifiCredentials, run_connect,
};

use super::{ble, remember, wifi};
use crate::platform::http;

struct LinkAttempt {
    peripheral_id: String,
    succeeded: bool,
}
impl Drop for LinkAttempt {
    fn drop(&mut self) {
        if !self.succeeded {
            ble::cancel_connection(&self.peripheral_id);
        }
        ble::resume_scanning();
    }
}

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

struct IosTransport {
    model_hint: CameraModel,
    package: Option<(CameraStatus, PackageInfo)>,
}

impl ConnectTransport for IosTransport {
    fn ble_setup(&mut self, peripheral_id: &str) -> BoxFut<'_, Result<BleSetup, LinkError>> {
        let model_hint = self.model_hint;
        let peripheral_id = peripheral_id.to_owned();
        Box::pin(async move {
            app_info!("connect step Bluetooth");
            ble::ble_setup(&peripheral_id, model_hint).await
        })
    }

    fn join_wifi(&mut self, creds: &WifiCredentials) -> BoxFut<'_, Result<(), LinkError>> {
        let creds = creds.clone();
        Box::pin(async move {
            app_info!("connect step Wi-Fi");
            wifi::join(&creds).await
        })
    }

    fn wait_status_ready(&mut self) -> BoxFut<'_, Result<(), LinkError>> {
        Box::pin(async {
            app_info!("connect waiting for HTTP status");
            http::wait_until_ready().await
        })
    }

    fn read_version(&mut self, model: CameraModel) -> BoxFut<'_, Result<(), LinkError>> {
        Box::pin(async move {
            app_info!("connect step Version");
            let (status, package) = http::read_package(model).await?;
            self.package = Some((status, package));
            Ok(())
        })
    }
}

/// Connect to a nearby camera and return status, package, and remembered snapshot.
pub async fn connect(
    peripheral_id: &str,
    model_hint: CameraModel,
    on_step: &mut dyn FnMut(ConnectStep),
) -> Result<(CameraStatus, PackageInfo, RememberedCamera), LinkError> {
    app_info!("link connect {}", short_id(peripheral_id));
    let mut attempt = LinkAttempt {
        peripheral_id: peripheral_id.to_owned(),
        succeeded: false,
    };
    let mut transport = IosTransport {
        model_hint,
        package: None,
    };
    let result = run_connect_and_package(&mut transport, peripheral_id, on_step).await;
    attempt.succeeded = result.is_ok();
    match &result {
        Ok(_) => app_info!("connect ok {}", short_id(peripheral_id)),
        Err(e) => log::warn!("connect failed {}: {e:?}", short_id(peripheral_id)),
    }
    result
}

async fn run_connect_and_package(
    transport: &mut IosTransport,
    peripheral_id: &str,
    on_step: &mut dyn FnMut(ConnectStep),
) -> Result<(CameraStatus, PackageInfo, RememberedCamera), LinkError> {
    let setup = run_connect(transport, peripheral_id, |step| on_step(step)).await?;
    let (status, package) = transport
        .package
        .take()
        .ok_or(LinkError::Http("missing package after connect".into()))?;
    let remembered = RememberedCamera::new(
        setup.peripheral_id,
        status.camera.model,
        setup.credentials.ssid.clone(),
    );
    if let Err(e) = remember::save(&remembered) {
        log::warn!("failed to remember camera: {e}");
    }
    Ok((status, package, remembered))
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}
