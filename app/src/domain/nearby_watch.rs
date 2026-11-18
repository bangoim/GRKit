//! Powered-on cameras nearby, built frame by frame from advertisements, and
//! which one may be offered in the nearby prompt. Sightings expire, a camera
//! that powers off leaves at once,
//! and a camera prompted for or just used is not offered again during its
//! current power-on.

use std::collections::{HashMap, HashSet};

use super::{CameraModel, NearbyCamera};

/// A sighting with no frame for this long is gone.
pub const STALE_SIGHTING_MS: u64 = 6_000;
/// How often sightings are pruned.
pub const PRUNE_INTERVAL_MS: u64 = 2_000;
/// A camera unseen this long starts a new power-on as far as the prompt goes.
pub const REARM_AFTER_UNSEEN_MS: u64 = 10 * 60_000;
/// Proximity enters at this signal strength and leaves below the next.
const NEAR_RSSI_DBM: i32 = -60;
const FAR_RSSI_DBM: i32 = -70;

/// What one advertisement says about camera power.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvertisedPower {
    On,
    Standby,
    /// The frame does not say; it is ignored.
    Unknown,
}

/// "On" needs both signals: boot and shutdown each advertise one of them
/// about a second ahead of the other.
pub fn advertised_power(powered_on: Option<bool>, camera_information: bool) -> AdvertisedPower {
    match powered_on {
        Some(false) => AdvertisedPower::Standby,
        Some(true) if camera_information => AdvertisedPower::On,
        _ => AdvertisedPower::Unknown,
    }
}

/// One advertisement frame from a camera.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub peripheral_id: String,
    pub model: CameraModel,
    pub power: AdvertisedPower,
    pub rssi: Option<i32>,
}

#[derive(Clone, Debug, Default)]
pub struct NearbyWatch {
    sightings: Vec<NearbyCamera>,
    last_seen: HashMap<String, u64>,
    suppressed: HashSet<String>,
    near: HashMap<String, bool>,
}

fn key(id: &str) -> String {
    id.to_ascii_lowercase()
}

impl NearbyWatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sightings(&self) -> &[NearbyCamera] {
        &self.sightings
    }

    /// Apply one frame. Returns whether the sightings changed.
    pub fn frame(&mut self, frame: &Frame, now_ms: u64) -> bool {
        let id = key(&frame.peripheral_id);
        match frame.power {
            AdvertisedPower::Unknown => return false,
            AdvertisedPower::Standby => {
                self.last_seen.remove(&id);
                self.suppressed.remove(&id);
                let before = self.sightings.len();
                self.sightings.retain(|c| key(&c.peripheral_id) != id);
                return self.sightings.len() != before;
            }
            AdvertisedPower::On => {}
        }
        if self.suppressed.contains(&id)
            && self
                .last_seen
                .get(&id)
                .is_some_and(|seen| now_ms.saturating_sub(*seen) > REARM_AFTER_UNSEEN_MS)
        {
            self.suppressed.remove(&id);
        }
        self.last_seen.insert(id.clone(), now_ms);
        let near = self.near.entry(id.clone()).or_default();
        if let Some(rssi) = frame.rssi {
            if rssi >= NEAR_RSSI_DBM {
                *near = true;
            } else if rssi < FAR_RSSI_DBM {
                *near = false;
            }
        }
        let next = NearbyCamera {
            peripheral_id: frame.peripheral_id.clone(),
            model: frame.model,
            powered_on: true,
            rssi: frame.rssi,
        };
        match self
            .sightings
            .iter_mut()
            .find(|c| key(&c.peripheral_id) == id)
        {
            Some(existing) if *existing == next => false,
            Some(existing) => {
                *existing = next;
                true
            }
            None => {
                self.sightings.push(next);
                true
            }
        }
    }

    /// Drop sightings with no recent frame. Returns whether any left.
    pub fn prune(&mut self, now_ms: u64) -> bool {
        let before = self.sightings.len();
        let last_seen = &self.last_seen;
        self.sightings.retain(|c| {
            last_seen
                .get(&key(&c.peripheral_id))
                .is_some_and(|seen| now_ms.saturating_sub(*seen) < STALE_SIGHTING_MS)
        });
        self.sightings.len() != before
    }

    /// The scan paused: nothing is known to be nearby.
    pub fn clear(&mut self) {
        self.sightings.clear();
    }

    /// Keep these cameras out of the prompt for their current power-on. The
    /// unseen allowance starts now.
    pub fn suppress(&mut self, peripheral_id: &str, now_ms: u64) {
        let id = key(peripheral_id);
        self.last_seen.insert(id.clone(), now_ms);
        self.suppressed.insert(id);
    }

    pub fn may_prompt(&self, peripheral_id: &str) -> bool {
        !self.suppressed.contains(&key(peripheral_id))
    }

    /// The camera to offer: a saved one first; an unknown one must be near
    /// and the only candidate.
    pub fn candidate(&self, remembered_id: Option<&str>) -> Option<NearbyCamera> {
        let eligible: Vec<&NearbyCamera> = self
            .sightings
            .iter()
            .filter(|c| self.may_prompt(&c.peripheral_id))
            .collect();
        if let Some(saved) = remembered_id
            && let Some(camera) = eligible
                .iter()
                .find(|c| c.peripheral_id.eq_ignore_ascii_case(saved))
        {
            return Some((*camera).clone());
        }
        let mut near = eligible.into_iter().filter(|c| {
            self.near
                .get(&key(&c.peripheral_id))
                .copied()
                .unwrap_or(false)
        });
        let first = near.next()?;
        near.next().is_none().then(|| first.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: &str, power: AdvertisedPower, rssi: i32) -> Frame {
        Frame {
            peripheral_id: id.into(),
            model: CameraModel::Gr3,
            power,
            rssi: Some(rssi),
        }
    }

    #[test]
    fn power_needs_both_signals() {
        assert_eq!(advertised_power(Some(true), true), AdvertisedPower::On);
        assert_eq!(
            advertised_power(Some(true), false),
            AdvertisedPower::Unknown
        );
        assert_eq!(advertised_power(None, true), AdvertisedPower::Unknown);
        assert_eq!(
            advertised_power(Some(false), true),
            AdvertisedPower::Standby
        );
    }

    #[test]
    fn sightings_expire_and_standby_removes_them() {
        let mut watch = NearbyWatch::new();
        assert!(watch.frame(&frame("a", AdvertisedPower::On, -40), 0));
        assert!(!watch.frame(&frame("a", AdvertisedPower::Unknown, -40), 1_000));
        assert!(!watch.prune(5_000));
        assert!(watch.prune(6_000));
        assert!(watch.sightings().is_empty());
        watch.frame(&frame("a", AdvertisedPower::On, -40), 7_000);
        assert!(watch.frame(&frame("a", AdvertisedPower::Standby, -40), 7_100));
        assert!(watch.sightings().is_empty());
    }

    #[test]
    fn suppression_lasts_for_the_power_on() {
        let mut watch = NearbyWatch::new();
        watch.frame(&frame("a", AdvertisedPower::On, -40), 0);
        watch.suppress("A", 0);
        assert!(watch.candidate(None).is_none());
        // Still on, seen within ten minutes: still suppressed.
        watch.frame(&frame("a", AdvertisedPower::On, -40), 60_000);
        assert!(watch.candidate(None).is_none());
        // Unseen for more than ten minutes: a new power-on.
        watch.frame(
            &frame("a", AdvertisedPower::On, -40),
            60_000 + REARM_AFTER_UNSEEN_MS + 1,
        );
        assert!(watch.candidate(None).is_some());
        // A standby frame ends the power-on at once.
        watch.suppress("a", 0);
        watch.frame(&frame("a", AdvertisedPower::Standby, -40), 1);
        watch.frame(&frame("a", AdvertisedPower::On, -40), 2);
        assert!(watch.candidate(None).is_some());
    }

    #[test]
    fn saved_cameras_win_and_unknown_ones_must_be_near_and_unique() {
        let mut watch = NearbyWatch::new();
        watch.frame(&frame("one", AdvertisedPower::On, -40), 0);
        watch.frame(&frame("two", AdvertisedPower::On, -50), 0);
        assert!(watch.candidate(None).is_none());
        assert_eq!(watch.candidate(Some("TWO")).unwrap().peripheral_id, "two");
        let mut watch = NearbyWatch::new();
        for (rssi, expected) in [(-65, false), (-60, true), (-65, true), (-71, false)] {
            watch.frame(&frame("one", AdvertisedPower::On, rssi), 0);
            assert_eq!(watch.candidate(None).is_some(), expected);
        }
    }
}
