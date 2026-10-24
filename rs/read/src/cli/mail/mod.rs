//! Shared mail transport for Phase 10 jobs.
//!
//! The provider client is the accepted Phase 2 [`MailClient`]; this module adds
//! only the logging rule. A delivery record names the recipient by a short
//! digest, never by the address, and the provider payload is never logged.
use crate::crud::referrals::invite::MailClient;
use serde_json::Value;
use sha2::Digest;

const DIGEST_BYTES: usize = 16;

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
        let delivery = if self.client.send(payload).await {
            Delivery::Sent
        } else {
            Delivery::Refused
        };
        MailLog::new(recipient, delivery).record();
        delivery
    }
}
