mod account;
mod codex;
mod daemon;
mod db;
mod migrate;
mod paths;
mod pool;
mod tmux;
mod util;

use anyhow::{anyhow, Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use rusqlite::Connection;
use std::{path::PathBuf, process::{Command, Stdio}};

#[derive(Parser, Debug)]
#[command(name = "cx")]
#[command(about = "tmux-first Codex account/session supervisor")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Account { #[command(subcommand)] command: AccountCommand },
    Pool { #[command(subcommand)] command: PoolCommand },
    Run {
        #[arg(short, long)] account: Option<String>,
        #[arg(short, long)] pool: Option<String>,
        #[arg(short = 'C', long)] cwd: Option<PathBuf>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)] args: Vec<String>,
    },
    Tmux { #[command(subcommand)] command: TmuxCommand },
    Migrate { name: String, #[arg(short, long)] account: Option<String>, #[arg(short, long)] pool: Option<String> },
    Restart { name: String },
    Status,
    Daemon { #[command(subcommand)] command: DaemonCommand },
}

#[derive(Subcommand, Debug)]
enum AccountCommand {
    Add { name: String },
    Login { name: String },
    Logout { name: String },
    List,
    Check {
        name: Option<String>,
        #[arg(long)] all: bool,
        #[arg(long, help = "Run `codex exec` as a real online health check. This may consume usage.")] online: bool,
    },
    Disable { name: String, #[arg(long)] reason: Option<String> },
    Enable { name: String },
}

#[derive(Subcommand, Debug)]
enum PoolCommand {
    Create { name: String, #[arg(long)] accounts: String, #[arg(long, default_value = "least-sessions")] strategy: String },
    List,
}

#[derive(Subcommand, Debug)]
enum TmuxCommand {
    Run {
        #[arg(short, long)] account: Option<String>,
        #[arg(short, long)] pool: Option<String>,
        #[arg(long)] name: String,
        #[arg(short = 'C', long)] cwd: Option<PathBuf>,
        #[arg(long, help = "Prompt cx should use when migrating across account homes.")] resume_prompt: Option<String>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)] args: Vec<String>,
    },
    List,
    Restart { name: String },
}

#[derive(Subcommand, Debug)]
enum DaemonCommand {
    Start,
    Stop,
    Status,
    Run { #[arg(long)] interval_secs: Option<u64> },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::try_init().ok();

    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.len() >= 2 && is_account_shorthand(&raw_args[1]) {
        let conn = db::connect()?;
        return run_account_shorthand(&conn, &raw_args[1], &raw_args[2..]);
    }

    let cli = Cli::parse();
    let conn = db::connect()?;

    match cli.command {
        Some(Commands::Account { command }) => handle_account(&conn, command),
        Some(Commands::Pool { command }) => handle_pool(&conn, command),
        Some(Commands::Run { account, pool, cwd, args }) => handle_run(&conn, account.as_deref(), pool.as_deref(), cwd, args),
        Some(Commands::Tmux { command }) => handle_tmux(&conn, command),
        Some(Commands::Migrate { name, account, pool }) => migrate::migrate(&conn, &name, account.as_deref(), pool.as_deref()),
        Some(Commands::Restart { name }) => migrate::restart(&conn, &name),
        Some(Commands::Status) => handle_status(&conn),
        Some(Commands::Daemon { command }) => handle_daemon(command),
        None => {
            println!("{}", Cli::command().render_help());
            Ok(())
        }
    }
}

fn handle_account(conn: &Connection, command: AccountCommand) -> Result<()> {
    match command {
        AccountCommand::Add { name } => account::add(conn, &name),
        AccountCommand::Login { name } => account::login(conn, &name),
        AccountCommand::Logout { name } => account::logout(conn, &name),
        AccountCommand::List => account::list(conn),
        AccountCommand::Check { name, all, online } => {
            if all {
                for acc in db::list_accounts(conn)? {
                    let status = account::check(conn, &acc.name, online)?;
                    println!("{:<20} {}", acc.name, status);
                }
                Ok(())
            } else {
                let name = name.ok_or_else(|| anyhow!("provide an account name or --all"))?;
                let status = account::check(conn, &name, online)?;
                println!("{:<20} {}", name, status);
                Ok(())
            }
        }
        AccountCommand::Disable { name, reason } => account::disable(conn, &name, reason.as_deref()),
        AccountCommand::Enable { name } => account::enable(conn, &name),
    }
}

fn handle_pool(conn: &Connection, command: PoolCommand) -> Result<()> {
    match command {
        PoolCommand::Create { name, accounts, strategy } => pool::create(conn, &name, &accounts, &strategy),
        PoolCommand::List => pool::list(conn),
    }
}

fn handle_run(conn: &Connection, account: Option<&str>, pool_name: Option<&str>, cwd: Option<PathBuf>, args: Vec<String>) -> Result<()> {
    let chosen = pool::choose(conn, account, pool_name, None)?;
    let args = util::normalize_passthrough(args);
    run_codex_direct(&chosen.codex_home, cwd, &args)
}

fn handle_tmux(conn: &Connection, command: TmuxCommand) -> Result<()> {
    match command {
        TmuxCommand::Run { account, pool: pool_name, name, cwd, resume_prompt, args } => {
            let chosen = pool::choose(conn, account.as_deref(), pool_name.as_deref(), None)?;
            let cwd = cwd.unwrap_or(std::env::current_dir()?);
            let args = util::normalize_passthrough(args);
            let shell_command = codex::shell_command(&chosen.codex_home, &args);
            let target = tmux::new_window(&name, &cwd, &shell_command)?;

            db::insert_session(conn, db::NewSession {
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
            })?;

            db::log_event(conn, "session.start", Some(&name), &format!("started in {} using account `{}`", target.pane_id, chosen.name))?;
            println!("started `{}` in pane {} using account `{}`", name, target.pane_id, chosen.name);
            Ok(())
        }
        TmuxCommand::List => migrate::print_sessions(conn),
        TmuxCommand::Restart { name } => migrate::restart(conn, &name),
    }
}

fn handle_status(conn: &Connection) -> Result<()> {
    println!("Accounts");
    account::list(conn)?;
    println!();
    println!("Sessions");
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

fn is_account_shorthand(name: &str) -> bool {
    if matches!(name, "account" | "pool" | "run" | "tmux" | "migrate" | "restart" | "status" | "daemon" | "help" | "--help" | "-h" | "--version" | "-V") {
        return false;
    }
    paths::account_home(name).map(|path| path.exists()).unwrap_or(false)
}

fn run_account_shorthand(conn: &Connection, account_name: &str, rest: &[String]) -> Result<()> {
    let account = pool::choose(conn, Some(account_name), None, None)?;
    run_codex_direct(&account.codex_home, None, rest)
}

fn run_codex_direct(account_home: &std::path::Path, cwd: Option<PathBuf>, args: &[String]) -> Result<()> {
    let mut cmd = Command::new("codex");
    cmd.env("CODEX_HOME", account_home)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if let Some(cwd) = cwd { cmd.current_dir(cwd); }

    let status = cmd.status().context("failed to run codex; is it installed and on PATH?")?;
    std::process::exit(status.code().unwrap_or(1));
}
