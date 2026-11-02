//! The ordered alert handoff journal between a local monitor and the alert
//! dispatcher.
//!
//! Each check (health, backup) has one producer: its monitor. Before the
//! monitor publishes a status, it admits the observation into its own journal.
//! The journal is private file state, not a queue service:
//!
//! - `<state>/.<check>.journal.lock` is the stable journal lock. The producer
//!   holds it for the whole admission; the dispatcher holds it only briefly to
//!   select or remove an entry, never during network I/O. It is never renamed
//!   or deleted.
//! - `<state>/<check>.sequence.json` is the producer's private cursor (0600):
//!   the next sequence, the last admitted state, and a reservation that is in
//!   progress.
//! - `<state>/alert-handoff/<sequence>.json` is one immutable committed entry.
//!   The set of committed entries is the journal.
//!
//! # Admission
//!
//! Under the journal lock the producer reserves a sequence in its cursor
//! (atomic write, `fsync`, rename, directory `fsync`), writes the entry to a
//! temp file, gives it the dispatcher's read ACL, `fsync`s it, renames it to its
//! final name and `fsync`s the directory. Only then does it record the admitted
//! state in the cursor. The rename is the admission point. A crash before the
//! rename loses the observation (the next run observes again); a crash after
//! the rename leaves a valid entry that the next producer run recovers under
//! the lock before it admits anything new, so an in-doubt admission ends with
//! one ordered entry, never two.
//!
//! An observation equal to the last admitted state is not admitted again: the
//! journal already holds or has delivered that state. When the last admitted
//! state is unknown (a lost cursor, or an uncertain reservation whose entry the
//! dispatcher already removed), the observation is always admitted.
//!
//! A full journal (256 entries or 1 MiB) or a journal lock held for 20 s
//! refuses the admission. The caller then fails with exit 4 and publishes no
//! status.
use crate::cli::Exit;
use crate::cli::monitors::status::{
    Admission, Category, Status, parse_bounded_timestamp, parse_strict_json, utc_text,
};
use crate::cli::state::{read_bounded, write_atomic};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Most committed entries one journal may hold.
pub const MAX_PENDING_ENTRIES: usize = 256;
/// Most bytes the committed entries of one journal may hold together.
pub const MAX_PENDING_BYTES: u64 = 1024 * 1024;
/// Largest committed entry a reader accepts.
pub const MAX_ENTRY_BYTES: u64 = 4096;
/// Longest wait for the journal lock before an admission is refused.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(20);
/// Directory of committed entries, inside the producer's state directory.
pub const HANDOFF_DIRECTORY: &str = "alert-handoff";
/// Version of the committed entry document.
pub const ENTRY_SCHEMA_VERSION: u32 = 1;
const CURSOR_SCHEMA_VERSION: u32 = 1;
const MAX_CURSOR_BYTES: u64 = 4096;
const LOCK_POLL: Duration = Duration::from_millis(25);
/// Where incomplete temp files and unexpected names are moved.
const QUARANTINE_DIRECTORY: &str = "alert-handoff-quarantine";
/// How many quarantined files are kept; older ones are removed.
const QUARANTINE_KEEP: usize = 32;
/// Most names a handoff directory may hold before a reader refuses it. It
/// bounds one listing even when someone fills the directory with junk.
const MAX_LISTED_NAMES: usize = 4 * MAX_PENDING_ENTRIES;

// ---- names -------------------------------------------------------------------

/// The two local checks that hand alerts to the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Check {
    Health,
    Backup,
}

impl Check {
    pub const ALL: [Check; 2] = [Check::Health, Check::Backup];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::Backup => "backup",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "health" => Some(Self::Health),
            "backup" => Some(Self::Backup),
            _ => None,
        }
    }

    /// The installed private state directory of this check's monitor.
    pub fn installed_state_directory(self) -> PathBuf {
        PathBuf::from(match self {
            Self::Health => "/var/lib/verdaxis-health",
            Self::Backup => "/var/lib/verdaxis-backup-verify",
        })
    }
}

/// The alert state of one observation. A warning alerts like a failure, as the
/// monitor's exit code already does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertState {
    Healthy,
    Failure,
}

impl AlertState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Failure => "failure",
        }
    }

    pub fn from_category(category: Category) -> Self {
        match category {
            Category::Healthy => Self::Healthy,
            Category::Warning | Category::Failure => Self::Failure,
        }
    }
}

/// One observation a monitor wants delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlertObservation {
    pub check: Check,
    pub state: AlertState,
    pub observed_at: DateTime<Utc>,
}

/// The journal position of one admitted observation. Sequences only grow; a
/// gap after a crash is valid, a reused number never happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sequence(pub u64);

/// A committed entry. Fields are in key order, so the compact JSON has sorted
/// keys; `tests/rust_contract/phase10/alert-journal.schema.json` describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalEntry {
    pub check: Check,
    pub observed_at: String,
    pub schema_version: u32,
    pub sequence: u64,
    pub state: AlertState,
}

/// The paths of one check's journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPaths {
    pub check: Check,
    pub state_directory: PathBuf,
}

impl JournalPaths {
    pub fn new(check: Check, state_directory: impl Into<PathBuf>) -> Self {
        Self {
            check,
            state_directory: state_directory.into(),
        }
    }

    /// The installed layout of `check`.
    pub fn installed(check: Check) -> Self {
        Self::new(check, check.installed_state_directory())
    }

    pub fn handoff_directory(&self) -> PathBuf {
        self.state_directory.join(HANDOFF_DIRECTORY)
    }

    pub fn cursor(&self) -> PathBuf {
        self.state_directory
            .join(format!("{}.sequence.json", self.check.as_str()))
    }

    pub fn journal_lock(&self) -> PathBuf {
        self.state_directory
            .join(format!(".{}.journal.lock", self.check.as_str()))
    }

    /// The producer lock, held across observation, admission and publication.
    pub fn producer_lock(&self) -> PathBuf {
        self.state_directory
            .join(format!(".{}.producer.lock", self.check.as_str()))
    }

    pub fn quarantine_directory(&self) -> PathBuf {
        self.state_directory.join(QUARANTINE_DIRECTORY)
    }

    pub fn entry(&self, sequence: Sequence) -> PathBuf {
        self.handoff_directory().join(entry_name(sequence))
    }
}

fn entry_name(sequence: Sequence) -> String {
    format!("{}.json", sequence.0)
}

/// The sequence an allowlisted entry name carries: decimal digits without a
/// leading zero, then `.json`. Every other name is not an entry.
pub fn entry_sequence(name: &str) -> Option<Sequence> {
    let digits = name.strip_suffix(".json")?;
    if digits.is_empty()
        || digits.len() > 20
        || digits.starts_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    digits.parse().ok().map(Sequence)
}

// ---- locks -------------------------------------------------------------------

/// A held `flock`. It is released when dropped or when the process ends, so a
/// killed holder never leaves the lock taken.
pub struct HeldLock(File);

impl Drop for HeldLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Why a lock could not be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockFailure {
    TimedOut,
    Unavailable,
}

/// Take an exclusive `flock` on `file`, polling until `timeout` has passed.
///
/// A read-only descriptor is enough: `flock` needs no write access, which is
/// how the dispatcher locks a producer's journal lock with only a read ACL.
pub fn lock_with_timeout(file: File, timeout: Duration) -> Result<HeldLock, LockFailure> {
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(HeldLock(file)),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(_)) => return Err(LockFailure::Unavailable),
        }
        if Instant::now() >= deadline {
            return Err(LockFailure::TimedOut);
        }
        std::thread::sleep(LOCK_POLL);
    }
}

const NO_FOLLOW: i32 = libc::O_NOFOLLOW | libc::O_CLOEXEC;

/// Open (creating on first use) the producer's stable journal lock.
fn open_producer_lock(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(NO_FOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    Ok(file)
}

/// Open an existing journal lock read-only, as the dispatcher does. `None`
/// when no producer has created it yet.
fn open_reader_lock(path: &Path) -> std::io::Result<Option<File>> {
    match OpenOptions::new()
        .read(true)
        .custom_flags(NO_FOLLOW)
        .open(path)
    {
        Ok(file) => {
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(std::io::ErrorKind::InvalidInput.into());
            }
            Ok(Some(file))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

// ---- listing -------------------------------------------------------------------

/// One bounded listing of a handoff directory.
struct Listing {
    /// Regular files with an allowlisted name, by sequence, with their size.
    entries: BTreeMap<Sequence, u64>,
    /// Allowlisted names that are not regular files (a link, a directory).
    /// They are never followed, and never skipped by the dispatcher.
    irregular: BTreeMap<Sequence, String>,
    /// Every other name: temp files, links, directories, unexpected names.
    others: Vec<String>,
}

fn list_handoff(directory: &Path) -> std::io::Result<Listing> {
    let mut listing = Listing {
        entries: BTreeMap::new(),
        irregular: BTreeMap::new(),
        others: Vec::new(),
    };
    let reader = match std::fs::read_dir(directory) {
        Ok(reader) => reader,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(listing),
        Err(error) => return Err(error),
    };
    for (index, item) in reader.enumerate() {
        if index >= MAX_LISTED_NAMES {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        // DirEntry::file_type does not follow a link.
        let file_type = item.file_type()?;
        match entry_sequence(&name) {
            Some(sequence) if file_type.is_file() => {
                let size = item.metadata()?.len();
                listing.entries.insert(sequence, size);
            }
            Some(sequence) => {
                listing.irregular.insert(sequence, name);
            }
            None => listing.others.push(name),
        }
    }
    Ok(listing)
}

/// Read and validate one committed entry. The file must be a regular file
/// with one link, owned by the directory's owner, not writable by anyone else
/// and not readable by others, at most [`MAX_ENTRY_BYTES`], with exactly the
/// schema's keys, the expected check and the sequence its name carries.
fn read_entry(
    paths: &JournalPaths,
    sequence: Sequence,
) -> Result<(JournalEntry, Vec<u8>), EntryFault> {
    let path = paths.entry(sequence);
    let directory_owner = std::fs::metadata(paths.handoff_directory())
        .map_err(|_| EntryFault::Unreadable)?
        .uid();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(NO_FOLLOW)
        .open(&path)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => EntryFault::Missing,
            _ if error.raw_os_error() == Some(libc::ELOOP) => EntryFault::Invalid,
            _ => EntryFault::Unreadable,
        })?;
    let metadata = file.metadata().map_err(|_| EntryFault::Unreadable)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != directory_owner
        || metadata.mode() & 0o027 != 0
        || metadata.len() > MAX_ENTRY_BYTES
    {
        return Err(EntryFault::Invalid);
    }
    let mut bytes = Vec::new();
    file.take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| EntryFault::Unreadable)?;
    if bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err(EntryFault::Invalid);
    }
    let value = parse_strict_json(&bytes).ok_or(EntryFault::Invalid)?;
    let entry: JournalEntry = serde_json::from_value(value).map_err(|_| EntryFault::Invalid)?;
    if entry.schema_version != ENTRY_SCHEMA_VERSION
        || entry.check != paths.check
        || entry.sequence != sequence.0
        || parse_bounded_timestamp(&entry.observed_at).is_none()
    {
        return Err(EntryFault::Invalid);
    }
    Ok((entry, bytes))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryFault {
    Missing,
    Invalid,
    Unreadable,
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn digest_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ---- POSIX ACLs ----------------------------------------------------------------

/// How a new entry gets its access rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryAccess {
    /// Owner-only (0600). For disposable fixtures where the dispatcher runs as
    /// the producer's own identity.
    OwnerOnly,
    /// Owner `rw-` plus `r--` for each named user of the handoff directory's
    /// default ACL, mask `r--`, group and other `---`. The installed layout:
    /// the directory's default ACL names the dispatcher identity. A directory
    /// without such a default ACL, or a platform without POSIX ACLs, refuses
    /// the admission rather than write an entry the dispatcher cannot read.
    DispatcherAcl,
}

impl EntryAccess {
    /// The access the installed monitors use on this platform.
    pub fn installed() -> Self {
        Self::DispatcherAcl
    }
}

/// POSIX ACL extended attributes in the Linux kernel's binary form.
#[cfg(target_os = "linux")]
mod acl {
    use std::ffi::CStr;
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    const ACCESS: &CStr = c"system.posix_acl_access";
    const DEFAULT: &CStr = c"system.posix_acl_default";
    const VERSION: u32 = 2;
    const USER_OBJ: u16 = 0x01;
    const USER: u16 = 0x02;
    const GROUP_OBJ: u16 = 0x04;
    const MASK: u16 = 0x10;
    const OTHER: u16 = 0x20;
    const UNDEFINED_ID: u32 = u32::MAX;
    const READ: u16 = 4;
    const WRITE: u16 = 2;
    const MAX_ACL_BYTES: usize = 4096;

    /// The named users of a directory's default ACL (empty when it has none).
    pub fn default_named_users(directory: &Path) -> std::io::Result<Vec<u32>> {
        let name = std::ffi::CString::new(directory.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let mut buffer = vec![0_u8; MAX_ACL_BYTES];
        // SAFETY: both strings are NUL-terminated and the buffer length is passed.
        let length = unsafe {
            libc::lgetxattr(
                name.as_ptr(),
                DEFAULT.as_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if length < 0 {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ENODATA) {
                Ok(Vec::new())
            } else {
                Err(error)
            };
        }
        buffer.truncate(length as usize);
        let mut users: Vec<u32> = decode(&buffer)?
            .into_iter()
            .filter(|(tag, _, _)| *tag == USER)
            .map(|(_, _, id)| id)
            .collect();
        users.sort_unstable();
        users.dedup();
        Ok(users)
    }

    /// Owner `rw-`, each named user `r--`, owning group `---`, mask `r--`,
    /// other `---`, in the kernel's canonical entry order.
    pub fn entry_acl(users: &[u32]) -> Vec<u8> {
        let mut entries = vec![(USER_OBJ, READ | WRITE, UNDEFINED_ID)];
        entries.extend(users.iter().map(|user| (USER, READ, *user)));
        entries.push((GROUP_OBJ, 0, UNDEFINED_ID));
        entries.push((MASK, READ, UNDEFINED_ID));
        entries.push((OTHER, 0, UNDEFINED_ID));
        let mut bytes = VERSION.to_le_bytes().to_vec();
        for (tag, permissions, id) in entries {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&permissions.to_le_bytes());
            bytes.extend_from_slice(&id.to_le_bytes());
        }
        bytes
    }

    /// Set the access ACL of `file`, then read it back and require the same
    /// entries: an ACL the file system silently changed is refused.
    pub fn set_and_verify(file: &File, acl: &[u8]) -> std::io::Result<()> {
        // SAFETY: the descriptor is open and the value length is passed.
        let set = unsafe {
            libc::fsetxattr(
                file.as_raw_fd(),
                ACCESS.as_ptr(),
                acl.as_ptr().cast(),
                acl.len(),
                0,
            )
        };
        if set != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut buffer = vec![0_u8; MAX_ACL_BYTES];
        // SAFETY: the descriptor is open and the buffer length is passed.
        let length = unsafe {
            libc::fgetxattr(
                file.as_raw_fd(),
                ACCESS.as_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if length < 0 {
            return Err(std::io::Error::last_os_error());
        }
        buffer.truncate(length as usize);
        if decode(&buffer)? != decode(acl)? {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        Ok(())
    }

    fn decode(bytes: &[u8]) -> std::io::Result<Vec<(u16, u16, u32)>> {
        let invalid = || std::io::Error::from(std::io::ErrorKind::InvalidData);
        if bytes.len() < 4 || (bytes.len() - 4) % 8 != 0 {
            return Err(invalid());
        }
        let version = u32::from_le_bytes(bytes[0..4].try_into().map_err(|_| invalid())?);
        if version != VERSION {
            return Err(invalid());
        }
        bytes[4..]
            .chunks_exact(8)
            .map(|chunk| {
                Ok((
                    u16::from_le_bytes([chunk[0], chunk[1]]),
                    u16::from_le_bytes([chunk[2], chunk[3]]),
                    u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
                ))
            })
            .collect()
    }
}

/// Give a new entry its dispatcher read access before it is published.
fn apply_entry_access(access: EntryAccess, directory: &Path, file: &File) -> std::io::Result<()> {
    match access {
        EntryAccess::OwnerOnly => Ok(()),
        #[cfg(target_os = "linux")]
        EntryAccess::DispatcherAcl => {
            let users = acl::default_named_users(directory)?;
            if users.is_empty() {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
            acl::set_and_verify(file, &acl::entry_acl(&users))?;
            // The mask is the group bits: the entry must now be exactly 0640.
            if file.metadata()?.mode() & 0o777 != 0o640 {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        EntryAccess::DispatcherAcl => {
            let _ = (directory, file);
            Err(std::io::ErrorKind::Unsupported.into())
        }
    }
}

// ---- producer ------------------------------------------------------------------

/// Bounds of one journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalLimits {
    pub max_entries: usize,
    pub max_bytes: u64,
    pub lock_timeout: Duration,
}

impl Default for JournalLimits {
    fn default() -> Self {
        Self {
            max_entries: MAX_PENDING_ENTRIES,
            max_bytes: MAX_PENDING_BYTES,
            lock_timeout: LOCK_TIMEOUT,
        }
    }
}

/// Points inside one admission. Tests stop a real producer process at each to
/// prove what a crash there leaves behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionStage {
    /// The journal lock is held and nothing is written yet.
    LockHeld,
    /// The sequence is reserved in the cursor; no entry exists.
    Reserved,
    /// The entry is complete in its temp file, before the rename.
    BeforeRename,
    /// The final name exists; the directory is not synced yet.
    AfterRename,
    /// The directory is synced; the cursor does not record the admission yet.
    AfterDirectorySync,
    /// The cursor records the admission; the lock is still held.
    Recorded,
}

/// Observes the stages of an admission.
pub trait AdmissionHook: Send + Sync {
    fn reached(&self, stage: AdmissionStage);
}

/// Why an observation was not admitted. Every refusal leaves the journal
/// unchanged and makes the monitor exit 4 without publishing a status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The journal holds its maximum number of entries or bytes.
    Full,
    /// The journal lock stayed held for the whole acquisition bound.
    LockTimeout,
    /// The file system refused a read, write, permission or ACL step.
    Unavailable,
}

impl Refusal {
    pub fn exit(self) -> Exit {
        Exit::Dependency
    }
}

/// How an observation was admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admitted {
    /// A new entry was committed with this sequence.
    Committed(Sequence),
    /// The last admitted state already equals the observation; nothing new was
    /// written. The sequence is that earlier admission.
    AlreadyAdmitted(Sequence),
}

impl Admitted {
    pub fn sequence(self) -> Sequence {
        match self {
            Self::Committed(sequence) | Self::AlreadyAdmitted(sequence) => sequence,
        }
    }
}

/// The last admitted observation, as the cursor records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedState {
    pub sequence: u64,
    pub state: AlertState,
}

/// The producer's private cursor. Fields are in key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    /// The last admitted state; `None` when it is unknown.
    pub admitted: Option<AdmittedState>,
    pub check: Check,
    pub next_sequence: u64,
    /// A sequence reserved by an admission that has not recorded its outcome.
    pub reserved: Option<u64>,
    pub schema_version: u32,
}

/// The single serialized producer of one check's journal.
#[derive(Clone)]
pub struct Producer {
    paths: JournalPaths,
    limits: JournalLimits,
    access: EntryAccess,
    hook: Option<Arc<dyn AdmissionHook>>,
}

impl Producer {
    /// A producer for a fixture layout: owner-only entries, default bounds.
    pub fn new(paths: JournalPaths) -> Self {
        Self {
            paths,
            limits: JournalLimits::default(),
            access: EntryAccess::OwnerOnly,
            hook: None,
        }
    }

    /// The installed producer of `check`: installed paths, dispatcher ACL.
    pub fn installed(check: Check) -> Self {
        Self::new(JournalPaths::installed(check)).with_access(EntryAccess::installed())
    }

    pub fn with_limits(mut self, limits: JournalLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_access(mut self, access: EntryAccess) -> Self {
        self.access = access;
        self
    }

    pub fn with_hook(mut self, hook: Arc<dyn AdmissionHook>) -> Self {
        self.hook = Some(hook);
        self
    }

    pub fn paths(&self) -> &JournalPaths {
        &self.paths
    }

    fn reached(&self, stage: AdmissionStage) {
        if let Some(hook) = &self.hook {
            hook.reached(stage);
        }
    }

    /// Admit `observation`, or refuse it without changing the journal.
    pub fn enqueue(&self, observation: &AlertObservation) -> Result<Admitted, Refusal> {
        if observation.check != self.paths.check {
            return Err(Refusal::Unavailable);
        }
        let handoff = self.paths.handoff_directory();
        create_private_directory(&self.paths.state_directory)?;
        create_private_directory(&handoff)?;
        let lock_file =
            open_producer_lock(&self.paths.journal_lock()).map_err(|_| Refusal::Unavailable)?;
        let _lock =
            lock_with_timeout(lock_file, self.limits.lock_timeout).map_err(
                |failure| match failure {
                    LockFailure::TimedOut => Refusal::LockTimeout,
                    LockFailure::Unavailable => Refusal::Unavailable,
                },
            )?;
        self.reached(AdmissionStage::LockHeld);

        let listing = self.quarantine_strays()?;
        let mut cursor = self.recover(&listing, observation.observed_at)?;
        if let Some(admitted) = cursor.admitted
            && admitted.state == observation.state
        {
            return Ok(Admitted::AlreadyAdmitted(Sequence(admitted.sequence)));
        }

        let sequence = Sequence(cursor.next_sequence);
        let entry = JournalEntry {
            check: observation.check,
            observed_at: utc_text(observation.observed_at),
            schema_version: ENTRY_SCHEMA_VERSION,
            sequence: sequence.0,
            state: observation.state,
        };
        let bytes = serde_json::to_vec(&entry).map_err(|_| Refusal::Unavailable)?;
        let pending_bytes: u64 = listing.entries.values().sum();
        if listing.entries.len() >= self.limits.max_entries
            || pending_bytes + bytes.len() as u64 > self.limits.max_bytes
        {
            return Err(Refusal::Full);
        }

        cursor.next_sequence = sequence.0.checked_add(1).ok_or(Refusal::Unavailable)?;
        cursor.reserved = Some(sequence.0);
        self.write_cursor(&cursor)?;
        self.reached(AdmissionStage::Reserved);

        self.commit_entry(sequence, &bytes)?;

        cursor.admitted = Some(AdmittedState {
            sequence: sequence.0,
            state: observation.state,
        });
        cursor.reserved = None;
        self.write_cursor(&cursor)?;
        self.reached(AdmissionStage::Recorded);
        Ok(Admitted::Committed(sequence))
    }

    /// Write the entry under a temp name, give it its access, sync it, rename
    /// it to its final name and sync the directory.
    fn commit_entry(&self, sequence: Sequence, bytes: &[u8]) -> Result<(), Refusal> {
        let handoff = self.paths.handoff_directory();
        let temporary = handoff.join(format!(
            ".entry-{}.{}.{}.tmp",
            sequence.0,
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let written = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(NO_FOLLOW)
                .open(&temporary)?;
            file.write_all(bytes)?;
            apply_entry_access(self.access, &handoff, &file)?;
            file.sync_all()
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
            return Err(Refusal::Unavailable);
        }
        self.reached(AdmissionStage::BeforeRename);
        if std::fs::rename(&temporary, self.paths.entry(sequence)).is_err() {
            let _ = std::fs::remove_file(&temporary);
            return Err(Refusal::Unavailable);
        }
        self.reached(AdmissionStage::AfterRename);
        // From here the entry is admitted even when the sync fails: the next
        // run recovers it under the lock.
        sync_directory(&handoff).map_err(|_| Refusal::Unavailable)?;
        self.reached(AdmissionStage::AfterDirectorySync);
        Ok(())
    }

    fn write_cursor(&self, cursor: &Cursor) -> Result<(), Refusal> {
        write_atomic(&self.paths.cursor(), cursor).map_err(|_| Refusal::Unavailable)
    }

    /// Move every non-entry name out of the handoff directory into the
    /// quarantine directory, then list again. A temp file without a completed
    /// rename was never admitted.
    fn quarantine_strays(&self) -> Result<Listing, Refusal> {
        let handoff = self.paths.handoff_directory();
        let listing = list_handoff(&handoff).map_err(|_| Refusal::Unavailable)?;
        if listing.others.is_empty() && listing.irregular.is_empty() {
            return Ok(listing);
        }
        let quarantine = self.paths.quarantine_directory();
        create_private_directory(&quarantine)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        for name in listing.others.iter().chain(listing.irregular.values()) {
            let destination = quarantine.join(format!("{stamp:039}-{name}"));
            std::fs::rename(handoff.join(name), destination).map_err(|_| Refusal::Unavailable)?;
        }
        sync_directory(&handoff).map_err(|_| Refusal::Unavailable)?;
        sync_directory(&quarantine).map_err(|_| Refusal::Unavailable)?;
        prune_quarantine(&quarantine);
        list_handoff(&handoff).map_err(|_| Refusal::Unavailable)
    }

    /// Load the cursor and settle it against the committed entries.
    ///
    /// A reservation whose final-name entry exists is an admission: the
    /// directory is synced again and the cursor records it. A reservation
    /// without an entry leaves the admitted state unknown, so the next
    /// observation is admitted. A missing or malformed cursor never lowers the
    /// next sequence below a committed entry, and starts at the current time in
    /// milliseconds, so a number the dispatcher already acknowledged is not
    /// reused.
    fn recover(&self, listing: &Listing, now: DateTime<Utc>) -> Result<Cursor, Refusal> {
        let newest = listing.entries.keys().next_back().copied();
        let newest_state = match newest {
            Some(sequence) => match read_entry(&self.paths, sequence) {
                Ok((entry, _)) => Some(AdmittedState {
                    sequence: sequence.0,
                    state: entry.state,
                }),
                Err(_) => None,
            },
            None => None,
        };
        let floor = newest.map_or(1, |sequence| sequence.0.saturating_add(1));
        let stored = self.load_cursor()?;
        let mut cursor = match stored.clone() {
            Some(cursor) => cursor,
            None => Cursor {
                admitted: newest_state,
                check: self.paths.check,
                next_sequence: floor.max(milliseconds_since_epoch(now)),
                reserved: None,
                schema_version: CURSOR_SCHEMA_VERSION,
            },
        };
        if let Some(reserved) = cursor.reserved.take() {
            let committed = listing.entries.contains_key(&Sequence(reserved));
            cursor.admitted = if committed {
                sync_directory(&self.paths.handoff_directory())
                    .map_err(|_| Refusal::Unavailable)?;
                newest_state.filter(|state| state.sequence == reserved)
            } else {
                None
            };
        }
        // An entry newer than the recorded admission decides the state.
        if let Some(sequence) = newest
            && cursor
                .admitted
                .is_none_or(|admitted| admitted.sequence < sequence.0)
        {
            cursor.admitted = newest_state;
        }
        cursor.next_sequence = cursor.next_sequence.max(floor);
        if stored.as_ref() != Some(&cursor) {
            self.write_cursor(&cursor)?;
        }
        Ok(cursor)
    }

    fn load_cursor(&self) -> Result<Option<Cursor>, Refusal> {
        let path = self.paths.cursor();
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Refusal::Unavailable),
            Ok(_) => {}
        }
        match read_bounded::<Cursor>(&path, MAX_CURSOR_BYTES) {
            Ok(cursor)
                if cursor.schema_version == CURSOR_SCHEMA_VERSION
                    && cursor.check == self.paths.check =>
            {
                Ok(Some(cursor))
            }
            Ok(_) | Err(Exit::Precondition) => {
                let quarantine = self.paths.quarantine_directory();
                create_private_directory(&quarantine)?;
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_nanos());
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                std::fs::rename(&path, quarantine.join(format!("{stamp:039}-{name}")))
                    .map_err(|_| Refusal::Unavailable)?;
                sync_directory(&self.paths.state_directory).map_err(|_| Refusal::Unavailable)?;
                Ok(None)
            }
            Err(_) => Err(Refusal::Unavailable),
        }
    }
}

fn milliseconds_since_epoch(at: DateTime<Utc>) -> u64 {
    u64::try_from(at.timestamp_millis()).unwrap_or(1).max(1)
}

fn create_private_directory(path: &Path) -> Result<(), Refusal> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(Refusal::Unavailable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|_| Refusal::Unavailable),
        Err(_) => Err(Refusal::Unavailable),
    }
}

/// Keep the newest [`QUARANTINE_KEEP`] quarantined files. Their names start
/// with a fixed-width time stamp, so name order is age order.
fn prune_quarantine(directory: &Path) {
    let Ok(reader) = std::fs::read_dir(directory) else {
        return;
    };
    let mut names: Vec<_> = reader
        .filter_map(Result::ok)
        .take(MAX_LISTED_NAMES)
        .map(|item| item.file_name())
        .collect();
    names.sort();
    let excess = names.len().saturating_sub(QUARANTINE_KEEP);
    for name in names.into_iter().take(excess) {
        let path = directory.join(name);
        let removed = match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(error) => Err(error),
        };
        let _ = removed;
    }
}

/// Admit `observation` with `producer`: the plan's `enqueue` interface.
pub fn enqueue(producer: &Producer, observation: AlertObservation) -> Result<Sequence, Refusal> {
    producer.enqueue(&observation).map(Admitted::sequence)
}

/// The monitors' admission seam: a status is published only after its
/// observation is admitted into the check's journal.
#[derive(Clone)]
pub struct JournalAdmission {
    producer: Producer,
}

impl JournalAdmission {
    pub fn new(producer: Producer) -> Self {
        Self { producer }
    }

    /// The installed journal of `check`.
    pub fn installed(check: Check) -> Self {
        Self::new(Producer::installed(check))
    }
}

impl Admission for JournalAdmission {
    fn admit(&self, status: &Status) -> Result<(), Exit> {
        let observed_at = parse_bounded_timestamp(&status.generated_at).ok_or(Exit::Dependency)?;
        let observation = AlertObservation {
            check: self.producer.paths.check,
            state: AlertState::from_category(status.category),
            observed_at,
        };
        self.producer
            .enqueue(&observation)
            .map(|_| ())
            .map_err(Refusal::exit)
    }
}

// ---- dispatcher side -------------------------------------------------------------

/// The oldest committed entry of a journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// No committed entry.
    Empty,
    /// The oldest entry, valid, with the SHA-256 of its bytes.
    Entry {
        sequence: Sequence,
        entry: JournalEntry,
        digest: String,
    },
    /// The oldest allowlisted name does not hold a valid entry. It is never
    /// skipped: delivery stops until an operator repairs the journal.
    Invalid(Sequence),
}

/// Read access to a journal for the dispatcher. It opens the journal lock and
/// the entries read-only and writes nothing but the removal of an entry whose
/// delivery it has already acknowledged.
#[derive(Debug, Clone)]
pub struct JournalReader {
    paths: JournalPaths,
    lock_timeout: Duration,
}

impl JournalReader {
    pub fn new(paths: JournalPaths, lock_timeout: Duration) -> Self {
        Self {
            paths,
            lock_timeout,
        }
    }

    pub fn paths(&self) -> &JournalPaths {
        &self.paths
    }

    /// Hold the journal lock briefly. `None` when no producer has created the
    /// lock yet, which also means no entry exists.
    fn lock(&self) -> Result<Option<HeldLock>, Exit> {
        let Some(file) =
            open_reader_lock(&self.paths.journal_lock()).map_err(|_| Exit::Dependency)?
        else {
            return Ok(None);
        };
        lock_with_timeout(file, self.lock_timeout)
            .map(Some)
            .map_err(|_| Exit::Dependency)
    }

    /// The oldest committed entry, by numeric sequence.
    pub fn head(&self) -> Result<Head, Exit> {
        let Some(_lock) = self.lock()? else {
            return Ok(Head::Empty);
        };
        let listing =
            list_handoff(&self.paths.handoff_directory()).map_err(|_| Exit::Dependency)?;
        let oldest_regular = listing.entries.keys().next().copied();
        let oldest_irregular = listing.irregular.keys().next().copied();
        if let Some(irregular) = oldest_irregular
            && oldest_regular.is_none_or(|regular| irregular < regular)
        {
            return Ok(Head::Invalid(irregular));
        }
        let Some(sequence) = oldest_regular else {
            return Ok(Head::Empty);
        };
        match read_entry(&self.paths, sequence) {
            Ok((entry, bytes)) => Ok(Head::Entry {
                sequence,
                entry,
                digest: digest_hex(&bytes),
            }),
            Err(EntryFault::Invalid) => Ok(Head::Invalid(sequence)),
            Err(EntryFault::Missing | EntryFault::Unreadable) => Err(Exit::Dependency),
        }
    }

    /// Every committed sequence, oldest first.
    pub fn pending(&self) -> Result<Vec<Sequence>, Exit> {
        let Some(_lock) = self.lock()? else {
            return Ok(Vec::new());
        };
        let listing =
            list_handoff(&self.paths.handoff_directory()).map_err(|_| Exit::Dependency)?;
        let mut pending: Vec<Sequence> = listing
            .entries
            .keys()
            .chain(listing.irregular.keys())
            .copied()
            .collect();
        pending.sort_unstable();
        Ok(pending)
    }

    /// Remove an acknowledged entry under the journal lock and sync the
    /// directory. The entry must still hold the acknowledged bytes; an entry
    /// that is already gone is not an error.
    pub fn remove(&self, sequence: Sequence, digest: &str) -> Result<(), Exit> {
        let Some(_lock) = self.lock()? else {
            return Ok(());
        };
        match read_entry(&self.paths, sequence) {
            Ok((_, bytes)) if digest_hex(&bytes) == digest => {}
            Ok(_) | Err(EntryFault::Invalid) => return Err(Exit::Precondition),
            Err(EntryFault::Missing) => return Ok(()),
            Err(EntryFault::Unreadable) => return Err(Exit::Dependency),
        }
        std::fs::remove_file(self.paths.entry(sequence)).map_err(|_| Exit::Dependency)?;
        sync_directory(&self.paths.handoff_directory()).map_err(|_| Exit::Dependency)
    }
}
