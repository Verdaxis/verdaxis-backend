//! Token verification and issuance, password primitives and the auth HTTP
//! boundary (cookies, origin rule, errors and body limit).
mod actor;
mod claims;
pub mod cookies;
pub mod error;
pub mod issue;
pub mod limits;
pub mod origin;
pub mod password;
pub mod request;
pub mod scope;
mod token;
pub mod token_bytes;
mod transaction;

pub use actor::{AuthenticatedRequest, CurrentActor, UserLockMode};
pub use transaction::{AuthenticatedError, AuthenticatedTransaction, run_authenticated};

pub use claims::{
    AccountState, AuthError, TokenIssuance, VerifiedAccess, cutoff_refusal, read_issuance,
    read_subject, validate_account,
};
pub use issue::TokenIssuer;
pub use token::{VerificationSettings, verify_access, verify_authorization};
