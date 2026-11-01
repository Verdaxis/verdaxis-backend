//! Shared mail transport for Phase 10 jobs.
//!
//! The provider client is the accepted Phase 2 [`MailClient`]; this module adds
//! the logging rule and the keyed send. A delivery record names the recipient
//! by a short digest, never by the address, and the provider payload is never
//! logged. A job that may retry a notice sends it with
//! [`MailTransport::send_keyed`].
pub mod approval;
pub mod expiry;
pub mod kyc;

use crate::crud::referrals::invite::MailClient;
use serde_json::Value;
use sha2::Digest;
use std::future::Future;
use std::time::Duration;

const DIGEST_BYTES: usize = 16;

/// The default bound for one keyed provider call: the Phase 2 client's own
/// request timeout.
pub const PROVIDER_CALL_DEADLINE: Duration = Duration::from_secs(10);

/// The result of one provider call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    Refused,
}

impl Delivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Refused => "refused",
        }
    }
}

/// A stable, correlatable recipient label for logs and state records.
///
/// Case and surrounding whitespace are normalized first, because a provider
/// treats those addresses as the same recipient. Sixteen bytes of unsalted
/// SHA-256 over the address is not a secret: it defeats reading the log, not
/// exhaustive enumeration of a known candidate set. Use it only to correlate
/// records, never as an authorization decision.
pub fn recipient_digest(address: &str) -> String {
    let digest = sha2::Sha256::digest(address.trim().to_lowercase().as_bytes());
    digest[..DIGEST_BYTES]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The one log record a mail delivery writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailLog {
    pub recipient_digest: String,
    pub delivery: Delivery,
}

impl MailLog {
    pub fn new(recipient: &str, delivery: Delivery) -> Self {
        Self {
            recipient_digest: recipient_digest(recipient),
            delivery,
        }
    }

    /// Write the delivery record. The address is not part of this type.
    pub fn record(&self) {
        tracing::info!(
            target: "verdaxis::cli::mail",
            recipient_digest = %self.recipient_digest,
            delivery = self.delivery.as_str(),
            "mail_delivery"
        );
    }

    pub fn json(&self) -> Value {
        serde_json::json!({
            "recipient_digest": self.recipient_digest,
            "delivery": self.delivery.as_str(),
        })
    }
}

/// The shared transport every mail job uses.
pub struct MailTransport {
    client: MailClient,
}

impl MailTransport {
    pub fn new(client: MailClient) -> Self {
        Self { client }
    }

    /// Send one prepared payload and log the delivery.
    ///
    /// A disabled client (no provider key) refuses without a network call; the
    /// caller decides whether that is a deferred or a failed outcome.
    pub async fn send(&self, recipient: &str, payload: &Value) -> Delivery {
        let accepted = self.client.send(payload).await;
        log_delivery(recipient, accepted)
    }

    /// Send one notice under its provider idempotency key and log the delivery.
    ///
    /// The provider keeps the key and answers a repeat of it without sending a
    /// second message, so a retry after a lost reply cannot send the notice
    /// twice. The key must name exactly one notice (for example
    /// `kyc-grace-<grace id>`). A blank key is refused without a network call,
    /// because a retry without a key could send twice. The caller's database
    /// claim stays the first check; the key covers only the provider side.
    pub async fn send_keyed(
        &self,
        recipient: &str,
        payload: &Value,
        idempotency_key: &str,
    ) -> Delivery {
        self.send_keyed_within(recipient, payload, idempotency_key, PROVIDER_CALL_DEADLINE)
            .await
    }

    /// [`Self::send_keyed`] with the caller's own deadline for the call.
    ///
    /// A call the deadline cuts off counts as refused, and its delivery record
    /// is written after the cut, so every call leaves exactly one record.
    pub async fn send_keyed_within(
        &self,
        recipient: &str,
        payload: &Value,
        idempotency_key: &str,
        deadline: Duration,
    ) -> Delivery {
        let call = self
            .client
            .send_with_idempotency(payload, Some(idempotency_key));
        deliver_keyed(recipient, idempotency_key, deadline, call).await
    }
}

/// Run one keyed provider call under `deadline`, then write its one delivery
/// record: after the call answers, or after the deadline cuts it off (counted as
/// refused). A blank key is refused before the call starts, because a retry
/// without a key could send twice.
///
/// [`MailTransport::send_keyed_within`] uses this with the provider client;
/// a job that injects its provider call for tests passes that call here, so
/// both follow the same key, deadline and logging rule.
pub async fn deliver_keyed<F>(
    recipient: &str,
    idempotency_key: &str,
    deadline: Duration,
    call: F,
) -> Delivery
where
    F: Future<Output = bool>,
{
    let accepted = !idempotency_key.trim().is_empty()
        && tokio::time::timeout(deadline, call).await.unwrap_or(false);
    log_delivery(recipient, accepted)
}

fn log_delivery(recipient: &str, accepted: bool) -> Delivery {
    let delivery = if accepted {
        Delivery::Sent
    } else {
        Delivery::Refused
    };
    MailLog::new(recipient, delivery).record();
    delivery
}
