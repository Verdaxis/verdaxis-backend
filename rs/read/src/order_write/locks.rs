//! Canonical lock identity and advisory keys for the order-book write boundary.
//!
//! Port of the reviewed key derivation in `app/services/market_locks.py` and
//! `app/services/idempotency.py`. Slice identity is side-independent: a BID and
//! an ASK on the same product, delivery point and canonical availability window
//! share one executable slice and therefore one advisory lock. Every lock key is
//! the first eight bytes of a SHA-256 digest, read as a signed big-endian
//! integer, with zero replaced by one.

use chrono::NaiveDate;
use sha2::Digest;
use uuid::Uuid;

use crate::market::windows::normalize_window;
use crate::{ApiError, ApiErrorKind};

/// Transaction-local bounds the boundary sets before its first lock. They match
/// the reviewed Python constants; runtime-wide timeout policy stays a
/// deployment concern.
pub const MARKET_LOCK_TIMEOUT: &str = "500ms";
pub const MARKET_STATEMENT_TIMEOUT: &str = "5s";
pub const IDEMPOTENCY_LOCK_TIMEOUT: &str = "500ms";
pub const IDEMPOTENCY_STATEMENT_TIMEOUT: &str = "5s";

pub const IDEMPOTENCY_KEY_MIN: usize = 1;
pub const IDEMPOTENCY_KEY_MAX: usize = 255;
pub const IDEMPOTENCY_KEY_BOUNDS: &str = "Idempotency-Key must be 1-255 characters";

/// One market slice's side-independent identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceKey {
    pub product_id: Uuid,
    pub delivery_point_id: Option<Uuid>,
    pub availability_window: String,
}

/// SHA-256 in lowercase hexadecimal. The single digest primitive this module
/// and the request hashes share.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Read the first eight digest bytes as a signed big-endian integer, replacing
/// the single unused value zero with one.
fn lock_key(canonical: &str) -> i64 {
    let digest = sha2::Sha256::digest(canonical.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    match i64::from_be_bytes(bytes) {
        0 => 1,
        value => value,
    }
}

fn point_text(delivery_point_id: Option<Uuid>) -> String {
    delivery_point_id
        .map(|id| id.to_string())
        .unwrap_or_default()
}

/// Normalize one availability window to the canonical slice label.
///
/// The window grammar is shared with the market reads, so an invalid window is
/// refused with that grammar's own message rather than a second spelling.
pub fn normalize_slice_window(availability_window: &str) -> Result<String, ApiError> {
    normalize_window(availability_window, NaiveDate::MIN).map_err(|detail| ApiError {
        status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        detail: serde_json::json!(detail),
        retry_after: None,
        kind: ApiErrorKind::Expected,
    })
}

/// The canonical order used before any slice advisory lock is taken:
/// product id, then delivery point, then normalized window, each as text.
pub fn market_slice_lock_sort_key(
    product_id: Uuid,
    delivery_point_id: Option<Uuid>,
    normalized_window: &str,
) -> (String, String, String) {
    (
        product_id.to_string(),
        point_text(delivery_point_id),
        normalized_window.to_owned(),
    )
}

/// One slice advisory key. `normalized_window` must already be canonical: pass
/// [`normalize_slice_window`] output, or use [`ordered_slice_lock_keys`].
pub fn market_slice_lock_key(
    product_id: Uuid,
    delivery_point_id: Option<Uuid>,
    normalized_window: &str,
) -> i64 {
    lock_key(&format!(
        "{}|{}|{}",
        product_id,
        point_text(delivery_point_id),
        normalized_window
    ))
}

/// The deduplicated, canonically sorted advisory keys for one lock plan.
///
/// The reviewed Python deduplicates on the side-independent identity and sorts
/// by [`market_slice_lock_sort_key`] before acquiring anything, so two requests
/// that touch the same slices always take them in the same order.
pub fn ordered_slice_lock_keys(slices: &[SliceKey]) -> Result<Vec<i64>, ApiError> {
    let mut unique: Vec<(String, String, String, Uuid, Option<Uuid>)> = Vec::new();
    for slice in slices {
        let normalized = normalize_slice_window(&slice.availability_window)?;
        let (product, point, window) =
            market_slice_lock_sort_key(slice.product_id, slice.delivery_point_id, &normalized);
        let duplicate = unique
            .iter()
            .any(|existing| existing.0 == product && existing.1 == point && existing.2 == window);
        if !duplicate {
            unique.push((
                product,
                point,
                window,
                slice.product_id,
                slice.delivery_point_id,
            ));
        }
    }
    unique.sort();
    Ok(unique
        .iter()
        .map(|entry| market_slice_lock_key(entry.3, entry.4, &entry.2))
        .collect())
}

/// One idempotency advisory key: the actor, the operation and the key.
pub fn idempotency_lock_key(lock_scope_id: Uuid, operation: &str, key: &str) -> i64 {
    lock_key(&format!("{lock_scope_id}|{operation}|{key}"))
}

/// Trim and bound-check one request idempotency key.
///
/// Python counts code points, so a multi-byte key is bounded by its characters,
/// not its bytes.
pub fn normalize_idempotency_key(raw: &str) -> Result<String, ApiError> {
    let key = raw.trim();
    let length = key.chars().count();
    if !(IDEMPOTENCY_KEY_MIN..=IDEMPOTENCY_KEY_MAX).contains(&length) {
        return Err(ApiError {
            status: axum::http::StatusCode::BAD_REQUEST,
            detail: serde_json::json!(IDEMPOTENCY_KEY_BOUNDS),
            retry_after: None,
            kind: ApiErrorKind::Expected,
        });
    }
    Ok(key.to_owned())
}
