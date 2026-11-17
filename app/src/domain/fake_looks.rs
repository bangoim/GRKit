//! In-memory Looks of the demo camera. Follows the camera service rules that
//! the UI depends on: official Looks are read-only, a Look in use needs Off
//! before deletion, recipes block deletion, and loading is capped.

use super::look_media;
use super::looks::{CatalogLook, Look, LookError, LookSource, LooksSnapshot, valid_name};

const PORTRA: &str = include_str!("../../../effects/portra-400-endura-v1/effect.json");
const KODACHROME: &str = include_str!("../../../effects/kodachrome-64-v1/effect.json");
const TRIX: &str = include_str!("../../../effects/trix-400-mono-v1/effect.json");
const ETERNA: &str = include_str!("../../../effects/eterna-500t-3523xd-v1/effect.json");
const VELVIA: &str = include_str!("../../../effects/velvia-100-v1/effect.json");
const GOLD: &str = include_str!("../../../effects/gold-200-premier-v1/effect.json");
const PROVIA: &str = include_str!("../../../effects/provia-100f-v1/effect.json");
const EKTAR: &str = include_str!("../../../effects/ektar-100-edge-v1/effect.json");
const HP5: &str = include_str!("../../../effects/hp5-plus-400-mono-v1/effect.json");
const INSTAX: &str = include_str!("../../../effects/instax-v1/effect.json");

/// The demo's imported Look.
pub const DEMO_CUSTOM_ID: &str = "lut-my-lut";

#[derive(Clone, Debug)]
struct Entry {
    id: String,
    name: String,
    enabled: bool,
    source: LookSource,
    grf: Vec<u8>,
    dependencies: Vec<String>,
}

/// Demo camera Looks.
#[derive(Clone, Debug)]
pub struct FakeLooks {
    entries: Vec<Entry>,
    selected: Option<String>,
    pub max_loaded: usize,
    pub max_listed: usize,
    /// A software update waits on the camera: changes are refused.
    pub update_pending: bool,
}

fn entry(definition: &str, enabled: bool) -> Entry {
    let grf = look_media::compile(definition).expect("bundled effect compiles");
    let (id, name, _) = look_media::label(&grf).expect("bundled effect decodes");
    Entry {
        id,
        name,
        enabled,
        source: LookSource::Official,
        grf,
        dependencies: Vec::new(),
    }
}

/// The official Looks the demo catalog offers, with their GRF files.
pub fn demo_catalog() -> Vec<(CatalogLook, Vec<u8>)> {
    [
        PORTRA, KODACHROME, TRIX, ETERNA, VELVIA, GOLD, PROVIA, EKTAR, HP5, INSTAX,
    ]
    .iter()
    .map(|definition| {
        let grf = look_media::compile(definition).expect("bundled effect compiles");
        let (id, name, monochrome) = look_media::label(&grf).expect("bundled effect decodes");
        let file = grkit_core::effects::filename(&id).expect("valid id");
        let look = CatalogLook {
            id,
            name,
            monochrome,
            file,
            sha256: grkit_core::packaging::sha256(&grf),
            cover: None,
        };
        (look, grf)
    })
    .collect()
}

impl FakeLooks {
    /// Portra 400 in use and used by two recipes, an imported Look, and
    /// Velvia 100 turned off.
    pub fn demo() -> Self {
        let mut portra = entry(PORTRA, true);
        portra.dependencies = vec!["recipe:1".into(), "recipe:2".into()];
        let gold = entry(GOLD, true);
        let custom = Entry {
            id: DEMO_CUSTOM_ID.into(),
            name: "My LUT".into(),
            enabled: true,
            source: LookSource::Custom,
            grf: look_media::copy_as(&gold.grf, DEMO_CUSTOM_ID, "My LUT").expect("copy"),
            dependencies: Vec::new(),
        };
        Self {
            entries: vec![
                portra,
                entry(KODACHROME, true),
                entry(TRIX, true),
                custom,
                entry(ETERNA, true),
                entry(VELVIA, false),
            ],
            selected: Some("portra-400-endura-v1".into()),
            max_loaded: 40,
            max_listed: 64,
            update_pending: false,
        }
    }

    pub fn snapshot(&self) -> LooksSnapshot {
        let mut loaded = 0;
        let looks = self
            .entries
            .iter()
            .enumerate()
            .map(|(position, e)| {
                let is_loaded = e.enabled && loaded < self.max_loaded;
                if is_loaded {
                    loaded += 1;
                }
                Look {
                    id: e.id.clone(),
                    name: e.name.clone(),
                    content_hash: grkit_core::packaging::sha256(&e.grf),
                    enabled: e.enabled,
                    position,
                    source: e.source,
                    loaded: is_loaded,
                    selected: self.selected.as_deref() == Some(&e.id),
                    dependencies: e.dependencies.clone(),
                    dependencies_complete: true,
                    blocked: false,
                }
            })
            .collect();
        LooksSnapshot {
            looks,
            loaded,
            max_loaded: self.max_loaded,
            max_listed: self.max_listed,
            boot_id: "demo-boot".into(),
            card_id: "demo-card".into(),
        }
    }

    /// Set the recipes and dial sets that use a Look.
    pub fn set_dependencies(&mut self, id: &str, dependencies: Vec<String>) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.dependencies = dependencies;
        }
    }

    pub fn content(&self, id: &str) -> Option<Vec<u8>> {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.grf.clone())
    }

    fn writable(&self) -> Result<(), LookError> {
        if self.update_pending {
            return Err(LookError::UpdatePending);
        }
        Ok(())
    }

    fn find(&mut self, id: &str) -> Result<&mut Entry, LookError> {
        self.entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or(LookError::NotFound)
    }

    fn enabled_count(&self) -> usize {
        self.entries.iter().filter(|e| e.enabled).count()
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), LookError> {
        self.writable()?;
        if enabled && self.enabled_count() >= self.max_loaded {
            return Err(LookError::NoRoom);
        }
        self.find(id)?.enabled = enabled;
        Ok(())
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), LookError> {
        self.writable()?;
        if !valid_name(name) {
            return Err(LookError::InvalidName);
        }
        let entry = self.find(id)?;
        if entry.source != LookSource::Custom {
            return Err(LookError::OfficialReadOnly);
        }
        entry.grf = look_media::rename(&entry.grf, name).map_err(|_| LookError::InvalidName)?;
        entry.name = name.to_owned();
        Ok(())
    }

    pub fn delete(&mut self, id: &str, active_off: bool) -> Result<(), LookError> {
        self.writable()?;
        let selected = self.selected.as_deref() == Some(id);
        let entry = self.find(id)?;
        if !entry.dependencies.is_empty() {
            return Err(LookError::InUse);
        }
        if selected && !active_off {
            return Err(LookError::ActiveLook);
        }
        self.entries.retain(|e| e.id != id);
        if selected {
            self.selected = None;
        }
        Ok(())
    }

    pub fn set_order(&mut self, ids: &[String]) -> Result<(), LookError> {
        self.writable()?;
        let mut current: Vec<&String> = self.entries.iter().map(|e| &e.id).collect();
        let mut wanted: Vec<&String> = ids.iter().collect();
        current.sort();
        wanted.sort();
        if current != wanted {
            return Err(LookError::Changed);
        }
        let mut entries = Vec::with_capacity(ids.len());
        for id in ids {
            let at = self.entries.iter().position(|e| &e.id == id).unwrap();
            entries.push(self.entries.remove(at));
        }
        self.entries = entries;
        Ok(())
    }

    /// Add a Look, or replace the content of an imported one.
    pub fn put(&mut self, grf: &[u8], official: bool) -> Result<(), LookError> {
        self.writable()?;
        let (id, name, _) =
            look_media::label(grf).map_err(|e| LookError::Service(e.to_string()))?;
        if let Some(existing) = self.entries.iter_mut().find(|e| e.id == id) {
            if existing.source != LookSource::Custom {
                return Err(LookError::OfficialReadOnly);
            }
            existing.grf = grf.to_vec();
            existing.name = name;
            return Ok(());
        }
        if self.entries.len() >= self.max_listed || self.enabled_count() >= self.max_loaded {
            return Err(LookError::NoRoom);
        }
        self.entries.push(Entry {
            id,
            name,
            enabled: true,
            source: if official {
                LookSource::Official
            } else {
                LookSource::Custom
            },
            grf: grf.to_vec(),
            dependencies: Vec::new(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_follows_the_service_rules() {
        let mut looks = FakeLooks::demo();
        let snapshot = looks.snapshot();
        assert_eq!(snapshot.looks.len(), 6);
        assert!(snapshot.get("portra-400-endura-v1").unwrap().selected);
        assert!(!snapshot.get("velvia-100-v1").unwrap().loaded);
        assert_eq!(
            looks.delete("portra-400-endura-v1", true),
            Err(LookError::InUse)
        );
        assert_eq!(
            looks.rename("kodachrome-64-v1", "Mine"),
            Err(LookError::OfficialReadOnly)
        );
        looks.rename(DEMO_CUSTOM_ID, "Night").unwrap();
        assert_eq!(looks.snapshot().get(DEMO_CUSTOM_ID).unwrap().name, "Night");
        looks.max_loaded = looks.snapshot().loaded;
        assert_eq!(
            looks.set_enabled("velvia-100-v1", true),
            Err(LookError::NoRoom)
        );
        let mut ids = looks.snapshot().ids();
        ids.reverse();
        looks.set_order(&ids).unwrap();
        assert_eq!(looks.snapshot().ids(), ids);
        looks.update_pending = true;
        assert_eq!(
            looks.set_enabled("velvia-100-v1", false),
            Err(LookError::UpdatePending)
        );
    }

    #[test]
    fn deleting_the_look_in_use_needs_off() {
        let mut looks = FakeLooks::demo();
        looks.entries[0].dependencies.clear();
        assert_eq!(
            looks.delete("portra-400-endura-v1", false),
            Err(LookError::ActiveLook)
        );
        looks.delete("portra-400-endura-v1", true).unwrap();
        assert!(looks.snapshot().looks.iter().all(|l| !l.selected));
    }

    #[test]
    fn catalog_adds_official_looks() {
        let mut looks = FakeLooks::demo();
        let (look, grf) = demo_catalog()
            .into_iter()
            .find(|(l, _)| l.id == "provia-100f-v1")
            .unwrap();
        looks.put(&grf, true).unwrap();
        assert_eq!(
            looks.snapshot().get(&look.id).unwrap().source,
            LookSource::Official
        );
    }
}
