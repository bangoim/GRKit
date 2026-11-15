//! Nearby prompts follow power cycles and proximity, rather than scan order.

use super::NearbyCamera;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default)]
pub struct NearbyPromptGate {
    dismissed: HashSet<String>,
    near: HashMap<String, bool>,
}

impl NearbyPromptGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, cameras: &[NearbyCamera]) {
        for camera in cameras {
            let id = camera.peripheral_id.to_ascii_lowercase();
            if !camera.powered_on {
                self.dismissed.remove(&id);
            }
            let near = self.near.entry(id).or_default();
            if let Some(rssi) = camera.rssi {
                if rssi >= -60 {
                    *near = true;
                } else if rssi < -70 {
                    *near = false;
                }
            }
        }
    }

    /// Saved powered-on cameras win; unknown cameras must be close and unique.
    pub fn candidate(
        &self,
        cameras: &[NearbyCamera],
        remembered_id: Option<&str>,
    ) -> Option<NearbyCamera> {
        let eligible = |camera: &&NearbyCamera| {
            camera.powered_on && self.should_auto_show(&camera.peripheral_id)
        };
        if let Some(saved) = remembered_id
            && let Some(camera) = cameras
                .iter()
                .filter(eligible)
                .find(|camera| camera.peripheral_id.eq_ignore_ascii_case(saved))
        {
            return Some(camera.clone());
        }
        let mut candidates = cameras.iter().filter(eligible).filter(|camera| {
            self.near
                .get(&camera.peripheral_id.to_ascii_lowercase())
                .copied()
                .unwrap_or(false)
        });
        let candidate = candidates.next()?;
        if candidates.next().is_some() {
            None
        } else {
            Some(candidate.clone())
        }
    }

    pub fn should_auto_show(&self, peripheral_id: &str) -> bool {
        !self.dismissed.contains(&peripheral_id.to_ascii_lowercase())
    }

    pub fn dismiss(&mut self, peripheral_id: &str) {
        self.dismissed.insert(peripheral_id.to_ascii_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::CameraModel;

    fn camera(id: &str, powered_on: bool, rssi: i32) -> NearbyCamera {
        NearbyCamera {
            peripheral_id: id.into(),
            model: CameraModel::Gr3,
            powered_on,
            rssi: Some(rssi),
        }
    }

    #[test]
    fn ignores_off_far_and_ambiguous_unknown_cameras() {
        let mut gate = NearbyPromptGate::new();
        let cameras = vec![camera("off", false, -40), camera("far", true, -80)];
        gate.observe(&cameras);
        assert!(gate.candidate(&cameras, None).is_none());
        let cameras = vec![camera("one", true, -40), camera("two", true, -50)];
        gate.observe(&cameras);
        assert!(gate.candidate(&cameras, None).is_none());
        assert_eq!(
            gate.candidate(&cameras, Some("TWO")).unwrap().peripheral_id,
            "two"
        );
    }

    #[test]
    fn remembers_near_threshold_until_camera_moves_away() {
        let mut gate = NearbyPromptGate::new();
        for (rssi, expected) in [(-65, false), (-60, true), (-65, true), (-71, false)] {
            let cameras = vec![camera("one", true, rssi)];
            gate.observe(&cameras);
            assert_eq!(gate.candidate(&cameras, None).is_some(), expected);
        }
    }

    #[test]
    fn dismissal_survives_rescans_but_resets_after_power_off() {
        let mut gate = NearbyPromptGate::new();
        gate.dismiss("CAMERA");
        gate.observe(&[camera("camera", true, -40)]);
        assert!(!gate.should_auto_show("camera"));
        gate.observe(&[]);
        assert!(!gate.should_auto_show("camera"));
        gate.observe(&[camera("camera", false, -40)]);
        assert!(
            gate.candidate(&[camera("camera", false, -40)], None)
                .is_none()
        );
        assert!(
            gate.candidate(&[camera("camera", true, -40)], None)
                .is_some()
        );
    }
}
