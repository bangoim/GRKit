//! Connection checklist steps and a transport-driven state machine.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::domain::{CameraModel, LinkError};

/// Checklist steps shown on the connecting sheet (A4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectStep {
    Bluetooth = 0,
    Wifi = 1,
    Version = 2,
}

impl ConnectStep {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Wi-Fi credentials read over BLE after the access point is started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiCredentials {
    pub ssid: String,
    pub passphrase: String,
}

/// What BLE setup returns before the phone joins Wi-Fi.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleSetup {
    pub peripheral_id: String,
    pub model: CameraModel,
    pub powered_on_already: bool,
    pub credentials: WifiCredentials,
}

/// Timing constants used by the real link and tests.
pub mod timing {
    use super::Duration;

    pub const GATT_TIMEOUT: Duration = Duration::from_secs(15);
    pub const BLE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
    pub const BLE_CONNECT_ATTEMPTS: u32 = 3;
    pub const BLE_CONNECT_RETRY_DELAY: Duration = Duration::from_secs(2);
    pub const SCAN_CONNECT_SETTLE: Duration = Duration::from_millis(500);
    pub const NETWORK_TYPE_NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);
    pub const NETWORK_TYPE_NOTIFY_ATTEMPTS: u32 = 2;
    pub const NETWORK_TYPE_RETRY_DELAY: Duration = Duration::from_millis(500);
    pub const AP_WARMUP_GR3: Duration = Duration::from_millis(1_500);
    pub const AP_WARMUP_GR4: Duration = Duration::from_millis(8_000);

    pub fn ap_warmup(model: crate::domain::CameraModel) -> Duration {
        match model {
            crate::domain::CameraModel::Gr4 => AP_WARMUP_GR4,
            _ => AP_WARMUP_GR3,
        }
    }
}

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Platform or fake operations the connect machine drives.
pub trait ConnectTransport {
    fn ble_setup(&mut self, peripheral_id: &str) -> BoxFut<'_, Result<BleSetup, LinkError>>;
    fn join_wifi(&mut self, creds: &WifiCredentials) -> BoxFut<'_, Result<(), LinkError>>;
    fn wait_status_ready(&mut self) -> BoxFut<'_, Result<(), LinkError>>;
    fn read_version(&mut self, model: CameraModel) -> BoxFut<'_, Result<(), LinkError>>;
}

/// Run Bluetooth → Wi-Fi → version, reporting each step as it starts.
pub async fn run_connect<T: ConnectTransport>(
    transport: &mut T,
    peripheral_id: &str,
    mut on_step: impl FnMut(ConnectStep),
) -> Result<BleSetup, LinkError> {
    on_step(ConnectStep::Bluetooth);
    let setup = transport.ble_setup(peripheral_id).await?;

    on_step(ConnectStep::Wifi);
    // A native association failure is not proof the camera is unreachable.
    // iOS can report a transient error while its route is still settling.
    match transport.join_wifi(&setup.credentials).await {
        Ok(()) | Err(LinkError::WifiJoinFailed) => {}
        Err(error) => return Err(error),
    }
    transport.wait_status_ready().await?;

    on_step(ConnectStep::Version);
    transport.read_version(setup.model).await?;
    Ok(setup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeTransport {
        steps: RefCell<Vec<&'static str>>,
        fail_at: Option<&'static str>,
    }

    impl ConnectTransport for FakeTransport {
        fn ble_setup(&mut self, peripheral_id: &str) -> BoxFut<'_, Result<BleSetup, LinkError>> {
            let peripheral_id = peripheral_id.to_owned();
            Box::pin(async move {
                self.steps.borrow_mut().push("ble");
                if self.fail_at == Some("ble") {
                    return Err(LinkError::PairingFailed);
                }
                Ok(BleSetup {
                    peripheral_id,
                    model: CameraModel::Gr3,
                    powered_on_already: false,
                    credentials: WifiCredentials {
                        ssid: "GR_AP".into(),
                        passphrase: "secret".into(),
                    },
                })
            })
        }

        fn join_wifi(&mut self, _creds: &WifiCredentials) -> BoxFut<'_, Result<(), LinkError>> {
            Box::pin(async {
                self.steps.borrow_mut().push("wifi");
                if self.fail_at == Some("denied") {
                    return Err(LinkError::WifiJoinRejected);
                }
                if self.fail_at == Some("cancelled") {
                    return Err(LinkError::Cancelled);
                }
                if self.fail_at == Some("wifi") {
                    Err(LinkError::WifiJoinFailed)
                } else {
                    Ok(())
                }
            })
        }

        fn wait_status_ready(&mut self) -> BoxFut<'_, Result<(), LinkError>> {
            Box::pin(async {
                self.steps.borrow_mut().push("status");
                if self.fail_at == Some("status") {
                    Err(LinkError::CardNotReady)
                } else {
                    Ok(())
                }
            })
        }

        fn read_version(&mut self, _model: CameraModel) -> BoxFut<'_, Result<(), LinkError>> {
            Box::pin(async {
                self.steps.borrow_mut().push("version");
                if self.fail_at == Some("version") {
                    Err(LinkError::NeedsGrkit)
                } else {
                    Ok(())
                }
            })
        }
    }

    fn block_on<T>(fut: impl Future<Output = T>) -> T {
        use std::task::{Context, Poll, Waker};
        let mut fut = Box::pin(fut);
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(v) => return v,
                Poll::Pending => {}
            }
        }
    }

    #[test]
    fn happy_path_reports_steps_in_order() {
        let mut t = FakeTransport {
            steps: RefCell::new(Vec::new()),
            fail_at: None,
        };
        let mut seen = Vec::new();
        let setup = block_on(run_connect(&mut t, "id-1", |s| seen.push(s))).unwrap();
        assert_eq!(setup.credentials.ssid, "GR_AP");
        assert_eq!(
            seen,
            [
                ConnectStep::Bluetooth,
                ConnectStep::Wifi,
                ConnectStep::Version
            ]
        );
        assert_eq!(t.steps.into_inner(), ["ble", "wifi", "status", "version"]);
    }

    #[test]
    fn soft_wifi_failure_still_verifies_http() {
        let mut t = FakeTransport {
            steps: RefCell::new(Vec::new()),
            fail_at: Some("wifi"),
        };
        block_on(run_connect(&mut t, "id-1", |_| {})).unwrap();
        assert_eq!(t.steps.into_inner(), ["ble", "wifi", "status", "version"]);
    }
    #[test]
    fn rejected_or_cancelled_wifi_does_not_probe_http() {
        for (failure, expected) in [
            ("denied", LinkError::WifiJoinRejected),
            ("cancelled", LinkError::Cancelled),
        ] {
            let mut transport = FakeTransport {
                steps: RefCell::new(Vec::new()),
                fail_at: Some(failure),
            };
            assert_eq!(
                block_on(run_connect(&mut transport, "id", |_| {})),
                Err(expected)
            );
            assert_eq!(transport.steps.into_inner(), ["ble", "wifi"]);
        }
    }
}
