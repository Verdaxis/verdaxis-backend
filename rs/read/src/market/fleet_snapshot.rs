//! One bounded, validated fleet observation; no publisher or fallback data.

use crate::{ApiError, routes::market_data_error::unavailable};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::{sync::Semaphore, time::Instant};

pub const MAX_BYTES: usize = 262_144;
const READ_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FleetSnapshot {
    pub entries: Vec<FleetEntry>,
    pub last_updated: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FleetEntry {
    pub fuel: String,
    pub ordered_vessels: i32,
    pub delivered_vessels: i32,
    pub avg_consumption_mt: i32,
    pub color: String,
}

pub fn decode_snapshot(bytes: &[u8]) -> Result<FleetSnapshot, ApiError> {
    if bytes.len() > MAX_BYTES {
        return Err(unavailable());
    }
    let document: FleetSnapshot = serde_json::from_slice(bytes).map_err(|_| unavailable())?;
    let date = &document.last_updated;
    if date.len() != 10
        || date.as_bytes().iter().enumerate().any(|(index, byte)| {
            if index == 4 || index == 7 {
                *byte != b'-'
            } else {
                !byte.is_ascii_digit()
            }
        })
        || chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err()
        || document.entries.len() > 32
        || document.sources.len() > 16
        || document.sources.iter().any(|source| source.len() > 2048)
        || document.entries.iter().any(|entry| {
            entry.fuel.len() > 64
                || entry.color.len() > 32
                || entry.ordered_vessels < 0
                || entry.delivered_vessels < 0
                || entry.avg_consumption_mt < 0
        })
    {
        return Err(unavailable());
    }
    Ok(document)
}

/// Admission belongs to the filesystem worker, not the HTTP caller. A cancelled
/// or expired caller cannot admit a replacement while its worker still runs.
pub async fn read_snapshot(
    path: &Path,
    permits: Arc<Semaphore>,
) -> Result<FleetSnapshot, ApiError> {
    let deadline = Instant::now() + READ_DEADLINE;
    let permit = permits.try_acquire_owned().map_err(|_| unavailable())?;
    let path = path.to_owned();
    run_worker(deadline, permit, move || read_document(&path, deadline)).await
}

async fn run_worker<T: Send + 'static>(
    deadline: Instant,
    permit: tokio::sync::OwnedSemaphorePermit,
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    });
    tokio::time::timeout_at(deadline, worker)
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())?
}

#[cfg(unix)]
fn read_document(path: &Path, deadline: Instant) -> Result<FleetSnapshot, ApiError> {
    use std::io::Read;
    if Instant::now() >= deadline {
        return Err(unavailable());
    }
    let file = open_regular_file(path)?;
    // take() bounds allocation even if a publisher replaces or grows a document.
    let mut bytes = Vec::with_capacity(MAX_BYTES + 1);
    let mut bounded = file.take((MAX_BYTES + 1) as u64);
    let mut chunk = [0_u8; 8192];
    loop {
        if Instant::now() >= deadline {
            return Err(unavailable());
        }
        let count = bounded.read(&mut chunk).map_err(|_| unavailable())?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let document = decode_snapshot(&bytes)?;
    if Instant::now() >= deadline {
        return Err(unavailable());
    }
    Ok(document)
}

/// Walk from a held root descriptor, never following links. Each opened child
/// remains relative to the held directory even if that directory is renamed.
#[cfg(unix)]
fn open_regular_file(path: &Path) -> Result<std::fs::File, ApiError> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd, OwnedFd},
            unix::ffi::OsStrExt,
        },
        path::Component,
    };
    if !path.is_absolute() {
        return Err(unavailable());
    }
    let mut parts = path.components().peekable();
    if parts.next() != Some(Component::RootDir) {
        return Err(unavailable());
    }
    let mut directory = std::fs::File::open("/").map_err(|_| unavailable())?;
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(unavailable());
        };
        let name = CString::new(name.as_bytes()).map_err(|_| unavailable())?;
        let last = parts.peek().is_none();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if last {
                libc::O_NONBLOCK
            } else {
                libc::O_DIRECTORY
            };
        // SAFETY: directory is a live descriptor; name is NUL-terminated. No
        // creation flag is used. A successful descriptor is owned exactly once.
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(unavailable());
        }
        // SAFETY: openat returned a new, valid descriptor owned by this call.
        let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
        let file = std::fs::File::from(descriptor);
        if last {
            let metadata = file.metadata().map_err(|_| unavailable())?;
            if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
                return Err(unavailable());
            }
            return Ok(file);
        }
        directory = file;
    }
    Err(unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fleet_document_limits_deadline_and_cleanup() {
        let permits = Arc::new(Semaphore::new(1));
        let permit = permits.clone().acquire_owned().await.unwrap();
        let (release, wait) = std::sync::mpsc::channel();
        let (finished, cleanup) = tokio::sync::oneshot::channel();
        let deadline = Instant::now() + READ_DEADLINE;
        let result = run_worker(deadline, permit, move || {
            let _ = wait.recv_timeout(Duration::from_secs(10));
            let _ = finished.send(());
            Ok(())
        })
        .await;
        assert!(result.is_err());
        assert_eq!(permits.available_permits(), 0);
        release.send(()).unwrap();
        cleanup.await.unwrap();
        // Join completion and the permit drop may follow the cleanup signal.
        let returned = tokio::time::timeout(Duration::from_secs(2), permits.acquire()).await;
        assert!(returned.is_ok());
    }

    #[tokio::test]
    async fn fleet_document_caller_cleanup() {
        let permits = Arc::new(Semaphore::new(1));
        let permit = permits.clone().acquire_owned().await.unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let request = tokio::spawn(run_worker(
            Instant::now() + READ_DEADLINE,
            permit,
            move || {
                let _ = started.send(());
                let _ = wait.recv_timeout(Duration::from_secs(10));
                Ok(())
            },
        ));
        ready.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(permits.available_permits(), 0);
        release.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), permits.acquire())
                .await
                .is_ok()
        );
    }
}

#[cfg(not(unix))]
fn read_document(_path: &Path, _deadline: Instant) -> Result<FleetSnapshot, ApiError> {
    // No descriptor-relative implementation is qualified on other platforms.
    Err(unavailable())
}
