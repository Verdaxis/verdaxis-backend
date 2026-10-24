//! bcrypt password primitives.
//!
//! The storage format is standard `$2b$` bcrypt at cost 12, so Python's login
//! keeps verifying hashes Rust writes and Rust keeps verifying existing hashes
//! (coexistence C2). Passwords longer than bcrypt's 72-byte limit are hashed
//! through their 32-byte SHA-256 digest instead of being silently truncated.
//! Hashing and verification run on a four-worker admission pool: the bound is
//! a CPU admission rule, not a benchmark.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::token_bytes::generate_token;

/// bcrypt cost, matching Python's `bcrypt.gensalt()` default.
pub const BCRYPT_COST: u32 = 12;
/// The shortest password a route accepts.
pub const MIN_PASSWORD_BYTES: usize = 8;
/// The longest password a route accepts, in UTF-8 bytes.
pub const MAX_PASSWORD_BYTES: usize = 1024;
/// bcrypt's own input limit; longer passwords are pre-hashed.
const BCRYPT_MAX_BYTES: usize = 72;
/// The same four-worker bound Python enforces with its capacity limiter.
const PASSWORD_WORKERS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordError {
    TooShort,
    TooLong,
    /// bcrypt could not process the input or the stored hash.
    InvalidHash,
}

/// Hash a password at cost 12 on a bounded blocking worker.
pub async fn hash(password: &str) -> Result<String, PasswordError> {
    let input = bcrypt_input(password)?;
    run_worker(move || bcrypt::hash(input, BCRYPT_COST))
        .await
        .map_err(|_| PasswordError::InvalidHash)
}

/// Verify a password against a stored hash on a bounded blocking worker.
///
/// A malformed stored hash or an over-long password is a failed comparison,
/// never a panic and never a silent truncation.
pub async fn verify(password: &str, hash: &str) -> bool {
    let Ok(input) = bcrypt_input(password) else {
        return false;
    };
    let hash = hash.to_owned();
    run_worker(move || bcrypt::verify(input, &hash).unwrap_or(false)).await
}

/// Validate a new password: 8 to 1024 UTF-8 bytes.
pub fn validate_bytes(password: &str) -> Result<(), PasswordError> {
    let length = password.len();
    if length < MIN_PASSWORD_BYTES {
        return Err(PasswordError::TooShort);
    }
    if length > MAX_PASSWORD_BYTES {
        return Err(PasswordError::TooLong);
    }
    Ok(())
}

/// A process-wide cost-12 bcrypt hash used by the login path.
///
/// Built lazily on first use, it matches no known password and is never logged.
pub async fn dummy_hash() -> &'static str {
    static DUMMY: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
    DUMMY
        .get_or_init(|| async {
            hash(&generate_token())
                .await
                .expect("a 43-character token hashes at cost 12")
        })
        .await
}

/// The bcrypt input for a password: the UTF-8 bytes up to 72, otherwise the
/// raw 32-byte SHA-256 digest.
fn bcrypt_input(password: &str) -> Result<Vec<u8>, PasswordError> {
    let bytes = password.as_bytes();
    if bytes.len() > MAX_PASSWORD_BYTES {
        return Err(PasswordError::TooLong);
    }
    if bytes.len() > BCRYPT_MAX_BYTES {
        Ok(Sha256::digest(bytes).to_vec())
    } else {
        Ok(bytes.to_vec())
    }
}

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static HIGH_WATER: AtomicUsize = AtomicUsize::new(0);

fn workers() -> &'static Arc<Semaphore> {
    static WORKERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    WORKERS.get_or_init(|| Arc::new(Semaphore::new(PASSWORD_WORKERS)))
}

/// Holds one admission permit and one in-flight count for the whole blocking
/// call, including when the async caller is cancelled or the work panics.
struct WorkerSlot {
    _permit: OwnedSemaphorePermit,
}

impl Drop for WorkerSlot {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn run_worker<T, F>(work: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let permit = workers()
        .clone()
        .acquire_owned()
        .await
        .expect("the password worker semaphore is never closed");
    let slot = WorkerSlot { _permit: permit };
    let current = IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
    HIGH_WATER.fetch_max(current, Ordering::SeqCst);
    tokio::task::spawn_blocking(move || {
        let _slot = slot;
        work()
    })
    .await
    .expect("password workers do not panic")
}

/// Test instrumentation: the largest number of password operations admitted at
/// once since the last reset. This proves the admission bound; it is not a
/// performance measurement.
#[doc(hidden)]
pub fn worker_high_water() -> usize {
    HIGH_WATER.load(Ordering::SeqCst)
}

/// Test instrumentation: reset the admission high-water mark.
#[doc(hidden)]
pub fn reset_worker_high_water() {
    HIGH_WATER.store(0, Ordering::SeqCst);
}
