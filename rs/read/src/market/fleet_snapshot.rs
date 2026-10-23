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
    let text = std::str::from_utf8(bytes).map_err(|_| unavailable())?;
    let document: FleetSnapshot = serde_json::from_str(text).map_err(|_| unavailable())?;
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
    read_document_with(path, deadline, |_| {})
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum ReadStage {
    Opened,
    Chunk,
}

#[cfg(unix)]
fn read_document_with(
    path: &Path,
    deadline: Instant,
    mut observe: impl FnMut(ReadStage),
) -> Result<FleetSnapshot, ApiError> {
    use std::io::Read;
    if Instant::now() >= deadline {
        return Err(unavailable());
    }
    let mut file = open_regular_file(path)?;
    observe(ReadStage::Opened);
    let before = file.metadata().map_err(|_| unavailable())?;
    // take() bounds allocation even if a publisher replaces or grows a document.
    let mut bytes = Vec::with_capacity(MAX_BYTES + 1);
    let mut bounded = (&mut file).take((MAX_BYTES + 1) as u64);
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
        observe(ReadStage::Chunk);
    }
    let after = file.metadata().map_err(|_| unavailable())?;
    if bytes.len() > MAX_BYTES || !same_observation(&before, &after) {
        return Err(unavailable());
    }
    let document = decode_snapshot(&bytes)?;
    if Instant::now() >= deadline {
        return Err(unavailable());
    }
    Ok(document)
}

#[cfg(unix)]
fn same_observation(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.size() == after.size()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

/// Walk from a held root descriptor, never following links. Each opened child
/// remains relative to the held directory even if that directory is renamed.
#[cfg(unix)]
fn open_regular_file(path: &Path) -> Result<std::fs::File, ApiError> {
    let file = crate::runtime::safe_fs::open_regular_file(path).map_err(|_| unavailable())?;
    let metadata = file.metadata().map_err(|_| unavailable())?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
        return Err(unavailable());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn fleet_document_change_cases() {
        use std::io::{Seek, SeekFrom, Write};
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("fleet-change-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("observation.json");
        let original = br#"{"entries":[],"last_updated":"2024-01-01","sources":[]}"#;
        let replacement = br#"{"entries":[],"last_updated":"2025-01-01","sources":[]}"#;
        std::fs::write(&path, original).unwrap();
        let next = directory.join("next.json");
        std::fs::write(&next, replacement).unwrap();
        let old = read_document_with(&path, Instant::now() + READ_DEADLINE, |stage| {
            if matches!(stage, ReadStage::Opened) {
                std::fs::rename(&next, &path).unwrap();
            }
        })
        .unwrap();
        assert_eq!(old.last_updated, "2024-01-01");
        assert_eq!(std::fs::read(&path).unwrap(), replacement);

        for change in ["growth", "truncation", "rewrite"] {
            let mut padded = original.to_vec();
            padded.resize(16_384, b' ');
            std::fs::write(&path, &padded).unwrap();
            let mut changed = false;
            let result = read_document_with(&path, Instant::now() + READ_DEADLINE, |stage| {
                if changed || !matches!(stage, ReadStage::Chunk) {
                    return;
                }
                changed = true;
                let mut writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
                match change {
                    "growth" => {
                        writer.seek(SeekFrom::End(0)).unwrap();
                        writer.write_all(b" ").unwrap();
                    }
                    "truncation" => writer.set_len(8192).unwrap(),
                    "rewrite" => {
                        writer.seek(SeekFrom::Start(0)).unwrap();
                        writer.write_all(replacement).unwrap();
                    }
                    _ => unreachable!(),
                }
                writer.sync_all().unwrap();
            });
            assert!(changed);
            let error = result.expect_err(change);
            assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(error.detail["code"], "READ_UNAVAILABLE");
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

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
