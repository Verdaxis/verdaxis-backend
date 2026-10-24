//! Typed command boundary. Execution is registered by each command's owning task.
pub mod context;
pub mod gate;
pub mod mail;
pub mod state;

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Bounded attempts for one dependency call: the first try plus two retries.
pub const RETRY_ATTEMPTS: u32 = 3;
/// Delay before each retry, in seconds. The third attempt starts at 4 s elapsed.
pub const RETRY_DELAYS_SECONDS: [u64; 2] = [1, 3];
/// Total deadline the delays fit inside.
pub const RETRY_TIMEOUT: Duration = Duration::from_secs(10);

/// Run one fallible dependency call with the job retry policy.
///
/// The helper retries only calls the caller marks retryable. Ambiguous sends,
/// committed claims and business transaction conflicts have their own owners
/// and must not be wrapped here.
pub async fn retry_bounded<T, E, F, Fut>(mut operation: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let started = tokio::time::Instant::now();
    let mut attempt = 0_u32;
    loop {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let Some(delay) = RETRY_DELAYS_SECONDS
                    .get(attempt as usize)
                    .map(|seconds| Duration::from_secs(*seconds))
                else {
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
    #[arg(long, value_parser = ["all"], conflicts_with = "event")]
    pub remind: Option<String>,
    #[arg(long)]
    pub telegram_disabled: bool,
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
    Preflight(IdentityArgs),
    PrepareBudget(IdentityArgs),
    PrepareRuntime(IdentityArgs),
    Inspect(InspectArgs),
    Ingest(IngestArgs),
    Import(ImportArgs),
    CacheAuthority(CacheArgs),
    Fleet(IdentityArgs),
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
    #[command(flatten)]
    pub input: FileArgs,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ImportArgs {
    pub kind: ImportKind,
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

fn report(command: &str, outcome: &str) {
    eprintln!(
        "{}",
        serde_json::json!({"command": command, "outcome": outcome, "counts": {}})
    );
}

async fn serve(args: &ServeArgs) -> Result<(), Exit> {
    use crate::runtime::policy::committed_policy;
    use crate::{AppState, connect, router};

    let _ = tracing_subscriber::fmt().json().try_init();
    let config = context::load_serve_config(args)?;
    let pools = connect(&config).await.map_err(|_| Exit::Dependency)?;
    let bind = config.bind;
    let policy = committed_policy().map_err(|_| Exit::Precondition)?;
    let state = AppState::new(config, pools, policy).map_err(|_| Exit::Precondition)?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|_| Exit::Dependency)?;
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|_| Exit::Dependency)
}
