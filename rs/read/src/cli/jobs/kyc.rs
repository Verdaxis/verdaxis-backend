//! `verdaxis-read job kyc-notices`: operator-only KYC grace notices.
//!
//! This is the interface the tests pin. The behaviour lands in the next commit.
use super::super::context::RunContext;
use super::super::{Environment, Exit, JobArgs};
use crate::crud::referrals::invite::MailClient;
use std::collections::HashMap;
use std::future::Future;
use std::path::Path;

pub struct KycJobPlan;

pub fn preflight(
    _args: &JobArgs,
    _vars: &HashMap<String, String>,
    _deploy_state: &Path,
) -> Result<KycJobPlan, Exit> {
    Err(Exit::Precondition)
}

pub async fn run_with<O, Fut>(
    _args: &JobArgs,
    _vars: &HashMap<String, String>,
    _deploy_state: &Path,
    _mail: Option<MailClient>,
    _open: O,
) -> Exit
where
    O: FnOnce(Environment, String) -> Fut,
    Fut: Future<Output = Result<RunContext, Exit>>,
{
    Exit::Precondition
}

pub async fn run(_args: &JobArgs) -> Exit {
    Exit::Precondition
}
