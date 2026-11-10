//! Phase 4 business and Phase 5 compliance tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary. The modules are
//! `pub`, as the items of a test file root were, so the unused-item lints
//! treat each file as they did in its own binary. A module behind
//! `#[cfg(feature = "contract-tests")]` was a target that required that feature.

#[path = "phase4_admission.rs"]
pub mod phase4_admission;
#[path = "phase4_approval_mail.rs"]
pub mod phase4_approval_mail;
#[cfg(feature = "contract-tests")]
#[path = "phase4_db_authority.rs"]
pub mod phase4_db_authority;
#[path = "phase4_foundation.rs"]
pub mod phase4_foundation;
#[path = "phase4_kyc_documents.rs"]
pub mod phase4_kyc_documents;
#[path = "phase4_kyc_vault.rs"]
pub mod phase4_kyc_vault;
#[path = "phase4_limits.rs"]
pub mod phase4_limits;
#[path = "phase4_market_invalidation.rs"]
pub mod phase4_market_invalidation;
// Phase 4 Task 21: the integrated race suite; its PostgreSQL cases are
// feature-gated by contract-tests.
#[cfg(feature = "contract-tests")]
#[path = "phase4_races.rs"]
pub mod phase4_races;
#[path = "phase5_compliance_fuels.rs"]
pub mod phase5_compliance_fuels;
#[path = "phase5_compliance_pricing_core.rs"]
pub mod phase5_compliance_pricing_core;
#[path = "phase5_compliance_scores_core.rs"]
pub mod phase5_compliance_scores_core;
#[path = "phase5_fueleu.rs"]
pub mod phase5_fueleu;
#[path = "phase5_public_boundary.rs"]
pub mod phase5_public_boundary;
#[path = "phase5_routing.rs"]
pub mod phase5_routing;
