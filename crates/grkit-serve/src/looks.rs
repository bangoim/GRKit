//! Read-only inspection of the data library. Disk contents do not prove what
//! the running camera loaded, selected, or enabled.
use anyhow::{ensure, Context, Result};
use grkit_core::{card::DATA_FOLDER_PATH, effects, packaging};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
};

/// Bound directory work, including unrelated entries, before returning a complete inventory.
pub const MAX_ENTRIES: usize = 256;
/// The camera can list 64 Looks; excess files remain visible in this inspection.
pub const MAX_LISTED: usize = 64;

/// Inspect actual GRFs. `official_hashes` must come from a trusted catalog,
/// never from an unsigned on-card package list. Without provenance, source is
/// unknown: absence from the current package does not prove a custom Look.
/// Missing or unreadable directories and scan-limit failures are errors, not
/// empty libraries. This is a disk inspection, not an atomic runtime snapshot.
pub fn inventory(root: &Path, official_hashes: &BTreeSet<String>) -> Result<Value> {
    let directory = root.join(DATA_FOLDER_PATH).join("EFFECTS");
    ensure!(
        fs::symlink_metadata(&directory)?.file_type().is_dir(),
        "The Looks library is not a directory"
    );
    let mut entries = Vec::new();
    for entry in fs::read_dir(&directory).context("Read the Looks library")? {
        ensure!(
            entries.len() < MAX_ENTRIES,
            "The Looks library exceeds the inspection limit"
        );
        entries.push(entry.context("Read a Looks directory entry")?);
    }
    entries.sort_by_key(|entry| entry.file_name());
    let mut looks = Vec::new();
    let mut diagnostics = Vec::new();
    let mut identities: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut codes: BTreeMap<u16, BTreeSet<String>> = BTreeMap::new();
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            diagnostics.push(json!({"code":"invalid_filename"}));
            continue;
        };
        if !name.to_ascii_lowercase().ends_with(".grf") {
            continue;
        }
        if !effects::valid_filename(name) {
            diagnostics.push(json!({"file":name,"code":"invalid_filename"}));
            continue;
        }
        let read = || -> Result<Vec<u8>> {
            ensure!(entry.file_type()?.is_file(), "Expected a regular GRF file");
            let file = fs::File::open(entry.path())?;
            let length = file.metadata()?.len();
            ensure!(
                length == effects::SIZE as u64 || length == effects::SIZE_WITH_LEVELS as u64,
                "Invalid GRF length"
            );
            let mut bytes = Vec::with_capacity(length as usize);
            file.take(effects::SIZE_WITH_LEVELS as u64 + 1)
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        };
        let bytes = match read() {
            Ok(bytes) => bytes,
            Err(_) => {
                diagnostics.push(json!({"file":name,"code":"unreadable_grf"}));
                continue;
            }
        };
        let effect = match effects::decode(&bytes) {
            Ok((effect, _)) => effect,
            Err(_) => {
                diagnostics.push(json!({"file":name,"code":"invalid_grf"}));
                continue;
            }
        };
        let hash = packaging::sha256(&bytes);
        let code = base_code(&effect.id);
        identities
            .entry(effect.id.clone())
            .or_default()
            .push(name.to_owned());
        codes.entry(code).or_default().insert(effect.id.clone());
        looks.push(json!({
            "id":effect.id,"name":effect.name,"file":name,
            "contentHash":hash,"size":bytes.len(),"baseEffectCode":code,
            "source":if official_hashes.contains(&hash) { "official" } else { "unknown" }
        }));
    }
    looks.sort_by(|a, b| {
        a["id"]
            .as_str()
            .cmp(&b["id"].as_str())
            .then(a["file"].as_str().cmp(&b["file"].as_str()))
    });
    for (id, files) in &identities {
        if files.len() > 1 {
            diagnostics.push(json!({"code":"duplicate_id","id":id,"files":files}));
        }
    }
    for (code, ids) in codes {
        if ids.len() > 1 {
            diagnostics
                .push(json!({"code":"effect_code_collision","baseEffectCode":code,"ids":ids}));
        }
    }
    if identities.len() > MAX_LISTED {
        diagnostics.push(
            json!({"code":"listed_capacity_exceeded","count":identities.len(),"limit":MAX_LISTED}),
        );
    }
    Ok(
        json!({"looks":looks,"diagnostics":diagnostics,"runtimeAvailable":false,
        "limits":{"listedLooks":MAX_LISTED,"inspectionEntries":MAX_ENTRIES}}),
    )
}

// The installer's raw ID code, before its catalog-dependent collision fixup.
fn base_code(id: &str) -> u16 {
    let hash = id.bytes().fold(2166136261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16777619)
    });
    0x8000 | (hash as u16 & 0x7fff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use grkit_core::testing;

    fn library() -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join(DATA_FOLDER_PATH).join("EFFECTS");
        fs::create_dir_all(&directory).unwrap();
        (root, directory)
    }

    #[test]
    fn reads_actual_content_without_inventing_runtime_or_provenance() {
        let (root, dir) = library();
        let bytes = testing::effect("sample");
        fs::write(dir.join("SAMPLE.GRF"), &bytes).unwrap();
        let result = inventory(root.path(), &BTreeSet::new()).unwrap();
        let look = &result["looks"][0];
        assert_eq!(look["id"], "sample");
        assert_eq!(look["contentHash"], packaging::sha256(&bytes));
        assert_eq!(look["source"], "unknown");
        for field in ["enabled", "selected", "loaded"] {
            assert!(look.get(field).is_none());
        }
        let trusted = BTreeSet::from([packaging::sha256(&bytes)]);
        assert_eq!(
            inventory(root.path(), &trusted).unwrap()["looks"][0]["source"],
            "official"
        );
    }

    #[test]
    fn reports_duplicate_and_corrupt_files_without_hiding_valid_copies() {
        let (root, dir) = library();
        for name in ["ONE.GRF", "TWO.GRF"] {
            fs::write(dir.join(name), testing::effect("same")).unwrap();
        }
        fs::write(dir.join("BAD.GRF"), vec![0; effects::SIZE]).unwrap();
        fs::write(
            dir.join("LARGE.GRF"),
            vec![0; effects::SIZE_WITH_LEVELS + 1],
        )
        .unwrap();
        let result = inventory(root.path(), &BTreeSet::new()).unwrap();
        assert_eq!(result["looks"].as_array().unwrap().len(), 2);
        let codes: Vec<_> = result["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["code"].as_str().unwrap())
            .collect();
        assert!(codes.contains(&"duplicate_id"));
        assert!(codes.contains(&"invalid_grf"));
        assert!(codes.contains(&"unreadable_grf"));
        assert!(!codes.contains(&"effect_code_collision"));
    }

    #[test]
    fn detects_different_ids_with_the_same_runtime_base_code() {
        let mut seen = BTreeMap::new();
        let (first, second) = (0..=32768)
            .find_map(|i| {
                let id = format!("look-{i}");
                seen.insert(base_code(&id), id.clone())
                    .map(|first| (first, id))
            })
            .unwrap();
        let (root, dir) = library();
        fs::write(dir.join("ONE.GRF"), testing::effect(&first)).unwrap();
        fs::write(dir.join("TWO.GRF"), testing::effect(&second)).unwrap();
        let result = inventory(root.path(), &BTreeSet::new()).unwrap();
        assert_eq!(result["diagnostics"][0]["code"], "effect_code_collision");
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_grf_symlinks() {
        let (root, dir) = library();
        let outside = root.path().join("outside.grf");
        fs::write(&outside, testing::effect("outside")).unwrap();
        std::os::unix::fs::symlink(outside, dir.join("LINK.GRF")).unwrap();
        let result = inventory(root.path(), &BTreeSet::new()).unwrap();
        assert!(result["looks"].as_array().unwrap().is_empty());
        assert_eq!(result["diagnostics"][0]["code"], "unreadable_grf");
    }

    #[test]
    fn missing_library_and_excess_entries_are_not_empty_successes() {
        let root = tempfile::tempdir().unwrap();
        assert!(inventory(root.path(), &BTreeSet::new()).is_err());
        let (root, dir) = library();
        for i in 0..=MAX_ENTRIES {
            fs::write(dir.join(format!("{i}.txt")), b"").unwrap();
        }
        assert!(inventory(root.path(), &BTreeSet::new()).is_err());
    }
}
