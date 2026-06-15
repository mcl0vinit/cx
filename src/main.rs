mod account;
mod codex;
mod config;
mod daemon;
mod dashboard;
mod db;
mod doctor;
mod limits;
mod migrate;
mod paths;
mod pool;
mod resume;
mod tmux;
mod ui;
mod util;

use anyhow::{anyhow, Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use rusqlite::Connection;
use std::{path::PathBuf, process::Stdio};

#[derive(Parser, Debug)]
#[command(name = "cx")]
#[command(about = "Codex account/session router with optional tmux supervision")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(about = "Manage Codex account profiles")]
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    #[command(about = "Manage account routing pools")]
    Pool {
        #[command(subcommand)]
        command: PoolCommand,
    },
    #[command(about = "Run Codex under a selected account or pool")]
    Run {
        #[arg(short, long, help = "Account profile to use")]
        account: Option<String>,
        #[arg(short, long, help = "Pool to route through")]
        pool: Option<String>,
        #[arg(short = 'C', long, help = "Working directory for Codex")]
        cwd: Option<PathBuf>,
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            help = "Arguments passed through to Codex"
        )]
        args: Vec<String>,
    },
    #[command(about = "Run Codex using limit-aware account selection")]
    Smart {
        #[arg(short, long, help = "Pool to route through")]
        pool: Option<String>,
        #[arg(
            long,
            help = "Refresh stale/missing limit snapshots before picking. This may consume usage."
        )]
        refresh: bool,
        #[arg(short = 'C', long, help = "Working directory for Codex")]
        cwd: Option<PathBuf>,
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            help = "Arguments passed through to Codex"
        )]
        args: Vec<String>,
    },
    #[command(about = "Inspect or initialize cx configuration")]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    #[command(about = "Generate shell completions")]
    Completion {
        #[arg(help = "Shell to generate completions for")]
        shell: Shell,
    },
    #[command(about = "Run and supervise Codex inside tmux")]
    Tmux {
        #[command(subcommand)]
        command: TmuxCommand,
    },
    #[command(about = "Move a managed tmux session to another account", hide = true)]
    Migrate {
        #[arg(help = "Managed session name")]
        name: String,
        #[arg(short, long, help = "Target account")]
        account: Option<String>,
        #[arg(short, long, help = "Target pool")]
        pool: Option<String>,
    },
    #[command(about = "Restart a managed tmux session", hide = true)]
    Restart {
        #[arg(help = "Managed session name")]
        name: String,
    },
    #[command(about = "Copy a Codex session into an account home")]
    Adopt {
        #[arg(help = "Session id or selector")]
        session: String,
        #[arg(short, long, help = "Target account")]
        account: String,
    },
    #[command(about = "Resume Codex after searching all known homes")]
    Resume {
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            help = "Arguments passed through after `codex resume`"
        )]
        args: Vec<String>,
    },
    #[command(about = "Resume the latest session for the current repo")]
    ResumeHere {
        #[arg(short, long, help = "Account to resume under")]
        account: Option<String>,
        #[arg(short, long, help = "Pool to route through")]
        pool: Option<String>,
        #[arg(long, help = "Choose the target account using limit-aware routing.")]
        smart: bool,
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            help = "Arguments passed through after `codex resume`"
        )]
        args: Vec<String>,
    },
    #[command(about = "List recent Codex sessions across known homes")]
    Sessions {
        #[arg(long, default_value_t = 20, help = "Maximum sessions to show")]
        limit: usize,
    },
    #[command(about = "Refresh account health and local limit snapshots")]
    Refresh {
        #[arg(help = "Account name to refresh")]
        name: Option<String>,
        #[arg(long, help = "Refresh every registered account")]
        all: bool,
        #[arg(short, long, help = "Refresh accounts in this pool")]
        pool: Option<String>,
        #[arg(long, help = "Only refresh accounts with stale or missing snapshots.")]
        stale: bool,
    },
    #[command(about = "Show account capacity and managed-session activity")]
    Watch {
        #[arg(
            long,
            default_value_t = 5,
            help = "Seconds between dashboard refreshes"
        )]
        interval_secs: u64,
        #[arg(long, help = "Print one dashboard frame and exit.")]
        once: bool,
    },
    #[command(about = "Run diagnostics for local cx setup")]
    Doctor,
    #[command(about = "Show account and managed-session overview")]
    Status,
    #[command(about = "Manage the background cx daemon")]
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
}

#[derive(Subcommand, Debug)]
enum AccountCommand {
    #[command(about = "Register an account profile")]
    Add {
        #[arg(help = "Account name")]
        name: String,
        #[arg(long, help = "Existing CODEX_HOME path to register")]
        codex_home: Option<PathBuf>,
    },
    #[command(about = "Log in to Codex for an account")]
    Login {
        #[arg(help = "Account name")]
        name: String,
    },
    #[command(about = "Log out of Codex for an account")]
    Logout {
        #[arg(help = "Account name")]
        name: String,
    },
    #[command(about = "List registered account profiles")]
    List,
    #[command(about = "Check account authentication status")]
    Check {
        #[arg(help = "Account name")]
        name: Option<String>,
        #[arg(long, help = "Check every registered account")]
        all: bool,
        #[arg(
            long,
            help = "Run `codex exec` as a real online health check. This may consume usage."
        )]
        online: bool,
    },
    #[command(about = "Show account capacity, identity, and session counts")]
    Status {
        #[arg(help = "Account name; omit to show every account")]
        name: Option<String>,
        #[arg(
            long,
            help = "Refresh limits by running `codex exec` first. This may consume usage."
        )]
        online: bool,
    },
    #[command(about = "Disable an account for routing")]
    Disable {
        #[arg(help = "Account name")]
        name: String,
        #[arg(long, help = "Reason recorded in account status")]
        reason: Option<String>,
    },
    #[command(about = "Re-enable an account for routing")]
    Enable {
        #[arg(help = "Account name")]
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum PoolCommand {
    #[command(about = "Create or update a routing pool")]
    Create {
        #[arg(help = "Pool name")]
        name: String,
        #[arg(long, help = "Comma-separated account names in priority order")]
        accounts: String,
        #[arg(
            long,
            help = "Routing strategy: first-healthy, least-sessions, or limit-aware"
        )]
        strategy: Option<String>,
    },
    #[command(about = "List routing pools and account capacity")]
    List,
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    #[command(about = "Create the default config file")]
    Init {
        #[arg(long, help = "Overwrite an existing config file")]
        force: bool,
    },
    #[command(about = "Print the config file path")]
    Path,
    #[command(about = "Print the active config")]
    Show,
}

#[derive(Subcommand, Debug)]
enum TmuxCommand {
    #[command(about = "Start a managed Codex tmux session")]
    Run {
        #[arg(short, long, help = "Account profile to use")]
        account: Option<String>,
        #[arg(short, long, help = "Pool to route through")]
        pool: Option<String>,
        #[arg(long, help = "Managed session name")]
        name: String,
        #[arg(short = 'C', long, help = "Working directory for Codex")]
        cwd: Option<PathBuf>,
        #[arg(
            long,
            help = "Prompt cx should use when migrating across account homes."
        )]
        resume_prompt: Option<String>,
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            help = "Arguments passed through to Codex"
        )]
        args: Vec<String>,
    },
    #[command(about = "List managed tmux sessions")]
    List,
    #[command(about = "Restart a managed tmux session")]
    Restart {
        #[arg(help = "Managed session name")]
        name: String,
    },
    #[command(about = "Move a managed tmux session to another account")]
    Migrate {
        #[arg(help = "Managed session name")]
        name: String,
        #[arg(short, long, help = "Target account")]
        account: Option<String>,
        #[arg(short, long, help = "Target pool")]
        pool: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum DaemonCommand {
    #[command(about = "Start the background daemon")]
    Start,
    #[command(about = "Stop the background daemon")]
    Stop,
    #[command(about = "Show daemon status")]
    Status,
    #[command(about = "Run the daemon in the foreground")]
    Run {
        #[arg(long, help = "Seconds between daemon ticks")]
        interval_secs: Option<u64>,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::try_init().ok();

    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.len() >= 2 && !is_reserved_command(&raw_args[1]) {
        let conn = db::connect()?;
        if is_account_shorthand(&conn, &raw_args[1])? {
            return run_account_shorthand(&conn, &raw_args[1], &raw_args[2..]);
        }
    }

    let cli = Cli::parse();
    let conn = db::connect()?;

    match cli.command {
        Some(Commands::Account { command }) => handle_account(&conn, command),
        Some(Commands::Pool { command }) => handle_pool(&conn, command),
        Some(Commands::Run {
            account,
            pool,
            cwd,
            args,
        }) => handle_run(&conn, account.as_deref(), pool.as_deref(), cwd, args),
        Some(Commands::Smart {
            pool,
            refresh,
            cwd,
            args,
        }) => handle_smart(&conn, pool.as_deref(), refresh, cwd, args),
        Some(Commands::Config { command }) => handle_config(command),
        Some(Commands::Completion { shell }) => handle_completion(shell),
        Some(Commands::Refresh {
            name,
            all,
            pool,
            stale,
        }) => handle_refresh(&conn, name.as_deref(), all, pool.as_deref(), stale),
        Some(Commands::Tmux { command }) => handle_tmux(&conn, command),
        Some(Commands::Migrate {
            name,
            account,
            pool,
        }) => migrate::migrate(&conn, &name, account.as_deref(), pool.as_deref()),
        Some(Commands::Restart { name }) => migrate::restart(&conn, &name),
        Some(Commands::Adopt { session, account }) => handle_adopt(&conn, &account, &session),
        Some(Commands::Resume { args }) => handle_resume(&conn, args),
        Some(Commands::ResumeHere {
            account,
            pool,
            smart,
            args,
        }) => handle_resume_here(&conn, account.as_deref(), pool.as_deref(), smart, args),
        Some(Commands::Sessions { limit }) => resume::print_sessions(&conn, limit),
        Some(Commands::Watch {
            interval_secs,
            once,
        }) => dashboard::watch(&conn, interval_secs, once),
        Some(Commands::Doctor) => doctor::run(&conn),
        Some(Commands::Status) => handle_status(&conn),
        Some(Commands::Daemon { command }) => handle_daemon(command),
        None => {
            println!("{}", Cli::command().render_help());
            Ok(())
        }
    }
}

fn handle_completion(shell: Shell) -> Result<()> {
    let mut command = Cli::command();
    let name = command.get_name().to_string();
    clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
    Ok(())
}

fn handle_account(conn: &Connection, command: AccountCommand) -> Result<()> {
    match command {
        AccountCommand::Add { name, codex_home } => account::add(conn, &name, codex_home),
        AccountCommand::Login { name } => account::login(conn, &name),
        AccountCommand::Logout { name } => account::logout(conn, &name),
        AccountCommand::List => account::list(conn),
        AccountCommand::Check { name, all, online } => {
            if all {
                let mut rows = Vec::new();
                for acc in db::list_accounts(conn)? {
                    let status = account::check(conn, &acc.name, online)?;
                    rows.push(vec![acc.name, status]);
                }
                println!("{}", ui::heading("Account Checks"));
                ui::print_table(&["ACCOUNT", "STATUS"], &rows, &[]);
                Ok(())
            } else {
                let name = name.ok_or_else(|| anyhow!("provide an account name or --all"))?;
                let status = account::check(conn, &name, online)?;
                println!("{:<20} {}", name, status);
                Ok(())
            }
        }
        AccountCommand::Status { name, online } => match name {
            Some(name) => account::status(conn, &name, online),
            None => account::status_all(conn, online),
        },
        AccountCommand::Disable { name, reason } => {
            account::disable(conn, &name, reason.as_deref())
        }
        AccountCommand::Enable { name } => account::enable(conn, &name),
    }
}

fn handle_pool(conn: &Connection, command: PoolCommand) -> Result<()> {
    match command {
        PoolCommand::Create {
            name,
            accounts,
            strategy,
        } => {
            let strategy = strategy.unwrap_or(pool::default_strategy()?);
            pool::create(conn, &name, &accounts, &strategy)
        }
        PoolCommand::List => pool::list(conn),
    }
}

fn handle_run(
    conn: &Connection,
    account: Option<&str>,
    pool_name: Option<&str>,
    cwd: Option<PathBuf>,
    args: Vec<String>,
) -> Result<()> {
    let chosen = pool::choose(conn, account, pool_name, None)?;
    let args = util::normalize_passthrough(args);
    if let Some(resolved) =
        resume::resolve_account_invocation(conn, &chosen.name, &chosen.codex_home, &args)?
    {
        return run_codex_direct(&resolved.home, cwd.or(resolved.cwd), &resolved.args);
    }
    run_codex_direct(&chosen.codex_home, cwd, &args)
}

fn handle_smart(
    conn: &Connection,
    pool_name: Option<&str>,
    refresh: bool,
    cwd: Option<PathBuf>,
    args: Vec<String>,
) -> Result<()> {
    if refresh || config::load()?.smart_refresh_before_pick() {
        refresh_targets(conn, None, false, pool_name, true)?;
    }
    let chosen = pool::choose_smart(conn, pool_name, None)?;
    let args = util::normalize_passthrough(args);
    eprintln!("using account `{}`", chosen.name);
    if let Some(resolved) =
        resume::resolve_account_invocation(conn, &chosen.name, &chosen.codex_home, &args)?
    {
        return run_codex_direct(&resolved.home, cwd.or(resolved.cwd), &resolved.args);
    }
    run_codex_direct(&chosen.codex_home, cwd, &args)
}

fn handle_config(command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Init { force } => config::init(force),
        ConfigCommand::Path => config::print_path(),
        ConfigCommand::Show => config::print_config(),
    }
}

fn handle_refresh(
    conn: &Connection,
    name: Option<&str>,
    all: bool,
    pool_name: Option<&str>,
    stale_only: bool,
) -> Result<()> {
    refresh_targets(conn, name, all, pool_name, stale_only)
}

fn refresh_targets(
    conn: &Connection,
    name: Option<&str>,
    all: bool,
    pool_name: Option<&str>,
    stale_only: bool,
) -> Result<()> {
    let names = refresh_names(conn, name, all, pool_name)?;
    account::refresh(conn, &names, stale_only)
}

fn refresh_names(
    conn: &Connection,
    name: Option<&str>,
    all: bool,
    pool_name: Option<&str>,
) -> Result<Vec<String>> {
    if name.is_some() && (all || pool_name.is_some()) {
        anyhow::bail!("use a name, --all, or --pool, not multiple targets");
    }
    if all && pool_name.is_some() {
        anyhow::bail!("use --all or --pool, not both");
    }

    if let Some(name) = name {
        db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
        return Ok(vec![name.to_string()]);
    }
    if all {
        return Ok(db::list_accounts(conn)?
            .into_iter()
            .map(|account| account.name)
            .collect());
    }

    if let Some(pool_name) = pool::configured_pool_name(pool_name)? {
        db::get_pool(conn, &pool_name)?.ok_or_else(|| anyhow!("unknown pool `{}`", pool_name))?;
        return db::get_pool_accounts(conn, &pool_name);
    }

    Ok(db::list_accounts(conn)?
        .into_iter()
        .map(|account| account.name)
        .collect())
}

fn handle_adopt(conn: &Connection, account_name: &str, session: &str) -> Result<()> {
    let account = pool::choose(conn, Some(account_name), None, None)?;
    let result = resume::adopt_session(conn, &account, session)?;
    if result.already_adopted {
        println!(
            "session `{}` is already adopted in `{}` at {}",
            result.session_id,
            account.name,
            result.target.display()
        );
    } else {
        println!(
            "adopted session `{}` into `{}`\nsource: {}\ntarget: {}",
            result.session_id,
            account.name,
            result.source.display(),
            result.target.display()
        );
    }
    Ok(())
}

fn handle_resume(conn: &Connection, args: Vec<String>) -> Result<()> {
    let mut codex_args = vec!["resume".to_string()];
    codex_args.extend(util::normalize_passthrough(args));
    if let Some(resolved) = resume::resolve_invocation(conn, None, &codex_args)? {
        return run_codex_direct(&resolved.home, resolved.cwd, &resolved.args);
    }
    let codex_home = resume::default_resume_home(conn, None)?;
    run_codex_direct(&codex_home, None, &codex_args)
}

fn handle_resume_here(
    conn: &Connection,
    account: Option<&str>,
    pool_name: Option<&str>,
    smart: bool,
    args: Vec<String>,
) -> Result<()> {
    if smart && account.is_some() {
        anyhow::bail!("use --smart with --pool or no account, not --account");
    }

    let selected = if smart {
        Some(pool::choose_smart(conn, pool_name, None)?)
    } else if account.is_some() || pool_name.is_some() {
        Some(pool::choose(conn, account, pool_name, None)?)
    } else {
        None
    };
    let target = selected
        .as_ref()
        .map(|account| (account.name.as_str(), account.codex_home.as_path()));
    let cwd = std::env::current_dir()?;
    let args = util::normalize_passthrough(args);
    let resolved = resume::resolve_here_invocation(conn, target, &cwd, &args)?;
    run_codex_direct(&resolved.home, resolved.cwd, &resolved.args)
}

fn handle_tmux(conn: &Connection, command: TmuxCommand) -> Result<()> {
    match command {
        TmuxCommand::Run {
            account,
            pool: pool_name,
            name,
            cwd,
            resume_prompt,
            args,
        } => {
            let chosen = pool::choose(conn, account.as_deref(), pool_name.as_deref(), None)?;
            let cwd = cwd.unwrap_or(std::env::current_dir()?);
            let args = util::normalize_passthrough(args);
            if let Some(existing) = db::get_session(conn, &name)? {
                let pane_exists = existing
                    .tmux_pane
                    .as_deref()
                    .map(|pane| tmux::pane_exists(pane).unwrap_or(false))
                    .unwrap_or(false);
                if pane_exists {
                    anyhow::bail!(
                        "session `{}` already exists in pane {}; run `cx tmux restart {}` or choose a new name",
                        name,
                        existing.tmux_pane.unwrap_or_else(|| "-".to_string()),
                        name
                    );
                }
                anyhow::bail!(
                    "session `{}` already exists in the registry; run `cx tmux restart {}` or choose a new name",
                    name,
                    name
                );
            }
            let shell_command = codex::shell_command(&chosen.codex_home, &args);
            let target = tmux::new_window(&name, &cwd, &shell_command)?;

            db::insert_session(
                conn,
                db::NewSession {
                    name: name.clone(),
                    pool: pool_name,
                    current_account: chosen.name.clone(),
                    cwd,
                    tmux_session: Some(target.session_name.clone()),
                    tmux_window: Some(target.window_id.clone()),
                    tmux_pane: Some(target.pane_id.clone()),
                    codex_args: args,
                    resume_prompt,
                    status: "running".to_string(),
                },
            )?;

            db::log_event(
                conn,
                "session.start",
                Some(&name),
                &format!(
                    "started in {} using account `{}`",
                    target.pane_id, chosen.name
                ),
            )?;
            println!(
                "started `{}` in pane {} using account `{}`",
                name, target.pane_id, chosen.name
            );
            Ok(())
        }
        TmuxCommand::List => migrate::print_sessions(conn),
        TmuxCommand::Restart { name } => migrate::restart(conn, &name),
        TmuxCommand::Migrate {
            name,
            account,
            pool,
        } => migrate::migrate(conn, &name, account.as_deref(), pool.as_deref()),
    }
}

fn handle_status(conn: &Connection) -> Result<()> {
    account::status_all(conn, false)?;
    println!();
    migrate::print_sessions(conn)?;
    Ok(())
}

fn handle_daemon(command: DaemonCommand) -> Result<()> {
    match command {
        DaemonCommand::Start => daemon::start(),
        DaemonCommand::Stop => daemon::stop(),
        DaemonCommand::Status => daemon::status(),
        DaemonCommand::Run { interval_secs } => daemon::run_forever(interval_secs),
    }
}

fn is_reserved_command(name: &str) -> bool {
    matches!(
        name,
        "account"
            | "pool"
            | "run"
            | "smart"
            | "config"
            | "completion"
            | "tmux"
            | "migrate"
            | "restart"
            | "adopt"
            | "resume"
            | "resume-here"
            | "sessions"
            | "refresh"
            | "watch"
            | "doctor"
            | "status"
            | "daemon"
            | "help"
            | "--help"
            | "-h"
            | "--version"
            | "-V"
    )
}

fn is_account_shorthand(conn: &Connection, name: &str) -> Result<bool> {
    if db::get_account(conn, name)?.is_some() {
        return Ok(true);
    }
    Ok(paths::account_home(name)
        .map(|path| path.exists())
        .unwrap_or(false))
}

fn run_account_shorthand(conn: &Connection, account_name: &str, rest: &[String]) -> Result<()> {
    let account = pool::choose(conn, Some(account_name), None, None)?;
    if rest.first().map(|arg| arg.as_str()) == Some("adopt") {
        let session = rest
            .get(1)
            .ok_or_else(|| anyhow!("usage: cx {} adopt <session-id>", account_name))?;
        return handle_adopt(conn, account_name, session);
    }
    if rest.first().map(|arg| arg.as_str()) == Some("resume-here") {
        return handle_resume_here(
            conn,
            Some(account_name),
            None,
            false,
            rest.iter().skip(1).cloned().collect(),
        );
    }
    let args = util::normalize_passthrough(rest.to_vec());
    if let Some(resolved) =
        resume::resolve_account_invocation(conn, &account.name, &account.codex_home, &args)?
    {
        return run_codex_direct(&resolved.home, resolved.cwd, &resolved.args);
    }
    run_codex_direct(&account.codex_home, None, &args)
}

fn run_codex_direct(
    account_home: &std::path::Path,
    cwd: Option<PathBuf>,
    args: &[String],
) -> Result<()> {
    let mut cmd = codex::command();
    cmd.env("CODEX_HOME", account_home)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }

    let status = cmd
        .status()
        .context("failed to run Codex; set CX_CODEX_BIN if the launcher is not on PATH")?;
    std::process::exit(status.code().unwrap_or(1));
}
