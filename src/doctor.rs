use crate::{codex, config, daemon, db, limits, maintenance, paths, ui, util};
use anyhow::Result;
use rusqlite::Connection;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn run(conn: &Connection, fix: bool) -> Result<()> {
    if fix {
        let fixes = apply_fixes(conn)?;
        print_fixes(&fixes);
        println!();
    }

    let mut report = Report::default();
    let cfg = match config::load() {
        Ok(cfg) => {
            report.ok("config", &format!("{}", paths::config_path()?.display()));
            cfg
        }
        Err(err) => {
            report.fail_with_hint("config", &err.to_string(), "cx config init");
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

fn apply_fixes(conn: &Connection) -> Result<Vec<FixEntry>> {
    Ok(vec![
        ensure_default_config()?,
        ensure_gitignore()?,
        clean_stale_pidfile()?,
        sync_indexes(conn)?,
    ])
}

fn ensure_default_config() -> Result<FixEntry> {
    paths::ensure_root_dirs()?;
    let path = paths::config_path()?;
    if path.exists() {
        return Ok(FixEntry::skipped("config", "already exists"));
    }

    fs::write(&path, config::DEFAULT_CONFIG)?;
    Ok(FixEntry::applied(
        "config",
        &format!("created {}", util::display_path(&path)),
    ))
}

fn ensure_gitignore() -> Result<FixEntry> {
    if !Path::new(".git").exists() {
        return Ok(FixEntry::skipped("gitignore", "not a git repo"));
    }

    let path = PathBuf::from(".gitignore");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let missing = ["docs/", "/target", "*.sqlite"]
        .into_iter()
        .filter(|pattern| !existing.lines().any(|line| line.trim() == *pattern))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(FixEntry::skipped(
            "gitignore",
            "already includes local artifacts",
        ));
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    for pattern in &missing {
        updated.push_str(pattern);
        updated.push('\n');
    }
    fs::write(&path, updated)?;
    Ok(FixEntry::applied(
        "gitignore",
        &format!("added {}", missing.join(", ")),
    ))
}

fn clean_stale_pidfile() -> Result<FixEntry> {
    if daemon::clean_stale_pidfile()? {
        Ok(FixEntry::applied("daemon", "removed stale pidfile"))
    } else {
        Ok(FixEntry::skipped("daemon", "pidfile absent or active"))
    }
}

fn sync_indexes(conn: &Connection) -> Result<FixEntry> {
    let results = maintenance::sync_indexes(
        conn,
        maintenance::IndexOptions {
            sessions: true,
            limits: true,
            rebuild: false,
        },
    )?;
    let detail = results
        .into_iter()
        .map(|result| format!("{} {}", result.count, result.component))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(FixEntry::applied("indexes", &detail))
}

fn check_paths(report: &mut Report) -> Result<()> {
    let root = paths::cx_root()?;
    let db_path = paths::db_path()?;
    if root.exists() {
        report.ok("cx home", &util::display_path(&root));
    } else {
        report.warn_with_hint(
            "cx home",
            &format!("missing {}", root.display()),
            "run any cx command to initialize it",
        );
    }
    if db_path.exists() {
        report.ok("registry", &util::display_path(&db_path));
    } else {
        report.warn_with_hint(
            "registry",
            &format!("missing {}", db_path.display()),
            "run any cx command to initialize it",
        );
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
        Ok(output) => report.fail_with_hint(
            "codex",
            &format!(
                "{} returned {}: {}",
                bin.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            "set CX_CODEX_BIN or reinstall Codex",
        ),
        Err(err) => report.fail_with_hint(
            "codex",
            &format!("{}: {err}", bin.display()),
            "set CX_CODEX_BIN or install Codex",
        ),
    }
}

fn check_default_pool(conn: &Connection, cfg: &config::Config, report: &mut Report) -> Result<()> {
    match cfg.default_pool.as_deref() {
        Some(pool) if db::get_pool(conn, pool)?.is_some() => report.ok("default pool", pool),
        Some(pool) => report.warn_with_hint(
            "default pool",
            &format!("{} is not registered", pool),
            "create it or update default_pool in config",
        ),
        None => report.warn_with_hint(
            "default pool",
            "not configured",
            "set default_pool in config or pass --pool",
        ),
    }
    Ok(())
}

fn check_accounts(conn: &Connection, cfg: &config::Config, report: &mut Report) -> Result<()> {
    let accounts = db::list_accounts(conn)?;
    if accounts.is_empty() {
        report.warn_with_hint("accounts", "none registered", "cx account add personal");
        return Ok(());
    }

    for account in accounts {
        let label = format!("account {}", account.name);
        if account.disabled {
            report.warn_with_hint(
                &label,
                "disabled",
                &format!("cx account enable {}", account.name),
            );
            continue;
        }
        if !account.codex_home.exists() {
            report.fail_with_hint(
                &label,
                "CODEX_HOME missing",
                &format!("cx account add {} --codex-home <path>", account.name),
            );
            continue;
        }
        if !account.codex_home.join("auth.json").exists() {
            report.fail_with_hint(
                &label,
                "auth.json missing",
                &format!("cx account login {}", account.name),
            );
            continue;
        }

        let snapshot = limits::latest_snapshot_cached(conn, &account.codex_home)?;
        if limits::is_stale(snapshot.as_ref(), cfg.limit_snapshot_max_age_minutes()) {
            report.warn_with_hint(
                &label,
                &format!(
                    "auth ok, limit snapshot stale/missing (status {})",
                    account.status
                ),
                &format!("cx account status {} --online", account.name),
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
        Ok(_) => report.warn_with_hint(
            "tmux",
            "installed but version check failed",
            "check tmux -V",
        ),
        Err(_) => report.warn_with_hint(
            "tmux",
            "not found; normal non-tmux cx usage still works",
            "install tmux for managed sessions",
        ),
    }
}

fn check_daemon(report: &mut Report) {
    match daemon::running_pid() {
        Ok(Some(pid)) => report.ok("daemon", &format!("running pid {pid}")),
        Ok(None) => report.warn_with_hint("daemon", "not running", "cx daemon start"),
        Err(err) => {
            report.warn_with_hint("daemon", &err.to_string(), "remove stale pidfile if needed")
        }
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
            report.warn_with_hint(
                "gitignore",
                &format!("missing {}", pattern),
                &format!("add `{pattern}` to .gitignore"),
            );
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
        report.fail_with_hint(
            "tracked secrets",
            &sensitive.join(", "),
            "remove these before publishing",
        );
    }
}

struct FixEntry {
    action: String,
    target: String,
    detail: String,
}

impl FixEntry {
    fn applied(target: &str, detail: &str) -> Self {
        Self {
            action: "fixed".to_string(),
            target: target.to_string(),
            detail: detail.to_string(),
        }
    }

    fn skipped(target: &str, detail: &str) -> Self {
        Self {
            action: "skipped".to_string(),
            target: target.to_string(),
            detail: detail.to_string(),
        }
    }
}

fn print_fixes(fixes: &[FixEntry]) {
    println!("{}", ui::heading("Fixes"));
    let rows = fixes
        .iter()
        .map(|fix| vec![fix.action.clone(), fix.target.clone(), fix.detail.clone()])
        .collect::<Vec<_>>();
    ui::print_table(&["ACTION", "TARGET", "DETAIL"], &rows, &[]);
}

#[derive(Default)]
struct Report {
    ok: usize,
    warn: usize,
    fail: usize,
    entries: Vec<CheckEntry>,
}

struct CheckEntry {
    status: String,
    label: String,
    detail: String,
    next: String,
}

impl Report {
    fn ok(&mut self, label: &str, detail: &str) {
        self.ok += 1;
        self.push("OK", label, detail, "-");
    }

    fn warn_with_hint(&mut self, label: &str, detail: &str, next: &str) {
        self.warn += 1;
        self.push("WARN", label, detail, next);
    }

    fn fail_with_hint(&mut self, label: &str, detail: &str, next: &str) {
        self.fail += 1;
        self.push("FAIL", label, detail, next);
    }

    fn push(&mut self, status: &str, label: &str, detail: &str, next: &str) {
        self.entries.push(CheckEntry {
            status: status.to_string(),
            label: label.to_string(),
            detail: detail.to_string(),
            next: next.to_string(),
        });
    }

    fn finish(&self) {
        println!("{}", ui::heading("Doctor"));
        let rows = self
            .entries
            .iter()
            .map(|entry| {
                vec![
                    entry.status.clone(),
                    entry.label.clone(),
                    entry.detail.clone(),
                    entry.next.clone(),
                ]
            })
            .collect::<Vec<_>>();
        ui::print_table(&["RESULT", "CHECK", "DETAIL", "NEXT"], &rows, &[]);
        println!();
        println!(
            "{} ok, {} warning, {} failed",
            self.ok, self.warn, self.fail
        );
    }
}
