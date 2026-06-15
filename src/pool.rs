use crate::db::{self, Account};
use anyhow::{anyhow, Result};
use rusqlite::Connection;

pub fn create(conn: &Connection, name: &str, accounts_csv: &str, strategy: &str) -> Result<()> {
    let accounts = crate::util::split_csv(accounts_csv);
    if accounts.is_empty() {
        anyhow::bail!("pool must include at least one account");
    }

    for account in &accounts {
        if db::get_account(conn, account)?.is_none() {
            anyhow::bail!("unknown account `{}`; add it first with `cx account add {}`", account, account);
        }
    }

    db::create_pool(conn, name, &accounts, strategy)?;
    println!("created pool `{}` with accounts: {}", name, accounts.join(", "));
    Ok(())
}

pub fn list(conn: &Connection) -> Result<()> {
    let pools = db::list_pools(conn)?;
    println!("{:<20} {:<16} {:<12} ACCOUNTS", "NAME", "STRATEGY", "FAILOVER");
    for pool in pools {
        let accounts = db::get_pool_accounts(conn, &pool.name)?;
        println!("{:<20} {:<16} {:<12} {}", pool.name, pool.strategy, pool.failover, accounts.join(","));
    }
    Ok(())
}

pub fn choose(conn: &Connection, explicit_account: Option<&str>, pool_name: Option<&str>, exclude_account: Option<&str>) -> Result<Account> {
    match (explicit_account, pool_name) {
        (Some(_), Some(_)) => anyhow::bail!("use either --account or --pool, not both"),
        (Some(account), None) => choose_account(conn, account),
        (None, Some(pool)) => choose_from_pool(conn, pool, exclude_account),
        (None, None) => anyhow::bail!("missing --account or --pool"),
    }
}

fn choose_account(conn: &Connection, name: &str) -> Result<Account> {
    let account = db::get_account(conn, name)?.ok_or_else(|| anyhow!("unknown account `{}`", name))?;
    if account.disabled || account.status == "disabled" {
        anyhow::bail!("account `{}` is disabled", name);
    }
    if account.status == "auth_failed" {
        anyhow::bail!("account `{}` is not authenticated; run `cx account login {}`", name, name);
    }
    if account.status == "limited" {
        anyhow::bail!("account `{}` is marked limited", name);
    }
    Ok(account)
}

fn choose_from_pool(conn: &Connection, pool_name: &str, exclude_account: Option<&str>) -> Result<Account> {
    let pool = db::get_pool(conn, pool_name)?.ok_or_else(|| anyhow!("unknown pool `{}`", pool_name))?;
    let names = db::get_pool_accounts(conn, pool_name)?;
    let mut candidates = Vec::new();

    for name in names {
        if exclude_account == Some(name.as_str()) { continue; }
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
        "least-sessions" | _ => {
            candidates.sort_by_key(|(active, _)| *active);
            Ok(candidates.remove(0).1)
        }
    }
}

fn is_eligible(account: &Account) -> bool {
    if account.disabled { return false; }
    matches!(account.status.as_str(), "healthy" | "unknown" | "degraded")
}
