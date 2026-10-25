//! Stored source provenance for authoritative analytics.
//!
//! Provenance is read from the stored order and trade facts; an organization
//! name or the current membership is never used to infer it. The three stored
//! classes are independent: a response reports `live`, `demo` and `unknown`
//! separately, and no total adds them together. Reference (benchmark) rows are
//! a fourth, separate coverage family that is never market activity and is
//! therefore not a `MarketProvenance` at all.
//!
//! Rules ported from `app/services/product_analytics.py`:
//!
//! - an order is demo when its stored provenance is `DEMO` or its
//!   organization is a seeded demo organization; live when the stored
//!   provenance is `REAL`; unknown otherwise;
//! - a trade is demo when either party is demo (a mixed trade is
//!   demo-contaminated, never live); live only when both stored party
//!   provenances are `REAL`; unknown otherwise.

use super::types::AnalyticsActivity;

/// Stored provenance class of one order or trade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketProvenance {
    Live,
    Demo,
    Unknown,
}

impl MarketProvenance {
    /// The lowercase label used by `ProductAnalyticsQuery` and the client.
    pub fn provenance_label(self) -> &'static str {
        match self {
            MarketProvenance::Live => "live",
            MarketProvenance::Demo => "demo",
            MarketProvenance::Unknown => "unknown",
        }
    }
}

/// Classify one order from its stored label and its organization identity.
pub fn classify_order(stored_label: &str, seeded_demo_organization: bool) -> MarketProvenance {
    if seeded_demo_organization || stored_label == "DEMO" {
        MarketProvenance::Demo
    } else if stored_label == "REAL" {
        MarketProvenance::Live
    } else {
        MarketProvenance::Unknown
    }
}

/// Classify one trade from both stored party labels and identities.
pub fn classify_trade(
    buyer_label: &str,
    buyer_seeded_demo: bool,
    seller_label: &str,
    seller_seeded_demo: bool,
) -> MarketProvenance {
    let demo =
        buyer_seeded_demo || seller_seeded_demo || buyer_label == "DEMO" || seller_label == "DEMO";
    if demo {
        MarketProvenance::Demo
    } else if buyer_label == "REAL" && seller_label == "REAL" {
        MarketProvenance::Live
    } else {
        MarketProvenance::Unknown
    }
}

impl AnalyticsActivity {
    /// Whether this activity filter admits one stored provenance class.
    ///
    /// `REFERENCE` admits no stored order or trade: reference coverage is
    /// separate and is never activity. `ALL` composes the three stored
    /// classes; callers still report each class separately rather than
    /// summing them into one figure.
    pub fn includes(self, provenance: MarketProvenance) -> bool {
        match self {
            AnalyticsActivity::All => matches!(
                provenance,
                MarketProvenance::Live | MarketProvenance::Demo | MarketProvenance::Unknown
            ),
            AnalyticsActivity::Live => provenance == MarketProvenance::Live,
            AnalyticsActivity::Demo => provenance == MarketProvenance::Demo,
            AnalyticsActivity::Reference => false,
        }
    }
}
