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
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Bound for a state file that carries small JSON records.
pub const MAX_STATE_BYTES: u64 = 64 * 1024;
const MAX_LOCK_WAIT: Duration = Duration::from_secs(5);
const LOCK_POLL: Duration = Duration::from_millis(10);

/// Read one bounded, regular, non-symlink JSON state file.
///
/// The size check runs on the opened file, and the bound is applied while
/// reading, so a file that grows after the metadata check is still refused.
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
/// The sequence is: create a fresh same-directory temp file with mode 0600,
/// write and `fsync` it, rename it over the target, then `fsync` the directory.
/// A caller that needs to set ACLs before publication uses [`PendingWrite`].
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

pub fn open_regular(path: &Path) -> Result<File, Exit> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| Exit::Precondition)?;
    if !metadata.file_type().is_file() {
        return Err(Exit::Precondition);
    }
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| Exit::Dependency)?;
    let opened = file.metadata().map_err(|_| Exit::Dependency)?;
    if !opened.file_type().is_file()
        || opened.dev() != metadata.dev()
        || opened.ino() != metadata.ino()
    {
        return Err(Exit::Precondition);
    }
    Ok(file)
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

fn open_lock_file(path: &Path) -> Result<File, Exit> {
    let create = || {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(|_| Exit::Dependency)
    };
    let file = create()?;
    // An existing file keeps its mode; converge it so a shared lock path never
    // stays wider than private even if it was created by an older release.
    if file.metadata().map_err(|_| Exit::Dependency)?.mode() & 0o777 != 0o600 {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| Exit::Dependency)?;
    }
    Ok(file)
}
