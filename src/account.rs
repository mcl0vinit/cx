use crate::{codex, config, db, limits, paths, resume, ui, util};
use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    io::IsTerminal,
    path::{Path, PathBuf},
    process::Stdio,
};

struct AccountStatusRow {
    name: String,
    status: String,
    email: String,
    codex_sessions: i64,
    managed_sessions: i64,
    five_hour: String,
    weekly: String,
    five_hour_reset: String,
    weekly_reset: String,
    observed: String,
    home: String,
    last_checked: String,
    last_error: Option<String>,
}

pub fn ensure_account_home(path: PathBuf) -> Result<PathBuf> {
    let home = util::expand_tilde(path);
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

fn default_account_home(name: &str) -> Result<PathBuf> {
    paths::account_home(name)
}

fn registered_or_default_home(conn: &Connection, name: &str) -> Result<PathBuf> {
    if let Some(account) = db::get_account(conn, name)? {
        return Ok(account.codex_home);
    }

    default_account_home(name)
}

pub fn add(conn: &Connection, name: &str, codex_home: Option<PathBuf>) -> Result<()> {
    let home = ensure_account_home(match codex_home {
        Some(path) => path,
        None => default_account_home(name)?,
    })?;
    db::upsert_account(conn, name, home.clone())?;
    let _ = local_check(conn, name);
    println!(
        "registered account `{}` at {}",
        name,
        util::display_path(&home)
    );
    Ok(())
}

pub fn login(conn: &Connection, name: &str) -> Result<()> {
    let home = ensure_account_home(registered_or_default_home(conn, name)?)?;
    db::upsert_account(conn, name, home.clone())?;

    let status = codex::command()
        .arg("login")
        .env("CODEX_HOME", &home)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to run Codex login; set CX_CODEX_BIN if the launcher is not on PATH")?;

    if status.success() {
        local_check(conn, name)?;
    } else {
        db::set_account_status(
            conn,
            name,
            "auth_failed",
            Some("codex login returned non-zero"),
        )?;
        anyhow::bail!("codex login failed for account `{}`", name);
    }

    Ok(())
}

pub fn logout(conn: &Connection, name: &str) -> Result<()> {
    let account =
        db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    let status = codex::command()
        .arg("logout")
        .env("CODEX_HOME", &account.codex_home)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to run Codex logout; set CX_CODEX_BIN if the launcher is not on PATH")?;

    if status.success() {
        db::set_account_status(conn, name, "auth_failed", Some("logged out"))?;
    }

    Ok(())
}

pub fn list(conn: &Connection) -> Result<()> {
    let accounts = db::list_accounts(conn)?;
    if accounts.is_empty() {
        println!("{}", ui::heading("Accounts"));
        println!("No accounts registered.");
        return Ok(());
    }

    let mut errors = Vec::new();
    let rows = accounts
        .into_iter()
        .map(|account| {
            let managed = db::active_session_count(conn, &account.name)?;
            let codex_sessions =
                resume::codex_session_count(conn, &account.name, &account.codex_home)?;
            let status = if account.disabled {
                "disabled".to_string()
            } else {
                account.status.clone()
            };
            if let Some(error) = &account.last_error {
                errors.push((account.name.clone(), error.clone()));
            }

            Ok(vec![
                account.name,
                status,
                auth_email(&account.codex_home).unwrap_or_else(|| "-".to_string()),
                codex_sessions.to_string(),
                managed.to_string(),
                util::display_path(&account.codex_home),
                account.last_checked_at.unwrap_or_else(|| "-".to_string()),
            ])
        })
        .collect::<Result<Vec<_>>>()?;

    println!("{}", ui::heading("Accounts"));
    ui::print_table(
        &[
            "ACCOUNT",
            "STATUS",
            "EMAIL",
            "CODEX",
            "MGD",
            "HOME",
            "LAST CHECK",
        ],
        &rows,
        &[3, 4],
    );

    if !errors.is_empty() {
        println!();
        println!("{}", ui::heading("Account Notes"));
        for (name, error) in errors {
            println!("{name}: {error}");
        }
    }
    Ok(())
}

pub fn disable(conn: &Connection, name: &str, reason: Option<&str>) -> Result<()> {
    db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    db::set_account_disabled(conn, name, true)?;
    db::set_account_status(conn, name, "disabled", reason)?;
    db::log_event(
        conn,
        "account.disabled",
        Some(name),
        reason.unwrap_or("disabled manually"),
    )?;
    println!("disabled account `{}`", name);
    Ok(())
}

pub fn enable(conn: &Connection, name: &str) -> Result<()> {
    db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    db::set_account_disabled(conn, name, false)?;
    local_check(conn, name)?;
    db::log_event(conn, "account.enabled", Some(name), "enabled manually")?;
    println!("enabled account `{}`", name);
    Ok(())
}

pub fn check(conn: &Connection, name: &str, online: bool) -> Result<String> {
    if online {
        online_check(conn, name)
    } else {
        local_check(conn, name)
    }
}

pub fn status(conn: &Connection, name: &str, online: bool) -> Result<()> {
    let row = status_row(conn, name, online)?;
    let codex_sessions = row.codex_sessions.to_string();
    let managed_sessions = row.managed_sessions.to_string();

    println!("{}", ui::heading("Account"));
    ui::print_key_values(&[
        ("Name", row.name.as_str()),
        ("Status", row.status.as_str()),
        ("Email", row.email.as_str()),
        ("Codex sessions", codex_sessions.as_str()),
        ("Managed active", managed_sessions.as_str()),
        ("CODEX_HOME", row.home.as_str()),
        ("Last checked", row.last_checked.as_str()),
    ]);
    if let Some(error) = &row.last_error {
        ui::print_key_values(&[("Last error", error.as_str())]);
    }
    println!();

    let account =
        db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    match limits::latest_snapshot_cached(conn, &account.codex_home)? {
        Some(snapshot) => limits::print_snapshot(&snapshot),
        None => {
            println!("Limits");
            println!("{:<18} none found", "Observed");
            println!(
                "{:<18} run `cx account status {} --online` to refresh with a Codex request",
                "Hint", account.name
            );
        }
    }

    Ok(())
}

pub fn status_all(conn: &Connection, online: bool) -> Result<()> {
    let accounts = db::list_accounts(conn)?;
    if accounts.is_empty() {
        println!("No accounts registered.");
        return Ok(());
    }

    let rows = accounts
        .iter()
        .map(|account| status_row(conn, &account.name, online))
        .collect::<Result<Vec<_>>>()?;
    print_status_table(&rows);

    Ok(())
}

fn status_row(conn: &Connection, name: &str, online: bool) -> Result<AccountStatusRow> {
    let check_status = check(conn, name, online)?;
    let account =
        db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    let managed_sessions = db::active_session_count(conn, &account.name)?;
    let codex_sessions = resume::codex_session_count(conn, &account.name, &account.codex_home)?;
    let snapshot = limits::latest_snapshot_cached(conn, &account.codex_home)?;
    let status = if account.disabled {
        "disabled".to_string()
    } else {
        check_status
    };
    let five = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.primary.as_ref());
    let weekly = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.secondary.as_ref());

    Ok(AccountStatusRow {
        name: account.name,
        status,
        email: auth_email(&account.codex_home).unwrap_or_else(|| "-".to_string()),
        codex_sessions,
        managed_sessions,
        five_hour: limits::compact_remaining(five),
        weekly: limits::compact_remaining(weekly),
        five_hour_reset: limits::compact_reset(five),
        weekly_reset: limits::compact_reset(weekly),
        observed: limits::compact_observed_age(snapshot.as_ref()),
        home: util::display_path(&account.codex_home),
        last_checked: account.last_checked_at.unwrap_or_else(|| "-".to_string()),
        last_error: account.last_error,
    })
}

fn print_status_table(rows: &[AccountStatusRow]) {
    println!("{}", ui::heading("Accounts"));
    let headers = [
        "ACCOUNT", "STATUS", "EMAIL", "CODEX", "MGD", "5H LEFT", "WK LEFT", "5H RESET", "WK RESET",
        "OBSERVED",
    ];
    let table_rows = rows
        .iter()
        .map(|row| {
            vec![
                row.name.clone(),
                row.status.clone(),
                row.email.clone(),
                row.codex_sessions.to_string(),
                row.managed_sessions.to_string(),
                row.five_hour.clone(),
                row.weekly.clone(),
                row.five_hour_reset.clone(),
                row.weekly_reset.clone(),
                row.observed.clone(),
            ]
        })
        .collect::<Vec<_>>();
    ui::print_table(&headers, &table_rows, &[3, 4]);

    let errors = rows
        .iter()
        .filter_map(|row| {
            row.last_error
                .as_ref()
                .map(|error| (row.name.as_str(), error))
        })
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        println!();
        println!("{}", ui::heading("Account Notes"));
        for (name, error) in errors {
            println!("{name}: {error}");
        }
    }
}

pub fn local_check(conn: &Connection, name: &str) -> Result<String> {
    let account =
        db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    if account.disabled {
        db::set_account_status(conn, name, "disabled", Some("account disabled"))?;
        return Ok("disabled".to_string());
    }

    if !account.codex_home.exists() {
        db::set_account_status(
            conn,
            name,
            "auth_failed",
            Some("CODEX_HOME directory missing"),
        )?;
        return Ok("auth_failed".to_string());
    }

    if account.status == "limited" {
        return Ok("limited".to_string());
    }

    if !account.codex_home.join("auth.json").exists() {
        db::set_account_status(
            conn,
            name,
            "auth_failed",
            Some("auth.json missing; run `cx account login`"),
        )?;
        return Ok("auth_failed".to_string());
    }

    db::set_account_status(conn, name, "healthy", None)?;
    Ok("healthy".to_string())
}

fn auth_email(codex_home: &Path) -> Option<String> {
    let text = fs::read_to_string(codex_home.join("auth.json")).ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    let token = value
        .get("tokens")
        .and_then(|tokens| tokens.get("id_token"))
        .and_then(Value::as_str)?;
    jwt_claim(token, "email")
}

fn jwt_claim(token: &str, claim: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = decode_base64_url(payload)?;
    let value = serde_json::from_slice::<Value>(&bytes).ok()?;
    value
        .get(claim)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn decode_base64_url(input: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u8;

    for byte in input.bytes().filter(|byte| *byte != b'=') {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32;

        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }

    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_claim_reads_email_from_payload() {
        let token = "header.eyJlbWFpbCI6InNhbUBleGFtcGxlLmNvbSJ9.signature";
        assert_eq!(
            jwt_claim(token, "email").as_deref(),
            Some("sam@example.com")
        );
    }
}

pub fn online_check(conn: &Connection, name: &str) -> Result<String> {
    let account =
        db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    if account.disabled {
        db::set_account_status(conn, name, "disabled", Some("account disabled"))?;
        return Ok("disabled".to_string());
    }

    let output = codex::command()
        .arg("exec")
        .arg("Reply with exactly: ok")
        .env("CODEX_HOME", &account.codex_home)
        .stdin(Stdio::null())
        .output()
        .context("failed to run Codex exec; set CX_CODEX_BIN if the launcher is not on PATH")?;

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let lower = combined.to_lowercase();

    let (status, error) = if output.status.success() {
        ("healthy", None)
    } else if lower.contains("usage limit")
        || lower.contains("rate limit")
        || lower.contains("quota")
        || lower.contains("too many requests")
    {
        ("limited", Some(first_line(&combined)))
    } else if lower.contains("unauthorized")
        || lower.contains("401")
        || lower.contains("login")
        || lower.contains("not authenticated")
    {
        ("auth_failed", Some(first_line(&combined)))
    } else {
        ("degraded", Some(first_line(&combined)))
    };

    db::set_account_status(conn, name, status, error.as_deref())?;
    if let Err(error) = limits::refresh_snapshot_cache(conn, &account.codex_home) {
        tracing::debug!(
            account = name,
            error = %error,
            "failed to update limit snapshot cache after online check"
        );
    }
    Ok(status.to_string())
}

pub fn refresh(conn: &Connection, names: &[String], stale_only: bool) -> Result<()> {
    let show_progress = names.len() > 1 && std::io::stderr().is_terminal();
    if show_progress {
        eprintln!("Refreshing {} accounts...", names.len());
    }

    let mut rows = Vec::new();
    for name in names {
        if show_progress {
            eprintln!("  {name}");
        }
        let outcome = refresh_one(conn, name, stale_only)?;
        rows.push(vec![name.clone(), outcome]);
    }

    println!("{}", ui::heading("Refresh"));
    ui::print_table(&["ACCOUNT", "RESULT"], &rows, &[]);
    Ok(())
}

pub fn refresh_one(conn: &Connection, name: &str, stale_only: bool) -> Result<String> {
    let account =
        db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    if stale_only {
        let cfg = config::load()?;
        let snapshot = limits::latest_snapshot_cached(conn, &account.codex_home)?;
        if !limits::is_stale(snapshot.as_ref(), cfg.limit_snapshot_max_age_minutes()) {
            return Ok("fresh".to_string());
        }
    }
    online_check(conn, name)
}

fn first_line(s: &str) -> String {
    s.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("unknown error")
        .trim()
        .to_string()
}
