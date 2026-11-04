//! Operator commands (`verdaxis-read ops <action>`).
//!
//! Each operator task adds its own file here and one `pub mod` line below.

pub mod budget;
pub mod cache;
pub mod csv;
pub mod dotenv;
pub mod gena;
pub mod ingest;
pub mod inspect;
pub mod operator;
pub mod preflight;
pub mod runtime_env;
