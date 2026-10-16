//! One provenance policy for every public market projection.
//!
//! Port of the policy constants in `app/services/market_provenance.py`. Public
//! rows are only ever independently labelled REAL or DEMO; a null-expiry row
//! may also be REAL or UNKNOWN; TEST, CANARY, and synthetic classes are never
//! promoted to evidence. The SQL predicates that apply this policy live in
//! `rs/read/sql/`, and these constants keep the Rust side in step with them.

/// Fuel types admitted to the public marketplace.
pub const ADMITTED_FUEL_TYPES: [&str; 5] = ["Methanol", "Ethanol", "Biofuel", "FAME", "LNG"];

/// Provenance labels a public order collection admits.
pub const PUBLIC_ORDER_EVIDENCE: [&str; 2] = ["REAL", "DEMO"];

/// Provenance labels that may keep a public order visible with a null expiry.
pub const NULL_EXPIRY_EVIDENCE: [&str; 2] = ["REAL", "UNKNOWN"];

/// Classes that are never formal evidence, even after an equity sale.
pub const SYNTHETIC_PROVENANCE: [&str; 3] = ["DEMO", "TEST", "CANARY"];

/// Trade states that count as formal, immutable evidence.
pub const FORMAL_TRADE_STATUSES: [&str; 3] = ["CONFIRMED", "DELIVERED", "PAID"];

/// A public aggregate is computed inside exactly one evidence class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MarketEvidenceScope {
    Real,
    Demo,
}

impl MarketEvidenceScope {
    /// The organization/order/trade provenance label for this scope.
    pub fn provenance(self) -> &'static str {
        match self {
            MarketEvidenceScope::Real => "REAL",
            MarketEvidenceScope::Demo => "DEMO",
        }
    }

    /// The response `value_prefix` for this scope.
    pub fn value_prefix(self) -> &'static str {
        match self {
            MarketEvidenceScope::Real => "real",
            MarketEvidenceScope::Demo => "demo",
        }
    }
}

/// The exact buyer/seller provenance pairs a formal trade snapshot must carry.
/// REAL/REAL and DEMO/DEMO are admitted separately; no mixed or synthetic pair
/// is evidence.
pub const PUBLIC_TRADE_EVIDENCE_PAIRS: [(&str, &str); 2] = [("REAL", "REAL"), ("DEMO", "DEMO")];

/// Whether a stored provenance label is formal historical trade evidence.
pub fn is_public_trade_evidence_pair(buyer: &str, seller: &str) -> bool {
    PUBLIC_TRADE_EVIDENCE_PAIRS
        .iter()
        .any(|(left, right)| *left == buyer && *right == seller)
}
