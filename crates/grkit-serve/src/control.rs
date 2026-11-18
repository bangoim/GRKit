//! Linux producer for the runtime's bounded control mailbox. ATTACH and PING
//! prove transport only; neither command persists data or changes rendering.
use anyhow::{bail, ensure, Context, Result};
use grkit_core::abi::*;
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Descriptor {
    pub header_address: u32,
    pub mailbox_address: u32,
    pub mailbox_size: u32,
    pub mailbox_version: u32,
    pub epoch: [u32; 4],
    /// The SD card CID that the firmware keeps in RAM, as 32 hex digits, on
    /// builds where the installer knows its address.
    #[serde(default)]
    pub card_cid: Option<String>,
}
impl Descriptor {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= 4096, "Control descriptor exceeds its limit");
        let descriptor: Self =
            serde_json::from_slice(bytes).context("Invalid control descriptor")?;
        descriptor.validate()?;
        Ok(descriptor)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.mailbox_address & 63 == 0,
            "The control mailbox is not cache-line aligned"
        );
        ensure!(
            self.mailbox_size as usize == CONTROL_SIZE
                && self.mailbox_version == CONTROL_VERSION_VALUE,
            "Unsupported control mailbox layout"
        );
        if let Some(cid) = &self.card_cid {
            crate::identity::normalized_cid(cid)?;
        }
        for (address, size) in [
            (self.header_address, HOST_HEADER_SIZE as u32),
            (self.mailbox_address, self.mailbox_size),
        ] {
            ensure!(
                address & 3 == 0
                    && address >= 0x5100_0000
                    && address
                        .checked_add(size)
                        .is_some_and(|end| end <= 0x6000_0000),
                "Control descriptor is outside RTOS memory"
            );
        }
        Ok(())
    }
}

/// Implementations provide coherent volatile words. `sync` must synchronize
/// cache aliases as well as CPU/compiler ordering, not merely issue a fence.
pub trait MailboxIo {
    fn sync(&mut self) -> Result<()>;
    fn read(&self, offset: usize) -> u32;
    fn write(&mut self, offset: usize, value: u32);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Idle {
        epoch: [u32; 4],
        generation: u32,
        next_sequence: u32,
    },
    Pending {
        sequence: u32,
        command: u32,
    },
    Completed {
        sequence: u32,
        command: u32,
        result: u32,
        generation: u32,
        epoch: [u32; 4],
    },
}
#[derive(Clone, Debug)]
pub struct Ticket {
    pub sequence: u32,
    pub command: u32,
    epoch: [u32; 4],
}

pub struct Client<M: MailboxIo> {
    memory: M,
}
impl<M: MailboxIo> Client<M> {
    #[cfg(test)]
    pub(crate) fn memory(&self) -> &M {
        &self.memory
    }
    pub fn new(memory: M) -> Result<Self> {
        let mut client = Self { memory };
        client.reconcile()?;
        Ok(client)
    }
    /// Read a previous service's ticket without resetting or releasing it.
    pub fn reconcile(&mut self) -> Result<State> {
        for _ in 0..3 {
            self.memory.sync()?;
            ensure!(
                self.memory.read(CONTROL_MAGIC) == CONTROL_MAGIC_VALUE
                    && self.memory.read(CONTROL_VERSION) == CONTROL_VERSION_VALUE
                    && self.memory.read(CONTROL_CAPACITY) as usize == CONTROL_PAYLOAD_BYTES,
                "The runtime control descriptor changed"
            );
            let request = self.memory.read(CONTROL_REQUEST);
            let ready = self.memory.read(CONTROL_READY);
            let release = self.memory.read(CONTROL_RELEASE);
            // Acquire READY before reading its result/epoch. If the consumer
            // publishes during this snapshot, retry without changing ownership.
            self.memory.sync()?;
            let epoch = self.epoch();
            let claimed = self.memory.read(CONTROL_CLAIMED);
            let command = self.memory.read(CONTROL_COMMAND);
            let generation = self.memory.read(CONTROL_GENERATION);
            let result = self.memory.read(CONTROL_RESULT);
            if request != self.memory.read(CONTROL_REQUEST)
                || ready != self.memory.read(CONTROL_READY)
                || release != self.memory.read(CONTROL_RELEASE)
            {
                continue;
            }
            ensure!(
                request <= CONTROL_SEQUENCE_MAX && ready <= request && release <= ready,
                "Invalid control ownership markers"
            );
            if request == ready && ready == release {
                ensure!(claimed == ready, "Invalid idle control claim");
                ensure!(
                    request < CONTROL_SEQUENCE_MAX,
                    "Control sequence exhausted; restart the camera"
                );
                return Ok(State::Idle {
                    epoch,
                    generation,
                    next_sequence: request + 1,
                });
            }
            if request == ready {
                ensure!(
                    claimed == request && release.checked_add(1) == Some(request),
                    "Invalid completed control ticket"
                );
                return Ok(State::Completed {
                    sequence: request,
                    command,
                    result,
                    generation,
                    epoch,
                });
            }
            ensure!(
                ready == release
                    && ready.checked_add(1) == Some(request)
                    && (claimed == ready || claimed == request),
                "Invalid pending control ticket"
            );
            return Ok(State::Pending {
                sequence: request,
                command,
            });
        }
        bail!("The control snapshot changed during reconciliation")
    }
    fn epoch(&self) -> [u32; 4] {
        std::array::from_fn(|i| self.memory.read(CONTROL_EPOCH + i * 4))
    }
    /// On first attachment use a fresh nonzero 128-bit nonce from the service.
    /// An attached runtime keeps its existing epoch across service restarts.
    pub fn attach_or_ping(&mut self, nonce: [u8; 16]) -> Result<Ticket> {
        let State::Idle {
            epoch,
            generation,
            next_sequence,
        } = self.reconcile()?
        else {
            bail!("Reconcile the outstanding control ticket before submitting another");
        };
        let attaching = epoch == [0; 4];
        ensure!(
            !attaching || nonce.iter().any(|byte| *byte != 0),
            "The attach nonce must be nonzero"
        );
        let payload = if attaching { nonce.as_slice() } else { &[] };
        let command = if attaching {
            CONTROL_ATTACH
        } else {
            CONTROL_PING
        };
        for (i, word) in epoch.iter().enumerate() {
            self.memory.write(CONTROL_REQUEST_EPOCH + i * 4, *word);
        }
        self.memory.write(CONTROL_COMMAND, command);
        self.memory.write(CONTROL_EXPECTED_GENERATION, generation);
        self.memory.write(CONTROL_LENGTH, payload.len() as u32);
        self.memory
            .write(CONTROL_CHECKSUM, crc32fast::hash(payload));
        self.memory.write(CONTROL_CANCEL, 0);
        for (i, bytes) in payload.as_chunks::<4>().0.iter().enumerate() {
            self.memory
                .write(CONTROL_PAYLOAD + i * 4, u32::from_le_bytes(*bytes));
        }
        self.memory.sync()?;
        self.memory.write(CONTROL_REQUEST, next_sequence);
        // If this sync fails, the request may already be visible. Never reset
        // it or retry blindly: reconcile its markers on the next invocation.
        self.memory.sync()?;
        let epoch = if attaching {
            std::array::from_fn(|i| {
                u32::from_le_bytes(nonce[i * 4..i * 4 + 4].try_into().expect("four bytes"))
            })
        } else {
            epoch
        };
        Ok(Ticket {
            sequence: next_sequence,
            command,
            epoch,
        })
    }
    /// Submit a bounded library protocol frame. Publication uncertainty must be
    /// reconciled; the caller must not retry a mutation after a sync failure.
    pub fn submit_library(
        &mut self,
        command: u32,
        expected_generation: u32,
        payload: &[u8],
    ) -> Result<Ticket> {
        self.submit_library_tracked(command, expected_generation, payload, &mut None)
    }
    /// Preserve ownership before publishing REQUEST: the final cache sync may
    /// fail after the consumer has already observed and executed the command.
    pub fn submit_library_tracked(
        &mut self,
        command: u32,
        expected_generation: u32,
        payload: &[u8],
        published: &mut Option<Ticket>,
    ) -> Result<Ticket> {
        *published = None;
        ensure!(
            (CONTROL_SNAPSHOT_BEGIN..=CONTROL_CARD_IDENTITY).contains(&command),
            "Unknown library command"
        );
        ensure!(
            payload.len() <= CONTROL_PAYLOAD_BYTES,
            "Library frame exceeds mailbox capacity"
        );
        let State::Idle {
            epoch,
            generation,
            next_sequence,
        } = self.reconcile()?
        else {
            bail!("An outstanding runtime ticket requires reconciliation");
        };
        ensure!(
            epoch != [0; 4] && generation == expected_generation,
            "The library runtime context changed"
        );
        for (i, word) in epoch.iter().enumerate() {
            self.memory.write(CONTROL_REQUEST_EPOCH + i * 4, *word);
        }
        self.memory.write(CONTROL_COMMAND, command);
        self.memory
            .write(CONTROL_EXPECTED_GENERATION, expected_generation);
        self.memory.write(CONTROL_LENGTH, payload.len() as u32);
        self.memory
            .write(CONTROL_CHECKSUM, crc32fast::hash(payload));
        self.memory.write(CONTROL_CANCEL, 0);
        for (i, chunk) in payload.chunks(4).enumerate() {
            let mut word = [0; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            self.memory
                .write(CONTROL_PAYLOAD + i * 4, u32::from_le_bytes(word));
        }
        self.memory.sync()?;
        let ticket = Ticket {
            sequence: next_sequence,
            command,
            epoch,
        };
        *published = Some(ticket.clone());
        self.memory.write(CONTROL_REQUEST, next_sequence);
        self.memory.sync()?;
        Ok(ticket)
    }
    /// Copy consumer-owned response bytes while READY is held. No Linux write
    /// or cache clean may touch this region. The caller consumes before release.
    pub fn response(&mut self, ticket: &Ticket) -> Result<Vec<u8>> {
        ensure!(
            self.poll(ticket)?.is_some(),
            "The runtime response is not ready"
        );
        let length = self.memory.read(CONTROL_RESPONSE_LENGTH) as usize;
        let checksum = self.memory.read(CONTROL_RESPONSE_CHECKSUM);
        ensure!(
            length <= CONTROL_PAYLOAD_BYTES,
            "Runtime response exceeds mailbox capacity"
        );
        let mut bytes = Vec::with_capacity(length);
        for offset in (0..length).step_by(4) {
            let word = self.memory.read(CONTROL_RESPONSE + offset).to_le_bytes();
            bytes.extend_from_slice(&word[..(length - offset).min(4)]);
        }
        ensure!(
            crc32fast::hash(&bytes) == checksum,
            "Runtime response checksum mismatch"
        );
        ensure!(
            self.poll(ticket)?.is_some()
                && self.memory.read(CONTROL_RESPONSE_LENGTH) as usize == length
                && self.memory.read(CONTROL_RESPONSE_CHECKSUM) == checksum,
            "The runtime response changed while reading"
        );
        Ok(bytes)
    }
    pub fn release_library(&mut self, ticket: &Ticket) -> Result<()> {
        ensure!(
            (CONTROL_SNAPSHOT_BEGIN..=CONTROL_CARD_IDENTITY).contains(&ticket.command),
            "Not a library ticket"
        );
        self.response(ticket)?;
        self.memory.sync()?;
        self.memory.write(CONTROL_RELEASE, ticket.sequence);
        self.memory.sync()
    }
    pub fn poll(&mut self, ticket: &Ticket) -> Result<Option<State>> {
        let state = self.reconcile()?;
        match &state {
            State::Pending { sequence, command }
                if *sequence == ticket.sequence && *command == ticket.command =>
            {
                Ok(None)
            }
            State::Completed {
                sequence,
                command,
                result,
                epoch,
                ..
            } if *sequence == ticket.sequence && *command == ticket.command => {
                ensure!(
                    (*result != 0 && ticket.command == CONTROL_ATTACH) || *epoch == ticket.epoch,
                    "The runtime acknowledged a different control context"
                );
                Ok(Some(state))
            }
            _ => bail!("The runtime control ticket changed"),
        }
    }
    /// Reconstruct only a completed library ticket from a previous producer.
    /// This consumes no HTTP request identity and never infers its outcome.
    pub fn completed_library_ticket(&mut self) -> Result<Ticket> {
        let before = self.reconcile()?;
        let State::Completed {
            sequence,
            command,
            epoch,
            result,
            ..
        } = before
        else {
            bail!("The inherited library command has not completed");
        };
        ensure!(
            (CONTROL_SNAPSHOT_BEGIN..=CONTROL_CARD_IDENTITY).contains(&command),
            "Unknown inherited library command"
        );
        ensure!(
            result == 0 || (CONTROL_BAD_CONTEXT..=CONTROL_BUSY).contains(&result),
            "Unknown inherited result"
        );
        ensure!(epoch != [0; 4], "The inherited library epoch is empty");
        let requested =
            std::array::from_fn::<_, 4, _>(|i| self.memory.read(CONTROL_REQUEST_EPOCH + i * 4));
        ensure!(requested == epoch, "The inherited library epoch changed");
        let ticket = Ticket {
            sequence,
            command,
            epoch,
        };
        self.response(&ticket)?;
        ensure!(
            self.reconcile()? == before
                && (0..4).all(|i| self.memory.read(CONTROL_REQUEST_EPOCH + i * 4) == epoch[i]),
            "The inherited receipt changed"
        );
        Ok(ticket)
    }

    /// Acknowledge only after the caller has consumed the result. This is also
    /// the explicit reconciliation step for a prior service's completed ping.
    pub fn release(&mut self, sequence: u32) -> Result<()> {
        let State::Completed {
            sequence: current,
            command,
            ..
        } = self.reconcile()?
        else {
            bail!("The control ticket has not completed");
        };
        ensure!(
            current == sequence && matches!(command, CONTROL_ATTACH | CONTROL_PING),
            "The control ticket is not owned by this client"
        );
        self.memory.sync()?;
        self.memory.write(CONTROL_RELEASE, sequence);
        self.memory.sync()
    }
}

/// Hashes must come from a verified release, not PACKAGE.TXT or filenames.
/// An empty trust set cannot authorize executing an installer from the card.
#[cfg(all(target_os = "linux", target_arch = "arm"))]
/// The mailbox client and the normalized card CID of the descriptor.
pub fn discover(
    root: &Path,
    trusted_installers: &BTreeMap<String, String>,
) -> Result<(Client<MappedMailbox>, Option<String>)> {
    platform::discover(root, trusted_installers)
}
#[cfg(not(all(target_os = "linux", target_arch = "arm")))]
pub fn discover(
    _root: &Path,
    _trusted_installers: &BTreeMap<String, String>,
) -> Result<(Client<MappedMailbox>, Option<String>)> {
    bail!("Runtime control is available only on the camera's ARM Linux")
}
#[cfg(all(target_os = "linux", target_arch = "arm"))]
pub use platform::MappedMailbox;
#[cfg(not(all(target_os = "linux", target_arch = "arm")))]
pub struct MappedMailbox {
    _private: (),
}
#[cfg(not(all(target_os = "linux", target_arch = "arm")))]
impl MailboxIo for MappedMailbox {
    fn sync(&mut self) -> Result<()> {
        bail!("Runtime control is unavailable on this platform")
    }
    fn read(&self, _offset: usize) -> u32 {
        unreachable!("no mapping can be created on this platform")
    }
    fn write(&mut self, _offset: usize, _value: u32) {
        unreachable!("no mapping can be created on this platform")
    }
}

// Match runtime::core_path_of. I*.BIN files are RTOS native loader images,
// not Linux installers, and must never be executed for discovery.
#[cfg(any(test, all(target_os = "linux", target_arch = "arm")))]
fn installer_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes == b"CORE.BIN"
        || (bytes.len() == 9
            && &bytes[..4] == b"CORE"
            && (b'1'..=b'9').contains(&bytes[4])
            && &bytes[5..] == b".BIN")
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
mod platform {
    use super::*;
    use std::{
        fs::{self, File, OpenOptions},
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::OpenOptionsExt,
        },
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
    const MAX_INSTALLERS: usize = 9;
    const MAX_ENTRIES: usize = 128;
    const MAX_BINARY: u64 = 8 * 1024 * 1024;
    pub struct MappedMailbox {
        mapping: *mut libc::c_void,
        length: usize,
        offset: usize,
        _lock: File,
    }
    // SAFETY: moving the mapping transfers its sole Linux producer and flock
    // ownership. Access still requires &mut Client; no Sync implementation is
    // provided, and all raw addresses remain private to this value.
    unsafe impl Send for MappedMailbox {}

    impl Drop for MappedMailbox {
        fn drop(&mut self) {
            // SAFETY: this mapping and its exact length belong to this value.
            unsafe {
                libc::munmap(self.mapping, self.length);
            }
        }
    }
    impl MappedMailbox {
        fn map(descriptor: &Descriptor, lock: File) -> Result<Self> {
            descriptor.validate()?;
            // SAFETY: sysconf reads a constant process property.
            let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            ensure!(
                page > 0 && (page as usize).is_power_of_two(),
                "Unsupported memory page size"
            );
            let page = page as usize;
            let physical = descriptor.mailbox_address as usize;
            let base = physical & !(page - 1);
            let offset = physical - base;
            let length = (offset + CONTROL_SIZE).div_ceil(page) * page;
            let device = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_SYNC)
                .open("/dev/mem")?;
            // SAFETY: a validated physical mailbox range is page aligned and
            // bounded. We retain this mapping until Drop and never expose it.
            let mapping = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    length,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    device.as_raw_fd(),
                    base as libc::off_t,
                )
            };
            ensure!(
                mapping != libc::MAP_FAILED,
                "Map runtime control mailbox: {}",
                std::io::Error::last_os_error()
            );
            Ok(Self {
                mapping,
                length,
                offset,
                _lock: lock,
            })
        }
        fn word(&self, offset: usize) -> *mut u32 {
            assert!(offset.is_multiple_of(4) && offset <= CONTROL_SIZE - 4);
            // SAFETY: the checked word falls wholly within the owned mapping.
            unsafe { self.mapping.cast::<u8>().add(self.offset + offset).cast() }
        }
    }
    impl MailboxIo for MappedMailbox {
        fn sync(&mut self) -> Result<()> {
            // Match INSTALL's alias synchronization. A successful PING is still
            // required to prove this target's cross-OS transport is responsive.
            unsafe {
                core::arch::asm!("dsb sy", options(nostack, preserves_flags));
            }
            let base = self.mapping as usize + self.offset;
            // O_SYNC /dev/mem maps the Linux view without normal data caching.
            // RTOS cleans descriptor and consumer lines to PoC. Never flush a
            // Linux alias across those RTOS-owned cache lines: only the producer
            // metadata and payload belong to this side of the protocol.
            for (start, end) in [
                (CONTROL_REQUEST, CONTROL_CLAIMED),
                (CONTROL_PAYLOAD, CONTROL_RESPONSE),
            ] {
                // SAFETY: these generated ranges are within this mapping; ARM
                // cacheflush accepts mapped virtual start/end addresses and 0.
                let result = unsafe { libc::syscall(0x0f0002, base + start, base + end, 0usize) };
                ensure!(
                    result == 0,
                    "Synchronize control producer alias: {}",
                    std::io::Error::last_os_error()
                );
            }
            unsafe {
                core::arch::asm!("dsb sy", "dmb ish", options(nostack, preserves_flags));
            }
            Ok(())
        }
        fn read(&self, offset: usize) -> u32 {
            // SAFETY: word checks alignment and bounds; firmware owns its data.
            unsafe { std::ptr::read_volatile(self.word(offset)) }
        }
        fn write(&mut self, offset: usize, value: u32) {
            // SAFETY: word checks alignment and bounds. Client writes only the
            // producer-owned fields after checking REQUEST/READY/RELEASE.
            unsafe {
                std::ptr::write_volatile(self.word(offset), value);
            }
        }
    }
    fn lock() -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open("/tmp/grkit-control.lock")?;
        ensure!(file.metadata()?.is_file(), "Invalid control producer lock");
        // SAFETY: flock operates on the live owned file descriptor.
        ensure!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "Another runtime control producer is active"
        );
        Ok(file)
    }
    pub(super) fn discover(
        root: &Path,
        trusted: &BTreeMap<String, String>,
    ) -> Result<(Client<MappedMailbox>, Option<String>)> {
        ensure!(
            !trusted.is_empty(),
            "No trusted control installer hashes are available"
        );
        let lock = lock()?;
        let directory = root.join(grkit_core::card::PACKAGE_FOLDER);
        let mut candidates = Vec::new();
        for (count, entry) in fs::read_dir(directory)?.enumerate() {
            ensure!(
                count < MAX_ENTRIES,
                "Package discovery entry limit exceeded"
            );
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Invalid package filename"))?;
            if !installer_name(&name) || !trusted.contains_key(&name) {
                continue;
            }
            ensure!(
                entry.file_type()?.is_file(),
                "Control installer is not a regular file"
            );
            ensure!(
                candidates.len() < MAX_INSTALLERS,
                "Control installer count limit exceeded"
            );
            candidates.push((name, entry.path()));
        }
        candidates.sort();
        // Another build's core can also find the mailbox, by relocation, but
        // only the core of this firmware build knows where the card CID is.
        // INSTALL.SH records the core that installed; try it first.
        if let Some(installed) = installed_core(root) {
            candidates.sort_by_key(|(name, _)| *name != installed);
        }
        let mut failures = Vec::new();
        for (name, path) in candidates {
            // O_NOFOLLOW closes the directory-entry/open race for symlinks.
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)?;
            ensure!(
                file.metadata()?.is_file(),
                "Control installer is not a regular file"
            );
            let mut bytes = Vec::new();
            file.take(MAX_BINARY + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_BINARY,
                "Control installer is oversized"
            );
            ensure!(
                trusted.get(&name) == Some(&grkit_core::packaging::sha256(&bytes)),
                "Control installer digest does not match its verified release"
            );
            match run_installer(&bytes) {
                Ok(descriptor) => {
                    let cid = descriptor
                        .card_cid
                        .as_deref()
                        .map(crate::identity::normalized_cid)
                        .transpose()?;
                    return Ok((Client::new(MappedMailbox::map(&descriptor, lock)?)?, cid));
                }
                Err(error) => failures.push(format!("{name}: {error:#}")),
            }
        }
        bail!(
            "No trusted installer found a live runtime control mailbox: {}",
            if failures.is_empty() {
                "no matching trusted CORE files".into()
            } else {
                failures.join("; ")
            }
        )
    }
    /// The core file that INSTALL.SH last installed with, from
    /// `GRKIT/CORE.TXT`. A start with the fast path does not write it.
    fn installed_core(root: &Path) -> Option<String> {
        let mut text = String::new();
        File::open(
            root.join(grkit_core::card::DATA_FOLDER_PATH)
                .join(grkit_core::runtime::CORE_CHOICE),
        )
        .ok()?
        .take(64)
        .read_to_string(&mut text)
        .ok()?;
        let name = text.split_whitespace().next()?;
        installer_name(name).then(|| name.to_owned())
    }
    fn run_installer(bytes: &[u8]) -> Result<Descriptor> {
        // Execute the verified RAM copy, so replacing a card file after hashing
        // cannot replace the executable or keep the card executable mapped.
        let fd = unsafe { libc::memfd_create(c"grkit-control-info".as_ptr(), libc::MFD_CLOEXEC) };
        ensure!(
            fd >= 0,
            "Create control discovery executable: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: fd is new and exclusively owned.
        let mut executable = unsafe { File::from_raw_fd(fd) };
        executable.write_all(bytes)?;
        let mut child = Command::new(format!("/proc/self/fd/{fd}"))
            .arg("--control-info")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdout = child.stdout.take().context("Missing discovery stdout")?;
        // SAFETY: stdout's descriptor remains alive while its flags are changed.
        let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                < 0
        {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Configure discovery output");
        }
        let deadline = Instant::now() + DISCOVERY_TIMEOUT;
        let mut output = Vec::new();
        let mut buffer = [0; 512];
        let result = loop {
            if Instant::now() >= deadline {
                break Err(anyhow::anyhow!("Control discovery timed out"));
            }
            match stdout.read(&mut buffer) {
                Ok(0) => match child.try_wait() {
                    Ok(Some(status)) if status.success() => break Descriptor::parse(&output),
                    Ok(Some(status)) => {
                        break Err(anyhow::anyhow!(
                            "Installer exited with {status}; output: {}",
                            String::from_utf8_lossy(&output)
                        ));
                    }
                    Ok(None) => {}
                    Err(error) => break Err(error.into()),
                },
                Ok(count) => {
                    output.extend_from_slice(&buffer[..count]);
                    if output.len() > 4096 {
                        break Err(anyhow::anyhow!("Control discovery output limit exceeded"));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => break Err(error.into()),
            }
            thread::sleep(Duration::from_millis(5));
        };
        let _ = child.kill();
        let _ = child.wait();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Memory {
        words: Vec<u32>,
        fail_sync: bool,
        fail_after_request: bool,
    }
    impl Memory {
        fn new() -> Self {
            let mut memory = Self {
                words: vec![0; CONTROL_WORDS],
                fail_sync: false,
                fail_after_request: false,
            };
            memory.words[CONTROL_MAGIC / 4] = CONTROL_MAGIC_VALUE;
            memory.words[CONTROL_VERSION / 4] = CONTROL_VERSION_VALUE;
            memory.words[CONTROL_CAPACITY / 4] = CONTROL_PAYLOAD_BYTES as u32;
            memory.words[CONTROL_GENERATION / 4] = 7;
            memory
        }
        fn complete(&mut self, ticket: &Ticket, result: u32) {
            self.words[CONTROL_CLAIMED / 4] = ticket.sequence;
            self.words[CONTROL_RESULT / 4] = result;
            if result == 0 {
                for (i, word) in ticket.epoch.iter().enumerate() {
                    self.words[CONTROL_EPOCH / 4 + i] = *word;
                }
            }
            self.words[CONTROL_READY / 4] = ticket.sequence;
        }
    }
    impl MailboxIo for Memory {
        fn sync(&mut self) -> Result<()> {
            ensure!(!self.fail_sync, "Injected cache sync failure");
            ensure!(
                !self.fail_after_request || self.read(CONTROL_REQUEST) == self.read(CONTROL_READY),
                "Injected failure after request publication"
            );
            Ok(())
        }
        fn read(&self, offset: usize) -> u32 {
            self.words[offset / 4]
        }
        fn write(&mut self, offset: usize, value: u32) {
            assert!(
                matches!(
                    offset,
                    CONTROL_REQUEST
                        | CONTROL_RELEASE
                        | CONTROL_CANCEL
                        | CONTROL_COMMAND
                        | CONTROL_EXPECTED_GENERATION
                        | CONTROL_LENGTH
                        | CONTROL_CHECKSUM
                ) || (CONTROL_REQUEST_EPOCH..CONTROL_REQUEST_EPOCH + 16).contains(&offset)
                    || (CONTROL_PAYLOAD..CONTROL_RESPONSE).contains(&offset),
                "producer wrote consumer field {offset}"
            );
            self.words[offset / 4] = value;
        }
    }
    #[test]
    fn publication_failure_retains_the_ticket_before_the_consumer_can_run() {
        let mut client = Client::new(Memory::new()).unwrap();
        let attach = client.attach_or_ping([1; 16]).unwrap();
        client.memory.complete(&attach, 0);
        client.release(attach.sequence).unwrap();
        client.memory.fail_after_request = true;
        let mut published = None;
        assert!(client
            .submit_library_tracked(CONTROL_APPLY_COMMIT, 7, &[], &mut published)
            .is_err());
        let ticket = published.unwrap();
        assert_eq!(ticket.command, CONTROL_APPLY_COMMIT);
        assert_eq!(ticket.sequence, attach.sequence + 1);
        assert_eq!(client.memory.read(CONTROL_REQUEST), ticket.sequence);
        client.memory.complete(&ticket, 0);
        client.memory.words[CONTROL_RESPONSE_CHECKSUM / 4] = crc32fast::hash(&[]);
        assert!(client.poll(&ticket).unwrap().is_some());
        client.release_library(&ticket).unwrap();
    }

    #[test]
    fn discovery_accepts_only_linux_core_installer_names() {
        for name in ["CORE.BIN", "CORE1.BIN", "CORE9.BIN"] {
            assert!(installer_name(name));
        }
        for name in [
            "I588221.BIN",
            "INSTALL.BIN",
            "CORE0.BIN",
            "CORE10.BIN",
            "COREA.BIN",
            "core.bin",
            "../CORE.BIN",
            "CORE1.BIN/evil",
            "CÖRE1.BIN",
        ] {
            assert!(!installer_name(name), "{name}");
        }
    }

    #[test]
    fn attaches_once_then_reuses_the_epoch_for_ping() {
        let mut client = Client::new(Memory::new()).unwrap();
        let ticket = client.attach_or_ping([9; 16]).unwrap();
        assert_eq!(ticket.command, CONTROL_ATTACH);
        assert_eq!(ticket.sequence, 1);
        assert_eq!(
            client.memory.read(CONTROL_CHECKSUM),
            crc32fast::hash(&[9; 16])
        );
        assert!(client.poll(&ticket).unwrap().is_none());
        assert!(client.release(1).is_err());
        client.memory.complete(&ticket, 0);
        assert!(matches!(
            client.poll(&ticket).unwrap(),
            Some(State::Completed {
                result: 0,
                generation: 7,
                ..
            })
        ));
        assert!(client.attach_or_ping([8; 16]).is_err());
        client.release(1).unwrap();
        let ping = client.attach_or_ping([8; 16]).unwrap();
        assert_eq!(ping.command, CONTROL_PING);
        assert_eq!(ping.sequence, 2);
        assert_eq!(ping.epoch, ticket.epoch);
        assert_eq!(client.memory.read(CONTROL_LENGTH), 0);
        assert_eq!(client.memory.read(CONTROL_CHECKSUM), 0);
    }
    #[test]
    fn library_response_is_bounded_verified_and_never_written_by_producer() {
        let mut client = Client::new(Memory::new()).unwrap();
        let attach = client.attach_or_ping([1; 16]).unwrap();
        client.memory.complete(&attach, 0);
        client.release(attach.sequence).unwrap();
        assert!(client
            .submit_library(CONTROL_SNAPSHOT_BEGIN, 6, &[])
            .is_err());
        assert!(client.submit_library(999, 7, &[]).is_err());
        assert!(client
            .submit_library(CONTROL_APPLY_WRITE, 7, &vec![0; CONTROL_PAYLOAD_BYTES + 1])
            .is_err());
        let ticket = client
            .submit_library(CONTROL_SNAPSHOT_BEGIN, 7, &[1, 2, 3])
            .unwrap();
        assert_eq!(client.memory.read(CONTROL_PAYLOAD), 0x00030201);
        assert!(client.response(&ticket).is_err());
        client.memory.complete(&ticket, 0);
        client.memory.words[CONTROL_RESPONSE_LENGTH / 4] = 3;
        client.memory.words[CONTROL_RESPONSE_CHECKSUM / 4] = crc32fast::hash(&[4, 5, 6]);
        client.memory.words[CONTROL_RESPONSE / 4] = 0xff060504;
        assert_eq!(client.response(&ticket).unwrap(), [4, 5, 6]);
        client.memory.words[CONTROL_RESPONSE / 4] ^= 1;
        assert!(client.release_library(&ticket).is_err());
        assert_eq!(client.memory.read(CONTROL_RELEASE), attach.sequence);
        client.memory.words[CONTROL_RESPONSE / 4] ^= 1;
        client.memory.words[CONTROL_RESPONSE_LENGTH / 4] = CONTROL_PAYLOAD_BYTES as u32 + 1;
        assert!(client.response(&ticket).is_err());
        client.memory.words[CONTROL_RESPONSE_LENGTH / 4] = 3;
        client.release_library(&ticket).unwrap();
        assert!(matches!(
            client.reconcile().unwrap(),
            State::Idle {
                next_sequence: 3,
                ..
            }
        ));
    }
    #[test]
    fn service_restart_preserves_pending_and_completed_tickets() {
        let mut original = Client::new(Memory::new()).unwrap();
        let ticket = original.attach_or_ping([1; 16]).unwrap();
        let mut restarted = Client::new(original.memory).unwrap();
        assert!(matches!(
            restarted.reconcile().unwrap(),
            State::Pending {
                sequence: 1,
                command: CONTROL_ATTACH
            }
        ));
        assert!(restarted.attach_or_ping([2; 16]).is_err());
        restarted.memory.complete(&ticket, 0);
        let mut restarted = Client::new(restarted.memory).unwrap();
        assert!(matches!(
            restarted.reconcile().unwrap(),
            State::Completed {
                sequence: 1,
                result: 0,
                ..
            }
        ));
        assert!(restarted.release(2).is_err());
        restarted.release(1).unwrap();
        assert_eq!(
            restarted.attach_or_ping([2; 16]).unwrap().command,
            CONTROL_PING
        );
    }
    #[test]
    fn rejects_bad_layout_zero_nonce_and_exhausted_or_invalid_markers() {
        let mut bad = Memory::new();
        bad.words[CONTROL_CAPACITY / 4] = 1;
        assert!(Client::new(bad).is_err());
        let mut client = Client::new(Memory::new()).unwrap();
        assert!(client.attach_or_ping([0; 16]).is_err());
        for offset in [CONTROL_REQUEST, CONTROL_READY, CONTROL_RELEASE] {
            client.memory.words[offset / 4] = CONTROL_SEQUENCE_MAX;
        }
        assert!(client.attach_or_ping([1; 16]).is_err());
        assert_eq!(client.memory.read(CONTROL_REQUEST), CONTROL_SEQUENCE_MAX);
        client.memory.words[CONTROL_REQUEST / 4] = 2;
        client.memory.words[CONTROL_READY / 4] = 0;
        client.memory.words[CONTROL_RELEASE / 4] = 0;
        assert!(client.reconcile().is_err());
    }
    #[test]
    fn failed_result_is_not_transport_success_and_sync_failure_does_not_reset_request() {
        let mut client = Client::new(Memory::new()).unwrap();
        let ticket = client.attach_or_ping([1; 16]).unwrap();
        client.memory.complete(&ticket, CONTROL_BAD_CONTEXT);
        assert!(matches!(
            client.poll(&ticket).unwrap(),
            Some(State::Completed {
                result: CONTROL_BAD_CONTEXT,
                ..
            })
        ));
        client.memory.fail_sync = true;
        assert!(client.release(ticket.sequence).is_err());
        assert_eq!(client.memory.read(CONTROL_RELEASE), 0);
        assert_eq!(client.memory.read(CONTROL_REQUEST), ticket.sequence);
    }
    #[test]
    fn rejects_success_with_a_different_epoch() {
        let mut client = Client::new(Memory::new()).unwrap();
        let ticket = client.attach_or_ping([1; 16]).unwrap();
        client.memory.complete(&ticket, 0);
        client.memory.words[CONTROL_EPOCH / 4] = 99;
        assert!(client.poll(&ticket).is_err());
    }
    #[test]
    fn descriptor_rejects_version_length_alignment_range_and_unknown_fields() {
        let descriptor = Descriptor {
            header_address: 0x53000000,
            mailbox_address: 0x53010000,
            mailbox_size: CONTROL_SIZE as u32,
            mailbox_version: CONTROL_VERSION_VALUE,
            epoch: [0; 4],
            card_cid: Some("aa585951454d552101deadbeef006201".into()),
        };
        descriptor.validate().unwrap();
        let mut invalid = descriptor.clone();
        invalid.card_cid = Some("00".into());
        assert!(invalid.validate().is_err());
        let mut invalid = descriptor.clone();
        invalid.mailbox_version += 1;
        assert!(invalid.validate().is_err());
        let mut invalid = descriptor.clone();
        invalid.mailbox_size += 4;
        assert!(invalid.validate().is_err());
        let mut invalid = descriptor.clone();
        invalid.mailbox_address += 1;
        assert!(invalid.validate().is_err());
        let mut invalid = descriptor.clone();
        invalid.mailbox_address = 0xfffff000;
        assert!(invalid.validate().is_err());
        let mut invalid = descriptor;
        invalid.header_address = 0;
        assert!(invalid.validate().is_err());
        assert!(Descriptor::parse(b"{\"shell\":\"anything\"}").is_err());
        assert!(Descriptor::parse(&vec![b' '; 4097]).is_err());
    }
}
