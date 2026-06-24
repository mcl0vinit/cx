use crate::{db, limits, ui, util};
use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use serde::Serialize;
use std::{
    fs,
    io::{self, IsTerminal, Read, Write},
};

#[derive(Debug, Clone, Copy)]
pub struct ShowOptions {
    pub json: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ResetOptions {
    pub yes: bool,
    pub json: bool,
}

#[derive(Serialize)]
struct UsageReport {
    account: String,
    codex_home: String,
    source: String,
    snapshot: Option<limits::LimitSnapshot>,
}

#[derive(Serialize)]
struct ResetReport {
    account: String,
    codex_home: String,
    available_before: Option<i64>,
    idempotency_key: Option<String>,
    outcome: String,
    windows_reset: Option<i64>,
    snapshot: Option<limits::LimitSnapshot>,
}

pub fn show(conn: &Connection, account_name: &str, options: ShowOptions) -> Result<()> {
    let account = db::get_account(conn, account_name)?
        .ok_or_else(|| anyhow!("unknown account `{}`", account_name))?;
    let snapshot = limits::latest_snapshot_cached(conn, &account.codex_home)?;
    let source = "cache".to_string();

    if options.json {
        print_json(&UsageReport {
            account: account.name,
            codex_home: util::display_path(&account.codex_home),
            source,
            snapshot,
        })
    } else {
        print_usage_text(
            &account.name,
            &account.codex_home,
            &source,
            snapshot.as_ref(),
        );
        Ok(())
    }
}

pub fn reset(conn: &Connection, account_name: &str, options: ResetOptions) -> Result<()> {
    let account = db::get_account(conn, account_name)?
        .ok_or_else(|| anyhow!("unknown account `{}`", account_name))?;
    if account.disabled {
        anyhow::bail!("account `{}` is disabled", account.name);
    }

    let before = limits::refresh_snapshot_from_backend(conn, &account.codex_home)?;
    update_account_status_from_snapshot(conn, &account.name, &before)?;
    let available_before = before
        .rate_limit_reset_credits
        .as_ref()
        .map(|credits| credits.available_count);

    if available_before.is_none() {
        let report = ResetReport {
            account: account.name,
            codex_home: util::display_path(&account.codex_home),
            available_before,
            idempotency_key: None,
            outcome: "unavailable".to_string(),
            windows_reset: None,
            snapshot: Some(before),
        };
        if options.json {
            return print_json(&report);
        }
        println!("Usage limit reset availability is unavailable for this account.");
        return Ok(());
    }

    if matches!(available_before, Some(count) if count <= 0) {
        let report = ResetReport {
            account: account.name,
            codex_home: util::display_path(&account.codex_home),
            available_before,
            idempotency_key: None,
            outcome: limits::ConsumeRateLimitResetCreditCode::NoCredit
                .as_str()
                .to_string(),
            windows_reset: None,
            snapshot: Some(before),
        };
        if options.json {
            return print_json(&report);
        }
        println!("No usage limit resets are available.");
        return Ok(());
    }

    if !options.yes && !confirm_reset(&account.name, available_before)? {
        println!("Cancelled.");
        return Ok(());
    }

    let idempotency_key = new_idempotency_key()?;
    let consumed = limits::consume_rate_limit_reset_credit(&account.codex_home, &idempotency_key)?;
    let snapshot = if consumed.code.is_success() {
        match limits::refresh_snapshot_from_backend(conn, &account.codex_home) {
            Ok(snapshot) => {
                update_account_status_from_snapshot(conn, &account.name, &snapshot)?;
                Some(snapshot)
            }
            Err(error) => {
                tracing::debug!(
                    account = %account.name,
                    error = %error,
                    "failed to refresh usage after consuming reset credit"
                );
                None
            }
        }
    } else {
        Some(before)
    };

    let report = ResetReport {
        account: account.name,
        codex_home: util::display_path(&account.codex_home),
        available_before,
        idempotency_key: Some(idempotency_key),
        outcome: consumed.code.as_str().to_string(),
        windows_reset: Some(consumed.windows_reset),
        snapshot,
    };

    if options.json {
        print_json(&report)
    } else {
        print_reset_text(&report);
        Ok(())
    }
}

fn update_account_status_from_snapshot(
    conn: &Connection,
    account_name: &str,
    snapshot: &limits::LimitSnapshot,
) -> Result<()> {
    let status = if snapshot_is_limited(snapshot) {
        "limited"
    } else {
        "healthy"
    };
    db::set_account_status(conn, account_name, status, None)
}

fn snapshot_is_limited(snapshot: &limits::LimitSnapshot) -> bool {
    snapshot
        .primary
        .as_ref()
        .map(limits::is_exhausted)
        .unwrap_or(false)
        || snapshot
            .secondary
            .as_ref()
            .map(limits::is_exhausted)
            .unwrap_or(false)
}

fn print_usage_text(
    account_name: &str,
    codex_home: &std::path::Path,
    source: &str,
    snapshot: Option<&limits::LimitSnapshot>,
) {
    let codex_home = util::display_path(codex_home);
    println!("{}", ui::heading("Usage"));
    ui::print_key_values(&[
        ("Account", account_name),
        ("CODEX_HOME", codex_home.as_str()),
        ("Source", source),
    ]);
    println!();
    match snapshot {
        Some(snapshot) => limits::print_snapshot(snapshot),
        None => {
            println!("Limits");
            println!("{:<18} none found", "Observed");
            println!("{:<18} run `cx refresh {}`", "Hint", account_name);
        }
    }
}

fn print_reset_text(report: &ResetReport) {
    match report.outcome.as_str() {
        "reset" => println!("Usage reset for `{}`.", report.account),
        "alreadyRedeemed" => println!("Usage reset already completed for this request."),
        "nothingToReset" => println!("Your usage does not need a reset right now."),
        "noCredit" => println!("No usage limit resets are available."),
        _ => println!("Reset outcome: {}", report.outcome),
    }

    if let Some(snapshot) = &report.snapshot {
        if let Some(credits) = &snapshot.rate_limit_reset_credits {
            println!(
                "{:<18} {}",
                "Usage resets left",
                limits::reset_credit_label(credits.available_count)
            );
        }
    }
}

fn confirm_reset(account_name: &str, available: Option<i64>) -> Result<bool> {
    if !io::stdin().is_terminal() {
        anyhow::bail!("refusing to consume a reset without a terminal prompt; pass --yes");
    }

    let available = available
        .map(limits::reset_credit_label)
        .unwrap_or_else(|| "unknown".to_string());
    print!(
        "Redeem one usage limit reset for `{account_name}`? Available resets: {available}. [y/N] "
    );
    io::stdout().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;
    Ok(matches!(
        response.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn new_idempotency_key() -> Result<String> {
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")
        .context("failed to open /dev/urandom for reset idempotency key")?
        .read_exact(&mut bytes)
        .context("failed to read reset idempotency key")?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    ))
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
