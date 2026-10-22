//! Token verification, plus the session storage and lock primitives that
//! credential routes build on. Python still issues every session.
mod actor;
mod claims;
pub mod limits;
pub mod lock;
pub mod request;
pub mod scope;
pub mod store;
mod token;
mod transaction;

pub use actor::{AuthenticatedRequest, CurrentActor, UserLockMode};
pub use transaction::{AuthenticatedError, AuthenticatedTransaction, run_authenticated};

pub use claims::{AccountState, AuthError, VerifiedAccess, validate_account};
pub use token::{VerificationSettings, verify_access, verify_authorization};
