use crate::{paths, util};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Account {
    pub name: String,
    pub codex_home: PathBuf,
    pub status: String,
    pub disabled: bool,
    pub last_checked_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Pool {
    pub name: String,
    pub strategy: String,
    pub failover: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: i64,
    pub name: String,
    pub pool: Option<String>,
    pub current_account: String,
    pub cwd: PathBuf,
    pub tmux_session: Option<String>,
    pub tmux_window: Option<String>,
    pub tmux_pane: Option<String>,
    pub codex_args: Vec<String>,
    pub resume_prompt: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewSession {
    pub name: String,
    pub pool: Option<String>,
    pub current_account: String,
    pub cwd: PathBuf,
    pub tmux_session: Option<String>,
    pub tmux_window: Option<String>,
    pub tmux_pane: Option<String>,
    pub codex_args: Vec<String>,
    pub resume_prompt: Option<String>,
    pub status: String,
}

pub fn connect() -> Result<Connection> {
    paths::ensure_root_dirs()?;
    let conn = Connection::open(paths::db_path()?).context("failed to open cx sqlite registry")?;
    init(&conn)?;
    Ok(conn)
}

pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        pragma foreign_keys = on;

        create table if not exists accounts (
          name text primary key,
          codex_home text not null,
          status text not null default 'unknown',
          disabled integer not null default 0,
          last_checked_at text,
          last_error text,
          created_at text not null
        );

        create table if not exists pools (
          name text primary key,
          strategy text not null default 'least-sessions',
          failover text not null default 'manual'
        );

        create table if not exists pool_accounts (
          pool text not null,
          account text not null,
          priority integer not null default 100,
          primary key (pool, account)
        );

        create table if not exists sessions (
          id integer primary key autoincrement,
          name text not null unique,
          pool text,
          current_account text not null,
          cwd text not null,
          tmux_session text,
          tmux_window text,
          tmux_pane text,
          codex_args text not null,
          resume_prompt text,
          status text not null default 'running',
          created_at text not null,
          updated_at text not null
        );

        create table if not exists events (
          id integer primary key autoincrement,
          kind text not null,
          target text,
          message text,
          created_at text not null
        );
        "#,
    )?;
    Ok(())
}

pub fn upsert_account(conn: &Connection, name: &str, codex_home: PathBuf) -> Result<()> {
    conn.execute(
        r#"
        insert into accounts (name, codex_home, created_at)
        values (?1, ?2, ?3)
        on conflict(name) do update set codex_home = excluded.codex_home
        "#,
        params![name, codex_home.to_string_lossy().to_string(), util::now()],
    )?;
    Ok(())
}

pub fn get_account(conn: &Connection, name: &str) -> Result<Option<Account>> {
    conn.query_row(
        "select name, codex_home, status, disabled, last_checked_at, last_error from accounts where name = ?1",
        params![name],
        |row| {
            Ok(Account {
                name: row.get(0)?,
                codex_home: PathBuf::from(row.get::<_, String>(1)?),
                status: row.get(2)?,
                disabled: row.get::<_, i64>(3)? != 0,
                last_checked_at: row.get(4)?,
                last_error: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_accounts(conn: &Connection) -> Result<Vec<Account>> {
    let mut stmt = conn.prepare(
        "select name, codex_home, status, disabled, last_checked_at, last_error from accounts order by name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Account {
            name: row.get(0)?,
            codex_home: PathBuf::from(row.get::<_, String>(1)?),
            status: row.get(2)?,
            disabled: row.get::<_, i64>(3)? != 0,
            last_checked_at: row.get(4)?,
            last_error: row.get(5)?,
        })
    })?;

    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn set_account_status(
    conn: &Connection,
    name: &str,
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "update accounts set status = ?1, last_checked_at = ?2, last_error = ?3 where name = ?4",
        params![status, util::now(), error, name],
    )?;
    Ok(())
}

pub fn set_account_disabled(conn: &Connection, name: &str, disabled: bool) -> Result<()> {
    conn.execute(
        "update accounts set disabled = ?1 where name = ?2",
        params![if disabled { 1 } else { 0 }, name],
    )?;
    Ok(())
}

pub fn create_pool(
    conn: &Connection,
    name: &str,
    accounts: &[String],
    strategy: &str,
) -> Result<()> {
    conn.execute(
        "insert into pools (name, strategy, failover) values (?1, ?2, 'manual') on conflict(name) do update set strategy = excluded.strategy",
        params![name, strategy],
    )?;
    conn.execute("delete from pool_accounts where pool = ?1", params![name])?;
    for (idx, account) in accounts.iter().enumerate() {
        conn.execute(
            "insert into pool_accounts (pool, account, priority) values (?1, ?2, ?3)",
            params![name, account, idx as i64],
        )?;
    }
    Ok(())
}

pub fn list_pools(conn: &Connection) -> Result<Vec<Pool>> {
    let mut stmt = conn.prepare("select name, strategy, failover from pools order by name")?;
    let rows = stmt.query_map([], |row| {
        Ok(Pool {
            name: row.get(0)?,
            strategy: row.get(1)?,
            failover: row.get(2)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn get_pool(conn: &Connection, name: &str) -> Result<Option<Pool>> {
    conn.query_row(
        "select name, strategy, failover from pools where name = ?1",
        params![name],
        |row| {
            Ok(Pool {
                name: row.get(0)?,
                strategy: row.get(1)?,
                failover: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_pool_accounts(conn: &Connection, pool: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "select account from pool_accounts where pool = ?1 order by priority asc, account asc",
    )?;
    let rows = stmt.query_map(params![pool], |row| row.get::<_, String>(0))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn active_session_count(conn: &Connection, account: &str) -> Result<i64> {
    let count = conn.query_row(
        "select count(*) from sessions where current_account = ?1 and status = 'running'",
        params![account],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub fn insert_session(conn: &Connection, session: NewSession) -> Result<()> {
    let now = util::now();
    let codex_args = serde_json::to_string(&session.codex_args)?;
    conn.execute(
        r#"
        insert into sessions (
          name, pool, current_account, cwd, tmux_session, tmux_window, tmux_pane,
          codex_args, resume_prompt, status, created_at, updated_at
        ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        on conflict(name) do update set
          pool = excluded.pool,
          current_account = excluded.current_account,
          cwd = excluded.cwd,
          tmux_session = excluded.tmux_session,
          tmux_window = excluded.tmux_window,
          tmux_pane = excluded.tmux_pane,
          codex_args = excluded.codex_args,
          resume_prompt = excluded.resume_prompt,
          status = excluded.status,
          updated_at = excluded.updated_at
        "#,
        params![
            session.name,
            session.pool,
            session.current_account,
            session.cwd.to_string_lossy().to_string(),
            session.tmux_session,
            session.tmux_window,
            session.tmux_pane,
            codex_args,
            session.resume_prompt,
            session.status,
            now,
            now,
        ],
    )?;
    Ok(())
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    let codex_args_json: String = row.get(7)?;
    let codex_args: Vec<String> = serde_json::from_str(&codex_args_json).unwrap_or_default();
    Ok(Session {
        id: row.get(0)?,
        name: row.get(1)?,
        pool: row.get(2)?,
        current_account: row.get(3)?,
        cwd: PathBuf::from(row.get::<_, String>(4)?),
        tmux_session: row.get(5)?,
        tmux_window: row.get(6)?,
        tmux_pane: row.get(8)?,
        codex_args,
        resume_prompt: row.get(9)?,
        status: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

pub fn get_session(conn: &Connection, name: &str) -> Result<Option<Session>> {
    conn.query_row(
        r#"
        select id, name, pool, current_account, cwd, tmux_session, tmux_window,
               codex_args, tmux_pane, resume_prompt, status, created_at, updated_at
        from sessions where name = ?1
        "#,
        params![name],
        row_to_session,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_sessions(conn: &Connection) -> Result<Vec<Session>> {
    let mut stmt = conn.prepare(
        r#"
        select id, name, pool, current_account, cwd, tmux_session, tmux_window,
               codex_args, tmux_pane, resume_prompt, status, created_at, updated_at
        from sessions order by name
        "#,
    )?;
    let rows = stmt.query_map([], row_to_session)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn update_session_after_respawn(
    conn: &Connection,
    session_name: &str,
    account: &str,
    tmux_session: Option<&str>,
    tmux_window: Option<&str>,
    tmux_pane: Option<&str>,
    status: &str,
) -> Result<()> {
    conn.execute(
        r#"
        update sessions
        set current_account = ?1,
            tmux_session = coalesce(?2, tmux_session),
            tmux_window = coalesce(?3, tmux_window),
            tmux_pane = coalesce(?4, tmux_pane),
            status = ?5,
            updated_at = ?6
        where name = ?7
        "#,
        params![
            account,
            tmux_session,
            tmux_window,
            tmux_pane,
            status,
            util::now(),
            session_name
        ],
    )?;
    Ok(())
}

pub fn set_session_status(conn: &Connection, session_name: &str, status: &str) -> Result<()> {
    conn.execute(
        "update sessions set status = ?1, updated_at = ?2 where name = ?3",
        params![status, util::now(), session_name],
    )?;
    Ok(())
}

pub fn log_event(conn: &Connection, kind: &str, target: Option<&str>, message: &str) -> Result<()> {
    conn.execute(
        "insert into events (kind, target, message, created_at) values (?1, ?2, ?3, ?4)",
        params![kind, target, message, util::now()],
    )?;
    Ok(())
}
