//! Persist the last camera the phone successfully joined.

use serde::{Deserialize, Serialize};

use crate::domain::CameraModel;

/// Saved after a successful connect so the app can reconnect later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberedCamera {
    pub peripheral_id: String,
    pub model: CameraModel,
    pub last_ssid: String,
}

impl RememberedCamera {
    pub fn new(
        peripheral_id: impl Into<String>,
        model: CameraModel,
        last_ssid: impl Into<String>,
    ) -> Self {
        Self {
            peripheral_id: peripheral_id.into(),
            model,
            last_ssid: last_ssid.into(),
        }
    }
}

/// Serialize for a small file in the app data directory.
pub fn encode(camera: &RememberedCamera) -> Result<String, String> {
    serde_json::to_string_pretty(camera).map_err(|e| e.to_string())
}

/// Load from the file contents.
pub fn decode(text: &str) -> Result<RememberedCamera, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let cam = RememberedCamera::new("UUID-1", CameraModel::Gr4, "GR_AP");
        let text = encode(&cam).unwrap();
        assert_eq!(decode(&text).unwrap(), cam);
        assert!(text.contains("\"gr4\""));
    }
}
