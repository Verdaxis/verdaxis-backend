//! Verification only. Session issuance and authentication-state writes stay in Python.
mod actor;
mod claims;
pub mod limits;
pub mod scope;
mod token;
mod transaction;

pub use actor::{AuthenticatedRequest, CurrentActor, UserLockMode};
pub use transaction::{AuthenticatedError, AuthenticatedTransaction, run_authenticated};

pub use claims::{AccountState, AuthError, VerifiedAccess, validate_account};
pub use token::{VerificationSettings, verify_access, verify_authorization};
