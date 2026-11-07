//! Phase 4 business and Phase 5 compliance tests, built as one test binary.
//!
//! Each module is an existing test file. One binary links the crate once
//! instead of once per file. `scripts/check_test_groups.py` checks that
//! every test file belongs to exactly one test binary. A module behind
//! `#[cfg(feature = "contract-tests")]` was a target that required that feature.

#[path = "phase4_admission.rs"]
mod phase4_admission;
#[path = "phase4_approval_mail.rs"]
mod phase4_approval_mail;
#[path = "phase4_completion.rs"]
mod phase4_completion;
#[cfg(feature = "contract-tests")]
#[path = "phase4_db_authority.rs"]
mod phase4_db_authority;
#[path = "phase4_foundation.rs"]
mod phase4_foundation;
#[path = "phase4_inventory.rs"]
mod phase4_inventory;
#[path = "phase4_kyc_documents.rs"]
mod phase4_kyc_documents;
#[path = "phase4_kyc_vault.rs"]
mod phase4_kyc_vault;
#[path = "phase4_limits.rs"]
mod phase4_limits;
#[path = "phase4_market_invalidation.rs"]
mod phase4_market_invalidation;
#[path = "phase4_mount_composition.rs"]
mod phase4_mount_composition;
// Phase 4 Task 21: the integrated race suite; its PostgreSQL cases are
// feature-gated by contract-tests.
#[cfg(feature = "contract-tests")]
#[path = "phase4_races.rs"]
mod phase4_races;
#[path = "phase4_rejection.rs"]
mod phase4_rejection;
#[path = "phase4_serving.rs"]
mod phase4_serving;
#[path = "phase5_compliance_fuels.rs"]
mod phase5_compliance_fuels;
#[path = "phase5_compliance_pricing_core.rs"]
mod phase5_compliance_pricing_core;
#[path = "phase5_compliance_scores_core.rs"]
mod phase5_compliance_scores_core;
#[path = "phase5_fueleu.rs"]
mod phase5_fueleu;
#[path = "phase5_public_boundary.rs"]
mod phase5_public_boundary;
#[path = "phase5_routing.rs"]
mod phase5_routing;
