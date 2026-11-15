//! An update of the package over Wi-Fi (`grkit_core::runtime::update`).
//!
//! The service checks the package (`grkit_core::release::verify_update`):
//! an official key signed its code, and the decoders check its data. It
//! checks that the package is for this camera and its firmware. Then it writes the files that
//! change to the update folder, reads each one back, and writes the journal.
//! The camera installs the update at the next power-on.
//!
//! The card has rules that the steps follow:
//! - It cannot rename a folder or replace a file, so the service never
//!   renames over a file, and the journal moves the files.
//! - After Linux deletes a file in a folder, a new name in that folder may not
//!   be found until the next power-on (GR IV). The service looks up each name
//!   that it creates in a folder before it deletes a file there, and it never
//!   deletes a folder.
//! - Linux keeps the result of a lookup of a name that does not exist, so it
//!   does not see a file that the camera creates under that name later. The
//!   service never looks up the journal by its name, which the camera
//!   creates; it reads the folder instead (`Card::journal_files`).

use anyhow::{bail, ensure, Context, Result};
use grkit_core::{
    camera::{Camera, CAMERAS},
    card::{
        BACKUP_FOLDER, BACKUP_RECORD, DRAFT_FILE, JOURNAL_FILE, MODULES_FOLDER, PACKAGE_FOLDER,
        PACKAGE_LIST, REQUEST_FILE, STARTUP_FILE, UPDATE_FOLDER, VERSION_FILE,
    },
    packaging, release,
    runtime::update::{journal, rollback_journal, update_paths, Change},
    Files,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};

/// The largest package that the service takes: the package limit, and room
/// for the ZIP structure.
pub const MAX_PACKAGE: usize = packaging::MAX_TOTAL_SIZE + 8 * 1024 * 1024;
/// The free space that the card keeps after the update folder.
const SPARE_BYTES: u64 = 4 * 1024 * 1024;

/// What the service knows of the camera: the model and the firmware version
/// that the camera's own HTTP API gives, and the free space of the card.
pub struct Device {
    pub model: String,
    pub firmware: String,
    pub free_bytes: Option<u64>,
}

/// The camera of `model`, a model name of the camera's HTTP API such as
/// `RICOH GR III`. An HDF model has the package of its camera.
fn camera_of(model: &str) -> Option<&'static Camera> {
    let name = model.strip_prefix("RICOH ").unwrap_or(model);
    CAMERAS
        .into_iter()
        .find(|camera| camera.name == name || camera.hdf == name)
}

/// The error of a card whose package folder cannot be read yet, as soon
/// after power-on. It is not the same as a card without an update; the
/// client can try again.
#[derive(Debug)]
pub struct CardNotReady;

impl std::fmt::Display for CardNotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The card is not ready yet. Try again.")
    }
}

impl std::error::Error for CardNotReady {}

/// Whether `error` comes from a card that is not ready (`CardNotReady`).
pub fn card_not_ready(error: &anyhow::Error) -> bool {
    error.downcast_ref::<CardNotReady>().is_some()
}

/// The card, from its root on Linux.
pub struct Card {
    root: PathBuf,
}

impl Card {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Card { root: root.into() }
    }

    fn path(&self, card_path: &str) -> PathBuf {
        self.root.join(card_path)
    }

    /// The files of the package folder on the card, at their card paths: the
    /// files of the folder and of its folders, one level down.
    fn package_files(&self) -> Result<Files> {
        let mut files = Files::new();
        let folder = self.path(PACKAGE_FOLDER);
        for entry in fs::read_dir(&folder).with_context(|| format!("Read {PACKAGE_FOLDER}"))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type()?.is_dir() {
                for inner in fs::read_dir(entry.path())? {
                    let inner = inner?;
                    if inner.file_type()?.is_file() {
                        let inner_name = inner.file_name().to_string_lossy().into_owned();
                        files.insert(
                            format!("{PACKAGE_FOLDER}/{name}/{inner_name}"),
                            fs::read(inner.path())?,
                        );
                    }
                }
            } else {
                files.insert(format!("{PACKAGE_FOLDER}/{name}"), fs::read(entry.path())?);
            }
        }
        Ok(files)
    }

    /// Looks up `paths`, so that the card finds them after a delete in their
    /// folders. A path that does not exist is fine.
    fn warm<'a>(&self, paths: impl IntoIterator<Item = &'a str>) {
        for path in paths {
            let _ = fs::symlink_metadata(self.path(path));
        }
    }

    /// Deletes the files in the folder `folder` and in its folders, and
    /// keeps the folders.
    fn clear(&self, folder: &str) -> Result<()> {
        let path = self.path(folder);
        if !path.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type()?.is_dir() {
                self.clear(&format!("{folder}/{name}"))?;
            } else {
                fs::remove_file(entry.path()).with_context(|| format!("Delete {folder}/{name}"))?;
            }
        }
        Ok(())
    }

    /// Writes `bytes` to `path`, then reads them back.
    fn write_checked(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let target = self.path(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).with_context(|| format!("Create the folder of {path}"))?;
        }
        let mut file = fs::File::create(&target).with_context(|| format!("Create {path}"))?;
        file.write_all(bytes)
            .with_context(|| format!("Write {path}"))?;
        file.sync_all().with_context(|| format!("Write {path}"))?;
        drop(file);
        ensure!(
            fs::read(&target).with_context(|| format!("Read back {path}"))? == bytes,
            "{path} differs after the write"
        );
        Ok(())
    }

    /// Reads the package folder. Missing or an I/O error means the card is
    /// not ready yet; the client can retry.
    pub fn ensure_readable(&self) -> Result<()> {
        fs::read_dir(self.path(PACKAGE_FOLDER))
            .map(|_| ())
            .context(CardNotReady)
    }

    /// The journal files in the package folder: of `JOURNAL_FILE`,
    /// `REQUEST_FILE`, `DRAFT_FILE`, and `VERSION_FILE`, those that the
    /// folder lists. An error means the package folder cannot be read.
    fn journal_files(&self) -> Result<Vec<&'static str>> {
        let names: Vec<String> = fs::read_dir(self.path(PACKAGE_FOLDER))
            .context(CardNotReady)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_ascii_uppercase())
            .collect();
        Ok([JOURNAL_FILE, REQUEST_FILE, DRAFT_FILE, VERSION_FILE]
            .into_iter()
            .filter(|path| names.iter().any(|name| path.ends_with(&format!("/{name}"))))
            .collect())
    }

    /// The state of the update: `none`, `waiting` for the confirmation on
    /// the camera, or `confirmed` and waiting for the next power-on. An
    /// error means the package folder cannot be read.
    pub fn state(&self) -> Result<&'static str> {
        let files = self.journal_files()?;
        Ok(if files.contains(&JOURNAL_FILE) {
            "confirmed"
        } else if files.contains(&REQUEST_FILE) {
            "waiting"
        } else {
            "none"
        })
    }

    /// Removes the journal of an update that has not been installed, and
    /// returns whether there was one. The files in the update folder stay
    /// until the next update.
    pub fn cancel(&self) -> Result<bool> {
        // The service creates these names again after the deletes.
        self.warm([REQUEST_FILE, DRAFT_FILE, VERSION_FILE]);
        let files = self.journal_files()?;
        let had = files.contains(&JOURNAL_FILE) || files.contains(&REQUEST_FILE);
        // The journal of the camera first, before a delete in the folder.
        for path in files {
            fs::remove_file(self.path(path)).with_context(|| format!("Delete {path}"))?;
        }
        Ok(had)
    }
}

/// The record of an update (`BACKUP_RECORD`), which the service writes with
/// the backup folder.
#[derive(Debug, PartialEq)]
struct Record {
    /// The GRKit version before the update.
    version: Option<String>,
    /// The SHA-256 of each file that the update installed.
    installed: BTreeMap<String, String>,
    /// The files that the update installed where the card had no file.
    added: Vec<String>,
    /// The files that the update replaced or removed: the backup.
    backup: Vec<String>,
}

impl Record {
    fn of(changes: &[Change], live: &Files, new: &Files, version: Option<String>) -> Record {
        let mut record = Record {
            version,
            installed: BTreeMap::new(),
            added: Vec::new(),
            backup: Vec::new(),
        };
        for change in changes {
            match change {
                Change::Install(path) => {
                    record
                        .installed
                        .insert(path.clone(), packaging::sha256(&new[path]));
                    if live.contains_key(path) {
                        record.backup.push(path.clone());
                    } else {
                        record.added.push(path.clone());
                    }
                }
                Change::Remove(path) => record.backup.push(path.clone()),
            }
        }
        record
    }

    fn to_bytes(&self) -> Vec<u8> {
        json!({
            "version": self.version,
            "installed": self.installed,
            "added": self.added,
            "backup": self.backup,
        })
        .to_string()
        .into_bytes()
    }

    fn parse(bytes: &[u8]) -> Option<Record> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        let paths = |key: &str| -> Option<Vec<String>> {
            value[key]
                .as_array()?
                .iter()
                .map(|path| path.as_str().map(str::to_owned))
                .collect()
        };
        Some(Record {
            version: value["version"].as_str().map(str::to_owned),
            installed: value["installed"]
                .as_object()?
                .iter()
                .map(|(path, hash)| Some((path.clone(), hash.as_str()?.to_owned())))
                .collect::<Option<_>>()?,
            added: paths("added")?,
            backup: paths("backup")?,
        })
    }
}

/// A rollback of the last update: the version that it returns to, and its
/// changes (`rollback_journal`).
pub struct Rollback {
    pub version: Option<String>,
    changes: Vec<Change>,
}

impl Card {
    /// The package files of the card with the startup script, at their card
    /// paths.
    fn live_files(&self) -> Result<Files> {
        let mut files = self.package_files()?;
        if let Ok(startup) = fs::read(self.path(STARTUP_FILE)) {
            files.insert(STARTUP_FILE.to_owned(), startup);
        }
        Ok(files)
    }

    /// The rollback of the last update, if the card can make it: no update
    /// waits, the backup of the update is complete, and each file that it
    /// installed is unchanged. After a rollback, or a copy of a package to
    /// the card, there is none. An error means the package folder cannot be
    /// read; that is not the same as no rollback.
    pub fn rollback(&self) -> Result<Option<Rollback>> {
        if self.state()? != "none" {
            return Ok(None);
        }
        let Ok(bytes) = fs::read(self.path(BACKUP_RECORD)) else {
            return Ok(None);
        };
        let Some(record) = Record::parse(&bytes) else {
            return Ok(None);
        };
        let backed_up = record
            .backup
            .iter()
            .all(|path| update_paths(path).is_ok_and(|(_, old)| self.path(&old).is_file()));
        let live = self.live_files()?;
        let unchanged = record.installed.iter().all(|(path, hash)| {
            live.get(path)
                .is_some_and(|bytes| packaging::sha256(bytes) == *hash)
        });
        Ok((backed_up && unchanged).then(|| Rollback {
            version: record.version,
            changes: record
                .backup
                .into_iter()
                .map(Change::Install)
                .chain(record.added.into_iter().map(Change::Remove))
                .collect(),
        }))
    }

    /// The installed package: its GRKit version, the module IDs and effect
    /// file names of `PACKAGE_LIST`, and the effects that the list leaves
    /// out. An error means the package folder cannot be read.
    pub fn package(&self) -> Result<Value> {
        let files = self.package_files().context(CardNotReady)?;
        let (modules, effects, left_out) = match files.get(PACKAGE_LIST) {
            Some(bytes) => parse_package_list(&String::from_utf8_lossy(bytes)),
            None => (Vec::new(), Vec::new(), Vec::new()),
        };
        Ok(json!({
            "version": version_of(&files),
            "modules": modules,
            "effects": effects,
            "left_out": left_out,
        }))
    }
}

/// The module IDs, effect file names, and left-out effect names of a
/// package list, each sorted.
fn parse_package_list(list: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut modules = Vec::new();
    let mut effects = Vec::new();
    let mut left_out = Vec::new();
    for line in list.lines() {
        if let Some(name) = line.strip_prefix('-') {
            left_out.push(name.to_owned());
        } else if let Some(id) = line.strip_suffix(".GRM") {
            modules.push(id.to_owned());
        } else if line.ends_with(".GRF") {
            effects.push(line.to_owned());
        }
    }
    modules.sort();
    effects.sort();
    left_out.sort();
    (modules, effects, left_out)
}

/// Writes the journal of a rollback of the last update (`Card::rollback`),
/// which waits for the confirmation on the camera, as an update does.
pub fn stage_rollback(card: &Card) -> Result<Value> {
    let rollback = card
        .rollback()?
        .context("The card has no update to roll back")?;
    card.clear(UPDATE_FOLDER)?;
    card.write_checked(VERSION_FILE, &version_digits(rollback.version.as_deref()))?;
    card.write_checked(DRAFT_FILE, &rollback_journal(&rollback.changes)?)?;
    fs::rename(card.path(DRAFT_FILE), card.path(REQUEST_FILE)).context("Write the journal")?;
    ensure!(card.state()? == "waiting", "The journal is not on the card");
    let installs = rollback
        .changes
        .iter()
        .filter(|change| matches!(change, Change::Install(_)))
        .count();
    Ok(json!({
        "state": "waiting",
        "version": rollback.version,
        "install": installs,
        "remove": rollback.changes.len() - installs,
    }))
}

/// Whether the camera puts a card file of `path` there with each package.
/// The update removes only these when the new package does not have them:
/// the runtime files, the modules, and the package effects. Other files of
/// the package folder, such as the import library of the site, stay.
fn package_owned(path: &str) -> bool {
    if packaging::is_runtime_path(path) {
        return true;
    }
    let Some(rest) = path
        .strip_prefix(PACKAGE_FOLDER)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };
    let in_folder = |folder: &str, extension: &str| {
        rest.strip_prefix(folder)
            .and_then(|name| name.strip_prefix('/'))
            .is_some_and(|name| !name.contains('/') && name.ends_with(extension))
    };
    let modules = MODULES_FOLDER.rsplit('/').next().unwrap_or_default();
    in_folder(modules, ".GRM")
        || in_folder("EFFECTS", ".GRF")
        || (rest.starts_with("CORE") && rest.ends_with(".BIN") && !rest.contains('/'))
}

/// The changes from the package files on the card, `live`, to `new`: each
/// new file that differs from the card, and each package file that the new
/// package does not have. The startup script is a change only if it differs.
fn changes(live: &Files, new: &Files, live_startup: Option<&[u8]>) -> Vec<Change> {
    let mut changes = Vec::new();
    for (path, bytes) in new {
        let current = if path == STARTUP_FILE {
            live_startup
        } else {
            live.get(path).map(Vec::as_slice)
        };
        if current != Some(bytes.as_slice()) {
            changes.push(Change::Install(path.clone()));
        }
    }
    for path in live.keys() {
        if !new.contains_key(path) && package_owned(path) {
            changes.push(Change::Remove(path.clone()));
        }
    }
    changes
}

/// The GRKit version that the core files of the package files `files` name.
fn version_of(files: &Files) -> Option<String> {
    files
        .iter()
        .filter(|(path, _)| {
            path.strip_prefix(&format!("{PACKAGE_FOLDER}/CORE"))
                .is_some_and(|rest| rest.ends_with(".BIN") && !rest.contains('/'))
        })
        .find_map(|(_, binary)| packaging::marked_version(binary))
        .map(str::to_owned)
}

/// The version file of the package files `files` (`card::VERSION_FILE`).
fn version_file(files: &Files) -> Vec<u8> {
    version_digits(version_of(files).as_deref())
}

/// The version file of `version`: six digits, or zeros.
fn version_digits(version: Option<&str>) -> Vec<u8> {
    let parts: Option<Vec<u32>> = version
        .map(|version| version.split('.').map(|part| part.parse().ok()).collect())
        .and_then(|parts: Option<Vec<u32>>| parts);
    match parts.as_deref() {
        Some([major, minor, patch]) if [major, minor, patch].iter().all(|part| **part < 100) => {
            format!("{major:02}{minor:02}{patch:02}").into_bytes()
        }
        _ => b"000000".to_vec(),
    }
}

/// Checks the package `zip` for `device`, writes its changes to the update
/// folder, and writes the journal. Returns what the update does.
pub fn stage(card: &Card, zip: &[u8], device: &Device, keys: &[[u8; 32]]) -> Result<Value> {
    ensure!(zip.len() <= MAX_PACKAGE, "The package is too large");
    let (update, files) = release::verify_update(zip, keys).context("Check the package")?;
    check_camera(update.statement.camera, device)?;
    // The package folder and the startup script; the update keeps the
    // ModeSet files of the package off the card, as SD scripts are on.
    let new: Files = files
        .into_iter()
        .filter(|(path, _)| path == STARTUP_FILE || path.starts_with(&format!("{PACKAGE_FOLDER}/")))
        .collect();
    let live = card.package_files().context(CardNotReady)?;
    let live_startup = fs::read(card.path(STARTUP_FILE)).ok();
    let changes = changes(&live, &new, live_startup.as_deref());
    let mut before = live.clone();
    if let Some(startup) = &live_startup {
        before.insert(STARTUP_FILE.to_owned(), startup.clone());
    }
    let record = Record::of(&changes, &before, &new, version_of(&live));
    let staged: Vec<(String, &Vec<u8>)> = changes
        .iter()
        .filter_map(|change| match change {
            Change::Install(path) => Some(path),
            Change::Remove(_) => None,
        })
        .map(|path| Ok((update_paths(path)?.0, &new[path])))
        .collect::<Result<_>>()?;
    let bytes: u64 = staged.iter().map(|(_, data)| data.len() as u64).sum();
    if let Some(free) = device.free_bytes {
        ensure!(
            free >= bytes + SPARE_BYTES,
            "The card needs {} more bytes for the update",
            bytes + SPARE_BYTES - free
        );
    }
    // A new update replaces an update that has not been installed.
    card.cancel()?;
    let summary = |state: &str, installs: usize| {
        json!({
            "state": state,
            "camera": update.statement.camera.id,
            "version": update.statement.version,
            "modules": update.modules,
            "effects": update.effect_ids.len(),
            "install": installs,
            "remove": changes.len() - installs,
            "bytes": bytes,
        })
    };
    if changes.is_empty() {
        // The backup of the last update stays, for a rollback.
        card.clear(UPDATE_FOLDER)?;
        return Ok(summary("none", 0));
    }
    card.warm(
        staged
            .iter()
            .map(|(path, _)| path.as_str())
            .chain([BACKUP_RECORD]),
    );
    card.clear(UPDATE_FOLDER)?;
    card.clear(BACKUP_FOLDER)?;
    for (path, data) in &staged {
        card.write_checked(path, data)?;
    }
    // The backup folder gets its files from the journal, and the record of
    // them now, for a rollback.
    card.write_checked(BACKUP_RECORD, &record.to_bytes())?;
    // The camera sees the journal only when it is complete, and runs it only
    // after the confirmation on the camera. It reads the version first.
    card.write_checked(VERSION_FILE, &version_file(&new))?;
    card.write_checked(DRAFT_FILE, &journal(&changes)?)?;
    fs::rename(card.path(DRAFT_FILE), card.path(REQUEST_FILE)).context("Write the journal")?;
    ensure!(card.state()? == "waiting", "The journal is not on the card");
    Ok(summary("waiting", staged.len()))
}

/// Checks that the package for `package` is for the camera of `device` and
/// its firmware.
fn check_camera(package: &Camera, device: &Device) -> Result<()> {
    let Some(camera) = camera_of(&device.model) else {
        bail!("GRKit does not support the {}", device.model);
    };
    ensure!(
        package == camera,
        "The package is for the {}, not the {}",
        package.name,
        camera.name
    );
    ensure!(
        camera.versions().contains(&device.firmware.as_str()),
        "The package does not support firmware {} of the {}",
        device.firmware,
        camera.name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use grkit_core::{camera, testing};

    /// A package of GRKit 1.7.0 with `effects`, which the key of the tests
    /// signed.
    fn package(effects: &[&str]) -> Vec<u8> {
        testing::signed_package(&camera::GR3, "1.7.0", effects)
    }

    fn keys() -> [[u8; 32]; 1] {
        [testing::release_key()]
    }

    fn gr3() -> Device {
        Device {
            model: "RICOH GR III".into(),
            firmware: "2.21".into(),
            free_bytes: Some(1 << 30),
        }
    }

    /// A card with the package `zip` on it, as a copy of `SD_ROOT` puts it.
    fn card_with(zip: &[u8]) -> (tempfile::TempDir, Card) {
        let dir = tempfile::tempdir().unwrap();
        for (path, bytes) in packaging::card_files(zip).unwrap() {
            let target = dir.path().join(&path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, bytes).unwrap();
        }
        let card = Card::new(dir.path());
        (dir, card)
    }

    fn read(card: &Card, path: &str) -> Option<Vec<u8>> {
        fs::read(card.path(path)).ok()
    }

    #[test]
    fn an_update_stages_only_the_files_that_change() {
        let (_dir, card) = card_with(&package(&["test-effect"]));
        let next = package(&["other-effect"]);
        let summary = stage(&card, &next, &gr3(), &keys()).unwrap();
        assert_eq!(summary["state"], "waiting");
        assert_eq!(summary["camera"], "gr3");
        // The new effect and the package list change; the old effect goes.
        let new = packaging::card_files(&next).unwrap();
        let other = new
            .keys()
            .find(|path| path.starts_with("SCRIPT/GRKIT/EFFECTS/"))
            .unwrap();
        let (staged, _) = update_paths(other).unwrap();
        assert_eq!(read(&card, &staged).as_ref(), Some(&new[other]));
        let (list, _) = update_paths("SCRIPT/GRKIT/PACKAGE.TXT").unwrap();
        assert_eq!(
            read(&card, &list).as_ref(),
            Some(&new["SCRIPT/GRKIT/PACKAGE.TXT"])
        );
        // The same files are not staged.
        let (core, _) = update_paths("SCRIPT/GRKIT/CORE1.BIN").unwrap();
        assert!(read(&card, &core).is_none());
        assert_eq!(summary["install"], 2);
        assert_eq!(summary["remove"], 1);
        // The journal waits for the confirmation on the camera.
        let journal = String::from_utf8(read(&card, REQUEST_FILE).unwrap()).unwrap();
        assert!(journal.contains(
            "filerename 'C:\\SCRIPT\\GRKITNEW\\PACKAGE.TXT' 'C:\\SCRIPT\\GRKIT\\PACKAGE.TXT'"
        ));
        assert!(read(&card, DRAFT_FILE).is_none());
        assert!(read(&card, JOURNAL_FILE).is_none());
        assert_eq!(card.state().unwrap(), "waiting");
    }

    #[test]
    fn an_update_keeps_the_files_that_no_package_has() {
        let (_dir, card) = card_with(&package(&["test-effect"]));
        fs::write(card.path("SCRIPT/GRKIT/IMPORTS.BIN"), b"imports").unwrap();
        stage(&card, &package(&["other-effect"]), &gr3(), &keys()).unwrap();
        let journal = String::from_utf8(read(&card, REQUEST_FILE).unwrap()).unwrap();
        assert!(!journal.contains("IMPORTS.BIN"));
    }

    #[test]
    fn a_new_update_replaces_the_files_of_the_last_one() {
        let (_dir, card) = card_with(&package(&["test-effect"]));
        stage(&card, &package(&["other-effect"]), &gr3(), &keys()).unwrap();
        fs::create_dir_all(card.path(BACKUP_FOLDER)).unwrap();
        fs::write(card.path(&format!("{BACKUP_FOLDER}/OLD.BIN")), b"old").unwrap();
        // The same package as the card: no update waits.
        let summary = stage(&card, &package(&["test-effect"]), &gr3(), &keys()).unwrap();
        assert_eq!(summary["state"], "none");
        assert_eq!(summary["install"], 0);
        assert_eq!(summary["remove"], 0);
        assert_eq!(card.state().unwrap(), "none");
        assert!(read(&card, &format!("{BACKUP_FOLDER}/OLD.BIN")).is_some());
        let staged_effects = fs::read_dir(card.path(&format!("{UPDATE_FOLDER}/EFFECTS")))
            .unwrap()
            .count();
        assert_eq!(staged_effects, 0);
    }

    #[test]
    fn an_update_empties_the_backup_folder_for_its_files() {
        let (_dir, card) = card_with(&package(&["test-effect"]));
        fs::create_dir_all(card.path(BACKUP_FOLDER)).unwrap();
        fs::write(card.path(&format!("{BACKUP_FOLDER}/CORE1.BIN")), b"old").unwrap();
        stage(&card, &package(&["other-effect"]), &gr3(), &keys()).unwrap();
        assert!(read(&card, &format!("{BACKUP_FOLDER}/CORE1.BIN")).is_none());
    }

    #[test]
    fn the_version_file_has_the_version_that_the_core_names() {
        let core = |version: &str| {
            let mut binary = testing::installer();
            binary
                .extend_from_slice(format!("{}{version}\0", packaging::VERSION_MARKER).as_bytes());
            Files::from([("SCRIPT/GRKIT/CORE1.BIN".to_owned(), binary)])
        };
        assert_eq!(version_file(&core("1.7.0")), b"010700");
        assert_eq!(version_file(&core("12.3.45")), b"120345");
        assert_eq!(version_file(&core("1.7")), b"000000");
        assert_eq!(version_file(&core("100.0.0")), b"000000");
        assert_eq!(version_file(&Files::new()), b"000000");
        let (_dir, card) = card_with(&package(&["test-effect"]));
        stage(&card, &package(&["other-effect"]), &gr3(), &keys()).unwrap();
        assert_eq!(read(&card, VERSION_FILE).unwrap(), b"010700");
        card.cancel().unwrap();
        assert!(read(&card, VERSION_FILE).is_none());
    }

    #[test]
    fn a_cancel_removes_the_journal() {
        let (_dir, card) = card_with(&package(&["test-effect"]));
        stage(&card, &package(&["other-effect"]), &gr3(), &keys()).unwrap();
        assert!(card.cancel().unwrap());
        assert_eq!(card.state().unwrap(), "none");
        // A confirmed update can be removed before the next power-on too.
        stage(&card, &package(&["other-effect"]), &gr3(), &keys()).unwrap();
        fs::rename(card.path(REQUEST_FILE), card.path(JOURNAL_FILE)).unwrap();
        assert_eq!(card.state().unwrap(), "confirmed");
        assert!(card.cancel().unwrap());
        assert_eq!(card.state().unwrap(), "none");
        assert!(!card.cancel().unwrap());
    }

    #[test]
    fn the_package_must_be_for_this_camera_and_its_firmware() {
        let (_dir, card) = card_with(&package(&["test-effect"]));
        let next = package(&["other-effect"]);
        for (model, firmware, error) in [
            ("RICOH GR IV", "1.21", "is for the GR III, not the GR IV"),
            ("RICOH GR III", "9.99", "does not support firmware 9.99"),
            ("RICOH GR II", "4.10", "does not support the RICOH GR II"),
        ] {
            let device = Device {
                model: model.into(),
                firmware: firmware.into(),
                free_bytes: None,
            };
            let message = format!("{:#}", stage(&card, &next, &device, &keys()).unwrap_err());
            assert!(message.contains(error), "{message}");
        }
        // An HDF model has the package of its camera.
        let hdf = Device {
            model: "RICOH GR III HDF".into(),
            ..gr3()
        };
        stage(&card, &next, &hdf, &keys()).unwrap();
    }

    #[test]
    fn a_package_that_is_not_valid_or_does_not_fit_changes_nothing() {
        let (_dir, card) = card_with(&package(&["test-effect"]));
        let mut broken = package(&["other-effect"]);
        let at = broken.len() / 2;
        broken[at] ^= 0xff;
        assert!(stage(&card, &broken, &gr3(), &keys()).is_err());
        let full = Device {
            free_bytes: Some(1024),
            ..gr3()
        };
        let message = format!(
            "{:#}",
            stage(&card, &package(&["other-effect"]), &full, &keys()).unwrap_err()
        );
        assert!(message.contains("more bytes"), "{message}");
        assert_eq!(card.state().unwrap(), "none");
        assert!(!card.path(UPDATE_FOLDER).exists());
    }

    #[test]
    fn only_a_package_that_a_release_key_signed_is_staged() {
        let (_dir, card) = card_with(&package(&["test-effect"]));
        let next = package(&["other-effect"]);
        // Another key.
        let message = format!(
            "{:#}",
            stage(&card, &next, &gr3(), &grkit_core::release::OFFICIAL_KEYS).unwrap_err()
        );
        assert!(message.contains("No GRKit release key"), "{message}");
        // No signature.
        let unsigned = packaging::assemble(
            &testing::release_runtimes(&camera::GR3, "1.7.0"),
            &testing::effect_set(&["other-effect"]),
            &testing::official_modules(),
            &camera::GR3,
            &packaging::Guide {
                language: grkit_core::language::Language::English,
                pdf: b"%PDF-1.7\n",
            },
        )
        .unwrap()
        .bytes;
        let message = format!(
            "{:#}",
            stage(&card, &unsigned, &gr3(), &keys()).unwrap_err()
        );
        assert!(message.contains("not signed"), "{message}");
        assert_eq!(card.state().unwrap(), "none");
        assert!(!card.path(UPDATE_FOLDER).exists());
    }

    /// Runs the journal on the card as the startup script does: each step
    /// that moves a file, in order.
    fn run_journal(card: &Card) {
        let text = String::from_utf8(read(card, REQUEST_FILE).unwrap()).unwrap();
        let path = |name: &str| name.trim_start_matches("C:\\").replace('\\', "/");
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("filerename '") else {
                continue;
            };
            let (from, to) = rest.trim_end_matches('\'').split_once("' '").unwrap();
            let (from, to) = (card.path(&path(from)), card.path(&path(to)));
            // The guards of the journal skip a step that is done.
            if from.exists() && !to.exists() {
                fs::create_dir_all(to.parent().unwrap()).unwrap();
                fs::rename(from, to).unwrap();
            }
        }
        fs::remove_file(card.path(REQUEST_FILE)).unwrap();
    }

    /// The SHA-256 of each file of the package on the card, without the
    /// journal files.
    fn package_hashes(card: &Card) -> BTreeMap<String, String> {
        card.live_files()
            .unwrap()
            .iter()
            .filter(|(path, _)| !path.starts_with("SCRIPT/GRKIT/COMMIT."))
            .map(|(path, bytes)| (path.clone(), packaging::sha256(bytes)))
            .collect()
    }

    #[test]
    fn a_rollback_returns_the_card_to_the_files_before_the_last_update() {
        let first = package(&["test-effect"]);
        let (_dir, card) = card_with(&first);
        assert!(card.rollback().unwrap().is_none());
        let before = package_hashes(&card);
        let next = testing::signed_package(&camera::GR3, "1.8.0", &["other-effect"]);
        stage(&card, &next, &gr3(), &keys()).unwrap();
        // No rollback while an update waits.
        assert!(card.rollback().unwrap().is_none());
        run_journal(&card);
        assert_ne!(package_hashes(&card), before);
        let rollback = card.rollback().unwrap().unwrap();
        assert_eq!(rollback.version.as_deref(), Some("1.7.0"));
        let summary = stage_rollback(&card).unwrap();
        assert_eq!(summary["state"], "waiting");
        assert_eq!(summary["version"], "1.7.0");
        assert_eq!(read(&card, VERSION_FILE).unwrap(), b"010700");
        assert!(card.rollback().unwrap().is_none());
        run_journal(&card);
        assert_eq!(package_hashes(&card), before);
        // The backup is used: no second rollback.
        assert!(card.rollback().unwrap().is_none());
        assert!(
            format!("{:#}", stage_rollback(&card).unwrap_err()).contains("no update to roll back")
        );
    }

    #[test]
    fn no_rollback_after_the_card_changes() {
        let (_dir, card) = card_with(&package(&["test-effect"]));
        stage(&card, &package(&["other-effect"]), &gr3(), &keys()).unwrap();
        run_journal(&card);
        assert!(card.rollback().unwrap().is_some());
        // A copy of a package to the card after the update.
        fs::write(card.path("SCRIPT/GRKIT/PACKAGE.TXT"), b"changed\n").unwrap();
        assert!(card.rollback().unwrap().is_none());
    }

    #[test]
    fn the_package_lists_its_modules_effects_and_left_out() {
        let (_dir, card) = card_with(&package(&["test-effect"]));
        let info = card.package().unwrap();
        assert_eq!(info["version"], "1.7.0");
        let modules = info["modules"].as_array().unwrap();
        assert!(modules.iter().any(|id| id == "FILMLOOK"));
        assert!(modules
            .windows(2)
            .all(|pair| { pair[0].as_str().unwrap() <= pair[1].as_str().unwrap() }));
        let effect = grkit_core::effects::filename("test-effect").unwrap();
        assert_eq!(info["effects"], json!([effect]));
        assert_eq!(info["left_out"], json!([]));
        let list = card.path(PACKAGE_LIST);
        let mut text = String::from_utf8(fs::read(&list).unwrap()).unwrap();
        text.push_str("-LEFT_OUT.GRF\n");
        fs::write(&list, text).unwrap();
        assert_eq!(card.package().unwrap()["left_out"], json!(["LEFT_OUT.GRF"]));
    }

    #[test]
    fn a_missing_package_folder_is_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let card = Card::new(dir.path());
        assert!(card_not_ready(&card.ensure_readable().unwrap_err()));
        assert!(card_not_ready(&card.state().unwrap_err()));
        assert!(card_not_ready(&card.rollback().err().unwrap()));
        assert!(card_not_ready(&card.package().unwrap_err()));
        assert!(card_not_ready(&stage_rollback(&card).unwrap_err()));
    }

    #[test]
    fn the_package_owns_its_runtime_modules_and_effects() {
        for path in [
            "SCRIPT/GRKIT/CORE.BIN",
            "SCRIPT/GRKIT/CORE3.BIN",
            "SCRIPT/GRKIT/SERVE.BIN",
            "SCRIPT/GRKIT/MODULES/GLOW.GRM",
            "SCRIPT/GRKIT/EFFECTS/1234ABCD.GRF",
        ] {
            assert!(package_owned(path), "{path}");
        }
        for path in [
            "SCRIPT/GRKIT/IMPORTS.BIN",
            "SCRIPT/GRKIT/COMMIT.TTL",
            "SCRIPT/GRKIT/MODULES/README.TXT",
            "GRKIT/EFFECTS/1234ABCD.GRF",
        ] {
            assert!(!package_owned(path), "{path}");
        }
    }
}
