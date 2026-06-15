use crate::{db, paths, util};
use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use std::{fs, path::PathBuf, process::{Command, Stdio}};

pub fn ensure_account_home(name: &str) -> Result<PathBuf> {
    let home = paths::account_home(name)?;
    fs::create_dir_all(&home)?;

    let config = home.join("config.toml");
    if !config.exists() {
        fs::write(&config, "cli_auth_credentials_store = \"file\"\n")?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&home, fs::Permissions::from_mode(0o700));
        let auth = home.join("auth.json");
        if auth.exists() {
            let _ = fs::set_permissions(auth, fs::Permissions::from_mode(0o600));
        }
    }

    Ok(home)
}

pub fn add(conn: &Connection, name: &str) -> Result<()> {
    let home = ensure_account_home(name)?;
    db::upsert_account(conn, name, home.clone())?;
    let _ = local_check(conn, name);
    println!("created account `{}` at {}", name, util::display_path(&home));
    Ok(())
}

pub fn login(conn: &Connection, name: &str) -> Result<()> {
    let home = ensure_account_home(name)?;
    db::upsert_account(conn, name, home.clone())?;

    let status = Command::new("codex")
        .arg("login")
        .env("CODEX_HOME", &home)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to run `codex login`; is Codex installed and on PATH?")?;

    if status.success() {
        local_check(conn, name)?;
    } else {
        db::set_account_status(conn, name, "auth_failed", Some("codex login returned non-zero"))?;
        anyhow::bail!("codex login failed for account `{}`", name);
    }

    Ok(())
}

pub fn logout(conn: &Connection, name: &str) -> Result<()> {
    let account = db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    let status = Command::new("codex")
        .arg("logout")
        .env("CODEX_HOME", &account.codex_home)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to run `codex logout`; is Codex installed and on PATH?")?;

    if status.success() {
        db::set_account_status(conn, name, "auth_failed", Some("logged out"))?;
    }

    Ok(())
}

pub fn list(conn: &Connection) -> Result<()> {
    let accounts = db::list_accounts(conn)?;
    println!("{:<20} {:<14} {:<8} {:<36} {}", "NAME", "STATUS", "ACTIVE", "CODEX_HOME", "LAST_ERROR");
    for account in accounts {
        let active = db::active_session_count(conn, &account.name)?;
        let status = if account.disabled { "disabled".to_string() } else { account.status.clone() };
        println!(
            "{:<20} {:<14} {:<8} {:<36} {}",
            account.name,
            status,
            active,
            util::display_path(&account.codex_home),
            account.last_error.unwrap_or_default()
        );
    }
    Ok(())
}

pub fn disable(conn: &Connection, name: &str, reason: Option<&str>) -> Result<()> {
    db::set_account_disabled(conn, name, true)?;
    db::set_account_status(conn, name, "disabled", reason)?;
    db::log_event(conn, "account.disabled", Some(name), reason.unwrap_or("disabled manually"))?;
    println!("disabled account `{}`", name);
    Ok(())
}

pub fn enable(conn: &Connection, name: &str) -> Result<()> {
    db::set_account_disabled(conn, name, false)?;
    local_check(conn, name)?;
    db::log_event(conn, "account.enabled", Some(name), "enabled manually")?;
    println!("enabled account `{}`", name);
    Ok(())
}

pub fn check(conn: &Connection, name: &str, online: bool) -> Result<String> {
    if online { online_check(conn, name) } else { local_check(conn, name) }
}

pub fn local_check(conn: &Connection, name: &str) -> Result<String> {
    let account = db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    if account.disabled {
        db::set_account_status(conn, name, "disabled", Some("account disabled"))?;
        return Ok("disabled".to_string());
    }

    if !account.codex_home.exists() {
        db::set_account_status(conn, name, "auth_failed", Some("CODEX_HOME directory missing"))?;
        return Ok("auth_failed".to_string());
    }

    if account.status == "limited" {
        return Ok("limited".to_string());
    }

    if !account.codex_home.join("auth.json").exists() {
        db::set_account_status(conn, name, "auth_failed", Some("auth.json missing; run `cx account login`"))?;
        return Ok("auth_failed".to_string());
    }

    db::set_account_status(conn, name, "healthy", None)?;
    Ok("healthy".to_string())
}

pub fn online_check(conn: &Connection, name: &str) -> Result<String> {
    let account = db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    if account.disabled {
        db::set_account_status(conn, name, "disabled", Some("account disabled"))?;
        return Ok("disabled".to_string());
    }

    let output = Command::new("codex")
        .arg("exec")
        .arg("Reply with exactly: ok")
        .env("CODEX_HOME", &account.codex_home)
        .stdin(Stdio::null())
        .output()
        .context("failed to run `codex exec`; is Codex installed and on PATH?")?;

    let combined = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let lower = combined.to_lowercase();

    let (status, error) = if output.status.success() {
        ("healthy", None)
    } else if lower.contains("usage limit") || lower.contains("rate limit") || lower.contains("quota") || lower.contains("too many requests") {
        ("limited", Some(first_line(&combined)))
    } else if lower.contains("unauthorized") || lower.contains("401") || lower.contains("login") || lower.contains("not authenticated") {
        ("auth_failed", Some(first_line(&combined)))
    } else {
        ("degraded", Some(first_line(&combined)))
    };

    db::set_account_status(conn, name, status, error.as_deref())?;
    Ok(status.to_string())
}

fn first_line(s: &str) -> String {
    s.lines().find(|line| !line.trim().is_empty()).unwrap_or("unknown error").trim().to_string()
}
