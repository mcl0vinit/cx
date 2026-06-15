use crate::{codex, config, daemon, db, limits, paths, util};
use anyhow::Result;
use rusqlite::Connection;
use std::{path::Path, process::Command};

pub fn run(conn: &Connection) -> Result<()> {
    let mut report = Report::default();
    let cfg = match config::load() {
        Ok(cfg) => {
            report.ok("config", &format!("{}", paths::config_path()?.display()));
            cfg
        }
        Err(err) => {
            report.fail("config", &err.to_string());
            config::Config::default()
        }
    };

    check_paths(&mut report)?;
    check_codex(&mut report);
    check_default_pool(conn, &cfg, &mut report)?;
    check_accounts(conn, &cfg, &mut report)?;
    check_tmux(&mut report);
    check_daemon(&mut report);
    check_git_hygiene(&mut report);

    report.finish();
    Ok(())
}

fn check_paths(report: &mut Report) -> Result<()> {
    let root = paths::cx_root()?;
    let db_path = paths::db_path()?;
    if root.exists() {
        report.ok("cx home", &util::display_path(&root));
    } else {
        report.warn("cx home", &format!("missing {}", root.display()));
    }
    if db_path.exists() {
        report.ok("registry", &util::display_path(&db_path));
    } else {
        report.warn("registry", &format!("missing {}", db_path.display()));
    }
    Ok(())
}

fn check_codex(report: &mut Report) {
    let bin = codex::bin_path();
    let output = Command::new(&bin).arg("--version").output();
    match output {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            report.ok("codex", &format!("{} ({})", bin.display(), version.trim()));
        }
        Ok(output) => report.fail(
            "codex",
            &format!(
                "{} returned {}: {}",
                bin.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ),
        Err(err) => report.fail("codex", &format!("{}: {err}", bin.display())),
    }
}

fn check_default_pool(conn: &Connection, cfg: &config::Config, report: &mut Report) -> Result<()> {
    match cfg.default_pool.as_deref() {
        Some(pool) if db::get_pool(conn, pool)?.is_some() => report.ok("default pool", pool),
        Some(pool) => report.warn("default pool", &format!("{} is not registered", pool)),
        None => report.warn("default pool", "not configured"),
    }
    Ok(())
}

fn check_accounts(conn: &Connection, cfg: &config::Config, report: &mut Report) -> Result<()> {
    let accounts = db::list_accounts(conn)?;
    if accounts.is_empty() {
        report.warn("accounts", "none registered");
        return Ok(());
    }

    for account in accounts {
        let label = format!("account {}", account.name);
        if account.disabled {
            report.warn(&label, "disabled");
            continue;
        }
        if !account.codex_home.exists() {
            report.fail(&label, "CODEX_HOME missing");
            continue;
        }
        if !account.codex_home.join("auth.json").exists() {
            report.fail(&label, "auth.json missing");
            continue;
        }

        let snapshot = limits::latest_snapshot(&account.codex_home)?;
        if limits::is_stale(snapshot.as_ref(), cfg.limit_snapshot_max_age_minutes()) {
            report.warn(
                &label,
                &format!(
                    "auth ok, limit snapshot stale/missing (status {})",
                    account.status
                ),
            );
        } else {
            report.ok(&label, &format!("auth ok, status {}", account.status));
        }
    }
    Ok(())
}

fn check_tmux(report: &mut Report) {
    match Command::new("tmux").arg("-V").output() {
        Ok(output) if output.status.success() => {
            report.ok("tmux", String::from_utf8_lossy(&output.stdout).trim())
        }
        Ok(_) => report.warn("tmux", "installed but version check failed"),
        Err(_) => report.warn("tmux", "not found; normal non-tmux cx usage still works"),
    }
}

fn check_daemon(report: &mut Report) {
    match daemon::running_pid() {
        Ok(Some(pid)) => report.ok("daemon", &format!("running pid {pid}")),
        Ok(None) => report.warn("daemon", "not running"),
        Err(err) => report.warn("daemon", &err.to_string()),
    }
}

fn check_git_hygiene(report: &mut Report) {
    if !Path::new(".git").exists() {
        return;
    }

    let gitignore = std::fs::read_to_string(".gitignore").unwrap_or_default();
    for pattern in ["docs/", "/target", "*.sqlite"] {
        if gitignore.lines().any(|line| line.trim() == pattern) {
            report.ok("gitignore", pattern);
        } else {
            report.warn("gitignore", &format!("missing {}", pattern));
        }
    }

    let output = Command::new("git").arg("ls-files").output();
    let Ok(output) = output else {
        return;
    };
    let tracked = String::from_utf8_lossy(&output.stdout);
    let sensitive = tracked
        .lines()
        .filter(|path| {
            path.ends_with("auth.json")
                || path.ends_with(".sqlite")
                || path.contains("/sessions/")
                || path.starts_with("docs/")
        })
        .collect::<Vec<_>>();
    if sensitive.is_empty() {
        report.ok("tracked secrets", "none found");
    } else {
        report.fail("tracked secrets", &sensitive.join(", "));
    }
}

#[derive(Default)]
struct Report {
    ok: usize,
    warn: usize,
    fail: usize,
}

impl Report {
    fn ok(&mut self, label: &str, detail: &str) {
        self.ok += 1;
        println!("{:<6} {:<18} {}", "OK", label, detail);
    }

    fn warn(&mut self, label: &str, detail: &str) {
        self.warn += 1;
        println!("{:<6} {:<18} {}", "WARN", label, detail);
    }

    fn fail(&mut self, label: &str, detail: &str) {
        self.fail += 1;
        println!("{:<6} {:<18} {}", "FAIL", label, detail);
    }

    fn finish(&self) {
        println!();
        println!(
            "{} ok, {} warning, {} failed",
            self.ok, self.warn, self.fail
        );
    }
}
