use crate::{account, config, db, migrate, paths, pool, tmux};
use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use std::{
    fs::{self, OpenOptions},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

const DEFAULT_INTERVAL_SECS: u64 = 30;

pub fn start() -> Result<()> {
    paths::ensure_root_dirs()?;
    if is_running()? {
        println!("cxd already appears to be running");
        return Ok(());
    }
    let _ = fs::remove_file(paths::pid_path()?);

    let exe = std::env::current_exe().context("could not determine current executable")?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::log_path()?)?;
    let log_err = log.try_clone()?;

    let child = Command::new(exe)
        .arg("daemon")
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .context("failed to start cxd daemon")?;

    fs::write(paths::pid_path()?, child.id().to_string())?;
    println!("started cxd pid {}", child.id());
    println!("log: {}", paths::log_path()?.display());
    Ok(())
}

pub fn stop() -> Result<()> {
    let pid = read_pid()?;
    if !is_cxd_process(pid)? {
        anyhow::bail!(
            "refusing to stop pid {}; pidfile does not point to a `cx daemon run` process",
            pid
        );
    }
    let status = Command::new("kill")
        .arg(pid.to_string())
        .status()
        .context("failed to run kill")?;
    if status.success() {
        let _ = fs::remove_file(paths::pid_path()?);
        println!("stopped cxd pid {}", pid);
    } else {
        anyhow::bail!("failed to stop cxd pid {}", pid);
    }
    Ok(())
}

pub fn status() -> Result<()> {
    if is_running()? {
        println!("cxd running pid {}", read_pid()?);
    } else {
        println!("cxd not running");
    }
    Ok(())
}

pub fn run_forever(interval_secs: Option<u64>) -> Result<()> {
    let interval = Duration::from_secs(interval_secs.unwrap_or(DEFAULT_INTERVAL_SECS));
    let conn = db::connect()?;
    db::log_event(&conn, "daemon.start", None, "cxd started")?;
    eprintln!("cxd started; interval={}s", interval.as_secs());

    loop {
        if let Err(err) = tick(&conn) {
            let _ = db::log_event(&conn, "daemon.error", None, &err.to_string());
            eprintln!("cxd tick error: {err:#}");
        }
        thread::sleep(interval);
    }
}

fn tick(conn: &Connection) -> Result<()> {
    let cfg = config::load()?;
    refresh_local_account_status(conn)?;
    supervise_sessions(conn, &cfg)?;
    Ok(())
}

fn refresh_local_account_status(conn: &Connection) -> Result<()> {
    let accounts = db::list_accounts(conn)?;
    for acc in accounts {
        let _ = account::local_check(conn, &acc.name);
    }
    Ok(())
}

fn supervise_sessions(conn: &Connection, cfg: &config::Config) -> Result<()> {
    let sessions = db::list_sessions(conn)?;
    for session in sessions {
        let account = match db::get_account(conn, &session.current_account)? {
            Some(account) => account,
            None => continue,
        };

        let pane_exists = match session.tmux_pane.as_deref() {
            Some(pane) => tmux::pane_exists(pane).unwrap_or(false),
            None => false,
        };

        if !pane_exists {
            db::set_session_status(conn, &session.name, "dead")?;
            db::log_event(
                conn,
                "session.dead",
                Some(&session.name),
                "tmux pane missing",
            )?;
            let _ = migrate::restart(conn, &session.name);
            continue;
        }

        if should_auto_migrate_account(&account.status, account.disabled, cfg) {
            if let Some(pool_name) = &session.pool {
                match pool::choose(conn, None, Some(pool_name), Some(&session.current_account)) {
                    Ok(target) => {
                        db::log_event(
                            conn,
                            "session.auto_migrate",
                            Some(&session.name),
                            &format!(
                                "{} -> {} because account status is {}",
                                session.current_account, target.name, account.status
                            ),
                        )?;
                        let _ = migrate::migrate_to_account(conn, &session, &target.name);
                    }
                    Err(err) => {
                        db::set_session_status(conn, &session.name, "paused")?;
                        db::log_event(
                            conn,
                            "session.paused",
                            Some(&session.name),
                            &format!("no migration target: {err}"),
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn should_auto_migrate_account(status: &str, disabled: bool, cfg: &config::Config) -> bool {
    if disabled || status == "disabled" {
        return true;
    }
    match status {
        "auth_failed" => cfg.daemon_auto_migrate_auth_failed(),
        "limited" => cfg.daemon_auto_migrate_limited(),
        "degraded" => cfg.daemon_auto_migrate_degraded(),
        _ => false,
    }
}

fn read_pid() -> Result<u32> {
    let text = fs::read_to_string(paths::pid_path()?).context("cxd pid file not found")?;
    text.trim().parse::<u32>().map_err(Into::into)
}

pub fn running_pid() -> Result<Option<u32>> {
    if is_running()? {
        Ok(Some(read_pid()?))
    } else {
        Ok(None)
    }
}

fn is_running() -> Result<bool> {
    let pid = match read_pid() {
        Ok(pid) => pid,
        Err(_) => return Ok(false),
    };
    match process_exists(pid) {
        Ok(false) => {
            let _ = fs::remove_file(paths::pid_path()?);
            Ok(false)
        }
        Ok(true) => {
            if is_cxd_process(pid)? {
                Ok(true)
            } else {
                let _ = fs::remove_file(paths::pid_path()?);
                Ok(false)
            }
        }
        Err(_) => Err(anyhow!("failed to run kill -0")),
    }
}

fn process_exists(pid: u32) -> Result<bool> {
    let status = Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .context("failed to run kill -0")?;
    Ok(status.success())
}

fn is_cxd_process(pid: u32) -> Result<bool> {
    if !process_exists(pid)? {
        return Ok(false);
    }

    let output = Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("command=")
        .output()
        .context("failed to inspect pid command with ps")?;
    if !output.status.success() {
        return Ok(false);
    }

    let command = String::from_utf8_lossy(&output.stdout);
    Ok(command.contains(" daemon run") && command.contains("cx"))
}
