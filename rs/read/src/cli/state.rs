//! Bounded private job-state files: reads, durable atomic writes and locks.
//!
//! State lives in a private directory (0600 files, 0700 parent). A reader
//! refuses anything but one bounded regular file; a writer publishes through a
//! same-directory temp file and never leaves a partial target behind.
use super::Exit;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Bound for a state file that carries small JSON records.
pub const MAX_STATE_BYTES: u64 = 64 * 1024;
const MAX_LOCK_WAIT: Duration = Duration::from_secs(5);
const LOCK_POLL: Duration = Duration::from_millis(10);

/// Read one bounded, regular, non-symlink JSON state file.
///
/// The path is opened with `O_NOFOLLOW` and its open descriptor must be the
/// file the path named, owned by the directory's owner, with one link. The size
/// check runs on the opened file, and the bound is applied while reading, so a
/// file that grows after the metadata check is still refused.
pub fn read_bounded<T: DeserializeOwned>(path: &Path, max_bytes: u64) -> Result<T, Exit> {
    let file = open_regular(path)?;
    let size = file.metadata().map_err(|_| Exit::Dependency)?.len();
    if size > max_bytes {
        return Err(Exit::Precondition);
    }
    let capacity = usize::try_from(size.min(max_bytes)).map_err(|_| Exit::Precondition)?;
    let mut document = Vec::with_capacity(capacity);
    file.take(max_bytes + 1)
        .read_to_end(&mut document)
        .map_err(|_| Exit::Dependency)?;
    if document.len() as u64 > max_bytes {
        return Err(Exit::Precondition);
    }
    serde_json::from_slice(&document).map_err(|_| Exit::Precondition)
}

/// Publish `value` as one private regular file.
///
/// `path` must be an absolute path in a private directory: a relative path or
/// one without a parent is refused. The sequence is: create a fresh
/// same-directory temp file with mode 0600, write and `fsync` it, rename it over
/// the target, then `fsync` the directory. A directory `fsync` failure returns
/// `Dependency` after publication, so an error does not always mean the old
/// value is still in place. A caller that needs to set ACLs before publication
/// uses [`PendingWrite`].
pub fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), Exit> {
    let pending = PendingWrite::create(path, value)?;
    pending.file().sync_all().map_err(|_| Exit::Dependency)?;
    pending.commit()
}

/// A prepared replacement that has not been published yet.
///
/// Dropping it without [`commit`](Self::commit) removes the temp file and
/// leaves the old target intact.
pub struct PendingWrite {
    target: PathBuf,
    temporary: PathBuf,
    file: Option<File>,
    committed: bool,
}

impl PendingWrite {
    /// Create and fill one fresh temp file beside `path`.
    ///
    /// `create_new` refuses an existing name, and the UUID keeps two writers
    /// from choosing the same name even after a crash left one behind.
    pub fn create<T: Serialize>(path: &Path, value: &T) -> Result<Self, Exit> {
        let parent = path.parent().ok_or(Exit::Precondition)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Exit::Precondition)?;
        let temporary = parent.join(format!(
            ".{name}.tmp.{}.{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        // Serialize before creating the file: a refused value must leave no
        // temp file behind.
        let document = serde_json::to_vec(value).map_err(|_| Exit::Precondition)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_| Exit::Dependency)?;
        if file.write_all(&document).is_err() {
            let _ = std::fs::remove_file(&temporary);
            return Err(Exit::Dependency);
        }
        Ok(Self {
            target: path.to_path_buf(),
            temporary,
            file: Some(file),
            committed: false,
        })
    }

    /// The private temp file, for callers that must set ACLs before publication.
    pub fn file(&self) -> &File {
        self.file.as_ref().expect("file is open until commit")
    }

    /// The exact temp path, never reused by a later writer.
    pub fn temporary_path(&self) -> &Path {
        &self.temporary
    }

    /// Rename the temp file over the target and make the rename durable.
    pub fn commit(mut self) -> Result<(), Exit> {
        let parent = self
            .target
            .parent()
            .ok_or(Exit::Precondition)?
            .to_path_buf();
        std::fs::rename(&self.temporary, &self.target).map_err(|_| Exit::Dependency)?;
        // From here the target is replaced; Drop must not unlink the new file.
        self.committed = true;
        self.file.take();
        File::open(&parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| Exit::Dependency)
    }
}

impl Drop for PendingWrite {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.file.take();
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}

/// Open one state file that must already be a private regular file.
///
/// `O_NOFOLLOW` refuses a symlink at the path itself; the `fstat` comparison
/// then refuses an entry replaced between the check and the open.
pub fn open_regular(path: &Path) -> Result<File, Exit> {
    let before = std::fs::symlink_metadata(path).map_err(|_| Exit::Precondition)?;
    if !before.file_type().is_file() {
        return Err(Exit::Precondition);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(NO_FOLLOW_FLAGS)
        .open(path)
        .map_err(|_| Exit::Precondition)?;
    verify_opened(&file, &before, path)?;
    Ok(file)
}

/// Flags every private state or lock path is opened with.
///
/// `O_NOFOLLOW` turns a symlinked entry into an error instead of following it;
/// `O_CLOEXEC` keeps the descriptor out of an exec'd child.
const NO_FOLLOW_FLAGS: i32 = libc::O_NOFOLLOW | libc::O_CLOEXEC;

/// Check that the open descriptor is the same private regular file the path
/// named, is owned by the directory's owner, and has no other link.
fn verify_opened(file: &File, before: &std::fs::Metadata, path: &Path) -> Result<(), Exit> {
    let opened = file.metadata().map_err(|_| Exit::Dependency)?;
    if !opened.file_type().is_file()
        || opened.dev() != before.dev()
        || opened.ino() != before.ino()
        || opened.nlink() != 1
    {
        return Err(Exit::Precondition);
    }
    if opened.uid() != parent_owner(path)? {
        return Err(Exit::Precondition);
    }
    Ok(())
}

/// The owner of the directory that contains `path`.
///
/// A private state directory belongs to the running identity, so its owner is
/// the only identity that may have created entries inside it. Comparing with
/// the parent avoids a process-wide identity lookup and keeps the check tied to
/// this path.
fn parent_owner(path: &Path) -> Result<u32, Exit> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let metadata = std::fs::metadata(parent).map_err(|_| Exit::Dependency)?;
    Ok(metadata.uid())
}

/// One nonblocking exclusive lock on a stable lock file.
///
/// The lock is held by the open descriptor and released on drop (or process
/// exit). It is an ordinary `flock`, so it interoperates with shell callers.
pub struct StateLock(File);

impl StateLock {
    /// Take the lock now or report that another holder has it.
    pub fn try_acquire(path: &Path) -> Result<Option<Self>, Exit> {
        let file = open_lock_file(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self(file))),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(_)) => Err(Exit::Dependency),
        }
    }

    /// Wait for the lock until `deadline`, polling a fixed interval.
    pub async fn acquire_until(path: &Path, deadline: Instant) -> Result<Self, Exit> {
        loop {
            if let Some(lock) = Self::try_acquire(path)? {
                return Ok(lock);
            }
            if Instant::now() >= deadline {
                return Err(Exit::Dependency);
            }
            tokio::time::sleep(LOCK_POLL).await;
        }
    }

    /// Wait for the lock for at most the job's bounded lock interval.
    pub async fn acquire(path: &Path) -> Result<Self, Exit> {
        Self::acquire_until(path, Instant::now() + MAX_LOCK_WAIT).await
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Open the stable lock file, creating it privately on first use.
///
/// The file is never replaced: an existing entry keeps its mode and identity.
/// A symlink, a non-regular entry, a foreign-owned file or a file that has
/// another link is refused before the lock is taken.
fn open_lock_file(path: &Path) -> Result<File, Exit> {
    let created = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(NO_FOLLOW_FLAGS)
        .open(path);
    let file = match created {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let before = std::fs::symlink_metadata(path).map_err(|_| Exit::Precondition)?;
            if !before.file_type().is_file() {
                return Err(Exit::Precondition);
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(NO_FOLLOW_FLAGS)
                .open(path)
                .map_err(|_| Exit::Precondition)?;
            verify_opened(&file, &before, path)?;
            return Ok(file);
        }
        Err(_) => return Err(Exit::Dependency),
    };
    let opened = file.metadata().map_err(|_| Exit::Dependency)?;
    if !opened.file_type().is_file() || opened.nlink() != 1 || opened.uid() != parent_owner(path)? {
        return Err(Exit::Precondition);
    }
    Ok(file)
}
