//! Token verification, token issuance and password primitives, plus the
//! session storage and lock primitives that credential routes build on.
mod actor;
mod claims;
pub mod cookies;
pub mod email;
pub mod email_verification;
pub mod error;
pub mod issue;
pub mod limits;
pub mod lock;
pub mod origin;
pub mod password;
pub mod password_reset;
pub mod request;
pub mod scope;
pub mod store;
pub mod stream;
mod token;
pub mod token_bytes;
mod transaction;

pub use actor::{AuthenticatedRequest, CurrentActor, UserLockMode};
pub use transaction::{AuthenticatedError, AuthenticatedTransaction, run_authenticated};

pub use claims::{
    AccountState, AuthError, TokenIssuance, VerifiedAccess, cutoff_refusal, read_issuance,
    read_subject, validate_account, validate_account_for_path,
};
pub use issue::TokenIssuer;
pub use token::{VerificationSettings, verify_access, verify_authorization};

pub mod logout;
pub mod password_change;

pub mod login;

pub mod refresh;

pub mod invitation_accept;

pub mod registration;
