//! `verdaxis-read job kyc-notices`: operator-only KYC grace notices.
//!
//! There is no timer. The command refuses, before any database contact, when:
//! the deployed identity differs from the arguments, deploy state exists, the
//! KYC execution gate is off (a notice would be misleading), `--apply` is
//! missing, or the live provider is selected without its own authorization.
//! Live grace mail stays on hold until live sending is explicitly authorized;
//! that authorization is the separate `KYC_GRACE_NOTICE_LIVE_SEND_AUTHORIZED`
//! switch in the job environment. Gate-enabled plus `--apply` is not
//! authorization.
//!
//! Exit codes: 0 done (including nothing to send), 2 usage refusal (gate off,
//! no `--apply`, live send not authorized), 3 precondition refusal, 4 database
//! unavailable, 5 a claimed notice whose provider result is not "accepted":
//! its claim stays and is never retried automatically.
use crate::cli::context::{self, Purpose, RunContext};
use crate::cli::gate::{require_apply, require_job_preflight};
use crate::cli::mail::kyc::{KycCounts, KycNotice, MAX_PER_INVOCATION, PAGE_SIZE};
use crate::cli::{Environment, Exit, JobArgs, JobKind};
use crate::crud::referrals::invite::MailClient;
use sqlx::postgres::PgConnectOptions;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::str::FromStr;

const DEFAULT_FRONTEND_URL: &str = "https://app.verdaxis.exchange";
const DEFAULT_SENDER: &str = "Verdaxis <noreply@verdaxis.exchange>";
const COMMAND: &str = "job kyc-notices";

/// Everything the job reads from its environment, checked before a pool opens.
pub struct KycJobPlan {
    pub environment: Environment,
    pub release_sha: String,
    /// The database name the connection must report, from `DATABASE_URL`.
    pub database: String,
    pub sender: String,
    pub frontend: String,
    provider_key: Option<String>,
    live_send_authorized: bool,
}

impl KycJobPlan {
    /// The live provider client. Refused unless the separate authorization is
    /// present and a provider key exists, so a claim is never spent on a
    /// provider that cannot be called.
    pub fn live_mail_client(&self) -> Result<MailClient, Exit> {
        if !self.live_send_authorized {
            return Err(Exit::Usage);
        }
        let key = self
            .provider_key
            .clone()
            .filter(|key| !key.trim().is_empty())
            .ok_or(Exit::Precondition)?;
        MailClient::production(Some(key)).map_err(|_| Exit::Dependency)
    }
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A boolean switch with the deployed spellings. Absent takes `default`.
fn switch(value: Option<&String>, default: bool) -> Result<bool, Exit> {
    match value {
        None => Ok(default),
        Some(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "t" | "y" | "yes" | "1" | "on" => Ok(true),
            "false" | "f" | "n" | "no" | "0" | "off" => Ok(false),
            _ => Err(Exit::Precondition),
        },
    }
}

/// Every refusal that needs no database or network, in this order: full SHA
/// and identity, deploy state, KYC gate, `--apply`.
pub fn preflight(
    args: &JobArgs,
    vars: &HashMap<String, String>,
    deploy_state: &Path,
) -> Result<KycJobPlan, Exit> {
    debug_assert_eq!(args.kind, JobKind::KycNotices);
    let expected_sha = &args.identity.release_sha;
    if !is_full_sha(expected_sha) {
        return Err(Exit::Precondition);
    }
    let value = |key: &str| vars.get(key).map(String::as_str).unwrap_or("");
    require_job_preflight(
        value("ENVIRONMENT"),
        value("RELEASE_SHA"),
        args.identity.environment,
        expected_sha,
        deploy_state,
    )?;
    // With the gate off no user depends on grace, so a notice would be wrong.
    if !switch(vars.get("KYC_EXECUTION_GATE_ENABLED"), true)? {
        return Err(Exit::Usage);
    }
    require_apply(args.apply)?;

    let database = vars
        .get("DATABASE_URL")
        .and_then(|url| PgConnectOptions::from_str(url).ok())
        .and_then(|options| options.get_database().map(str::to_owned))
        .filter(|name| !name.is_empty())
        .ok_or(Exit::Precondition)?;
    let frontend = vars
        .get("FRONTEND_URL")
        .map_or(DEFAULT_FRONTEND_URL, String::as_str)
        .to_owned();
    if !(frontend.starts_with("https://") || frontend.starts_with("http://")) {
        return Err(Exit::Precondition);
    }
    let live_send_authorized = matches!(
        vars.get("KYC_GRACE_NOTICE_LIVE_SEND_AUTHORIZED")
            .map(|text| text.trim().to_ascii_lowercase())
            .as_deref(),
        Some("true" | "t" | "y" | "yes" | "1" | "on")
    );
    Ok(KycJobPlan {
        environment: args.identity.environment,
        release_sha: expected_sha.clone(),
        database,
        sender: vars
            .get("EMAIL_FROM")
            .map_or(DEFAULT_SENDER, String::as_str)
            .to_owned(),
        frontend,
        provider_key: vars.get("RESEND_API_KEY").cloned(),
        live_send_authorized,
    })
}

fn report(outcome: &str, counts: KycCounts) {
    eprintln!(
        "{}",
        serde_json::json!({
            "command": COMMAND,
            "outcome": outcome,
            "counts": {
                "sent": counts.sent,
                "uncertain_or_failed": counts.failed,
                "skipped": counts.skipped,
            },
        })
    );
}

/// Run pages until nothing is left or [`MAX_PER_INVOCATION`] rows were tried.
async fn execute(notice: &KycNotice, ctx: &RunContext) -> Exit {
    let mut total = KycCounts::default();
    let mut remaining = u64::from(MAX_PER_INVOCATION);
    while remaining > 0 {
        let limit = remaining.min(u64::from(PAGE_SIZE)) as u32;
        let page = match notice.send_page(ctx, limit).await {
            Ok(page) => page,
            Err(exit) => {
                report("failed", total);
                return exit;
            }
        };
        total.add(page);
        let attempted = page.attempted();
        remaining = remaining.saturating_sub(attempted);
        if attempted < u64::from(limit) {
            break;
        }
    }
    if total.failed > 0 {
        // A claimed notice has an unknown or refused provider result: its claim
        // stays and an operator reviews it.
        report("ambiguous", total);
        Exit::Ambiguous
    } else {
        report("ok", total);
        Exit::Ok
    }
}

/// The job with its environment, deploy-state path, provider and pool opener
/// supplied by the caller. `mail` is `None` for the live provider, which needs
/// its own authorization; tests pass a loopback client.
pub async fn run_with<O, Fut>(
    args: &JobArgs,
    vars: &HashMap<String, String>,
    deploy_state: &Path,
    mail: Option<MailClient>,
    open: O,
) -> Exit
where
    O: FnOnce(Environment, String) -> Fut,
    Fut: Future<Output = Result<RunContext, Exit>>,
{
    let plan = match preflight(args, vars, deploy_state) {
        Ok(plan) => plan,
        Err(exit) => {
            report("refused", KycCounts::default());
            return exit;
        }
    };
    let mail = match mail {
        Some(client) => client,
        None => match plan.live_mail_client() {
            Ok(client) => client,
            Err(exit) => {
                report("refused", KycCounts::default());
                return exit;
            }
        },
    };
    let ctx = match open(plan.environment, plan.release_sha.clone()).await {
        Ok(ctx) => ctx,
        Err(exit) => {
            report("refused", KycCounts::default());
            return exit;
        }
    };
    let notice = KycNotice::new(mail, plan.sender, plan.frontend, plan.database);
    execute(&notice, &ctx).await
}

/// The deploy state path of the environment: `<state dir>/<environment>.state`.
fn deploy_state_path(vars: &HashMap<String, String>, environment: Environment) -> PathBuf {
    let directory = vars
        .get("DEPLOY_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".runtime-deploy"));
    directory.join(format!("{}.state", environment.as_str()))
}

/// Entry point for `job kyc-notices`.
pub async fn run(args: &JobArgs) -> Exit {
    let _ = tracing_subscriber::fmt()
        .json()
        .with_writer(std::io::stderr)
        .try_init();
    let vars: HashMap<String, String> = std::env::vars().collect();
    let state = deploy_state_path(&vars, args.identity.environment);
    run_with(
        args,
        &vars,
        &state,
        None,
        |environment, release_sha| async move {
            context::open(Purpose::Job(JobKind::KycNotices), environment, &release_sha).await
        },
    )
    .await
}
