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
        PACKAGE_LIST, REQUEST_FILE, SCAN_FILE, STARTUP_FILE, UPDATE_FOLDER, VERSION_FILE,
    },
    packaging, release, runtime,
    runtime::update::{journal, rollback_journal, update_paths, Change},
    Files,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::PathBuf,
};

/// The largest package that the service takes: the package limit, and room
/// for the ZIP structure.
pub const MAX_PACKAGE: usize = packaging::MAX_TOTAL_SIZE + 8 * 1024 * 1024;
/// The free space that the card keeps after the update folder.
const SPARE_BYTES: u64 = 4 * 1024 * 1024;

/// A bounded signed-software identity. Staging metadata and user data are excluded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SoftwareIdentity {
    pub digest: String,
    pub content_hashes: BTreeMap<String, String>,
}

impl SoftwareIdentity {
    fn of(files: &Files) -> Self {
        let content_hashes: BTreeMap<_, _> = files
            .iter()
            .filter(|(path, _)| software_owned(path))
            .map(|(path, bytes)| (path.clone(), packaging::sha256(bytes)))
            .collect();
        let digest =
            packaging::sha256(&serde_json::to_vec(&content_hashes).expect("hashes serialize"));
        Self {
            digest,
            content_hashes,
        }
    }
    fn valid(&self) -> bool {
        self.content_hashes.len() <= 1024
            && self
                .content_hashes
                .iter()
                .all(|(path, hash)| path.len() <= 192 && software_owned(path) && valid_hash(hash))
            && self.digest
                == packaging::sha256(
                    &serde_json::to_vec(&self.content_hashes).expect("hashes serialize"),
                )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SoftwareDomain {
    Update,
    Rollback,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SoftwareStatus {
    Running,
    Succeeded,
    Cancelled,
    /// Ended without a change of the installed software; `error` says why.
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SoftwarePhase {
    AwaitingConfirmation,
    AwaitingRestart,
    Cancelling,
    Recovering,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SoftwareRequest {
    pub request_id: String,
    pub domain: SoftwareDomain,
    pub boot_id: String,
    pub card_id: String,
    pub status: SoftwareStatus,
    pub phase: SoftwarePhase,
    pub cancellable: bool,
    pub expected: SoftwareIdentity,
    pub before: SoftwareIdentity,
    /// The code of a failed request. Absent from other checkpoints, so that
    /// their bytes and checksums stay as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The camera deleted the request that waited for the confirmation on the
/// camera: nobody confirmed it in time.
pub const CONFIRMATION_EXPIRED: &str = "confirmation_expired";
/// The journal of the request did not reach the card.
pub const STAGING_FAILED: &str = "staging_failed";

#[derive(Clone, Debug)]
pub struct SoftwareContext {
    pub request_id: String,
    pub boot_id: String,
    pub card_id: String,
}
impl SoftwareContext {
    fn valid(&self) -> bool {
        crate::protocol::valid_request_id(&self.request_id)
            && valid_context_id(&self.boot_id)
            && valid_context_id(&self.card_id)
    }
}
impl SoftwareRequest {
    fn valid(&self) -> bool {
        crate::protocol::valid_request_id(&self.request_id)
            && valid_context_id(&self.boot_id)
            && valid_context_id(&self.card_id)
            && self.expected.valid()
            && self.before.valid()
            && self.error.is_some() == (self.status == SoftwareStatus::Failed)
            && self.error.as_deref().is_none_or(valid_context_id)
            && match self.status {
                SoftwareStatus::Succeeded | SoftwareStatus::Cancelled | SoftwareStatus::Failed => {
                    self.phase == SoftwarePhase::Completed && !self.cancellable
                }
                SoftwareStatus::Running => {
                    self.phase != SoftwarePhase::Completed
                        && (!self.cancellable || self.phase == SoftwarePhase::AwaitingConfirmation)
                }
            }
    }
}
fn valid_context_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|c| c.is_ascii_graphic())
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn software_owned(path: &str) -> bool {
    path == STARTUP_FILE || package_owned(path)
}

const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_CHECKPOINTS: usize = 16;

/// Append-only software transaction checkpoints share RECORD.JSN. They are not
/// a generic request history. A newline commits one checksummed JSON frame.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Checkpoint {
    request: SoftwareRequest,
    sha256: String,
}
impl Checkpoint {
    fn new(request: SoftwareRequest) -> Self {
        let sha256 = packaging::sha256(&serde_json::to_vec(&request).expect("request serializes"));
        Self { request, sha256 }
    }
    fn valid(&self) -> bool {
        self.request.valid()
            && self.sha256
                == packaging::sha256(
                    &serde_json::to_vec(&self.request).expect("request serializes"),
                )
    }
}

/// What the service knows of the camera: the model and the firmware version
/// that the camera's own HTTP API gives, and the free space of the card.
pub struct Device {
    pub model: String,
    pub firmware: String,
    pub free_bytes: Option<u64>,
    pub serial_number: Option<String>,
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

    pub(crate) fn root(&self) -> &std::path::Path {
        &self.root
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

    /// The camera deletes an unconfirmed request on its own side. Linux then
    /// keeps the names that it created (COMMIT.REQ, COMMIT.VER) and answers
    /// an I/O error for them, and a rename onto such a name fails. Drop the
    /// kernel's cached names when one of `paths` answers so.
    fn forget_deleted(&self, paths: &[&str]) {
        let stale = paths.iter().any(|path| {
            fs::symlink_metadata(self.path(path))
                .is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound)
        });
        if stale {
            // 2: free the cached names and file nodes, not file contents.
            let _ = fs::write("/proc/sys/vm/drop_caches", "2");
        }
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
            .map(|entry| {
                entry.map(|entry| entry.file_name().to_string_lossy().to_ascii_uppercase())
            })
            .collect::<std::io::Result<_>>()
            .context(CardNotReady)?;
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackAvailability {
    pub available: bool,
    pub allowed: bool,
    pub reason: &'static str,
    pub identity: Option<SoftwareIdentity>,
    pub version: Option<String>,
    /// This must come from verified signed target metadata, never a version guess.
    pub trusted_format_support: Option<Vec<u32>>,
}

impl Card {
    fn read_record_bytes(&self) -> Result<Option<Vec<u8>>> {
        let file = match fs::File::open(self.path(BACKUP_RECORD)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("Read the software transaction record"),
        };
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_RECORD_BYTES,
            "The software transaction record exceeds its size limit"
        );
        Ok(Some(bytes))
    }
    fn read_record(&self) -> Result<Option<Record>> {
        self.read_record_bytes()?
            .map(|bytes| {
                Record::parse(&bytes).context("The software transaction record is invalid")
            })
            .transpose()
    }
    fn append_request(&self, request: SoftwareRequest) -> Result<()> {
        ensure!(request.valid(), "The software request is invalid");
        let mut before = self
            .read_record_bytes()?
            .context("The software transaction record is missing")?;
        ensure!(
            Record::parse(&before).is_some(),
            "The software transaction record is invalid"
        );
        // A torn final append is uncommitted. Truncate only that tail; never
        // rewrite the complete base record or an acknowledged checkpoint.
        let complete_length = before
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .context("The base software record is incomplete")?;
        before.truncate(complete_length);
        ensure!(
            before.iter().filter(|byte| **byte == b'\n').count() <= MAX_CHECKPOINTS,
            "The software checkpoint limit has been reached"
        );
        let mut frame = serde_json::to_vec(&Checkpoint::new(request))?;
        frame.push(b'\n');
        ensure!(
            before.len() + frame.len() <= MAX_RECORD_BYTES,
            "The software transaction record exceeds its size limit"
        );
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(self.path(BACKUP_RECORD))?;
        file.set_len(complete_length as u64)?;
        file.sync_all()?;
        file.write_all(&frame)?;
        file.sync_all()?;
        drop(file);
        let after = self
            .read_record_bytes()?
            .context("The software transaction record disappeared")?;
        ensure!(
            after.len() == before.len() + frame.len()
                && after.starts_with(&before)
                && after.ends_with(&frame),
            "The software checkpoint differs after writing"
        );
        Ok(())
    }

    /// Recover the current/latest request from durable software metadata. A
    /// missing journal requires complete installed-identity verification. Once a
    /// terminal outcome is verified, append its checkpoint for later boots.
    pub fn software_request(&self) -> Result<Option<SoftwareRequest>> {
        self.ensure_readable()?;
        let Some(mut request) = self.read_record()?.and_then(|record| record.request) else {
            return Ok(None);
        };
        if request.status != SoftwareStatus::Running {
            return Ok(Some(request));
        }
        match self.state()? {
            "waiting" => {
                request.phase = SoftwarePhase::AwaitingConfirmation;
                request.cancellable = true;
            }
            "confirmed" => {
                request.phase = SoftwarePhase::AwaitingRestart;
                request.cancellable = false;
            }
            _ => {
                let actual = SoftwareIdentity::of(&self.live_files()?);
                if actual == request.expected {
                    request.status = SoftwareStatus::Succeeded;
                    request.phase = SoftwarePhase::Completed;
                } else if request.phase == SoftwarePhase::Cancelling && actual == request.before {
                    request.status = SoftwareStatus::Cancelled;
                    request.phase = SoftwarePhase::Completed;
                } else if request.phase == SoftwarePhase::AwaitingConfirmation
                    && actual == request.before
                {
                    // Only the camera removes a request that it did not
                    // confirm (after its confirmation time), and the
                    // installed software is unchanged.
                    request.status = SoftwareStatus::Failed;
                    request.phase = SoftwarePhase::Completed;
                    request.error = Some(CONFIRMATION_EXPIRED.into());
                } else {
                    request.phase = SoftwarePhase::Recovering;
                }
                request.cancellable = false;
                if request.status != SoftwareStatus::Running {
                    self.append_request(request.clone())?;
                    // No journal refers to the update folder any more. It
                    // holds the files that a rollback put aside, or a package
                    // that nobody confirmed: free their space now instead of
                    // at the next update.
                    let _ = self.clear(UPDATE_FOLDER);
                }
            }
        }
        Ok(Some(request))
    }

    /// Cancel only the identified waiting transaction. Never unlink the RTOS's
    /// confirmed journal, even if confirmation races this request.
    pub fn cancel_software_request(
        &self,
        target_request_id: &str,
        domain: SoftwareDomain,
    ) -> Result<SoftwareRequest> {
        ensure!(
            crate::protocol::valid_request_id(target_request_id),
            "Invalid target request ID"
        );
        let mut request = self
            .software_request()?
            .context("The software request is unknown")?;
        ensure!(
            request.request_id.eq_ignore_ascii_case(target_request_id) && request.domain == domain,
            "The target software request does not match"
        );
        if request.status == SoftwareStatus::Cancelled {
            return Ok(request);
        }
        ensure!(
            request.status == SoftwareStatus::Running && self.state()? == "waiting",
            "The software request cannot be cancelled"
        );
        request.phase = SoftwarePhase::Cancelling;
        request.cancellable = false;
        self.append_request(request.clone())?;
        // Atomic unlink vs the RTOS rename decides who crossed the boundary.
        ensure!(
            self.state()? == "waiting",
            "The camera already confirmed the software request"
        );
        fs::remove_file(self.path(REQUEST_FILE))
            .context("The camera may already have confirmed the software request")?;
        ensure!(
            !self.journal_files()?.contains(&JOURNAL_FILE),
            "The camera already confirmed the software request"
        );
        ensure!(
            SoftwareIdentity::of(&self.live_files()?) == request.before,
            "Recover changed software before completing cancellation"
        );
        request.status = SoftwareStatus::Cancelled;
        request.phase = SoftwarePhase::Completed;
        self.append_request(request.clone())?;
        // The staged content stays for the next update to reclaim. VERSION_FILE
        // is harmless without a request and does not affect software identity.
        Ok(request)
    }

    pub fn rollback_availability(&self) -> Result<RollbackAvailability> {
        self.rollback_availability_with_keys(&release::OFFICIAL_KEYS)
    }

    /// A backup version string is not a format proof. Reconstruct the exact
    /// target from OLD plus unchanged live files, then verify its signed release
    /// evidence against that complete target before trusting dataFormats.
    pub fn rollback_availability_with_keys(
        &self,
        keys: &[[u8; 32]],
    ) -> Result<RollbackAvailability> {
        let rollback = self.rollback()?;
        let record = self.read_record()?;
        let identity = if rollback.is_some() {
            record.as_ref().and_then(|record| record.before.clone())
        } else {
            None
        };
        let mut availability = RollbackAvailability {
            available: identity.is_some(),
            allowed: false,
            reason: if identity.is_some() {
                "data_format_incompatible"
            } else {
                "backup_unavailable"
            },
            identity,
            version: rollback.and_then(|rollback| rollback.version),
            trusted_format_support: None,
        };
        let Some(record) = record.filter(|_| availability.available) else {
            return Ok(availability);
        };
        let proof = (|| -> Result<release::Statement> {
            let target = self.rollback_target(&record)?;
            ensure!(
                record
                    .backup
                    .iter()
                    .chain(&record.added)
                    .all(|path| rollback_owned(path)),
                "The backup contains an unsupported rollback path"
            );
            ensure!(
                availability.identity.as_ref() == Some(&SoftwareIdentity::of(&target)),
                "The target backup identity changed"
            );
            let evidence = target
                .get(release::CARD_STATEMENT)
                .context("The target has no signed data-format evidence")?;
            let signature = target
                .get(release::CARD_SIGNATURE)
                .context("The target has no release signature")?;
            let statement = release::verify(evidence, signature, keys)
                .context("Verify the target release evidence")?;
            let mut entries: Files = target
                .iter()
                .filter(|(path, _)| rollback_owned(path))
                .map(|(path, bytes)| (format!("SD_ROOT/{path}"), bytes.clone()))
                .collect();
            entries.insert(release::STATEMENT_ENTRY.into(), evidence.clone());
            entries.insert(release::SIGNATURE_ENTRY.into(), signature.clone());
            release::check_files(&entries, &statement)
                .context("Verify the complete rollback target")?;
            Ok(statement)
        })();
        if let Ok(statement) = proof {
            availability.version = Some(statement.version);
            if let Some(version) = statement.data_formats.get("lookLibrary") {
                availability.trusted_format_support = Some(vec![*version]);
                if *version == crate::library::FORMAT_VERSION {
                    availability.allowed = true;
                    availability.reason = "ready";
                }
            }
        }
        Ok(availability)
    }

    /// Build the target without writing or consuming its backup. Every restored
    /// file is checked against the transaction's recorded bytes.
    fn rollback_target(&self, record: &Record) -> Result<Files> {
        let mut target = self.live_files()?;
        for path in &record.added {
            target.remove(path);
        }
        for path in &record.backup {
            let (_, old) = update_paths(path)?;
            let bytes = fs::read(self.path(&old)).with_context(|| format!("Read backup {path}"))?;
            ensure!(
                record.backup_hashes.get(path) == Some(&packaging::sha256(&bytes)),
                "The backup content changed: {path}"
            );
            target.insert(path.clone(), bytes);
        }
        ensure!(
            record.before.as_ref() == Some(&SoftwareIdentity::of(&target)),
            "The complete target backup identity changed"
        );
        Ok(target)
    }
}

pub fn stage_rollback_request(
    card: &Card,
    target_identity: &str,
    context: &SoftwareContext,
) -> Result<SoftwareRequest> {
    stage_rollback_request_with_keys(card, target_identity, context, &release::OFFICIAL_KEYS)
}

pub fn stage_rollback_request_with_keys(
    card: &Card,
    target_identity: &str,
    context: &SoftwareContext,
    keys: &[[u8; 32]],
) -> Result<SoftwareRequest> {
    ensure!(context.valid(), "Invalid software request context");
    ensure!(
        card.state()? == "none",
        "A software request is already pending"
    );
    let availability = card.rollback_availability_with_keys(keys)?;
    let expected = availability
        .identity
        .filter(|identity| identity.digest == target_identity)
        .context("The target backup identity does not match")?;
    ensure!(
        availability.allowed,
        "data_format_incompatible: the backup has no verified compatible data-format support"
    );
    // All target/context/proof checks above are read-only. Only a valid rollback
    // may reconcile and append the previous transaction's terminal checkpoint.
    if let Some(previous) = card.software_request()? {
        ensure!(
            previous.status != SoftwareStatus::Running,
            "Recover the previous software request before rolling back"
        );
    }
    let before = SoftwareIdentity::of(&card.live_files()?);
    ensure!(
        before != expected,
        "The current software already matches the rollback target"
    );
    let rollback = card
        .rollback()?
        .context("The target backup changed before staging")?;
    let request = SoftwareRequest {
        request_id: context.request_id.to_ascii_lowercase(),
        domain: SoftwareDomain::Rollback,
        boot_id: context.boot_id.clone(),
        card_id: context.card_id.clone(),
        status: SoftwareStatus::Running,
        phase: SoftwarePhase::AwaitingConfirmation,
        cancellable: true,
        expected,
        before,
        error: None,
    };
    if let Err(error) = stage_rollback_inner(
        card,
        rollback,
        availability.version.as_deref(),
        Some(&request),
    ) {
        fail_unstaged(card, &request.request_id);
        return Err(error);
    }
    card.software_request()?
        .context("The rollback request was not retained")
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
    backup_hashes: BTreeMap<String, String>,
    before: Option<SoftwareIdentity>,
    request: Option<SoftwareRequest>,
}

impl Record {
    fn of(changes: &[Change], live: &Files, new: &Files, version: Option<String>) -> Record {
        let mut record = Record {
            version,
            installed: BTreeMap::new(),
            added: Vec::new(),
            backup: Vec::new(),
            backup_hashes: BTreeMap::new(),
            before: Some(SoftwareIdentity::of(live)),
            request: None,
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
        for path in &record.backup {
            if let Some(bytes) = live.get(path) {
                record
                    .backup_hashes
                    .insert(path.clone(), packaging::sha256(bytes));
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
            "backupHashes": self.backup_hashes,
            "before": self.before,
            "request": self.request,
        })
        .to_string()
        .into_bytes()
        .into_iter()
        .chain(*b"\n")
        .collect()
    }

    fn parse(bytes: &[u8]) -> Option<Record> {
        if bytes.len() > MAX_RECORD_BYTES {
            return None;
        }
        let mut lines = bytes.split_inclusive(|byte| *byte == b'\n');
        let base = lines.next()?;
        let value: Value = grkit_core::effects::parse_json(base).ok()?;
        let paths = |key: &str| -> Option<Vec<String>> {
            value[key]
                .as_array()?
                .iter()
                .map(|path| path.as_str().map(str::to_owned))
                .collect()
        };
        let mut record = Record {
            version: value["version"].as_str().map(str::to_owned),
            installed: value["installed"]
                .as_object()?
                .iter()
                .map(|(path, hash)| Some((path.clone(), hash.as_str()?.to_owned())))
                .collect::<Option<_>>()?,
            added: paths("added")?,
            backup: paths("backup")?,
            backup_hashes: serde_json::from_value(
                value
                    .get("backupHashes")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )
            .ok()?,
            before: serde_json::from_value(value.get("before").cloned().unwrap_or(Value::Null))
                .ok()?,
            request: serde_json::from_value(value.get("request").cloned().unwrap_or(Value::Null))
                .ok()?,
        };
        if record
            .request
            .as_ref()
            .is_some_and(|request| !request.valid())
            || record
                .before
                .as_ref()
                .is_some_and(|identity| !identity.valid())
        {
            return None;
        }
        for (index, line) in lines.enumerate() {
            if index >= MAX_CHECKPOINTS {
                return None;
            }
            // A torn append never replaces the previous complete request state.
            if !line.ends_with(b"\n") {
                break;
            }
            let checkpoint: Checkpoint = serde_json::from_slice(line).ok()?;
            if !checkpoint.valid() {
                return None;
            }
            record.request = Some(checkpoint.request);
        }
        Some(record)
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
        let Some(record) = self.read_record()? else {
            return Ok(None);
        };
        if record.installed.is_empty() && record.backup.is_empty() && record.added.is_empty() {
            return Ok(None);
        }
        let backed_up = record.backup.iter().all(|path| {
            update_paths(path).is_ok_and(|(_, old)| {
                fs::read(self.path(&old)).is_ok_and(|bytes| {
                    record
                        .backup_hashes
                        .get(path)
                        .is_some_and(|hash| packaging::sha256(&bytes) == *hash)
                })
            })
        });
        let live = self.live_files()?;
        let unchanged = record.installed.iter().all(|(path, hash)| {
            live.get(path)
                .is_some_and(|bytes| packaging::sha256(bytes) == *hash)
        });
        let mut target = live;
        for path in &record.added {
            target.remove(path);
        }
        for path in &record.backup {
            if let Ok((_, old)) = update_paths(path) {
                if let Ok(bytes) = fs::read(self.path(&old)) {
                    target.insert(path.clone(), bytes);
                }
            }
        }
        let complete = record
            .before
            .as_ref()
            .is_some_and(|expected| SoftwareIdentity::of(&target) == *expected);
        Ok((backed_up && unchanged && complete).then(|| Rollback {
            version: record.version,
            changes: record
                .backup
                .into_iter()
                .map(Change::Install)
                .chain(record.added.into_iter().map(Change::Remove))
                .collect(),
        }))
    }

    /// Actual installed bytes, including the startup script. The content
    /// identity changes even when two builds have the same version label.
    pub fn inventory(&self) -> Result<Value> {
        let files = self.live_files()?;
        let hashes: std::collections::BTreeMap<_, _> = files
            .iter()
            .filter(|(path, _)| software_owned(path))
            .map(|(path, bytes)| (path, packaging::sha256(bytes)))
            .collect();
        let backup = self
            .rollback()?
            .map(|value| json!({"version": value.version}));
        Ok(json!({"package": self.package()?, "contentHashes": hashes,
            "state": self.state()?, "backup": backup}))
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
    let version = rollback.version.clone();
    stage_rollback_inner(card, rollback, version.as_deref(), None)
}

fn stage_rollback_inner(
    card: &Card,
    rollback: Rollback,
    version: Option<&str>,
    request: Option<&SoftwareRequest>,
) -> Result<Value> {
    let journal = rollback_journal(&rollback.changes)?;
    card.forget_deleted(&[VERSION_FILE, DRAFT_FILE, REQUEST_FILE]);
    card.warm([VERSION_FILE, DRAFT_FILE, REQUEST_FILE]);
    card.clear(UPDATE_FOLDER)?;
    card.write_checked(VERSION_FILE, &version_digits(version))?;
    card.write_checked(DRAFT_FILE, &journal)?;
    // Persist and read back the transaction identity before the camera can see
    // REQUEST_FILE. A power failure here leaves recoveryRequired, never an
    // unassociated confirmation prompt or an invented terminal success.
    if let Some(request) = request {
        card.append_request(request.clone())?;
    }
    fs::rename(card.path(DRAFT_FILE), card.path(REQUEST_FILE)).context("Write the journal")?;
    ensure!(card.state()? == "waiting", "The journal is not on the card");
    let installs = rollback
        .changes
        .iter()
        .filter(|change| matches!(change, Change::Install(_)))
        .count();
    Ok(
        json!({"state":"waiting","version":version,"install":installs,"remove":rollback.changes.len()-installs}),
    )
}

fn rollback_owned(path: &str) -> bool {
    software_owned(path)
        || path == grkit_core::card::IMPORTS_FILE
        || path == packaging::RECIPES_PACKAGE_PATH
}

/// Whether the camera puts a card file of `path` there with each package.
/// The update removes only these when the new package does not have them:
/// the runtime files, the modules, and the package effects. Other files of
/// the package folder, such as the import library of the site, stay.
fn package_owned(path: &str) -> bool {
    if path == PACKAGE_LIST {
        return true;
    }
    if matches!(
        path,
        "SCRIPT/GRKIT/RELEASE.JSN" | "SCRIPT/GRKIT/RELEASE.SIG"
    ) || packaging::is_runtime_path(path)
    {
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
    // Only the core file that the package's SCAN.SH runs: a core file left
    // on the card by an earlier package names that package's version.
    let core = runtime::scanned_core(files.get(SCAN_FILE)?)?;
    packaging::marked_version(files.get(&core)?).map(str::to_owned)
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
    stage_inner(card, zip, device, keys, None)
}

pub fn stage_request(
    card: &Card,
    zip: &[u8],
    device: &Device,
    keys: &[[u8; 32]],
    context: &SoftwareContext,
) -> Result<SoftwareRequest> {
    ensure!(context.valid(), "Invalid software request context");
    ensure!(
        card.state()? == "none",
        "A software request is already pending"
    );
    if let Some(previous) = card.software_request()? {
        ensure!(
            previous.status != SoftwareStatus::Running,
            "Recover the previous software request before writing"
        );
    }
    if let Err(error) = stage_inner(card, zip, device, keys, Some(context)) {
        fail_unstaged(card, &context.request_id);
        return Err(error);
    }
    card.software_request()?
        .context("The software request was not retained")
}

/// A request whose journal never reached the card did not wait for a
/// confirmation: record its own failure, so that it does not read as one
/// that the camera let expire.
fn fail_unstaged(card: &Card, request_id: &str) {
    if card.state().ok() != Some("none") {
        return;
    }
    let Ok(Some(mut request)) = card
        .read_record()
        .map(|record| record.and_then(|r| r.request))
    else {
        return;
    };
    if request.request_id.eq_ignore_ascii_case(request_id)
        && request.status == SoftwareStatus::Running
    {
        request.status = SoftwareStatus::Failed;
        request.phase = SoftwarePhase::Completed;
        request.cancellable = false;
        request.error = Some(STAGING_FAILED.into());
        let _ = card.append_request(request);
    }
}

fn stage_inner(
    card: &Card,
    zip: &[u8],
    device: &Device,
    keys: &[[u8; 32]],
    context: Option<&SoftwareContext>,
) -> Result<Value> {
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
    let mut record = Record::of(&changes, &before, &new, version_of(&live));
    if let Some(context) = context {
        record.request = Some(SoftwareRequest {
            request_id: context.request_id.to_ascii_lowercase(),
            domain: SoftwareDomain::Update,
            boot_id: context.boot_id.clone(),
            card_id: context.card_id.clone(),
            status: if changes.is_empty() {
                SoftwareStatus::Succeeded
            } else {
                SoftwareStatus::Running
            },
            phase: if changes.is_empty() {
                SoftwarePhase::Completed
            } else {
                SoftwarePhase::AwaitingConfirmation
            },
            cancellable: !changes.is_empty(),
            expected: SoftwareIdentity::of(&new),
            before: SoftwareIdentity::of(&before),
            error: None,
        });
        ensure!(
            record.to_bytes().len() <= MAX_RECORD_BYTES,
            "The software request record is too large"
        );
    }
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
        if let Some(request) = record.request.clone() {
            if card.read_record()?.is_some() {
                card.append_request(request)?;
            } else {
                card.write_checked(BACKUP_RECORD, &record.to_bytes())?;
            }
        }
        return Ok(summary("none", 0));
    }
    card.forget_deleted(&[VERSION_FILE, DRAFT_FILE, REQUEST_FILE]);
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

    fn compatible_package(version: &str, effects: &[&str], format: Option<u32>) -> Vec<u8> {
        let runtimes = testing::release_runtimes(&camera::GR3, version);
        let modules = testing::official_modules();
        let mut statement =
            release::Statement::of_release(&runtimes, &modules, &camera::GR3).unwrap();
        statement.data_formats.clear();
        if let Some(version) = format {
            statement.data_formats.insert("lookLibrary".into(), version);
        }
        let statement = statement.to_bytes().unwrap();
        let signature = release::SigningKey::from_hex(testing::RELEASE_SEED)
            .unwrap()
            .sign(&statement);
        packaging::assemble_with(
            &runtimes,
            &testing::effect_set(effects),
            &modules,
            &camera::GR3,
            &packaging::Guide {
                language: grkit_core::language::Language::English,
                pdf: b"%PDF-1.7\n% test guide\n",
            },
            packaging::Extras {
                release: Some((&statement, &signature)),
                ..packaging::Extras::default()
            },
        )
        .unwrap()
        .bytes
    }

    fn card_snapshot(card: &Card) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut pending = vec![card.root.clone()];
        let mut files = BTreeMap::new();
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path());
                } else {
                    files.insert(
                        entry.path().strip_prefix(&card.root).unwrap().to_owned(),
                        fs::read(entry.path()).unwrap(),
                    );
                }
            }
        }
        files
    }

    fn keys() -> [[u8; 32]; 1] {
        [testing::release_key()]
    }

    fn gr3() -> Device {
        Device {
            model: "RICOH GR III".into(),
            firmware: "2.21".into(),
            free_bytes: Some(1 << 30),
            serial_number: Some("test-camera".into()),
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

    fn request_context() -> SoftwareContext {
        SoftwareContext {
            request_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            boot_id: "service-boot".into(),
            card_id: "card-identity".into(),
        }
    }

    #[test]
    fn request_survives_restart_and_success_requires_installed_hashes() {
        let (dir, card) = card_with(&package(&["first"]));
        let next = package(&["second"]); // Deliberately the same version.
        let request = stage_request(&card, &next, &gr3(), &keys(), &request_context()).unwrap();
        assert_eq!(request.status, SoftwareStatus::Running);
        assert_eq!(request.phase, SoftwarePhase::AwaitingConfirmation);
        assert!(request.cancellable);
        assert_ne!(request.expected, request.before);
        assert_eq!(
            Card::new(dir.path())
                .software_request()
                .unwrap()
                .unwrap()
                .request_id,
            request.request_id
        );
        fs::rename(card.path(REQUEST_FILE), card.path(JOURNAL_FILE)).unwrap();
        assert_eq!(
            card.software_request().unwrap().unwrap().phase,
            SoftwarePhase::AwaitingRestart
        );
        fs::rename(card.path(JOURNAL_FILE), card.path(REQUEST_FILE)).unwrap();
        run_journal(&card);
        let complete = card.software_request().unwrap().unwrap();
        assert_eq!(complete.status, SoftwareStatus::Succeeded);
        assert_eq!(
            complete.expected,
            SoftwareIdentity::of(&card.live_files().unwrap())
        );
        assert_eq!(
            Card::new(dir.path()).software_request().unwrap().unwrap(),
            complete
        );
        assert!(!complete
            .expected
            .content_hashes
            .keys()
            .any(|path| path.contains("COMMIT.")));
    }

    #[test]
    fn a_request_whose_journal_was_not_written_fails_as_unstaged() {
        let (_dir, card) = card_with(&package(&["first"]));
        let request = stage_request(
            &card,
            &package(&["second"]),
            &gr3(),
            &keys(),
            &request_context(),
        )
        .unwrap();
        // As when the rename of the journal failed after the checkpoint.
        fs::remove_file(card.path(REQUEST_FILE)).unwrap();
        fail_unstaged(&card, &request.request_id);
        let failed = card.software_request().unwrap().unwrap();
        assert_eq!(failed.status, SoftwareStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some(STAGING_FAILED));
        let mut next = request_context();
        next.request_id = "650e8400-e29b-41d4-a716-446655440000".into();
        stage_request(&card, &package(&["second"]), &gr3(), &keys(), &next).unwrap();
        assert_eq!(card.state().unwrap(), "waiting");
    }
    #[test]
    fn an_expired_request_fails_durably_and_a_changed_card_still_blocks_updates() {
        let (dir, card) = card_with(&package(&["first"]));
        stage_request(
            &card,
            &package(&["second"]),
            &gr3(),
            &keys(),
            &request_context(),
        )
        .unwrap();
        // The camera deletes a request that nobody confirms in time.
        fs::remove_file(card.path(REQUEST_FILE)).unwrap();
        let expired = card.software_request().unwrap().unwrap();
        assert_eq!(expired.status, SoftwareStatus::Failed);
        assert_eq!(expired.phase, SoftwarePhase::Completed);
        assert_eq!(expired.error.as_deref(), Some(CONFIRMATION_EXPIRED));
        assert!(!expired.cancellable);
        // The outcome survives a restart of the service.
        assert_eq!(
            Card::new(dir.path()).software_request().unwrap().unwrap(),
            expired
        );
        let mut next = request_context();
        next.request_id = "650e8400-e29b-41d4-a716-446655440000".into();
        stage_request(&card, &package(&["third"]), &gr3(), &keys(), &next).unwrap();
        // Software that is neither the old nor the new one is uncertain.
        fs::remove_file(card.path(REQUEST_FILE)).unwrap();
        let live = card.path(grkit_core::card::PACKAGE_LIST);
        let mut list = fs::read(&live).unwrap();
        list.extend_from_slice(b"\n");
        fs::write(&live, list).unwrap();
        let recovered = card.software_request().unwrap().unwrap();
        assert_eq!(recovered.status, SoftwareStatus::Running);
        assert_eq!(recovered.phase, SoftwarePhase::Recovering);
        assert!(!recovered.cancellable);
        next.request_id = "750e8400-e29b-41d4-a716-446655440000".into();
        assert!(stage_request(&card, &package(&["fourth"]), &gr3(), &keys(), &next).is_err());
    }

    #[test]
    fn cancellation_is_targeted_durable_and_cannot_remove_a_confirmed_journal() {
        let (dir, card) = card_with(&package(&["first"]));
        let ctx = request_context();
        stage_request(&card, &package(&["second"]), &gr3(), &keys(), &ctx).unwrap();
        assert!(card
            .cancel_software_request(
                "650e8400-e29b-41d4-a716-446655440000",
                SoftwareDomain::Update
            )
            .is_err());
        assert!(card
            .cancel_software_request(&ctx.request_id, SoftwareDomain::Rollback)
            .is_err());
        assert_eq!(card.state().unwrap(), "waiting");
        fs::rename(card.path(REQUEST_FILE), card.path(JOURNAL_FILE)).unwrap();
        assert!(card
            .cancel_software_request(&ctx.request_id, SoftwareDomain::Update)
            .is_err());
        assert!(card.path(JOURNAL_FILE).is_file());
        fs::rename(card.path(JOURNAL_FILE), card.path(REQUEST_FILE)).unwrap();
        let cancelled = card
            .cancel_software_request(&ctx.request_id, SoftwareDomain::Update)
            .unwrap();
        assert_eq!(cancelled.status, SoftwareStatus::Cancelled);
        assert_eq!(card.state().unwrap(), "none");
        assert_eq!(
            Card::new(dir.path()).software_request().unwrap().unwrap(),
            cancelled
        );
        assert_eq!(
            card.cancel_software_request(&ctx.request_id, SoftwareDomain::Update)
                .unwrap(),
            cancelled
        );
    }

    #[test]
    fn interrupted_cancellation_recovers_without_overwriting_the_backup_record() {
        let (dir, card) = card_with(&package(&["first"]));
        let ctx = request_context();
        let mut request =
            stage_request(&card, &package(&["second"]), &gr3(), &keys(), &ctx).unwrap();
        request.phase = SoftwarePhase::Cancelling;
        request.cancellable = false;
        card.append_request(request).unwrap();
        fs::remove_file(card.path(REQUEST_FILE)).unwrap();
        let before = card.read_record_bytes().unwrap().unwrap();
        // A torn terminal checkpoint is ignored, then safely truncated when
        // recovery appends its verified terminal outcome.
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(card.path(BACKUP_RECORD))
            .unwrap();
        file.write_all(b"{\"request\":").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let result = Card::new(dir.path()).software_request().unwrap().unwrap();
        assert_eq!(result.status, SoftwareStatus::Cancelled);
        assert!(card
            .read_record_bytes()
            .unwrap()
            .unwrap()
            .starts_with(&before));
    }

    #[test]
    fn compatible_signed_rollback_retains_its_request_and_restores_exact_identity() {
        let first = compatible_package("1.7.0", &["first"], Some(1));
        let (dir, card) = card_with(&first);
        let original = SoftwareIdentity::of(&card.live_files().unwrap());
        let personal = card.path("GRKIT/EFFECTS/CUSTOM.GRF");
        fs::create_dir_all(personal.parent().unwrap()).unwrap();
        fs::write(&personal, b"personal content").unwrap();
        // Package journals must preserve the entire activated library, including
        // official removal identities and immutable custom content generations.
        let content = testing::effect("custom");
        let hash = packaging::sha256(&content);
        let mut manifest = crate::library::Manifest::default();
        let custom = crate::library::Entry {
            name: "Test".into(),
            content_hash: hash.clone(),
            enabled: true,
            origin: crate::library::Origin::Custom,
            removed: false,
            features: 0,
            monochrome: false,
        };
        manifest.entries.insert("custom".into(), custom.clone());
        manifest.order.push("custom".into());
        manifest.entries.insert(
            "first".into(),
            crate::library::Entry {
                enabled: false,
                removed: true,
                origin: crate::library::Origin::Official,
                content_hash: packaging::sha256(&testing::effect("first")),
                ..custom
            },
        );
        let payload = manifest.encode_binary().unwrap();
        let crc = crc32fast::hash(&payload);
        let mut slot = vec![0u8; 32768];
        slot[..8].copy_from_slice(b"GRKSLOT1");
        for (offset, value) in [(8, 1u32), (12, payload.len() as u32), (16, crc), (24, 1)] {
            slot[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let header_crc = crc32fast::hash(&slot[..32]);
        slot[20..24].copy_from_slice(&header_crc.to_le_bytes());
        slot[32..32 + payload.len()].copy_from_slice(&payload);
        for (offset, value) in [(32752, 0x54494d43u32), (32756, 1), (32760, crc)] {
            slot[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let commit_crc = crc32fast::hash(&slot[32752..]);
        slot[32764..].copy_from_slice(&commit_crc.to_le_bytes());
        let mut activation = vec![0u8; 32];
        activation[..8].copy_from_slice(b"GRKACT01");
        for (offset, value) in [(8, 1u32), (12, 1), (16, crc)] {
            activation[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let activation_crc = crc32fast::hash(&activation);
        activation[24..28].copy_from_slice(&activation_crc.to_le_bytes());
        let library_files = BTreeMap::from([
            ("GRKIT/LIB0.BIN".to_owned(), slot),
            ("GRKIT/LIB1.BIN".to_owned(), vec![0u8; 32768]),
            ("GRKIT/LIB.ACT".to_owned(), activation),
            (
                format!("GRKIT/CONTENT/{}.GRF", hash[..8].to_uppercase()),
                content,
            ),
        ]);
        for (name, bytes) in &library_files {
            let path = card.path(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let assert_library_preserved = || {
            for (name, bytes) in &library_files {
                assert_eq!(fs::read(card.path(name)).unwrap(), *bytes, "{name}");
            }
        };
        let ctx = request_context();
        let update = stage_request(
            &card,
            &compatible_package("1.8.0", &["second"], Some(1)),
            &gr3(),
            &keys(),
            &ctx,
        )
        .unwrap();
        run_journal(&card);
        assert_library_preserved();
        let availability = card.rollback_availability_with_keys(&keys()).unwrap();
        assert!(availability.available && availability.allowed);
        assert_eq!(availability.reason, "ready");
        assert_eq!(availability.trusted_format_support, Some(vec![1]));
        assert_eq!(availability.identity.as_ref(), Some(&original));
        let mut rollback_context = ctx;
        rollback_context.request_id = "650e8400-e29b-41d4-a716-446655440000".into();
        let rollback =
            stage_rollback_request_with_keys(&card, &original.digest, &rollback_context, &keys())
                .unwrap();
        assert_eq!(rollback.domain, SoftwareDomain::Rollback);
        assert_eq!(rollback.expected, original);
        assert_eq!(rollback.before, update.expected);
        assert_eq!(rollback.phase, SoftwarePhase::AwaitingConfirmation);
        assert_eq!(
            Card::new(dir.path()).software_request().unwrap().unwrap(),
            rollback
        );
        // The durable link exists while REQUEST is visible to the camera.
        assert_eq!(
            card.read_record().unwrap().unwrap().request.as_ref(),
            Some(&rollback)
        );
        assert!(card.path(REQUEST_FILE).exists());
        fs::rename(card.path(REQUEST_FILE), card.path(JOURNAL_FILE)).unwrap();
        assert_eq!(
            card.software_request().unwrap().unwrap().phase,
            SoftwarePhase::AwaitingRestart
        );
        assert!(card
            .cancel_software_request(&rollback.request_id, SoftwareDomain::Rollback)
            .is_err());
        fs::rename(card.path(JOURNAL_FILE), card.path(REQUEST_FILE)).unwrap();
        run_journal(&card);
        let completed = Card::new(dir.path()).software_request().unwrap().unwrap();
        assert_eq!(completed.status, SoftwareStatus::Succeeded);
        // The files that the rollback put aside have no later use.
        let mut left = Vec::new();
        let mut folders = vec![card.path(UPDATE_FOLDER)];
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(folder).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    folders.push(entry.path());
                } else {
                    left.push(entry.path());
                }
            }
        }
        assert!(left.is_empty(), "{left:?}");
        assert_eq!(
            completed.expected,
            SoftwareIdentity::of(&card.live_files().unwrap())
        );
        assert_eq!(completed.expected, original);
        assert_eq!(fs::read(personal).unwrap(), b"personal content");
        assert_library_preserved();
        assert_eq!(
            Card::new(dir.path()).software_request().unwrap().unwrap(),
            completed
        );
        assert!(
            !card
                .rollback_availability_with_keys(&keys())
                .unwrap()
                .available
        );
    }

    #[test]
    fn compatible_rollback_can_be_cancelled_and_restarted_without_losing_backup() {
        let (dir, card) = card_with(&compatible_package("1.7.0", &["first"], Some(1)));
        let mut ctx = request_context();
        stage_request(
            &card,
            &compatible_package("1.8.0", &["second"], Some(1)),
            &gr3(),
            &keys(),
            &ctx,
        )
        .unwrap();
        run_journal(&card);
        let target = card
            .rollback_availability_with_keys(&keys())
            .unwrap()
            .identity
            .unwrap();
        ctx.request_id = "650e8400-e29b-41d4-a716-446655440000".into();
        let request =
            stage_rollback_request_with_keys(&card, &target.digest, &ctx, &keys()).unwrap();
        let before = request.before.clone();
        assert!(card
            .cancel_software_request(&ctx.request_id, SoftwareDomain::Update)
            .is_err());
        let cancelled = card
            .cancel_software_request(&ctx.request_id, SoftwareDomain::Rollback)
            .unwrap();
        assert_eq!(cancelled.status, SoftwareStatus::Cancelled);
        assert_eq!(SoftwareIdentity::of(&card.live_files().unwrap()), before);
        assert_eq!(
            Card::new(dir.path()).software_request().unwrap().unwrap(),
            cancelled
        );
        assert!(
            card.rollback_availability_with_keys(&keys())
                .unwrap()
                .allowed
        );
        ctx.request_id = "750e8400-e29b-41d4-a716-446655440000".into();
        stage_rollback_request_with_keys(&card, &target.digest, &ctx, &keys()).unwrap();
        fs::remove_file(card.path(REQUEST_FILE)).unwrap();
        let expired = Card::new(dir.path()).software_request().unwrap().unwrap();
        assert_eq!(expired.status, SoftwareStatus::Failed);
        assert_eq!(expired.error.as_deref(), Some(CONFIRMATION_EXPIRED));
        // The backup is unchanged, so the rollback can be requested again.
        ctx.request_id = "850e8400-e29b-41d4-a716-446655440000".into();
        stage_rollback_request_with_keys(&card, &target.digest, &ctx, &keys()).unwrap();
        assert_eq!(card.state().unwrap(), "waiting");
    }

    #[test]
    fn stale_target_unknown_keys_and_unsupported_formats_are_rejected_before_writes() {
        for format in [None, Some(1), Some(2)] {
            let (_dir, card) = card_with(&compatible_package("1.7.0", &["first"], format));
            stage_request(
                &card,
                &compatible_package("1.8.0", &["second"], Some(1)),
                &gr3(),
                &keys(),
                &request_context(),
            )
            .unwrap();
            run_journal(&card);
            let target = card
                .rollback_availability_with_keys(&keys())
                .unwrap()
                .identity
                .unwrap();
            let before = card_snapshot(&card);
            assert!(stage_rollback_request_with_keys(
                &card,
                &"0".repeat(64),
                &request_context(),
                &keys()
            )
            .is_err());
            assert_eq!(card_snapshot(&card), before);
            assert!(!card.rollback_availability().unwrap().allowed);
            assert!(stage_rollback_request(&card, &target.digest, &request_context()).is_err());
            assert_eq!(card_snapshot(&card), before);
            if format != Some(1) {
                let availability = card.rollback_availability_with_keys(&keys()).unwrap();
                assert!(!availability.allowed);
                assert_eq!(
                    availability.trusted_format_support,
                    format.map(|value| vec![value])
                );
                assert!(stage_rollback_request_with_keys(
                    &card,
                    &target.digest,
                    &request_context(),
                    &keys()
                )
                .is_err());
                assert_eq!(card_snapshot(&card), before);
            }
        }
    }

    #[test]
    fn unchanged_live_release_evidence_can_prove_the_rollback_target() {
        // Only the selected GRF/list differs. Both releases have byte-identical
        // runtime evidence, so that evidence stays live instead of in OLD.
        let (_dir, card) = card_with(&compatible_package("1.7.0", &["first"], Some(1)));
        stage_request(
            &card,
            &compatible_package("1.7.0", &["second"], Some(1)),
            &gr3(),
            &keys(),
            &request_context(),
        )
        .unwrap();
        run_journal(&card);
        let record = card.read_record().unwrap().unwrap();
        assert!(!record.backup.contains(&release::CARD_STATEMENT.to_owned()));
        assert!(!record.backup.contains(&release::CARD_SIGNATURE.to_owned()));
        let target = card.rollback_availability_with_keys(&keys()).unwrap();
        assert!(target.allowed);
        let mut ctx = request_context();
        ctx.request_id = "650e8400-e29b-41d4-a716-446655440000".into();
        stage_rollback_request_with_keys(&card, &target.identity.unwrap().digest, &ctx, &keys())
            .unwrap();
        run_journal(&card);
        assert_eq!(
            card.software_request().unwrap().unwrap().status,
            SoftwareStatus::Succeeded
        );
    }

    #[test]
    fn target_proof_must_match_runtime_even_if_unsigned_record_is_rewritten() {
        for tamper_signature in [false, true] {
            let (_dir, card) = card_with(&compatible_package("1.7.0", &["first"], Some(1)));
            stage_request(
                &card,
                &compatible_package("1.8.0", &["second"], Some(1)),
                &gr3(),
                &keys(),
                &request_context(),
            )
            .unwrap();
            run_journal(&card);
            let mut record = card.read_record().unwrap().unwrap();
            // Either substitute genuine evidence for the wrong runtime, or alter
            // the target statement while retaining its original signature. Rewrite
            // every unsigned hash so only verification of signed evidence catches it.
            for path in [release::CARD_STATEMENT, release::CARD_SIGNATURE] {
                let (_, old) = update_paths(path).unwrap();
                let bytes = if tamper_signature {
                    let original = fs::read(card.path(&old)).unwrap();
                    if path == release::CARD_STATEMENT {
                        let mut value: serde_json::Value =
                            serde_json::from_slice(&original).unwrap();
                        value["version"] = "9.9.9".into();
                        serde_json::to_vec(&value).unwrap()
                    } else {
                        original
                    }
                } else {
                    fs::read(card.path(path)).unwrap()
                };
                fs::write(card.path(&old), &bytes).unwrap();
                record
                    .backup_hashes
                    .insert(path.into(), packaging::sha256(&bytes));
            }
            let mut target = card.live_files().unwrap();
            for path in &record.added {
                target.remove(path);
            }
            for path in &record.backup {
                let (_, old) = update_paths(path).unwrap();
                target.insert(path.clone(), fs::read(card.path(&old)).unwrap());
            }
            record.before = Some(SoftwareIdentity::of(&target));
            fs::write(card.path(BACKUP_RECORD), record.to_bytes()).unwrap();
            let availability = card.rollback_availability_with_keys(&keys()).unwrap();
            assert!(availability.available);
            assert!(!availability.allowed);
            assert!(availability.trusted_format_support.is_none());
            let before = card_snapshot(&card);
            assert!(stage_rollback_request_with_keys(
                &card,
                &availability.identity.unwrap().digest,
                &request_context(),
                &keys()
            )
            .is_err());
            assert_eq!(card_snapshot(&card), before);
        }
    }

    #[test]
    fn rollback_reports_identity_but_blocks_unknown_format_support_before_writing() {
        let (_dir, card) = card_with(&package(&["first"]));
        let ctx = request_context();
        stage_request(&card, &package(&["second"]), &gr3(), &keys(), &ctx).unwrap();
        run_journal(&card);
        let state = card.rollback_availability().unwrap();
        assert!(state.available);
        assert!(!state.allowed);
        assert_eq!(state.reason, "data_format_incompatible");
        let identity = state.identity.unwrap();
        assert!(stage_rollback_request(&card, &identity.digest, &ctx).is_err());
        assert_eq!(card.state().unwrap(), "none");
        assert!(!card.path(REQUEST_FILE).exists());
        let record = card.read_record().unwrap().unwrap();
        let (_, old) = update_paths(&record.backup[0]).unwrap();
        fs::write(card.path(&old), b"tampered").unwrap();
        assert!(!card.rollback_availability().unwrap().available);
    }

    #[test]
    fn noop_request_keeps_the_existing_backup_and_invalid_context_writes_nothing() {
        let (_dir, card) = card_with(&package(&["first"]));
        let mut invalid = request_context();
        invalid.request_id = "not-a-uuid".into();
        assert!(stage_request(&card, &package(&["second"]), &gr3(), &keys(), &invalid).is_err());
        assert!(!card.path(BACKUP_RECORD).exists());
        let next = package(&["second"]);
        stage_request(&card, &next, &gr3(), &keys(), &request_context()).unwrap();
        run_journal(&card);
        let backup = card.rollback_availability().unwrap().identity;
        let mut second = request_context();
        second.request_id = "650e8400-e29b-41d4-a716-446655440000".into();
        let noop = stage_request(&card, &next, &gr3(), &keys(), &second).unwrap();
        assert_eq!(noop.status, SoftwareStatus::Succeeded);
        assert_eq!(noop.request_id, second.request_id);
        assert_eq!(card.rollback_availability().unwrap().identity, backup);
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
            runtime::package_runtime_files(&[&binary, &binary], runtime::ICON_STRIDE)
        };
        assert_eq!(version_file(&core("1.7.0")), b"010700");
        assert_eq!(version_file(&core("12.3.45")), b"120345");
        assert_eq!(version_file(&core("1.7")), b"000000");
        assert_eq!(version_file(&core("100.0.0")), b"000000");
        assert_eq!(version_file(&Files::new()), b"000000");
        // A core file left by an earlier package does not name the version.
        let mut left = core("1.7.0");
        let mut old = testing::installer();
        old.extend_from_slice(format!("{}1.6.2\0", packaging::VERSION_MARKER).as_bytes());
        left.insert(format!("{PACKAGE_FOLDER}/CORE.BIN"), old);
        assert_eq!(version_file(&left), b"010700");
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
                serial_number: None,
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
