use crate::{
    config,
    db::{self, Account},
    limits, ui, util,
};
use anyhow::{anyhow, Result};
use rusqlite::Connection;

pub fn create(conn: &Connection, name: &str, accounts_csv: &str, strategy: &str) -> Result<()> {
    validate_strategy(strategy)?;

    let accounts = crate::util::split_csv(accounts_csv);
    if accounts.is_empty() {
        anyhow::bail!("pool must include at least one account");
    }

    for account in &accounts {
        if db::get_account(conn, account)?.is_none() {
            anyhow::bail!(
                "unknown account `{}`; add it first with `cx account add {}`",
                account,
                account
            );
        }
    }

    db::create_pool(conn, name, &accounts, strategy)?;
    println!(
        "created pool `{}` with accounts: {}",
        name,
        accounts.join(", ")
    );
    Ok(())
}

pub fn list(conn: &Connection) -> Result<()> {
    let pools = db::list_pools(conn)?;
    println!("{}", ui::heading("Pools"));
    if pools.is_empty() {
        println!("No pools configured.");
        println!("Create one with `cx pool create coding --accounts personal,work`.");
        return Ok(());
    }

    let mut pool_rows = Vec::new();
    let mut account_rows = Vec::new();
    for pool in pools {
        let accounts = db::get_pool_accounts(conn, &pool.name)?;
        pool_rows.push(vec![
            pool.name.clone(),
            pool.strategy.clone(),
            pool.failover.clone(),
            accounts.join(", "),
        ]);

        for (index, account_name) in accounts.iter().enumerate() {
            match db::get_account(conn, account_name)? {
                Some(account) => {
                    let snapshot = limits::latest_snapshot(&account.codex_home)?;
                    let five = snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.primary.as_ref());
                    let weekly = snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.secondary.as_ref());
                    let status = if account.disabled {
                        "disabled".to_string()
                    } else {
                        account.status.clone()
                    };
                    account_rows.push(vec![
                        pool.name.clone(),
                        (index + 1).to_string(),
                        account.name,
                        status,
                        limits::compact_remaining(five),
                        limits::compact_remaining(weekly),
                        db::active_session_count(conn, account_name)?.to_string(),
                        util::display_path(&account.codex_home),
                    ]);
                }
                None => account_rows.push(vec![
                    pool.name.clone(),
                    (index + 1).to_string(),
                    account_name.clone(),
                    "missing".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                ]),
            }
        }
    }

    ui::print_table(
        &["POOL", "STRATEGY", "FAILOVER", "ACCOUNTS"],
        &pool_rows,
        &[],
    );

    println!();
    println!("{}", ui::heading("Pool Accounts"));
    ui::print_table(
        &[
            "POOL", "ORDER", "ACCOUNT", "STATUS", "5H LEFT", "WK LEFT", "ACTIVE", "HOME",
        ],
        &account_rows,
        &[1, 6],
    );
    Ok(())
}

pub fn choose(
    conn: &Connection,
    explicit_account: Option<&str>,
    pool_name: Option<&str>,
    exclude_account: Option<&str>,
) -> Result<Account> {
    let configured_pool = configured_pool_name(pool_name)?;
    match (explicit_account, pool_name) {
        (Some(_), Some(_)) => anyhow::bail!("use either --account or --pool, not both"),
        (Some(account), None) => choose_account(conn, account),
        (None, Some(pool)) => choose_from_pool(conn, pool, exclude_account),
        (None, None) => match configured_pool.as_deref() {
            Some(pool) => choose_from_pool(conn, pool, exclude_account),
            None => anyhow::bail!("missing --account or --pool"),
        },
    }
}

pub fn choose_smart(
    conn: &Connection,
    pool_name: Option<&str>,
    exclude_account: Option<&str>,
) -> Result<Account> {
    let configured_pool = configured_pool_name(pool_name)?;
    let accounts = match configured_pool.as_deref() {
        Some(pool_name) => {
            db::get_pool(conn, pool_name)?
                .ok_or_else(|| anyhow!("unknown pool `{}`", pool_name))?;
            db::get_pool_accounts(conn, pool_name)?
                .into_iter()
                .filter_map(|name| db::get_account(conn, &name).transpose())
                .collect::<Result<Vec<_>>>()?
        }
        None => db::list_accounts(conn)?,
    };

    choose_limit_aware(
        conn,
        accounts,
        configured_pool.as_deref().unwrap_or("all accounts"),
        exclude_account,
    )
}

pub fn default_strategy() -> Result<String> {
    Ok(config::load()?
        .default_strategy
        .filter(|strategy| !strategy.trim().is_empty())
        .unwrap_or_else(|| "least-sessions".to_string()))
}

pub fn configured_pool_name(pool_name: Option<&str>) -> Result<Option<String>> {
    if let Some(pool_name) = pool_name {
        return Ok(Some(pool_name.to_string()));
    }

    Ok(config::load()?
        .default_pool
        .filter(|pool| !pool.trim().is_empty()))
}

fn choose_account(conn: &Connection, name: &str) -> Result<Account> {
    let account =
        db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    if account.disabled || account.status == "disabled" {
        anyhow::bail!("account `{}` is disabled", name);
    }
    if account.status == "auth_failed" {
        anyhow::bail!(
            "account `{}` is not authenticated; run `cx account login {}`",
            name,
            name
        );
    }
    if account.status == "limited" {
        anyhow::bail!("account `{}` is marked limited", name);
    }
    Ok(account)
}

fn choose_from_pool(
    conn: &Connection,
    pool_name: &str,
    exclude_account: Option<&str>,
) -> Result<Account> {
    let pool =
        db::get_pool(conn, pool_name)?.ok_or_else(|| anyhow!("unknown pool `{}`", pool_name))?;
    let names = db::get_pool_accounts(conn, pool_name)?;
    let mut candidates = Vec::new();

    for name in names {
        if exclude_account == Some(name.as_str()) {
            continue;
        }
        if let Some(account) = db::get_account(conn, &name)? {
            if is_eligible(&account) {
                let active = db::active_session_count(conn, &account.name)?;
                candidates.push((active, account));
            }
        }
    }

    if candidates.is_empty() {
        anyhow::bail!("no healthy accounts available in pool `{}`", pool_name);
    }

    match pool.strategy.as_str() {
        "first-healthy" => Ok(candidates.remove(0).1),
        "least-sessions" => {
            candidates.sort_by_key(|(active, _)| *active);
            Ok(candidates.remove(0).1)
        }
        "limit-aware" => {
            let accounts = candidates
                .into_iter()
                .map(|(_, account)| account)
                .collect::<Vec<_>>();
            choose_limit_aware(conn, accounts, pool_name, None)
        }
        _ => {
            candidates.sort_by_key(|(active, _)| *active);
            Ok(candidates.remove(0).1)
        }
    }
}

fn choose_limit_aware(
    conn: &Connection,
    accounts: Vec<Account>,
    label: &str,
    exclude_account: Option<&str>,
) -> Result<Account> {
    let mut candidates = Vec::new();

    for account in accounts {
        if exclude_account == Some(account.name.as_str()) || !is_eligible(&account) {
            continue;
        }
        let active = db::active_session_count(conn, &account.name)?;
        let snapshot = limits::latest_snapshot(&account.codex_home)?;
        if snapshot
            .as_ref()
            .map(|snapshot| {
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
            })
            .unwrap_or(false)
        {
            continue;
        }

        let five_key = window_score(
            snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.primary.as_ref()),
        );
        let weekly_key = window_score(
            snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.secondary.as_ref()),
        );
        candidates.push((five_key, weekly_key, active, account.name.clone(), account));
    }

    if candidates.is_empty() {
        anyhow::bail!("no limit-eligible accounts available in `{}`", label);
    }

    candidates
        .sort_by_key(|(five, weekly, active, name, _)| (*five, *weekly, *active, name.clone()));
    Ok(candidates.remove(0).4)
}

fn window_score(window: Option<&limits::LimitWindow>) -> u32 {
    let Some(window) = window else {
        return 750;
    };
    if window
        .resets_at
        .map(|reset| reset <= chrono::Utc::now())
        .unwrap_or(false)
    {
        return 0;
    }
    (window.used_percent.clamp(0.0, 100.0) * 10.0).round() as u32
}

fn validate_strategy(strategy: &str) -> Result<()> {
    match strategy {
        "first-healthy" | "least-sessions" | "limit-aware" => Ok(()),
        _ => anyhow::bail!(
            "unknown pool strategy `{}`; use first-healthy, least-sessions, or limit-aware",
            strategy
        ),
    }
}

fn is_eligible(account: &Account) -> bool {
    if account.disabled {
        return false;
    }
    matches!(account.status.as_str(), "healthy" | "unknown" | "degraded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("cx-{name}-{}-{nanos}", std::process::id()))
    }

    fn write_limits(home: &Path, five_used: f64, weekly_used: f64) {
        let file = home.join("sessions/2026/06/15/rollout.jsonl");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            file,
            format!(
                r#"{{"timestamp":"2026-06-15T12:00:00Z","type":"event_msg","payload":{{"type":"token_count","rate_limits":{{"limit_id":"codex","primary":{{"used_percent":{five_used},"window_minutes":300,"resets_at":4102444800}},"secondary":{{"used_percent":{weekly_used},"window_minutes":10080,"resets_at":4102444800}}}}}}}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn limit_aware_strategy_selects_lowest_five_hour_usage() {
        let root = temp_root("pool-limit-aware");
        let high_home = root.join("high");
        let low_home = root.join("low");
        write_limits(&high_home, 80.0, 10.0);
        write_limits(&low_home, 20.0, 90.0);

        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        db::upsert_account(&conn, "high", high_home).unwrap();
        db::upsert_account(&conn, "low", low_home).unwrap();
        db::set_account_status(&conn, "high", "healthy", None).unwrap();
        db::set_account_status(&conn, "low", "healthy", None).unwrap();
        db::create_pool(
            &conn,
            "coding",
            &["high".to_string(), "low".to_string()],
            "limit-aware",
        )
        .unwrap();

        let chosen = choose(&conn, None, Some("coding"), None).unwrap();

        assert_eq!(chosen.name, "low");

        let _ = fs::remove_dir_all(root);
    }
}
