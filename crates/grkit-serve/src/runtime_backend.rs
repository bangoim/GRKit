//! Runtime management boundary. A successful apply is a durable RTOS commit
//! with a matching runtime generation, never merely an uploaded file.
use crate::{
    library::{Context, Manifest, Prepared, MAX_IDENTITIES, MAX_LISTED, MAX_LOADED},
    protocol::{strong_etag, Problem},
};
use grkit_core::{abi::GRF_FEATURES, effects, packaging};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Problem>;

#[derive(Clone, Debug)]
pub struct RuntimeSnapshot {
    pub manifest: Manifest,
    /// Storage-owner identity, verified independently of the HTTP request.
    pub card_id: String,
    pub runtime_epoch: [u32; 4],
    pub context: Context,
    pub loaded_ids: BTreeSet<String>,
    /// Exact immutable GRF bytes keyed by their SHA-256, not by a filename.
    pub contents: BTreeMap<String, Vec<u8>>,
    /// Monotonic within the runtime epoch. Never wrap or reset on HTTP reconnect.
    pub generation: u32,
}
impl RuntimeSnapshot {
    pub fn revision(&self) -> Result<String> {
        // Internal commit pins also bind the owner generation. Resource ETags
        // remain content hashes, but an intervening edit-and-undo must not reuse
        // a stale receipt baseline merely because the configuration is equal.
        let mut bytes = self.manifest.encode()?;
        bytes.extend_from_slice(&(self.card_id.len() as u32).to_le_bytes());
        bytes.extend_from_slice(self.card_id.as_bytes());
        for word in self.runtime_epoch {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&self.generation.to_le_bytes());
        Ok(strong_etag(&bytes))
    }
    pub fn content(&self, id: &str) -> Result<&[u8]> {
        let entry = self
            .manifest
            .entries
            .get(id)
            .filter(|entry| !entry.removed)
            .ok_or_else(|| Problem::new(404, "look_not_found", "The Look does not exist."))?;
        self.contents
            .get(&entry.content_hash)
            .map(Vec::as_slice)
            .ok_or_else(|| invalid("The runtime snapshot is missing a content generation."))
    }
    pub fn validate(&self) -> Result<()> {
        if self.card_id.is_empty()
            || self.card_id == "unavailable"
            || self.card_id.len() > 128
            || self.card_id.bytes().any(|byte| byte < 32 || byte == 127)
            || self.runtime_epoch == [0; 4]
        {
            return Err(invalid(
                "The runtime snapshot has no verified card and epoch identity.",
            ));
        }
        self.manifest
            .validate()
            .map_err(|_| invalid("The runtime manifest is invalid or unsupported."))?;
        if self.contents.len() > MAX_LISTED
            || self.loaded_ids.len() > MAX_LOADED
            || self.context.blocked_ids.len() > MAX_IDENTITIES
            || self.context.official_hashes.len() > MAX_IDENTITIES
            || self
                .context
                .dependencies
                .values()
                .map(Vec::len)
                .sum::<usize>()
                > MAX_IDENTITIES
            || self
                .context
                .dependencies
                .values()
                .flatten()
                .any(|reference| reference.len() > 128)
        {
            return Err(invalid(
                "The runtime snapshot exceeds its supported bounds.",
            ));
        }
        for (hash, bytes) in &self.contents {
            if !matches!(bytes.len(), effects::SIZE | effects::SIZE_WITH_LEVELS)
                || packaging::sha256(bytes) != *hash
            {
                return Err(invalid(
                    "A runtime content generation has an invalid digest or size.",
                ));
            }
        }
        for (id, entry) in self
            .manifest
            .entries
            .iter()
            .filter(|(_, entry)| !entry.removed)
        {
            let bytes = self.content(id)?;
            let (effect, _) = effects::decode(bytes)
                .map_err(|_| invalid("A runtime content generation is not a valid GRF."))?;
            let features = u32::from_le_bytes(
                bytes[GRF_FEATURES..GRF_FEATURES + 4]
                    .try_into()
                    .expect("valid GRF"),
            );
            if effect.id != *id
                || effect.name != entry.name
                || features != entry.features
                || effect.monochrome() != entry.monochrome
            {
                return Err(invalid(
                    "Runtime content metadata does not match the managed manifest.",
                ));
            }
            if entry.enabled && !self.loaded_ids.contains(id) {
                return Err(invalid(
                    "An enabled Look has no acknowledged runtime record.",
                ));
            }
        }
        if self.loaded_ids.iter().any(|id| {
            self.manifest
                .entries
                .get(id)
                .is_none_or(|entry| entry.removed)
        }) || self.context.selected.as_ref().is_some_and(|id| {
            self.manifest
                .entries
                .get(id)
                .is_none_or(|entry| entry.removed)
        }) {
            return Err(invalid(
                "Runtime Look identities do not match the managed manifest.",
            ));
        }
        Ok(())
    }
    pub fn verify_receipt(&self, before: &Self, prepared: &Prepared) -> Result<()> {
        self.validate()?;
        if self.card_id != before.card_id
            || self.runtime_epoch != before.runtime_epoch
            || self.manifest != prepared.manifest
            || before.generation.checked_add(1) != Some(self.generation)
            || (prepared.select_off && self.context.selected.is_some())
        {
            return Err(invalid(
                "The acknowledged runtime generation does not match the committed candidate.",
            ));
        }
        if let Some(content) = &prepared.content {
            if self.content(&content.id)? != content.bytes.as_slice() {
                return Err(invalid(
                    "The runtime did not acknowledge the candidate content generation.",
                ));
            }
        }
        Ok(())
    }
}

/// Implementations must fail closed when the card, runtime epoch, reference
/// scan, publication guard, or durable recovery state is unavailable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeCapabilities {
    pub readable: bool,
    pub hot_reload: bool,
}
pub trait RuntimeBackend: Send {
    fn card_identity(&mut self) -> Result<String> {
        Err(unavailable())
    }
    fn invalidate_session(&mut self) {}
    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::default()
    }
    /// A coherent view from the runtime coordinator, including menu changes.
    fn snapshot(&mut self) -> Result<RuntimeSnapshot>;
    /// Under the RTOS application guard, recheck expected_revision AND current
    /// dependencies/capture/menu safety, persist through the storage owner, then
    /// publish runtime references. Return only a verified durable receipt.
    /// Before the durable boundary, ordinary errors guarantee no live change.
    /// At or after an uncertain durable boundary (including timeout), return
    /// code recovery_required and block later writes until verified recovery.
    /// The HTTP layer retains that request as running/recovering, not failed.
    fn apply(&mut self, expected_revision: &str, prepared: Prepared) -> Result<RuntimeSnapshot>;
    /// Reconcile a retained COMMIT receipt without submitting another mutation.
    fn reconcile_apply(&mut self) -> Result<Option<RuntimeSnapshot>> {
        Ok(None)
    }
}
#[derive(Default)]
pub struct UnavailableBackend;
impl RuntimeBackend for UnavailableBackend {
    fn snapshot(&mut self) -> Result<RuntimeSnapshot> {
        Err(unavailable())
    }
    fn apply(&mut self, _expected_revision: &str, _prepared: Prepared) -> Result<RuntimeSnapshot> {
        Err(unavailable())
    }
}
fn unavailable() -> Problem {
    Problem::new(
        503,
        "runtime_unavailable",
        "The runtime management bridge is unavailable.",
    )
}
fn invalid(detail: &'static str) -> Problem {
    Problem::new(503, "recovery_required", detail)
}

/// A discovered production bridge. Transport health is deliberately separate
/// from library capability: the owner must implement and acknowledge the stream protocol.
pub struct ControlBackend<M: crate::control::MailboxIo> {
    client: crate::control::Client<M>,
    nonce: [u8; 16],
    transport_verified: bool,
    snapshot_verified: bool,
    hot_reload_verified: bool,
    recovery_required: bool,
    startup_reconciling: bool,
    official_hashes: BTreeMap<String, String>,
    pending_receipt: Option<PendingReceipt>,
    pending_read: Option<crate::control::Ticket>,
    /// The owner's answer to the card identity probe, kept for the session:
    /// removing the card powers the camera off, so the card cannot change
    /// while the service runs, and each probe reads the card.
    card_identity: Option<Result<String>>,
}
struct PendingReceipt {
    before: RuntimeSnapshot,
    prepared: Prepared,
    ticket: Option<crate::control::Ticket>,
    acknowledged: bool,
}
pub type LinuxBackend = ControlBackend<crate::control::MappedMailbox>;

impl LinuxBackend {
    /// Execute only installer bytes authorized by signed release evidence.
    /// Discovery does not read or write the managed GRKIT data directory.
    pub fn discover(
        root: &std::path::Path,
        keys: &[[u8; 32]],
        nonce: [u8; 16],
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(nonce != [0; 16], "The HTTP boot nonce must be nonzero");
        let statement = crate::trust::installed_release(root, keys)?;
        let hashes = crate::trust::installer_hashes(&statement);
        let client = crate::control::discover(root, &hashes)?;
        let mut backend = Self::new(client, nonce);
        backend.official_hashes = statement.official_looks;
        Ok(backend)
    }
}
impl<M: crate::control::MailboxIo> ControlBackend<M> {
    fn new(client: crate::control::Client<M>, nonce: [u8; 16]) -> Self {
        Self {
            client,
            nonce,
            transport_verified: false,
            snapshot_verified: false,
            hot_reload_verified: false,
            recovery_required: false,
            startup_reconciling: false,
            official_hashes: BTreeMap::new(),
            pending_receipt: None,
            pending_read: None,
            card_identity: None,
        }
    }
    pub fn transport_verified(&self) -> bool {
        self.transport_verified
    }
    fn probe(&mut self) -> Result<()> {
        use crate::control::State;
        use grkit_core::abi::*;
        if self.recovery_required {
            return Err(invalid(
                "An outstanding runtime command requires reconciliation.",
            ));
        }
        if self.transport_verified {
            return Ok(());
        }
        let result = (|| -> Result<()> {
            match self
                .client
                .reconcile()
                .map_err(|_| invalid("The inherited control context is invalid."))?
            {
                State::Idle {
                    epoch,
                    next_sequence,
                    ..
                } => {
                    // A previous producer may already have released its last
                    // COMMIT or staging ticket before exiting.
                    self.startup_reconciling |= epoch != [0; 4] && next_sequence > 1;
                }
                State::Pending { command, .. } => {
                    if !(CONTROL_ATTACH..=CONTROL_CARD_IDENTITY).contains(&command) {
                        return Err(invalid("Unknown inherited runtime command."));
                    }
                    self.startup_reconciling |= command >= CONTROL_SNAPSHOT_BEGIN;
                    return Err(runtime_busy());
                }
                State::Completed {
                    sequence,
                    command,
                    result: 0,
                    epoch,
                    ..
                } if matches!(command, CONTROL_ATTACH | CONTROL_PING) && epoch != [0; 4] => {
                    self.startup_reconciling |= sequence > 1;
                    self.client
                        .release(sequence)
                        .map_err(|_| invalid("The inherited probe could not be released."))?;
                }
                State::Completed { command, .. }
                    if (CONTROL_SNAPSHOT_BEGIN..=CONTROL_CARD_IDENTITY).contains(&command) =>
                {
                    self.startup_reconciling = true;
                    let ticket = self
                        .client
                        .completed_library_ticket()
                        .map_err(|_| invalid("The inherited library receipt is invalid."))?;
                    self.client.release_library(&ticket).map_err(|_| {
                        invalid("The inherited library receipt could not be released.")
                    })?;
                }
                _ => return Err(invalid("The inherited runtime command is not verifiable.")),
            }
            let ticket = self
                .client
                .attach_or_ping(self.nonce)
                .map_err(|_| invalid("The runtime probe could not be published."))?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                if let Some(state) = self
                    .client
                    .poll(&ticket)
                    .map_err(|_| invalid("The runtime probe context changed."))?
                {
                    if !matches!(state, State::Completed { result: 0, .. }) {
                        return Err(invalid("The runtime rejected its transport probe."));
                    }
                    self.client
                        .release(ticket.sequence)
                        .map_err(|_| invalid("The runtime probe could not be released."))?;
                    return Ok(());
                }
                if std::time::Instant::now() >= deadline {
                    return Err(runtime_busy());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        })();
        if let Err(error) = result {
            if error.code == "recovery_required" {
                self.recovery_required = true;
            }
            return Err(error);
        }
        self.transport_verified = true;
        Ok(())
    }
}
fn exchange_timeout(command: u32) -> std::time::Duration {
    // BEGIN may perform the first durable migration before pinning a snapshot.
    // Chunk reads retain the shorter transport deadline.
    std::time::Duration::from_secs(if command == grkit_core::abi::CONTROL_SNAPSHOT_BEGIN {
        10
    } else {
        2
    })
}

impl<M: crate::control::MailboxIo> ControlBackend<M> {
    fn exchange(
        &mut self,
        command: u32,
        generation: u32,
        payload: &[u8],
    ) -> Result<(u32, Vec<u8>)> {
        use crate::control::State;
        let result = (|| -> Result<_> {
            let mut published = None;
            let submission =
                self.client
                    .submit_library_tracked(command, generation, payload, &mut published);
            if command == grkit_core::abi::CONTROL_APPLY_COMMIT {
                if let Some(pending) = &mut self.pending_receipt {
                    pending.ticket = published.clone();
                }
            }
            if (grkit_core::abi::CONTROL_SNAPSHOT_BEGIN..=grkit_core::abi::CONTROL_SNAPSHOT_END)
                .contains(&command)
                || command == grkit_core::abi::CONTROL_CARD_IDENTITY
            {
                self.pending_read = published;
            }
            let ticket = submission
                .map_err(|_| invalid("The runtime command could not be safely published."))?;
            let deadline = std::time::Instant::now() + exchange_timeout(command);
            loop {
                match self
                    .client
                    .poll(&ticket)
                    .map_err(|_| invalid("The runtime command context changed."))?
                {
                    Some(State::Completed {
                        result, generation, ..
                    }) => {
                        let bytes = self
                            .client
                            .response(&ticket)
                            .map_err(|_| invalid("The runtime response is invalid."))?;
                        if command == grkit_core::abi::CONTROL_APPLY_COMMIT {
                            if let Some(pending) = &mut self.pending_receipt {
                                pending.acknowledged = result == 0
                                    && bytes.is_empty()
                                    && pending.before.generation.checked_add(1) == Some(generation);
                            }
                        }
                        self.client
                            .release_library(&ticket)
                            .map_err(|_| invalid("The runtime receipt could not be released."))?;
                        if self
                            .pending_read
                            .as_ref()
                            .is_some_and(|pending| pending.sequence == ticket.sequence)
                        {
                            self.pending_read = None;
                        }
                        if command == grkit_core::abi::CONTROL_APPLY_COMMIT {
                            if let Some(pending) = &mut self.pending_receipt {
                                pending.ticket = None;
                            }
                        }
                        return match result {
                            0 => Ok((generation, bytes)),
                            grkit_core::abi::CONTROL_BUSY
                                if self.startup_reconciling
                                    && command == grkit_core::abi::CONTROL_SNAPSHOT_BEGIN
                                    && payload.is_empty()
                                    && bytes.is_empty() =>
                            {
                                Err(runtime_busy())
                            }
                            grkit_core::abi::CONTROL_UNSUPPORTED if bytes.is_empty() => {
                                Err(protocol_unsupported())
                            }
                            grkit_core::abi::CONTROL_CHANGED if bytes.is_empty() => {
                                Err(Problem::new(
                                    409,
                                    "runtime_changed",
                                    "The pinned runtime generation changed.",
                                ))
                            }
                            _ => Err(invalid("The runtime did not provide a verified outcome.")),
                        };
                    }
                    None if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(10))
                    }
                    _ if self.startup_reconciling
                        || command == grkit_core::abi::CONTROL_SNAPSHOT_BEGIN =>
                    {
                        // BEGIN may bootstrap/reconcile storage. Preserve its
                        // ticket and let probe observe completion on retry.
                        self.transport_verified = false;
                        self.startup_reconciling = true;
                        return Err(runtime_busy());
                    }
                    _ => {
                        return Err(invalid(
                            "The runtime command timed out without a verified receipt.",
                        ));
                    }
                }
            }
        })();
        if result
            .as_ref()
            .is_err_and(|error| error.code == "recovery_required")
        {
            self.recovery_required = true;
        }
        result
    }
    fn read_card_identity(&mut self) -> Result<String> {
        self.probe()?;
        let crate::control::State::Idle { generation, .. } = self
            .client
            .reconcile()
            .map_err(|_| invalid("The runtime context is unavailable."))?
        else {
            return Err(invalid(
                "An outstanding runtime ticket requires reconciliation.",
            ));
        };
        let (ack, bytes) =
            self.exchange(grkit_core::abi::CONTROL_CARD_IDENTITY, generation, &[])?;
        if ack != generation {
            return Err(invalid("The card probe changed runtime generation."));
        }
        crate::identity::owner_evidence(&bytes)
            .map_err(|_| invalid("The owner card identity evidence is invalid."))
    }

    fn read_snapshot(&mut self) -> Result<RuntimeSnapshot> {
        use grkit_core::abi::*;
        self.probe()?;
        let crate::control::State::Idle {
            epoch, generation, ..
        } = self
            .client
            .reconcile()
            .map_err(|_| invalid("The runtime context is unavailable."))?
        else {
            return Err(invalid(
                "An outstanding runtime ticket requires reconciliation.",
            ));
        };
        let (ack_generation, bytes) = self.exchange(CONTROL_SNAPSHOT_BEGIN, generation, &[])?;
        let first = StreamFrame::decode(&bytes)?;
        // BEGIN can recover the durable generation or reconcile camera-menu
        // edits. Its response establishes the pin used by READ and END.
        if first.epoch != epoch
            || first.generation < generation
            || ack_generation != first.generation
        {
            return Err(invalid(
                "The owner snapshot does not match the attached runtime.",
            ));
        }
        let generation = first.generation;
        let pin = first.clone();
        let mut stream = SnapshotStream::begin(first)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !stream.complete() {
            if std::time::Instant::now() >= deadline {
                return Err(invalid("The complete snapshot exceeded its deadline."));
            }
            let request = stream.next_request().encode()?;
            let (ack, bytes) = self.exchange(CONTROL_SNAPSHOT_READ, generation, &request)?;
            if ack != generation {
                return Err(invalid("The snapshot generation changed during transfer."));
            }
            stream.append(StreamFrame::decode(&bytes)?)?;
        }
        let end = stream.next_request().encode()?;
        let mut snapshot = decode_snapshot(&stream.finish()?, &pin)?;
        // Official provenance comes from signed release evidence. Do not change
        // the persisted manifest preimage used by the owner commit check.
        snapshot.context.official_hashes = self.official_hashes.clone();
        let (ack, bytes) = self.exchange(CONTROL_SNAPSHOT_END, generation, &end)?;
        if ack != generation || !bytes.is_empty() {
            return Err(invalid("The snapshot pin release was not acknowledged."));
        }
        if self.startup_reconciling {
            if self.read_card_identity()? != snapshot.card_id {
                return Err(invalid(
                    "The recovered snapshot belongs to a different card.",
                ));
            }
            self.startup_reconciling = false;
        }
        self.snapshot_verified = true;
        self.hot_reload_verified = snapshot.context.hot_reload;
        Ok(snapshot)
    }
}
impl<M: crate::control::MailboxIo + Send> RuntimeBackend for ControlBackend<M> {
    fn card_identity(&mut self) -> Result<String> {
        self.probe()?;
        if self.startup_reconciling {
            return self.snapshot().map(|snapshot| snapshot.card_id).map_err(|error| {
                if error.code == "runtime_protocol_unsupported" {
                    Problem::new(503,"startup_reconciliation_unsupported",
                        "The inherited runtime session cannot provide a coherent recovery snapshot.")
                } else { error }
            });
        }
        if let Some(known) = &self.card_identity {
            return known.clone();
        }
        let result = self.read_card_identity();
        // Keep an identity, or a runtime that cannot probe the card; probe
        // again after any other error.
        if result.as_ref().map_or_else(
            |error| error.code == "runtime_protocol_unsupported",
            |_| true,
        ) {
            self.card_identity = Some(result.clone());
        }
        result
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities {
            readable: self.snapshot_verified
                && !self.recovery_required
                && !self.startup_reconciling,
            hot_reload: self.snapshot_verified
                && self.hot_reload_verified
                && !self.recovery_required
                && !self.startup_reconciling,
        }
    }
    fn snapshot(&mut self) -> Result<RuntimeSnapshot> {
        let result = self.read_snapshot();
        if result
            .as_ref()
            .is_err_and(|error| error.code == "recovery_required")
        {
            self.recovery_required = true;
        }
        result
    }
    fn apply(&mut self, expected_revision: &str, prepared: Prepared) -> Result<RuntimeSnapshot> {
        let mut staging_started = false;
        let result = (|| {
            use grkit_core::abi::*;
            use sha2::{Digest, Sha256};
            let before = self.snapshot()?;
            if before.revision()? != expected_revision {
                return Err(Problem::new(
                    412,
                    "revision_changed",
                    "The Looks configuration changed.",
                ));
            }
            let manifest = prepared.manifest.encode_binary()?;
            let content = prepared
                .content
                .as_ref()
                .map_or(&[][..], |content| content.bytes.as_slice());
            let mut candidate = Sha256::digest(before.manifest.encode_binary()?).to_vec();
            candidate.extend_from_slice(&(manifest.len() as u32).to_le_bytes());
            candidate.extend_from_slice(&manifest);
            candidate.extend_from_slice(&(content.len() as u32).to_le_bytes());
            candidate.extend_from_slice(content);
            candidate.extend_from_slice(&u32::from(prepared.select_off).to_le_bytes());
            let digest: [u8; 32] = Sha256::digest(&candidate).into();
            let mut token_input = self.nonce.to_vec();
            token_input.extend_from_slice(&digest);
            token_input.extend_from_slice(&before.generation.to_le_bytes());
            let token_hash = Sha256::digest(&token_input);
            let mut pin = StreamFrame {
                token: token_hash[..16].try_into().unwrap(),
                epoch: before.runtime_epoch,
                card_hash: Sha256::digest(before.card_id.as_bytes()).into(),
                generation: before.generation,
                total: candidate.len() as u32,
                offset: 0,
                digest,
                bytes: Vec::new(),
            };
            for chunk in candidate.chunks(CONTROL_PAYLOAD_BYTES - StreamFrame::HEADER) {
                pin.bytes = chunk.to_vec();
                let command = if pin.offset == 0 {
                    CONTROL_APPLY_BEGIN
                } else {
                    CONTROL_APPLY_WRITE
                };
                let (ack, bytes) = self.exchange(command, before.generation, &pin.encode()?)?;
                staging_started = true;
                pin.offset += chunk.len() as u32;
                pin.bytes.clear();
                if ack != before.generation || StreamFrame::decode(&bytes)? != pin {
                    self.recovery_required = true;
                    return Err(invalid(
                        "The runtime did not acknowledge the staged candidate offset.",
                    ));
                }
            }
            // COMMIT is the sole durable boundary. Any uncertain/malformed result
            // quarantines this backend; never retry or issue ABORT after it.
            self.pending_receipt = Some(PendingReceipt {
                before: before.clone(),
                prepared: prepared.clone(),
                ticket: None,
                acknowledged: false,
            });
            let committed = (|| -> Result<_> {
                let (generation, bytes) =
                    self.exchange(CONTROL_APPLY_COMMIT, before.generation, &pin.encode()?)?;
                if before.generation.checked_add(1) != Some(generation) || !bytes.is_empty() {
                    return Err(invalid("The runtime commit generation is invalid."));
                }
                let after = self.read_snapshot()?;
                after.verify_receipt(&before, &prepared)?;
                self.pending_receipt = None;
                Ok(after)
            })();
            if committed.is_err() {
                self.recovery_required = true;
                return Err(invalid(
                    "The durable runtime commit requires receipt reconciliation.",
                ));
            }
            committed
        })();
        if staging_started && result.is_err() {
            self.recovery_required = true;
            return Err(invalid(
                "The staged runtime candidate requires reconciliation.",
            ));
        }
        if result
            .as_ref()
            .is_err_and(|error: &Problem| error.code == "recovery_required")
        {
            self.recovery_required = true;
        }
        result
    }
    fn reconcile_apply(&mut self) -> Result<Option<RuntimeSnapshot>> {
        use crate::control::State;
        let Some(pending) = &self.pending_receipt else {
            return Ok(None);
        };
        let before = pending.before.clone();
        let prepared = pending.prepared.clone();
        if let Some(ticket) = pending.ticket.clone() {
            match self
                .client
                .reconcile()
                .map_err(|_| invalid("The pending receipt is unavailable."))?
            {
                State::Idle { .. } if pending.acknowledged => {}
                State::Completed { .. } => {
                    let Some(State::Completed {
                        result: 0,
                        generation,
                        ..
                    }) = self
                        .client
                        .poll(&ticket)
                        .map_err(|_| invalid("The pending receipt context changed."))?
                    else {
                        return Ok(None);
                    };
                    let bytes = self
                        .client
                        .response(&ticket)
                        .map_err(|_| invalid("The pending receipt is invalid."))?;
                    if before.generation.checked_add(1) != Some(generation) || !bytes.is_empty() {
                        return Err(invalid("The pending commit generation is invalid."));
                    }
                    self.pending_receipt.as_mut().unwrap().acknowledged = true;
                    self.client
                        .release_library(&ticket)
                        .map_err(|_| invalid("The pending receipt could not be released."))?;
                }
                _ => return Ok(None),
            }
            self.pending_receipt.as_mut().unwrap().ticket = None;
        }
        if !self.pending_receipt.as_ref().unwrap().acknowledged {
            return Ok(None);
        }
        if let Some(ticket) = self.pending_read.clone() {
            match self
                .client
                .reconcile()
                .map_err(|_| invalid("The pending snapshot is unavailable."))?
            {
                State::Idle { .. } => {}
                State::Completed { .. } => {
                    self.client
                        .release_library(&ticket)
                        .map_err(|_| invalid("The pending snapshot could not be released."))?;
                }
                _ => return Ok(None),
            }
            self.pending_read = None;
        }
        // Only a validated COMMIT acknowledgement permits a fresh read. An
        // equal resource alone cannot establish that this request completed.
        self.recovery_required = false;
        let result = self
            .card_identity()
            .and_then(|card| {
                if card != before.card_id {
                    return Err(invalid("The pending commit belongs to a different card."));
                }
                self.read_snapshot()
            })
            .and_then(|after| {
                after.verify_receipt(&before, &prepared)?;
                Ok(after)
            });
        match result {
            Ok(after) => {
                self.pending_receipt = None;
                Ok(Some(after))
            }
            Err(error) => {
                self.recovery_required = true;
                Err(error)
            }
        }
    }
}

fn runtime_busy() -> Problem {
    Problem::new(
        503,
        "runtime_busy",
        "The runtime is finishing an inherited command or releasing its stream.",
    )
}

fn protocol_unsupported() -> Problem {
    Problem::new(
        503,
        "runtime_protocol_unsupported",
        "This runtime supports transport probes but has no verified Looks snapshot and commit protocol.",
    )
}

/// Library stream frame v1, all integers little-endian. The immutable header
/// pins every chunk to a single owner snapshot. Card identity is SHA-256 of the
/// UTF-8 owner cardId; the final snapshot must independently carry that value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamFrame {
    pub token: [u8; 16],
    pub epoch: [u32; 4],
    pub card_hash: [u8; 32],
    pub generation: u32,
    pub total: u32,
    pub offset: u32,
    pub digest: [u8; 32],
    pub bytes: Vec<u8>,
}
impl StreamFrame {
    pub const HEADER: usize = 128;
    pub const MAX_TOTAL: usize = 1024 * 1024;
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        use grkit_core::abi::CONTROL_PAYLOAD_BYTES;
        if !(Self::HEADER..=CONTROL_PAYLOAD_BYTES).contains(&bytes.len())
            || &bytes[..4] != b"GRS1"
            || bytes[112..128] != [0; 16]
        {
            return Err(invalid("The runtime stream frame is malformed."));
        }
        let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let frame = Self {
            token: bytes[4..20].try_into().unwrap(),
            epoch: std::array::from_fn(|i| word(20 + i * 4)),
            card_hash: bytes[36..68].try_into().unwrap(),
            generation: word(68),
            total: word(72),
            offset: word(76),
            digest: bytes[80..112].try_into().unwrap(),
            bytes: bytes[128..].to_vec(),
        };
        frame.validate()?;
        Ok(frame)
    }
    fn validate(&self) -> Result<()> {
        if self.token == [0; 16]
            || self.epoch == [0; 4]
            || self.card_hash == [0; 32]
            || self.digest == [0; 32]
            || self.total == 0
            || self.total as usize > Self::MAX_TOTAL
            || self.offset > self.total
            || self.bytes.len() > (self.total - self.offset) as usize
            || self.bytes.len() > grkit_core::abi::CONTROL_PAYLOAD_BYTES - Self::HEADER
        {
            return Err(invalid(
                "The runtime stream bounds or ownership context is invalid.",
            ));
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut bytes = vec![0; Self::HEADER];
        bytes[..4].copy_from_slice(b"GRS1");
        bytes[4..20].copy_from_slice(&self.token);
        for (i, word) in self.epoch.iter().enumerate() {
            bytes[20 + i * 4..24 + i * 4].copy_from_slice(&word.to_le_bytes());
        }
        bytes[36..68].copy_from_slice(&self.card_hash);
        bytes[68..72].copy_from_slice(&self.generation.to_le_bytes());
        bytes[72..76].copy_from_slice(&self.total.to_le_bytes());
        bytes[76..80].copy_from_slice(&self.offset.to_le_bytes());
        bytes[80..112].copy_from_slice(&self.digest);
        bytes.extend_from_slice(&self.bytes);
        Ok(bytes)
    }
}

/// No partially assembled stream can become an HTTP snapshot. Offset gaps,
/// repeated chunks and context changes are errors, not implicit restarts.
pub struct SnapshotStream {
    pinned: StreamFrame,
    bytes: Vec<u8>,
}
impl SnapshotStream {
    pub fn begin(frame: StreamFrame) -> Result<Self> {
        if frame.offset != 0 {
            return Err(invalid("A snapshot stream must start at offset zero."));
        }
        let mut stream = Self {
            pinned: frame.clone(),
            bytes: Vec::new(),
        };
        stream.pinned.bytes.clear();
        stream.append(frame)?;
        Ok(stream)
    }
    pub fn append(&mut self, frame: StreamFrame) -> Result<()> {
        frame.validate()?;
        let mut header = frame.clone();
        header.offset = 0;
        header.bytes.clear();
        if header != self.pinned
            || frame.offset as usize != self.bytes.len()
            || frame.bytes.is_empty()
        {
            return Err(invalid(
                "A runtime chunk does not match its pinned snapshot.",
            ));
        }
        self.bytes.extend_from_slice(&frame.bytes);
        Ok(())
    }
    pub fn next_request(&self) -> StreamFrame {
        let mut frame = self.pinned.clone();
        frame.offset = self.bytes.len() as u32;
        frame
    }
    pub fn complete(&self) -> bool {
        self.bytes.len() == self.pinned.total as usize
    }
    pub fn finish(self) -> Result<Vec<u8>> {
        use sha2::{Digest, Sha256};
        if !self.complete() || Sha256::digest(&self.bytes).as_slice() != self.pinned.digest {
            return Err(invalid(
                "The complete runtime snapshot digest does not match.",
            ));
        }
        Ok(self.bytes)
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotMetadata {
    manifest: Manifest,
    card_id: String,
    runtime_epoch: [u32; 4],
    generation: u32,
    loaded_ids: BTreeSet<String>,
    context: SnapshotContext,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotContext {
    hot_reload: bool,
    selected: Option<String>,
    dependencies_complete: bool,
    dependencies: BTreeMap<String, Vec<String>>,
    blocked_ids: BTreeSet<String>,
    reserved_codes: BTreeSet<u16>,
    supported_features: u32,
    monochrome_only: bool,
    official_hashes: BTreeMap<String, String>,
}
/// Snapshot bytes: metadata JSON length (u32 LE), strict JSON, then one length
/// (u32 LE) and raw GRF for each distinct live content hash in ascending order.
/// The frame and metadata must independently agree about owner/card/generation.
pub fn decode_snapshot(bytes: &[u8], pin: &StreamFrame) -> Result<RuntimeSnapshot> {
    use sha2::{Digest, Sha256};
    if bytes.len() < 4
        || bytes.len() != pin.total as usize
        || Sha256::digest(bytes).as_slice() != pin.digest
    {
        return Err(invalid(
            "The snapshot envelope does not match its pinned digest.",
        ));
    }
    let length = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    if length > crate::library::MAX_MANIFEST_BYTES || length > bytes.len() - 4 {
        return Err(invalid("The snapshot metadata exceeds its bounds."));
    }
    let value = effects::parse_json(&bytes[4..4 + length])
        .map_err(|_| invalid("The runtime snapshot metadata is malformed."))?;
    let meta: SnapshotMetadata = serde_json::from_value(value)
        .map_err(|_| invalid("The runtime snapshot metadata schema is invalid."))?;
    if meta.runtime_epoch != pin.epoch
        || meta.generation != pin.generation
        || Sha256::digest(meta.card_id.as_bytes()).as_slice() != pin.card_hash
    {
        return Err(invalid(
            "The snapshot metadata belongs to a different runtime or card.",
        ));
    }
    meta.manifest.validate()?;
    let mut remaining = &bytes[4 + length..];
    let hashes: BTreeSet<_> = meta
        .manifest
        .entries
        .values()
        .filter(|entry| !entry.removed)
        .map(|entry| entry.content_hash.clone())
        .collect();
    let mut contents = BTreeMap::new();
    for hash in hashes {
        if remaining.len() < 4 {
            return Err(invalid("A snapshot content length is missing."));
        }
        let length = u32::from_le_bytes(remaining[..4].try_into().unwrap()) as usize;
        remaining = &remaining[4..];
        if !matches!(length, effects::SIZE | effects::SIZE_WITH_LEVELS) || length > remaining.len()
        {
            return Err(invalid(
                "A snapshot content generation is truncated or oversized.",
            ));
        }
        contents.insert(hash, remaining[..length].to_vec());
        remaining = &remaining[length..];
    }
    if !remaining.is_empty() {
        return Err(invalid("The snapshot contains unclaimed trailing bytes."));
    }
    let snapshot = RuntimeSnapshot {
        manifest: meta.manifest,
        card_id: meta.card_id,
        runtime_epoch: meta.runtime_epoch,
        generation: meta.generation,
        loaded_ids: meta.loaded_ids,
        contents,
        context: Context {
            hot_reload: meta.context.hot_reload,
            selected: meta.context.selected,
            dependencies_complete: meta.context.dependencies_complete,
            dependencies: meta.context.dependencies,
            blocked_ids: meta.context.blocked_ids,
            reserved_codes: meta.context.reserved_codes,
            supported_features: meta.context.supported_features,
            monochrome_only: meta.context.monochrome_only,
            official_hashes: meta.context.official_hashes,
        },
    };
    snapshot.validate()?;
    Ok(snapshot)
}

/// Retry discovery only while no session has been acquired. A failed apply is
/// never replayed. Card/epoch invalidation is an explicit coordinator action;
/// the next discovery reconciles the mailbox and cannot clear an old command.
pub struct LazyLinuxBackend {
    root: std::path::PathBuf,
    keys: Vec<[u8; 32]>,
    nonce: [u8; 16],
    session: Option<LinuxBackend>,
    retry_after: Option<std::time::Instant>,
}
impl LazyLinuxBackend {
    pub fn new(root: std::path::PathBuf, keys: Vec<[u8; 32]>, nonce: [u8; 16]) -> Self {
        Self {
            root,
            keys,
            nonce,
            session: None,
            retry_after: None,
        }
    }
    pub fn invalidate_session(&mut self) {
        self.session = None;
        self.retry_after = None;
    }
    fn session(&mut self) -> Result<&mut LinuxBackend> {
        if self.session.is_none() {
            if self
                .retry_after
                .is_some_and(|deadline| std::time::Instant::now() < deadline)
            {
                return Err(unavailable());
            }
            // Discovery launches bounded, verified code and can fail while the
            // resident installer is starting. Preserve normal HTTP operation.
            match LinuxBackend::discover(&self.root, &self.keys, self.nonce) {
                Ok(session) => {
                    self.session = Some(session);
                    self.retry_after = None;
                }
                Err(_) => {
                    self.retry_after =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                    return Err(unavailable());
                }
            }
        }
        Ok(self.session.as_mut().expect("discovered session"))
    }
}
impl RuntimeBackend for LazyLinuxBackend {
    fn card_identity(&mut self) -> Result<String> {
        self.session()?.card_identity()
    }
    fn invalidate_session(&mut self) {
        LazyLinuxBackend::invalidate_session(self);
    }
    fn capabilities(&self) -> RuntimeCapabilities {
        self.session
            .as_ref()
            .map_or_else(RuntimeCapabilities::default, RuntimeBackend::capabilities)
    }
    fn snapshot(&mut self) -> Result<RuntimeSnapshot> {
        self.session()?.snapshot()
    }
    fn apply(&mut self, revision: &str, prepared: Prepared) -> Result<RuntimeSnapshot> {
        self.session()?.apply(revision, prepared)
    }
    fn reconcile_apply(&mut self) -> Result<Option<RuntimeSnapshot>> {
        match &mut self.session {
            Some(session) => session.reconcile_apply(),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod bridge_tests {
    use super::*;
    use sha2::{Digest, Sha256};
    fn identity_evidence() -> Vec<u8> {
        let mut evidence = b"GRC1".to_vec();
        evidence.extend([1; 16]);
        let mut sector = [0; 512];
        sector[510..].copy_from_slice(&[0x55, 0xaa]);
        sector[82..90].copy_from_slice(b"FAT32   ");
        sector[66] = 0x29;
        sector[67..71].copy_from_slice(&[1, 2, 3, 4]);
        evidence.extend(sector);
        evidence
    }
    fn card_id() -> String {
        crate::identity::owner_evidence(&identity_evidence()).unwrap()
    }
    fn frame(bytes: &[u8]) -> StreamFrame {
        StreamFrame {
            token: [1; 16],
            epoch: [2; 4],
            card_hash: Sha256::digest(card_id().as_bytes()).into(),
            generation: 7,
            total: bytes.len() as u32,
            offset: 0,
            digest: Sha256::digest(bytes).into(),
            bytes: bytes.to_vec(),
        }
    }
    fn snapshot() -> Vec<u8> {
        snapshot_generation(7)
    }
    fn snapshot_generation(generation: u32) -> Vec<u8> {
        let meta = serde_json::json!({
            "manifest": Manifest::default(), "cardId":card_id(), "runtimeEpoch":[2,2,2,2], "generation":generation,
            "loadedIds":[], "context": {"hotReload":false,"selected":null,"dependenciesComplete":true,"dependencies":{},
            "blockedIds":[],"reservedCodes":[],"supportedFeatures":0,"monochromeOnly":false,"officialHashes":{}}
        });
        let json = serde_json::to_vec(&meta).unwrap();
        let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
        bytes.extend(json);
        bytes
    }
    struct ScriptedMailbox {
        words: Vec<u32>,
        replies: std::collections::VecDeque<(u32, u32, Vec<u8>)>,
    }
    impl ScriptedMailbox {
        fn new(replies: Vec<(u32, u32, Vec<u8>)>) -> Self {
            use grkit_core::abi::*;
            let mut words = vec![0; CONTROL_WORDS];
            words[CONTROL_MAGIC / 4] = CONTROL_MAGIC_VALUE;
            words[CONTROL_VERSION / 4] = CONTROL_VERSION_VALUE;
            words[CONTROL_CAPACITY / 4] = CONTROL_PAYLOAD_BYTES as u32;
            words[CONTROL_GENERATION / 4] = 7;
            for i in 0..4 {
                words[CONTROL_EPOCH / 4 + i] = 2;
            }
            Self {
                words,
                replies: replies.into(),
            }
        }
    }
    impl crate::control::MailboxIo for ScriptedMailbox {
        fn sync(&mut self) -> anyhow::Result<()> {
            use grkit_core::abi::*;
            let seq = self.words[CONTROL_REQUEST / 4];
            if seq != self.words[CONTROL_READY / 4] {
                let (command, result, bytes) =
                    self.replies.pop_front().expect("unexpected request");
                assert_eq!(self.words[CONTROL_COMMAND / 4], command);
                self.words[CONTROL_RESPONSE_LENGTH / 4] = bytes.len() as u32;
                self.words[CONTROL_RESPONSE_CHECKSUM / 4] = crc32fast::hash(&bytes);
                for (i, chunk) in bytes.chunks(4).enumerate() {
                    let mut word = [0; 4];
                    word[..chunk.len()].copy_from_slice(chunk);
                    self.words[CONTROL_RESPONSE / 4 + i] = u32::from_le_bytes(word);
                }
                if command == CONTROL_APPLY_COMMIT && result == 0 {
                    self.words[CONTROL_GENERATION / 4] += 1;
                }
                if command == CONTROL_SNAPSHOT_BEGIN && result == 0 && bytes.len() >= 128 {
                    self.words[CONTROL_GENERATION / 4] =
                        u32::from_le_bytes(bytes[68..72].try_into().unwrap());
                }
                self.words[CONTROL_RESULT / 4] = result;
                self.words[CONTROL_CLAIMED / 4] = seq;
                self.words[CONTROL_READY / 4] = seq;
            }
            Ok(())
        }
        fn read(&self, offset: usize) -> u32 {
            self.words[offset / 4]
        }
        fn write(&mut self, offset: usize, value: u32) {
            self.words[offset / 4] = value;
        }
    }
    fn inherited(command: u32, replies: Vec<(u32, u32, Vec<u8>)>) -> ScriptedMailbox {
        use grkit_core::abi::*;
        let mut memory = ScriptedMailbox::new(replies);
        memory.words[CONTROL_REQUEST / 4] = 1;
        memory.words[CONTROL_CLAIMED / 4] = 1;
        memory.words[CONTROL_READY / 4] = 1;
        memory.words[CONTROL_COMMAND / 4] = command;
        for i in 0..4 {
            memory.words[CONTROL_REQUEST_EPOCH / 4 + i] = 2;
        }
        memory
    }
    fn recovery_replies() -> Vec<(u32, u32, Vec<u8>)> {
        use grkit_core::abi::*;
        vec![
            (CONTROL_PING, 0, vec![]),
            (
                CONTROL_SNAPSHOT_BEGIN,
                0,
                frame(&snapshot()).encode().unwrap(),
            ),
            (CONTROL_SNAPSHOT_END, 0, vec![]),
            (CONTROL_CARD_IDENTITY, 0, identity_evidence()),
        ]
    }
    #[test]
    fn snapshot_begin_has_a_bounded_migration_deadline() {
        use grkit_core::abi::*;
        use std::time::Duration;
        assert_eq!(
            exchange_timeout(CONTROL_SNAPSHOT_BEGIN),
            Duration::from_secs(10)
        );
        for command in [
            CONTROL_PING,
            CONTROL_CARD_IDENTITY,
            CONTROL_SNAPSHOT_READ,
            CONTROL_SNAPSHOT_END,
            CONTROL_APPLY_BEGIN,
            CONTROL_APPLY_WRITE,
            CONTROL_APPLY_COMMIT,
        ] {
            assert_eq!(exchange_timeout(command), Duration::from_secs(2));
        }
    }

    #[test]
    fn restart_drains_completed_library_tickets_but_requires_a_fresh_snapshot() {
        use grkit_core::abi::*;
        for command in CONTROL_SNAPSHOT_BEGIN..=CONTROL_CARD_IDENTITY {
            for result in [0, 20] {
                let mut memory = inherited(command, recovery_replies());
                memory.words[CONTROL_RESULT / 4] = result;
                let mut backend =
                    ControlBackend::new(crate::control::Client::new(memory).unwrap(), [9; 16]);
                backend.probe().unwrap();
                assert!(backend.startup_reconciling && !backend.recovery_required);
                assert!(!backend.capabilities().readable && !backend.capabilities().hot_reload);
                assert!(backend.pending_receipt.is_none());
                assert_eq!(backend.card_identity().unwrap(), card_id());
                assert!(!backend.startup_reconciling && backend.snapshot_verified);
                assert!(backend.pending_receipt.is_none()); // No old HTTP success inferred.
            }
        }
    }
    #[test]
    fn restart_pending_commit_can_finish_without_release_abort_or_replay() {
        use grkit_core::abi::*;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        };
        struct PausedOwner {
            shared: Arc<Mutex<ScriptedMailbox>>,
            paused: Arc<AtomicBool>,
        }
        impl crate::control::MailboxIo for PausedOwner {
            fn sync(&mut self) -> anyhow::Result<()> {
                if self.paused.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    crate::control::MailboxIo::sync(&mut *self.shared.lock().unwrap())
                }
            }
            fn read(&self, offset: usize) -> u32 {
                self.shared.lock().unwrap().words[offset / 4]
            }
            fn write(&mut self, offset: usize, value: u32) {
                assert!(
                    !self.paused.load(Ordering::SeqCst),
                    "no writes while inherited COMMIT is pending"
                );
                self.shared.lock().unwrap().words[offset / 4] = value;
            }
        }
        let mut replies = recovery_replies();
        let mut next = frame(&snapshot_generation(8));
        next.generation = 8;
        replies[1].2 = next.encode().unwrap();
        let mut memory = inherited(CONTROL_APPLY_COMMIT, replies);
        memory.words[CONTROL_READY / 4] = 0;
        let shared = Arc::new(Mutex::new(memory));
        let paused = Arc::new(AtomicBool::new(true));
        let client = crate::control::Client::new(PausedOwner {
            shared: shared.clone(),
            paused: paused.clone(),
        })
        .unwrap();
        let mut backend = ControlBackend::new(client, [9; 16]);
        for _ in 0..2 {
            assert_eq!(backend.card_identity().unwrap_err().code, "runtime_busy");
            assert!(!backend.recovery_required);
            assert_eq!(shared.lock().unwrap().words[CONTROL_RELEASE / 4], 0);
        }
        shared.lock().unwrap().words[CONTROL_READY / 4] = 1;
        shared.lock().unwrap().words[CONTROL_GENERATION / 4] = 8;
        paused.store(false, Ordering::SeqCst);
        assert_eq!(backend.card_identity().unwrap(), card_id());
        assert!(!backend.startup_reconciling && !backend.recovery_required);
        assert!(backend.pending_receipt.is_none());
        assert!(shared.lock().unwrap().replies.is_empty());
    }
    #[test]
    fn startup_staging_waits_for_owner_busy_to_clear_without_guessing_a_ttl() {
        use grkit_core::abi::*;
        for command in [
            CONTROL_APPLY_BEGIN,
            CONTROL_APPLY_WRITE,
            CONTROL_SNAPSHOT_BEGIN,
            CONTROL_SNAPSHOT_READ,
        ] {
            let mut replies = recovery_replies();
            replies.insert(1, (CONTROL_SNAPSHOT_BEGIN, CONTROL_BUSY, vec![]));
            replies.insert(2, (CONTROL_SNAPSHOT_BEGIN, CONTROL_BUSY, vec![]));
            let memory = inherited(command, replies);
            let mut backend =
                ControlBackend::new(crate::control::Client::new(memory).unwrap(), [9; 16]);
            for _ in 0..2 {
                assert_eq!(backend.card_identity().unwrap_err().code, "runtime_busy");
                assert!(backend.startup_reconciling && !backend.recovery_required);
                assert!(!backend.capabilities().hot_reload);
            }
            assert_eq!(backend.card_identity().unwrap(), card_id());
        }
    }
    #[test]
    fn startup_rejects_corrupt_receipts_and_owner_recovery_without_card_authorization() {
        use grkit_core::abi::*;
        for bad_epoch in [false, true] {
            let mut memory = inherited(CONTROL_CARD_IDENTITY, vec![]);
            if bad_epoch {
                memory.words[CONTROL_REQUEST_EPOCH / 4] = 3;
            } else {
                memory.words[CONTROL_RESPONSE_CHECKSUM / 4] = 1;
            }
            let mut backend =
                ControlBackend::new(crate::control::Client::new(memory).unwrap(), [9; 16]);
            assert_eq!(
                backend.card_identity().unwrap_err().code,
                "recovery_required"
            );
            assert!(backend.recovery_required && !backend.capabilities().readable);
            assert!(matches!(
                backend.client.reconcile().unwrap(),
                crate::control::State::Completed { sequence: 1, .. }
            ));
        }
        for (result, expected) in [
            (CONTROL_INVALID, "recovery_required"),
            (CONTROL_UNSUPPORTED, "startup_reconciliation_unsupported"),
        ] {
            let memory = inherited(
                CONTROL_APPLY_COMMIT,
                vec![
                    (CONTROL_PING, 0, vec![]),
                    (CONTROL_SNAPSHOT_BEGIN, result, vec![]),
                ],
            );
            let mut backend =
                ControlBackend::new(crate::control::Client::new(memory).unwrap(), [9; 16]);
            assert_eq!(backend.card_identity().unwrap_err().code, expected);
            assert!(backend.startup_reconciling && !backend.capabilities().readable);
            assert_eq!(backend.recovery_required, result == CONTROL_INVALID);
        }
    }

    #[test]
    fn startup_rejects_a_changed_epoch_or_card_during_its_fresh_snapshot() {
        use grkit_core::abi::*;
        for change_epoch in [false, true] {
            let mut pin = frame(&snapshot());
            let mut evidence = identity_evidence();
            let mut replies = vec![(CONTROL_PING, 0, vec![])];
            if change_epoch {
                pin.epoch = [3; 4];
            }
            replies.push((CONTROL_SNAPSHOT_BEGIN, 0, pin.encode().unwrap()));
            if !change_epoch {
                evidence[20 + 67] ^= 1;
                replies.push((CONTROL_SNAPSHOT_END, 0, vec![]));
                replies.push((CONTROL_CARD_IDENTITY, 0, evidence));
            }
            let memory = inherited(CONTROL_CARD_IDENTITY, replies);
            let mut backend =
                ControlBackend::new(crate::control::Client::new(memory).unwrap(), [9; 16]);
            assert_eq!(
                backend.card_identity().unwrap_err().code,
                "recovery_required"
            );
            assert!(backend.startup_reconciling && backend.recovery_required);
            assert!(!backend.capabilities().readable);
        }
    }

    #[test]
    fn production_bridge_reads_owner_snapshot_and_releases_its_pin() {
        use grkit_core::abi::*;
        let bytes = snapshot();
        let first = frame(&bytes).encode().unwrap();
        let memory = ScriptedMailbox::new(vec![
            (CONTROL_PING, 0, vec![]),
            (CONTROL_SNAPSHOT_BEGIN, 0, first),
            (CONTROL_SNAPSHOT_END, 0, vec![]),
        ]);
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        assert_eq!(backend.snapshot().unwrap().card_id, card_id());
        assert!(backend.transport_verified());
        assert!(!backend.recovery_required);
    }
    #[test]
    fn snapshot_begin_can_recover_a_newer_durable_generation_but_never_regress() {
        use grkit_core::abi::*;
        for generation in [6, 8] {
            let initial = snapshot();
            let mut metadata: serde_json::Value = serde_json::from_slice(&initial[4..]).unwrap();
            metadata["generation"] = generation.into();
            let json = serde_json::to_vec(&metadata).unwrap();
            let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
            bytes.extend(json);
            let mut pin = frame(&bytes);
            pin.generation = generation;
            let mut replies = vec![
                (CONTROL_PING, 0, vec![]),
                (CONTROL_SNAPSHOT_BEGIN, 0, pin.encode().unwrap()),
            ];
            if generation > 7 {
                replies.push((CONTROL_SNAPSHOT_END, 0, vec![]));
            }
            let memory = ScriptedMailbox::new(replies);
            let mut backend =
                ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
            if generation > 7 {
                assert_eq!(backend.snapshot().unwrap().generation, generation);
                assert!(!backend.recovery_required);
            } else {
                assert_eq!(backend.snapshot().unwrap_err().code, "recovery_required");
                assert!(backend.recovery_required);
            }
        }
    }

    #[test]
    fn apply_rejects_a_new_generation_even_when_configuration_returns_to_the_same_value() {
        use grkit_core::abi::*;
        let old_bytes = snapshot();
        let before = decode_snapshot(&old_bytes, &frame(&old_bytes)).unwrap();
        let bytes = snapshot_generation(8);
        let mut pin = frame(&bytes);
        pin.generation = 8;
        // No APPLY reply is supplied: a stale writer must stop before staging.
        let memory = ScriptedMailbox::new(vec![
            (CONTROL_PING, 0, vec![]),
            (CONTROL_SNAPSHOT_BEGIN, 0, pin.encode().unwrap()),
            (CONTROL_SNAPSHOT_END, 0, vec![]),
        ]);
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        let prepared = Prepared {
            manifest: before.manifest.clone(),
            content: None,
            select_off: false,
        };
        assert_eq!(
            backend
                .apply(&before.revision().unwrap(), prepared)
                .unwrap_err()
                .code,
            "revision_changed"
        );
        assert!(!backend.recovery_required);
    }

    #[test]
    fn hot_reload_requires_an_explicit_verified_owner_capability() {
        use grkit_core::abi::*;
        for supported in [false, true] {
            let initial = snapshot();
            let mut metadata: serde_json::Value = serde_json::from_slice(&initial[4..]).unwrap();
            metadata["context"]["hotReload"] = supported.into();
            let json = serde_json::to_vec(&metadata).unwrap();
            let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
            bytes.extend(json);
            let memory = ScriptedMailbox::new(vec![
                (CONTROL_PING, 0, vec![]),
                (CONTROL_SNAPSHOT_BEGIN, 0, frame(&bytes).encode().unwrap()),
                (CONTROL_SNAPSHOT_END, 0, vec![]),
            ]);
            let mut backend =
                ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
            assert!(!backend.capabilities().hot_reload);
            backend.snapshot().unwrap();
            assert!(backend.capabilities().readable);
            assert_eq!(backend.capabilities().hot_reload, supported);
            backend.recovery_required = true;
            assert!(!backend.capabilities().hot_reload);
        }
    }

    #[test]
    fn bridge_uses_only_the_verified_release_catalog_for_official_provenance() {
        use grkit_core::abi::*;
        let initial = snapshot();
        let mut metadata: serde_json::Value = serde_json::from_slice(&initial[4..]).unwrap();
        metadata["context"]["officialHashes"] = serde_json::json!({"untrusted": "00".repeat(32)});
        let json = serde_json::to_vec(&metadata).unwrap();
        let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
        bytes.extend(json);
        let memory = ScriptedMailbox::new(vec![
            (CONTROL_PING, 0, vec![]),
            (CONTROL_SNAPSHOT_BEGIN, 0, frame(&bytes).encode().unwrap()),
            (CONTROL_SNAPSHOT_END, 0, vec![]),
        ]);
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        backend
            .official_hashes
            .insert("verified".into(), "11".repeat(32));
        let result = backend.snapshot().unwrap();
        assert_eq!(result.context.official_hashes, backend.official_hashes);
        assert_eq!(result.manifest, Manifest::default());
    }

    #[test]
    fn the_card_is_probed_once_per_session() {
        use grkit_core::abi::*;
        // The scripted mailbox panics on a request it has no reply for.
        let memory = ScriptedMailbox::new(vec![
            (CONTROL_PING, 0, vec![]),
            (CONTROL_CARD_IDENTITY, 0, identity_evidence()),
        ]);
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        for _ in 0..3 {
            assert_eq!(backend.card_identity().unwrap(), card_id());
        }
        let memory = ScriptedMailbox::new(vec![
            (CONTROL_PING, 0, vec![]),
            (CONTROL_CARD_IDENTITY, CONTROL_UNSUPPORTED, vec![]),
        ]);
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        for _ in 0..3 {
            assert_eq!(
                backend.card_identity().unwrap_err().code,
                "runtime_protocol_unsupported"
            );
        }
    }
    #[test]
    fn unsupported_owner_never_becomes_an_empty_successful_snapshot() {
        use grkit_core::abi::*;
        let memory = ScriptedMailbox::new(vec![
            (CONTROL_PING, 0, vec![]),
            (CONTROL_SNAPSHOT_BEGIN, CONTROL_UNSUPPORTED, vec![]),
        ]);
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        assert_eq!(
            backend.snapshot().unwrap_err().code,
            "runtime_protocol_unsupported"
        );
        assert!(backend.transport_verified());
        assert!(!backend.recovery_required);
    }
    #[test]
    fn malformed_owner_snapshot_quarantines_bridge_without_retrying() {
        use grkit_core::abi::*;
        let memory = ScriptedMailbox::new(vec![
            (CONTROL_PING, 0, vec![]),
            (CONTROL_SNAPSHOT_BEGIN, 0, vec![0; 128]),
        ]);
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        assert_eq!(backend.snapshot().unwrap_err().code, "recovery_required");
        assert!(backend.recovery_required);
        assert_eq!(backend.snapshot().unwrap_err().code, "recovery_required");
    }
    #[test]
    fn apply_requires_commit_and_a_matching_fresh_owner_snapshot() {
        use grkit_core::abi::*;
        let manifest = Manifest::default();
        let binary = manifest.encode_binary().unwrap();
        let mut candidate = Sha256::digest(manifest.encode_binary().unwrap()).to_vec();
        candidate.extend((binary.len() as u32).to_le_bytes());
        candidate.extend(&binary);
        candidate.extend([0; 8]);
        let digest: [u8; 32] = Sha256::digest(&candidate).into();
        let mut token_input = vec![3; 16];
        token_input.extend(digest);
        token_input.extend(7u32.to_le_bytes());
        let mut ack = frame(&candidate);
        ack.token = Sha256::digest(token_input)[..16].try_into().unwrap();
        ack.bytes.clear();
        ack.offset = ack.total;
        for (malformed_commit, fail_snapshot) in [(false, false), (true, false), (false, true)] {
            let initial = frame(&snapshot()).encode().unwrap();
            let mut replies = vec![
                (CONTROL_PING, 0, vec![]),
                (CONTROL_SNAPSHOT_BEGIN, 0, initial),
                (CONTROL_SNAPSHOT_END, 0, vec![]),
                (CONTROL_APPLY_BEGIN, 0, ack.encode().unwrap()),
                (
                    CONTROL_APPLY_COMMIT,
                    0,
                    if malformed_commit { vec![1] } else { vec![] },
                ),
            ];
            if !malformed_commit {
                if fail_snapshot {
                    replies.push((CONTROL_SNAPSHOT_BEGIN, CONTROL_CHANGED, vec![]));
                    replies.push((CONTROL_CARD_IDENTITY, 0, identity_evidence()));
                }
                let mut next = frame(&snapshot_generation(8));
                next.generation = 8;
                replies.extend([
                    (CONTROL_SNAPSHOT_BEGIN, 0, next.encode().unwrap()),
                    (CONTROL_SNAPSHOT_END, 0, vec![]),
                ]);
            }
            let memory = ScriptedMailbox::new(replies);
            let mut backend =
                ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
            let initial_bytes = snapshot();
            let revision = decode_snapshot(&initial_bytes, &frame(&initial_bytes))
                .unwrap()
                .revision()
                .unwrap();
            let result = backend.apply(
                &revision,
                Prepared {
                    manifest: manifest.clone(),
                    content: None,
                    select_off: false,
                },
            );
            if malformed_commit {
                assert_eq!(result.unwrap_err().code, "recovery_required");
                assert!(backend.recovery_required);
                assert_eq!(backend.snapshot().unwrap_err().code, "recovery_required");
                assert!(backend.reconcile_apply().unwrap().is_none());
            } else if fail_snapshot {
                assert_eq!(result.unwrap_err().code, "recovery_required");
                assert_eq!(backend.reconcile_apply().unwrap().unwrap().generation, 8);
                assert!(!backend.recovery_required);
                assert!(backend.reconcile_apply().unwrap().is_none());
            } else {
                assert_eq!(result.unwrap().generation, 8);
                assert!(!backend.recovery_required);
            }
        }
    }
    #[test]
    fn retained_commit_ticket_recovers_without_resubmitting_the_mutation() {
        use grkit_core::abi::*;
        let bytes = snapshot();
        let before = decode_snapshot(&bytes, &frame(&bytes)).unwrap();
        let prepared = Prepared {
            manifest: before.manifest.clone(),
            content: None,
            select_off: false,
        };
        let mut next = frame(&snapshot_generation(8));
        next.generation = 8;
        let memory = ScriptedMailbox::new(vec![
            (CONTROL_APPLY_COMMIT, 0, vec![]),
            (CONTROL_CARD_IDENTITY, 0, identity_evidence()),
            (CONTROL_SNAPSHOT_BEGIN, 0, next.encode().unwrap()),
            (CONTROL_SNAPSHOT_END, 0, vec![]),
        ]);
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        let ticket = backend
            .client
            .submit_library(CONTROL_APPLY_COMMIT, 7, &[])
            .unwrap();
        backend.transport_verified = true;
        backend.recovery_required = true;
        backend.pending_receipt = Some(PendingReceipt {
            before,
            prepared,
            ticket: Some(ticket),
            acknowledged: false,
        });
        assert_eq!(backend.reconcile_apply().unwrap().unwrap().generation, 8);
        assert!(!backend.recovery_required);
        assert!(backend.pending_receipt.is_none());
    }
    fn pending_baseline() -> PendingReceipt {
        let bytes = snapshot();
        let before = decode_snapshot(&bytes, &frame(&bytes)).unwrap();
        PendingReceipt {
            prepared: Prepared {
                manifest: before.manifest.clone(),
                content: None,
                select_off: false,
            },
            before,
            ticket: None,
            acknowledged: false,
        }
    }
    #[test]
    fn acknowledged_commit_recovers_after_a_delayed_identity_or_read_ticket() {
        use grkit_core::abi::*;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        };
        struct Delayed {
            inner: Arc<Mutex<ScriptedMailbox>>,
            ready: Arc<AtomicBool>,
            command: u32,
        }
        impl crate::control::MailboxIo for Delayed {
            fn sync(&mut self) -> anyhow::Result<()> {
                let mut inner = self.inner.lock().unwrap();
                if inner.words[CONTROL_REQUEST / 4] != inner.words[CONTROL_READY / 4]
                    && inner.words[CONTROL_COMMAND / 4] == self.command
                    && !self.ready.load(Ordering::SeqCst)
                {
                    return Ok(());
                }
                crate::control::MailboxIo::sync(&mut *inner)
            }
            fn read(&self, offset: usize) -> u32 {
                self.inner.lock().unwrap().words[offset / 4]
            }
            fn write(&mut self, offset: usize, value: u32) {
                self.inner.lock().unwrap().words[offset / 4] = value;
            }
        }
        for command in [CONTROL_CARD_IDENTITY, CONTROL_SNAPSHOT_READ] {
            let mut next = frame(&snapshot_generation(8));
            next.generation = 8;
            let memory = Arc::new(Mutex::new(ScriptedMailbox::new(vec![
                (CONTROL_APPLY_COMMIT, 0, vec![]),
                (
                    command,
                    0,
                    if command == CONTROL_CARD_IDENTITY {
                        identity_evidence()
                    } else {
                        vec![1, 2, 3]
                    },
                ),
                (CONTROL_CARD_IDENTITY, 0, identity_evidence()),
                (CONTROL_SNAPSHOT_BEGIN, 0, next.encode().unwrap()),
                (CONTROL_SNAPSHOT_END, 0, vec![]),
            ])));
            let ready = Arc::new(AtomicBool::new(false));
            let mut backend = ControlBackend::new(
                crate::control::Client::new(Delayed {
                    inner: memory.clone(),
                    ready: ready.clone(),
                    command,
                })
                .unwrap(),
                [3; 16],
            );
            backend.transport_verified = true;
            backend.pending_receipt = Some(pending_baseline());
            assert_eq!(
                backend.exchange(CONTROL_APPLY_COMMIT, 7, &[]).unwrap(),
                (8, vec![])
            );
            assert!(backend.pending_receipt.as_ref().unwrap().acknowledged);
            assert_eq!(
                backend.exchange(command, 8, &[]).unwrap_err().code,
                "recovery_required"
            );
            assert_eq!(backend.pending_read.as_ref().unwrap().command, command);
            assert!(backend.reconcile_apply().unwrap().is_none());
            assert_eq!(memory.lock().unwrap().replies.len(), 4);
            ready.store(true, Ordering::SeqCst);
            let receipt = backend.reconcile_apply().unwrap().unwrap();
            assert_eq!(receipt.generation, 8);
            assert!(
                !backend.recovery_required
                    && backend.pending_read.is_none()
                    && backend.pending_receipt.is_none()
            );
            let memory = memory.lock().unwrap();
            assert!(memory.replies.is_empty());
            assert_eq!(
                memory.words[CONTROL_RELEASE / 4],
                memory.words[CONTROL_READY / 4]
            );
            assert_eq!(
                memory.words[CONTROL_REQUEST / 4],
                5,
                "only one COMMIT may be issued"
            );
        }
    }
    #[test]
    fn acknowledged_commit_recovers_when_release_is_visible_but_sync_fails() {
        use grkit_core::abi::*;
        struct FailedRelease {
            inner: ScriptedMailbox,
            fail: bool,
        }
        impl crate::control::MailboxIo for FailedRelease {
            fn sync(&mut self) -> anyhow::Result<()> {
                crate::control::MailboxIo::sync(&mut self.inner)?;
                if self.fail && self.inner.words[CONTROL_RELEASE / 4] == 1 {
                    self.fail = false;
                    anyhow::bail!("Injected sync failure after RELEASE became visible");
                }
                Ok(())
            }
            fn read(&self, offset: usize) -> u32 {
                self.inner.words[offset / 4]
            }
            fn write(&mut self, offset: usize, value: u32) {
                self.inner.words[offset / 4] = value;
            }
        }
        let mut next = frame(&snapshot_generation(8));
        next.generation = 8;
        let memory = FailedRelease {
            fail: true,
            inner: ScriptedMailbox::new(vec![
                (CONTROL_APPLY_COMMIT, 0, vec![]),
                (CONTROL_CARD_IDENTITY, 0, identity_evidence()),
                (CONTROL_SNAPSHOT_BEGIN, 0, next.encode().unwrap()),
                (CONTROL_SNAPSHOT_END, 0, vec![]),
            ]),
        };
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        backend.transport_verified = true;
        backend.pending_receipt = Some(pending_baseline());
        assert_eq!(
            backend
                .exchange(CONTROL_APPLY_COMMIT, 7, &[])
                .unwrap_err()
                .code,
            "recovery_required"
        );
        let retained = backend.pending_receipt.as_ref().unwrap();
        assert!(retained.acknowledged && retained.ticket.is_some());
        assert!(matches!(
            backend.client.reconcile().unwrap(),
            crate::control::State::Idle { generation: 8, .. }
        ));
        assert_eq!(backend.reconcile_apply().unwrap().unwrap().generation, 8);
        assert!(backend.pending_receipt.is_none() && !backend.recovery_required);
    }
    #[test]
    fn acknowledged_commit_rejects_a_different_card_before_starting_a_snapshot() {
        use grkit_core::abi::*;
        let mut other = identity_evidence();
        other[4] ^= 1;
        assert_ne!(crate::identity::owner_evidence(&other).unwrap(), card_id());
        // No snapshot response is installed: any unexpected snapshot request
        // would fail this scripted mailbox instead of fabricating a receipt.
        let memory = ScriptedMailbox::new(vec![
            (CONTROL_APPLY_COMMIT, 0, vec![]),
            (CONTROL_CARD_IDENTITY, 0, other),
        ]);
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        backend.transport_verified = true;
        backend.pending_receipt = Some(pending_baseline());
        backend.exchange(CONTROL_APPLY_COMMIT, 7, &[]).unwrap();
        backend.recovery_required = true;
        assert_eq!(
            backend.reconcile_apply().unwrap_err().code,
            "recovery_required"
        );
        assert!(backend.recovery_required && backend.pending_receipt.is_some());
    }
    #[test]
    fn equivalent_resources_cannot_replace_a_missing_commit_acknowledgement() {
        // The baseline and prepared manifests are deliberately equal. Without
        // a COMMIT ticket/ACK, reconciliation must not even query a snapshot.
        let memory = ScriptedMailbox::new(vec![]);
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        backend.transport_verified = true;
        backend.recovery_required = true;
        backend.pending_receipt = Some(pending_baseline());
        assert!(backend.reconcile_apply().unwrap().is_none());
        assert!(backend.recovery_required && !backend.pending_receipt.unwrap().acknowledged);
    }
    #[test]
    fn unavailable_lazy_discovery_is_backed_off_and_explicitly_invalidatable() {
        let mut backend = LazyLinuxBackend::new(
            std::path::PathBuf::from("/nonexistent-grkit-test-card"),
            vec![],
            [3; 16],
        );
        assert_eq!(backend.snapshot().unwrap_err().code, "runtime_unavailable");
        let first = backend.retry_after;
        assert_eq!(backend.snapshot().unwrap_err().code, "runtime_unavailable");
        assert_eq!(backend.retry_after, first);
        assert_eq!(backend.capabilities(), RuntimeCapabilities::default());
        backend.invalidate_session();
        assert!(backend.retry_after.is_none());
    }
    #[test]
    fn owner_identity_does_not_require_a_look_manifest() {
        use grkit_core::abi::*;
        let mut evidence = b"GRC1".to_vec();
        evidence.extend([1; 16]);
        let mut sector = [0; 512];
        sector[510..].copy_from_slice(&[0x55, 0xaa]);
        sector[82..90].copy_from_slice(b"FAT32   ");
        sector[66] = 0x29;
        sector[67..71].copy_from_slice(&[1, 2, 3, 4]);
        evidence.extend(sector);
        let expected = crate::identity::owner_evidence(&evidence).unwrap();
        let memory = ScriptedMailbox::new(vec![
            (CONTROL_PING, 0, vec![]),
            (CONTROL_CARD_IDENTITY, 0, evidence),
        ]);
        let mut backend =
            ControlBackend::new(crate::control::Client::new(memory).unwrap(), [3; 16]);
        assert_eq!(backend.card_identity().unwrap(), expected);
        assert!(!backend.capabilities().readable);
        assert!(!backend.capabilities().hot_reload);
    }
    #[test]
    fn stream_rejects_gaps_replays_context_changes_and_corruption() {
        let pin = frame(b"abcdef");
        let mut first = pin.clone();
        first.bytes.truncate(3);
        assert_eq!(
            StreamFrame::decode(&first.encode().unwrap()).unwrap(),
            first
        );
        let mut stream = SnapshotStream::begin(first.clone()).unwrap();
        assert!(stream.append(first).is_err());
        let mut next = pin.clone();
        next.offset = 3;
        next.bytes = b"def".to_vec();
        for change in 0..4 {
            let mut wrong = next.clone();
            match change {
                0 => wrong.epoch[0] += 1,
                1 => wrong.card_hash[0] ^= 1,
                2 => wrong.generation += 1,
                _ => wrong.token[0] ^= 1,
            }
            assert!(stream.append(wrong).is_err());
        }
        stream.append(next).unwrap();
        assert_eq!(stream.finish().unwrap(), b"abcdef");
        let mut corrupt = pin;
        corrupt.bytes[0] ^= 1;
        assert!(SnapshotStream::begin(corrupt).unwrap().finish().is_err());
    }
    #[test]
    fn strict_snapshot_decoder_binds_card_epoch_and_complete_envelope() {
        let bytes = snapshot();
        let pin = frame(&bytes);
        let decoded = decode_snapshot(&bytes, &pin).unwrap();
        assert_eq!(decoded.card_id, card_id());
        assert_eq!(decoded.generation, 7);
        let mut bad = pin.clone();
        bad.card_hash[0] ^= 1;
        assert!(decode_snapshot(&bytes, &bad).is_err());
        bad = pin.clone();
        bad.epoch[0] += 1;
        assert!(decode_snapshot(&bytes, &bad).is_err());
        let mut trailing = bytes;
        trailing.push(0);
        assert!(decode_snapshot(&trailing, &frame(&trailing)).is_err());
    }
    #[test]
    fn frame_decoder_rejects_unbounded_empty_and_reserved_fields() {
        let mut pin = frame(b"test");
        let mut bytes = pin.encode().unwrap();
        bytes[112] = 1;
        assert!(StreamFrame::decode(&bytes).is_err());
        pin.total = StreamFrame::MAX_TOTAL as u32 + 1;
        assert!(pin.encode().is_err());
        pin = frame(b"test");
        pin.offset = u32::MAX;
        assert!(pin.encode().is_err());
        pin = frame(b"test");
        pin.token = [0; 16];
        assert!(pin.encode().is_err());
        assert!(StreamFrame::decode(&[0; 127]).is_err());
    }
}
