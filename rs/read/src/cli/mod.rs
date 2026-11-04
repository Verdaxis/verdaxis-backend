//! Typed command boundary. Execution is registered by each command's owning task.
pub mod acl;
pub mod canary;
pub mod context;
pub mod gate;
pub mod guard;
pub mod jobs;
pub mod mail;
pub mod migrate;
pub mod monitors;
pub mod ops;
pub mod state;

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Delay before each retry. With the first attempt, this caps a call at three
/// attempts: it starts at 0 s, retries at 1 s and 4 s.
pub const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];
/// Total deadline the delays fit inside.
pub const RETRY_TIMEOUT: Duration = Duration::from_secs(10);

/// Run one fallible dependency call with the job retry policy.
///
/// The helper retries only calls the caller marks retryable. Ambiguous sends,
/// committed claims and business transaction conflicts have their own owners
/// and must not be wrapped here.
///
/// The bound covers scheduling, not one in-flight attempt: a hung future stays
/// hung until its own timeout or the systemd unit deadline. Network calls keep
/// their own client timeout (the Phase 2 `MailClient` uses 10 s).
pub async fn retry_bounded<T, E, F, Fut>(mut operation: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let started = tokio::time::Instant::now();
    let mut attempt = 0_usize;
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let Some(delay) = RETRY_DELAYS.get(attempt).copied() else {
                    return Err(error);
                };
                if started.elapsed() + delay >= RETRY_TIMEOUT {
                    return Err(error);
                }
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Exit {
    Ok = 0,
    /// `monitor outbox` only: a backlog threshold is breached. The external
    /// monitor reads exit 1 as a breach, as with the Python probe.
    Breached = 1,
    Usage = 2,
    Precondition = 3,
    Dependency = 4,
    Ambiguous = 5,
}

#[derive(Debug, Parser)]
#[command(
    name = "verdaxis-read",
    about = "Verdaxis service and bounded operator commands"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

// The command is parsed once per process; the size of its largest variant
// (the monitor flags) does not matter, and boxing it would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    Serve(ServeArgs),
    Migrate(MigrateArgs),
    Acl(AclArgs),
    Job(JobArgs),
    Monitor(MonitorArgs),
    Recover(RecoverArgs),
    Ops(OpsArgs),
    Guard(GuardArgs),
    Canary(CanaryArgs),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
pub struct ServeArgs {
    #[arg(long)]
    pub bind: Option<SocketAddr>,
    #[arg(long, value_parser = absolute_path)]
    pub budget_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Environment {
    Staging,
    Production,
}

impl Environment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct IdentityArgs {
    #[arg(long, value_enum)]
    pub environment: Environment,
    #[arg(long, value_parser = release_sha)]
    pub release_sha: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MigrationAction {
    Status,
    Baseline,
    Run,
    Verify,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct MigrateArgs {
    pub action: MigrationAction,
    #[command(flatten)]
    pub identity: IdentityArgs,
    #[arg(long)]
    pub check: bool,
    #[arg(long)]
    pub expected_current: Option<String>,
    #[arg(long)]
    pub target: Option<String>,
    #[arg(long)]
    pub apply: bool,
    /// The nonce of the deploy that runs a writing `migrate`; it must match
    /// the deploy state (see `migrate::require_deploy_ownership`).
    #[arg(long)]
    pub deploy_nonce: Option<String>,
    /// The deployed `.env`: its identity and migrator URL replace the process
    /// environment's, so the deploy never handles the URL itself.
    #[arg(long, value_parser = absolute_path)]
    pub environment_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AclAction {
    Converge,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct AclArgs {
    pub action: AclAction,
    #[command(flatten)]
    pub identity: IdentityArgs,
    #[arg(long, value_parser = absolute_path)]
    pub env_file: PathBuf,
    /// The verified ACL bundle (`deploy/postgres/*.sql` of the release).
    #[arg(long, value_parser = absolute_path)]
    pub bundle_root: PathBuf,
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum JobKind {
    News,
    AuthMaintenance,
    PruneAnalytics,
    Reminders,
    KycNotices,
    Demo,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct JobArgs {
    pub kind: JobKind,
    #[command(flatten)]
    pub identity: IdentityArgs,
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MonitorKind {
    Health,
    BackupVerify,
    Alert,
    Outbox,
    External,
    ExternalChecks,
    Onboarding,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct MonitorArgs {
    pub kind: MonitorKind,
    #[arg(long, value_enum, requires = "release_sha")]
    pub environment: Option<Environment>,
    #[arg(long, value_parser = release_sha, requires = "environment")]
    pub release_sha: Option<String>,
    #[arg(long, conflicts_with = "remind")]
    pub event: Option<String>,
    #[arg(long, value_parser = ["all", "health", "backup"], conflicts_with = "event")]
    pub remind: Option<String>,
    /// `monitor alert`: an operator's finding for an uncertain send,
    /// `<check>:<sequence>:<destination>:<delivered|not-delivered>`.
    #[arg(long)]
    pub reconcile: Option<String>,
    /// `monitor alert`: new attempts for an exhausted head, `<check>:<sequence>`.
    #[arg(long)]
    pub rearm: Option<String>,
    /// `monitor alert`: clear an invalid or reused head after a journal repair,
    /// `<check>:<sequence>`; needs `--reason`.
    #[arg(long)]
    pub repair: Option<String>,
    /// `monitor alert --repair`: the operator's reason, logged with the repair.
    #[arg(long)]
    pub reason: Option<String>,
    /// `monitor alert`: hand a drained check back to the Python dispatcher.
    #[arg(long)]
    pub release: Option<String>,
    /// `monitor outbox`: the read-only libpq DSN (never printed).
    #[arg(long)]
    pub dsn: Option<String>,
    /// `monitor outbox`: breach above this many unsequenced rows.
    #[arg(long, allow_negative_numbers = true)]
    pub max_pending: Option<i64>,
    /// `monitor outbox`: breach above this age of the oldest unsequenced row.
    #[arg(long, allow_negative_numbers = true)]
    pub max_age_seconds: Option<i64>,
    /// `monitor outbox`: the query deadline in seconds (1 to 20).
    #[arg(long)]
    pub query_timeout: Option<u64>,
    #[arg(long)]
    pub telegram_disabled: bool,
    /// `monitor onboarding`: the alert state file.
    #[arg(long, value_parser = absolute_path)]
    pub state_file: Option<PathBuf>,
    /// `monitor onboarding`: print the alerts that would be sent.
    #[arg(long, conflicts_with = "bootstrap")]
    pub dry_run: bool,
    /// `monitor onboarding`: record the present stages without alerting.
    #[arg(long)]
    pub bootstrap: bool,
    /// `monitor onboarding --dry-run`: print recipient addresses in full.
    #[arg(long)]
    pub show_emails: bool,
}

impl MonitorArgs {
    /// Flags that only `monitor onboarding` understands.
    fn uses_onboarding_flags(&self) -> bool {
        self.state_file.is_some() || self.dry_run || self.bootstrap || self.show_emails
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct RecoverArgs {
    #[command(flatten)]
    pub identity: IdentityArgs,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct OpsArgs {
    #[command(subcommand)]
    pub action: OpsAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum OpsAction {
    Preflight(PreflightArgs),
    PrepareBudget(BudgetArgs),
    PrepareRuntime(PrepareRuntimeArgs),
    Inspect(InspectArgs),
    Ingest(IngestArgs),
    Import(ImportArgs),
    CacheAuthority(CacheArgs),
    Fleet(IdentityArgs),
}

/// `ops preflight`: the deployed identity and database role, read-only.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct PreflightArgs {
    #[command(flatten)]
    pub identity: IdentityArgs,
    /// The deployed `.env`.
    #[arg(long)]
    pub environment_file: PathBuf,
}

/// `ops prepare-budget`: write (or `--verify`) the connection budget artifact.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct BudgetArgs {
    #[command(flatten)]
    pub identity: IdentityArgs,
    #[arg(long)]
    pub environment_file: PathBuf,
    #[arg(long)]
    pub output: PathBuf,
    /// Check the existing artifact against the inputs instead of writing it.
    #[arg(long)]
    pub verify: bool,
    /// Budget the dedicated cache listener connection (prepare only).
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=1))]
    pub cache_listener: u8,
    /// Budget the dedicated stream election connection (prepare only).
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=1))]
    pub stream_listener: u8,
}

/// `ops prepare-runtime`: one private environment file per purpose.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct PrepareRuntimeArgs {
    #[command(flatten)]
    pub identity: IdentityArgs,
    #[arg(long)]
    pub environment_file: PathBuf,
    /// The private runtime directory (`.runtime-rust`), absolute.
    #[arg(long)]
    pub output_dir: PathBuf,
    /// The literal migration revision this release starts at.
    #[arg(long)]
    pub migration_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InspectKind {
    Users,
    Blockers,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct InspectArgs {
    pub kind: InspectKind,
    #[command(flatten)]
    pub identity: IdentityArgs,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub show_emails: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum IngestKind {
    Signals,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ImportKind {
    Gena,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct FileArgs {
    #[command(flatten)]
    pub identity: IdentityArgs,
    #[arg(long, value_parser = absolute_path)]
    pub file: PathBuf,
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct IngestArgs {
    pub kind: IngestKind,
    /// The signal family of the whole file.
    #[arg(long, value_enum)]
    pub family: SignalFamily,
    /// The run's source id (`^[a-z0-9][a-z0-9_-]{1,63}$`).
    #[arg(long)]
    pub source: String,
    #[command(flatten)]
    pub input: FileArgs,
}

/// The three trusted signal families (`ops ingest signals --family`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SignalFamily {
    #[value(name = "indications")]
    Indications,
    #[value(name = "fair_bands")]
    FairBands,
    #[value(name = "stems")]
    Stems,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ImportArgs {
    pub kind: ImportKind,
    /// The fuel type of every project in the file (Methanol, Ethanol, ...).
    #[arg(long)]
    pub fuel_type: String,
    #[command(flatten)]
    pub input: FileArgs,
    #[arg(long)]
    pub allow_blank_id: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CacheAction {
    Status,
    Revoke,
    Activate,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct CacheArgs {
    pub action: CacheAction,
    #[command(flatten)]
    pub identity: IdentityArgs,
    /// The Rust runtime directory that holds `cache-authority.json`.
    #[arg(long, value_parser = absolute_path)]
    pub runtime_dir: PathBuf,
    /// `activate`: the deployed `.env`; only ENVIRONMENT, RELEASE_SHA and
    /// MIGRATOR_DATABASE_URL are read from it.
    #[arg(long, value_parser = absolute_path)]
    pub environment_file: Option<PathBuf>,
    #[arg(long)]
    pub apply: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum GuardKind {
    DeployStart,
    RustStart,
    Health,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct GuardArgs {
    pub kind: GuardKind,
    /// `health`: the environment the readiness body must name.
    #[arg(long)]
    pub environment: Option<String>,
    /// `health`: the release the readiness body must name.
    #[arg(long)]
    pub release_sha: Option<String>,
    /// `rust-start`: the release the prepared files must name.
    #[arg(long)]
    pub expected_release_sha: Option<String>,
    #[arg(long, value_parser = absolute_path)]
    pub budget_file: Option<PathBuf>,
    #[arg(long)]
    pub expected_revision: Option<String>,
    #[arg(long, value_parser = absolute_path)]
    pub revision_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CanaryAction {
    Cleanup,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct CanaryArgs {
    pub action: CanaryAction,
    #[command(flatten)]
    pub identity: IdentityArgs,
    #[arg(long)]
    pub email: String,
    #[arg(long)]
    pub apply: bool,
}

fn release_sha(value: &str) -> Result<String, &'static str> {
    if value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(value.to_owned())
    } else {
        Err("release SHA must be 40 lowercase hexadecimal characters")
    }
}

fn absolute_path(value: &str) -> Result<PathBuf, &'static str> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err("path must be absolute")
    }
}

fn parse(args: Vec<OsString>) -> Result<Command, clap::Error> {
    let mut argv = vec![OsString::from("verdaxis-read")];
    // A flag (or no arguments with RUST_CONFIG) is the legacy service form.
    // Unknown positional words remain errors, never a fallback to serving.
    if args.is_empty()
        || args
            .first()
            .is_some_and(|value| value == "--bind" || value == "--budget-file")
    {
        argv.push(OsString::from("serve"));
    }
    argv.extend(args);
    Cli::try_parse_from(argv).map(|cli| cli.command)
}

pub fn parse_arguments(args: Vec<OsString>) -> Result<Command, Exit> {
    parse(args).map_err(|error| {
        if error.use_stderr() {
            Exit::Usage
        } else {
            Exit::Ok
        }
    })
}

/// Synchronous entry point for operator callers. The binary uses the async form
/// so it can keep the service in its existing Tokio runtime.
pub fn dispatch(args: Vec<String>) -> Exit {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return Exit::Dependency,
    };
    runtime.block_on(dispatch_os(args.into_iter().map(OsString::from).collect()))
}

pub async fn dispatch_os(args: Vec<OsString>) -> Exit {
    // args_os avoids panics from non-UTF-8 arguments, even after this flag.
    if args.first().is_some_and(|arg| arg == "--version-json") {
        println!("{}", crate::version_json());
        return Exit::Ok;
    }
    if args
        .first()
        .is_some_and(|arg| arg == crate::runtime::config::BOOTSTRAP_PRODUCER_FLAG)
    {
        return bootstrap_producer(&args).await;
    }
    let command = match parse(args) {
        Ok(command) => command,
        Err(error) if !error.use_stderr() => {
            let _ = error.print();
            return Exit::Ok;
        }
        Err(_) => {
            // clap diagnostics can echo arbitrary values, including credentials.
            report("arguments", "usage_refused");
            return Exit::Usage;
        }
    };
    match command {
        Command::Serve(args) => match serve(&args).await {
            Ok(()) => Exit::Ok,
            Err(exit) => {
                report("serve", "startup_failed");
                exit
            }
        },
        Command::Job(args) if args.kind == JobKind::News => jobs::news::run(&args).await,
        Command::Job(args) if args.kind == JobKind::AuthMaintenance => {
            jobs::auth::execute(&args).await
        }
        Command::Job(args) if args.kind == JobKind::Reminders => {
            jobs::reminders::command(&args).await
        }
        Command::Job(job) if job.kind == JobKind::KycNotices => jobs::kyc::run(&job).await,
        Command::Job(job) if job.kind == JobKind::Demo => jobs::demo::command(&job).await,
        Command::Job(job) if job.kind == JobKind::PruneAnalytics => {
            jobs::prune::command(&job).await
        }
        Command::Monitor(args) if args.kind == MonitorKind::Onboarding => {
            jobs::onboarding::run_command(jobs::onboarding::Invocation {
                environment: args.environment,
                release_sha: args.release_sha,
                state_file: args
                    .state_file
                    .unwrap_or_else(|| PathBuf::from(jobs::onboarding::DEFAULT_STATE_FILE)),
                dry_run: args.dry_run,
                bootstrap: args.bootstrap,
                show_emails: args.show_emails,
                telegram_disabled: args.telegram_disabled,
                // These flags belong to `monitor alert` and `monitor outbox`.
                other_monitor_flags: args.event.is_some()
                    || args.remind.is_some()
                    || args.reconcile.is_some()
                    || args.rearm.is_some()
                    || args.repair.is_some()
                    || args.reason.is_some()
                    || args.release.is_some()
                    || args.dsn.is_some()
                    || args.max_pending.is_some()
                    || args.max_age_seconds.is_some()
                    || args.query_timeout.is_some(),
            })
            .await
        }
        Command::Monitor(args) if args.kind == MonitorKind::Health => {
            run_local_monitor(&args, monitors::health::run_default)
        }
        Command::Monitor(args) if args.kind == MonitorKind::BackupVerify => {
            run_local_monitor(&args, monitors::backup::run_default)
        }
        Command::Monitor(args) if args.kind == MonitorKind::Alert => {
            monitors::alert::run_command(&args).await
        }
        Command::Monitor(args) if args.kind == MonitorKind::External => {
            monitors::external::run(args.telegram_disabled).await
        }
        Command::Monitor(args) if args.kind == MonitorKind::Outbox => {
            monitors::outbox::run_command(&args).await
        }
        Command::Monitor(args) if args.uses_onboarding_flags() => {
            // A flag another monitor does not read is refused, never ignored.
            report("monitor", "usage_refused");
            Exit::Usage
        }
        Command::Ops(OpsArgs {
            action: OpsAction::Inspect(args),
        }) => {
            // Read-only: no `--apply`, but the same identity-checked context.
            match context::open(
                context::Purpose::Operator,
                args.identity.environment,
                &args.identity.release_sha,
            )
            .await
            {
                Ok(run_context) => ops::inspect::run(&run_context, &args).await,
                Err(exit) => {
                    report("ops", "not_available");
                    exit
                }
            }
        }
        Command::Migrate(args) => migrate::run(&args).await,
        Command::Canary(args) => canary::command(&args).await,
        Command::Guard(args) => run_guard(&args),
        Command::Acl(args) => ops_outcome("acl converge", acl::run(&args)),
        Command::Ops(OpsArgs {
            action: OpsAction::Ingest(args),
        }) => ops::ingest::run(&args).await,
        Command::Ops(OpsArgs {
            action: OpsAction::Import(args),
        }) => ops::gena::run(&args).await,
        // Task 9: the deploy transaction's projections. Each reports one
        // outcome line naming the refused rule, never a value.
        Command::Ops(OpsArgs {
            action: OpsAction::PrepareRuntime(args),
        }) => ops_outcome("ops prepare-runtime", ops::runtime_env::run(&args)),
        Command::Ops(OpsArgs {
            action: OpsAction::PrepareBudget(args),
        }) => ops_outcome("ops prepare-budget", ops::budget::run(&args)),
        Command::Ops(OpsArgs {
            action: OpsAction::CacheAuthority(args),
        }) => ops::cache::command(&args).await,
        Command::Ops(OpsArgs {
            action: OpsAction::Preflight(args),
        }) => ops_outcome("ops preflight", ops::preflight::run(&args).await),
        command => {
            // Parsing never grants execution authority. Until its owning task
            // supplies the implementation and gates, a command cannot open a pool.
            report(command.name(), "not_available");
            Exit::Precondition
        }
    }
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Serve(_) => "serve",
            Self::Migrate(_) => "migrate",
            Self::Acl(_) => "acl",
            Self::Job(_) => "job",
            Self::Monitor(_) => "monitor",
            Self::Recover(_) => "recover",
            Self::Ops(_) => "ops",
            Self::Guard(_) => "guard",
            Self::Canary(_) => "canary",
        }
    }
}

/// Whether `args` is exactly the bare command `monitor <kind>`: every other
/// field holds the value the parser gives when the flag is absent. A flag added
/// to `MonitorArgs` later is therefore refused by default.
fn is_bare_monitor(args: &MonitorArgs) -> bool {
    let Some(name) = args.kind.to_possible_value() else {
        return false;
    };
    let bare = parse(vec![
        OsString::from("monitor"),
        OsString::from(name.get_name()),
    ]);
    matches!(bare, Ok(Command::Monitor(bare)) if bare == *args)
}

/// Run a local monitor that takes no flags.
///
/// The monitor reads local files only and opens no pool. It runs on the calling
/// thread: a oneshot has no other task, and a blocking-pool thread could wait
/// forever under a unit's `TasksMax`.
fn run_local_monitor(args: &MonitorArgs, run: fn() -> Exit) -> Exit {
    if !is_bare_monitor(args) {
        report("monitor", "usage_refused");
        return Exit::Usage;
    }
    std::panic::catch_unwind(run).unwrap_or(Exit::Dependency)
}

/// `guard health` reads the readiness body on stdin; `guard rust-start` checks
/// the prepared identity files. Both need every option (a missing one is a
/// usage refusal) and print only the fixed refusal text, never file contents.
/// The deploy-start guard is the shell helper, which runs before any binary.
fn run_guard(args: &GuardArgs) -> Exit {
    let outcome = match args.kind {
        GuardKind::Health => match (&args.environment, &args.release_sha) {
            (Some(environment), Some(release_sha)) => {
                guard::health(Some(environment), Some(release_sha), &mut std::io::stdin())
            }
            _ => return Exit::Usage,
        },
        GuardKind::RustStart => match (
            &args.expected_release_sha,
            &args.budget_file,
            &args.expected_revision,
            &args.revision_file,
        ) {
            (Some(release_sha), Some(budget), Some(revision), Some(record)) => guard::rust_start(
                Some(release_sha),
                Some(budget),
                Some(revision),
                Some(record),
            ),
            _ => return Exit::Usage,
        },
        GuardKind::DeployStart => {
            report("guard", "not_available");
            return Exit::Precondition;
        }
    };
    match outcome {
        Ok(()) => Exit::Ok,
        Err(reason) => {
            eprintln!(
                "{}",
                serde_json::json!({"command": "guard", "outcome": "refused", "reason": reason})
            );
            Exit::Precondition
        }
    }
}

/// Report an `ops` projection's result and map it to the exit code.
fn ops_outcome(command: &str, result: Result<(), (Exit, &'static str)>) -> Exit {
    match result {
        Ok(()) => {
            report(command, "ok");
            Exit::Ok
        }
        Err((exit, reason)) => {
            report(command, &format!("refused_{reason}"));
            exit
        }
    }
}

fn report(command: &str, outcome: &str) {
    eprintln!(
        "{}",
        serde_json::json!({"command": command, "outcome": outcome, "counts": {}})
    );
}

async fn serve(args: &ServeArgs) -> Result<(), Exit> {
    use crate::runtime::config::{fleet_startup_probe, resolve_fleet_artifact_path};
    use crate::runtime::policy::committed_policy;
    use crate::streams::runtime::{CapacityConfig, start_streams};
    use crate::{AppState, connect, router, validate_config};

    let _ = tracing_subscriber::fmt().json().try_init();
    let mut config = context::load_serve_config(args)?;
    validate_config(&config).map_err(|_| Exit::Precondition)?;
    let working_directory = std::env::current_dir().map_err(|_| Exit::Dependency)?;
    config.fleet_artifact_path =
        resolve_fleet_artifact_path(&config.fleet_artifact_path, &working_directory)
            .map_err(|_| Exit::Precondition)?;
    fleet_startup_probe(&config.fleet_artifact_path).await;
    let pools = connect(&config).await.map_err(|_| Exit::Dependency)?;
    let bind = config.bind;
    let policy = committed_policy().map_err(|_| Exit::Precondition)?;
    let mut state = AppState::new(config, pools, policy).map_err(|_| Exit::Precondition)?;
    log_host_policy(&state.host_policy);
    let streams = start_streams(&mut state, CapacityConfig::default()).await;
    // The trusted watchlist producer runs only when the release activates it.
    let producer = crate::watchlist_producer::start(
        &state,
        streams
            .as_ref()
            .map(crate::streams::runtime::StreamRuntime::producer_wake),
    );
    // Disabled caching starts no task; an enabled cache starts the authority
    // refresh and invalidation loop before the service accepts traffic.
    state.start_trading_cache();
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|_| Exit::Dependency)?;
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        if let Some(producer) = producer {
            producer.shutdown().await;
        }
        if let Some(streams) = streams {
            streams.shutdown().await;
        }
    })
    .await
    .map_err(|_| Exit::Dependency)
}

/// The one-shot watchlist producer bootstrap that `deploy.sh` runs. It prints
/// exactly one line: the outcome with the watermark, or the refusal reason.
/// Exit 0 is created or already present, 1 a producer refusal and 2 an invalid
/// configuration, the codes of the earlier service binary.
async fn bootstrap_producer(args: &[OsString]) -> Exit {
    let arguments: Option<Vec<String>> = args
        .iter()
        .map(|arg| arg.clone().into_string().ok())
        .collect();
    let Some(config) = arguments.and_then(|arguments| context::load_bootstrap_config(&arguments))
    else {
        println!("refused reason=configuration_invalid");
        return Exit::Usage;
    };
    match crate::watchlist_producer::bootstrap_command(&config).await {
        Ok(outcome) => {
            println!("{}", outcome.report());
            Exit::Ok
        }
        Err(reason) => {
            println!("refused reason={reason}");
            Exit::Breached
        }
    }
}

/// Say at startup whether the Host check is on, so a missing `ALLOWED_HOSTS`
/// is visible in the journal instead of a silent default.
fn log_host_policy(policy: &crate::runtime::host::HostPolicy) {
    if policy.is_enforced() {
        tracing::info!(
            allowed_hosts = policy.configured_hosts().join(","),
            "Host validation is enforced; loopback hosts are always allowed"
        );
    } else {
        tracing::warn!("Host validation is off: ALLOWED_HOSTS is not set");
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate()).expect("termination signal handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
