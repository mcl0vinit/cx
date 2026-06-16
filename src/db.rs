use crate::{paths, util};
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

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

#[derive(Debug, Clone)]
pub struct IndexedCodexSession {
    pub path: PathBuf,
    pub home_path: PathBuf,
    pub home_label: String,
    pub session_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub modified_nanos: i64,
    pub size_bytes: i64,
}

#[derive(Debug, Clone)]
pub struct IndexedCodexSessionUpsert {
    pub path: PathBuf,
    pub home_path: PathBuf,
    pub home_label: String,
    pub session_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub modified_nanos: i64,
    pub size_bytes: i64,
}

#[derive(Debug, Clone)]
pub struct CanonicalCodexSession {
    pub session_id: String,
    pub canonical_path: PathBuf,
    pub modified_nanos: i64,
}

#[derive(Debug, Clone)]
pub struct CodexSessionAttachment {
    pub session_id: String,
    pub home_label: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CanonicalCodexSessionUpsert {
    pub session_id: String,
    pub canonical_path: PathBuf,
    pub source_path: PathBuf,
    pub source_home_label: String,
    pub cwd: Option<PathBuf>,
    pub modified_nanos: i64,
    pub size_bytes: i64,
}

#[derive(Debug, Clone)]
pub struct CodexSessionAttachmentUpsert {
    pub session_id: String,
    pub home_path: PathBuf,
    pub home_label: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CachedLimitSnapshot {
    pub home_path: PathBuf,
    pub observed_at: String,
    pub source_path: PathBuf,
    pub snapshot_json: String,
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

        create table if not exists codex_sessions (
          path text primary key,
          home_path text not null,
          home_label text not null,
          session_id text,
          cwd text,
          modified_nanos integer not null,
          size_bytes integer not null,
          indexed_at text not null
        );

        create index if not exists idx_codex_sessions_home
          on codex_sessions(home_path);

        create index if not exists idx_codex_sessions_session_id
          on codex_sessions(session_id);

        create index if not exists idx_codex_sessions_modified
          on codex_sessions(modified_nanos);

        create table if not exists canonical_codex_sessions (
          session_id text primary key,
          canonical_path text not null,
          source_path text not null,
          source_home_label text not null,
          cwd text,
          modified_nanos integer not null,
          size_bytes integer not null,
          created_at text not null,
          updated_at text not null
        );

        create index if not exists idx_canonical_codex_sessions_modified
          on canonical_codex_sessions(modified_nanos);

        create table if not exists codex_session_attachments (
          session_id text not null,
          home_path text not null,
          home_label text not null,
          path text not null,
          attached_at text not null,
          primary key (session_id, home_path)
        );

        create index if not exists idx_codex_session_attachments_path
          on codex_session_attachments(path);

        create table if not exists limit_snapshots (
          home_path text primary key,
          observed_at text not null,
          source_path text not null,
          snapshot_json text not null,
          indexed_at text not null
        );

        create index if not exists idx_limit_snapshots_observed
          on limit_snapshots(observed_at);
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

pub fn rename_account(
    conn: &Connection,
    old_name: &str,
    new_name: &str,
    new_codex_home: PathBuf,
) -> Result<()> {
    conn.execute_batch("begin immediate")?;
    let result = rename_account_rows(conn, old_name, new_name, new_codex_home);
    match result {
        Ok(()) => {
            conn.execute_batch("commit")?;
            Ok(())
        }
        Err(error) => {
            let _ = conn.execute_batch("rollback");
            Err(error)
        }
    }
}

fn rename_account_rows(
    conn: &Connection,
    old_name: &str,
    new_name: &str,
    new_codex_home: PathBuf,
) -> Result<()> {
    let updated = conn.execute(
        "update accounts set name = ?1, codex_home = ?2 where name = ?3",
        params![
            new_name,
            new_codex_home.to_string_lossy().to_string(),
            old_name
        ],
    )?;
    if updated == 0 {
        return Err(anyhow!("unknown account `{}`", old_name));
    }
    conn.execute(
        "update pool_accounts set account = ?1 where account = ?2",
        params![new_name, old_name],
    )?;
    conn.execute(
        "update sessions set current_account = ?1, updated_at = ?2 where current_account = ?3",
        params![new_name, util::now(), old_name],
    )?;
    conn.execute(
        "update events set target = ?1 where target = ?2 and kind like 'account.%'",
        params![new_name, old_name],
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

pub fn list_indexed_codex_sessions_for_homes(
    conn: &Connection,
    home_paths: &[PathBuf],
) -> Result<Vec<IndexedCodexSession>> {
    let wanted = home_paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<HashSet<_>>();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        r#"
        select path, home_path, home_label, session_id, cwd, modified_nanos, size_bytes
        from codex_sessions
        "#,
    )?;
    let rows = stmt.query_map([], row_to_indexed_codex_session)?;
    let sessions = rows
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|session| wanted.contains(&session.home_path.to_string_lossy().to_string()))
        .collect();
    Ok(sessions)
}

pub fn list_indexed_codex_sessions_for_home(
    conn: &Connection,
    home_path: &Path,
) -> Result<Vec<IndexedCodexSession>> {
    let mut stmt = conn.prepare(
        r#"
        select path, home_path, home_label, session_id, cwd, modified_nanos, size_bytes
        from codex_sessions
        where home_path = ?1
        "#,
    )?;
    let rows = stmt.query_map(
        params![home_path.to_string_lossy().to_string()],
        row_to_indexed_codex_session,
    )?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn count_indexed_codex_sessions_for_home(conn: &Connection, home_path: &Path) -> Result<i64> {
    let count = conn.query_row(
        "select count(*) from codex_sessions where home_path = ?1",
        params![home_path.to_string_lossy().to_string()],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub fn upsert_indexed_codex_session(
    conn: &Connection,
    session: IndexedCodexSessionUpsert,
) -> Result<()> {
    conn.execute(
        r#"
        insert into codex_sessions (
          path, home_path, home_label, session_id, cwd, modified_nanos, size_bytes, indexed_at
        ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        on conflict(path) do update set
          home_path = excluded.home_path,
          home_label = excluded.home_label,
          session_id = excluded.session_id,
          cwd = excluded.cwd,
          modified_nanos = excluded.modified_nanos,
          size_bytes = excluded.size_bytes,
          indexed_at = excluded.indexed_at
        "#,
        params![
            session.path.to_string_lossy().to_string(),
            session.home_path.to_string_lossy().to_string(),
            session.home_label,
            session.session_id,
            session.cwd.map(|cwd| cwd.to_string_lossy().to_string()),
            session.modified_nanos,
            session.size_bytes,
            util::now(),
        ],
    )?;
    Ok(())
}

pub fn delete_indexed_codex_session(conn: &Connection, path: &Path) -> Result<()> {
    conn.execute(
        "delete from codex_sessions where path = ?1",
        params![path.to_string_lossy().to_string()],
    )?;
    Ok(())
}

pub fn delete_indexed_codex_sessions_for_home(conn: &Connection, home_path: &Path) -> Result<()> {
    conn.execute(
        "delete from codex_sessions where home_path = ?1",
        params![home_path.to_string_lossy().to_string()],
    )?;
    Ok(())
}

pub fn clear_indexed_codex_sessions(conn: &Connection) -> Result<()> {
    conn.execute("delete from codex_sessions", [])?;
    Ok(())
}

pub fn get_canonical_codex_session(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<CanonicalCodexSession>> {
    conn.query_row(
        r#"
        select session_id, canonical_path, source_path, source_home_label,
               cwd, modified_nanos, size_bytes
        from canonical_codex_sessions
        where session_id = ?1
        "#,
        params![session_id],
        row_to_canonical_codex_session,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_canonical_codex_sessions(conn: &Connection) -> Result<Vec<CanonicalCodexSession>> {
    let mut stmt = conn.prepare(
        r#"
        select session_id, canonical_path, source_path, source_home_label,
               cwd, modified_nanos, size_bytes
        from canonical_codex_sessions
        order by modified_nanos desc
        "#,
    )?;
    let rows = stmt.query_map([], row_to_canonical_codex_session)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn upsert_canonical_codex_session(
    conn: &Connection,
    session: CanonicalCodexSessionUpsert,
) -> Result<()> {
    let now = util::now();
    conn.execute(
        r#"
        insert into canonical_codex_sessions (
          session_id, canonical_path, source_path, source_home_label,
          cwd, modified_nanos, size_bytes, created_at, updated_at
        ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
        on conflict(session_id) do update set
          canonical_path = excluded.canonical_path,
          source_path = excluded.source_path,
          source_home_label = excluded.source_home_label,
          cwd = excluded.cwd,
          modified_nanos = excluded.modified_nanos,
          size_bytes = excluded.size_bytes,
          updated_at = excluded.updated_at
        "#,
        params![
            session.session_id,
            session.canonical_path.to_string_lossy().to_string(),
            session.source_path.to_string_lossy().to_string(),
            session.source_home_label,
            session.cwd.map(|cwd| cwd.to_string_lossy().to_string()),
            session.modified_nanos,
            session.size_bytes,
            now,
        ],
    )?;
    Ok(())
}

pub fn upsert_codex_session_attachment(
    conn: &Connection,
    attachment: CodexSessionAttachmentUpsert,
) -> Result<()> {
    conn.execute(
        r#"
        insert into codex_session_attachments (
          session_id, home_path, home_label, path, attached_at
        ) values (?1, ?2, ?3, ?4, ?5)
        on conflict(session_id, home_path) do update set
          home_label = excluded.home_label,
          path = excluded.path,
          attached_at = excluded.attached_at
        "#,
        params![
            attachment.session_id,
            attachment.home_path.to_string_lossy().to_string(),
            attachment.home_label,
            attachment.path.to_string_lossy().to_string(),
            util::now(),
        ],
    )?;
    Ok(())
}

pub fn list_codex_session_attachments(conn: &Connection) -> Result<Vec<CodexSessionAttachment>> {
    let mut stmt = conn.prepare(
        r#"
        select session_id, home_label, path
        from codex_session_attachments
        order by home_label, session_id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(CodexSessionAttachment {
            session_id: row.get(0)?,
            home_label: row.get(1)?,
            path: PathBuf::from(row.get::<_, String>(2)?),
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn get_cached_limit_snapshot(
    conn: &Connection,
    home_path: &Path,
) -> Result<Option<CachedLimitSnapshot>> {
    conn.query_row(
        r#"
        select home_path, observed_at, source_path, snapshot_json
        from limit_snapshots
        where home_path = ?1
        "#,
        params![home_path.to_string_lossy().to_string()],
        |row| {
            Ok(CachedLimitSnapshot {
                home_path: PathBuf::from(row.get::<_, String>(0)?),
                observed_at: row.get(1)?,
                source_path: PathBuf::from(row.get::<_, String>(2)?),
                snapshot_json: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn upsert_cached_limit_snapshot(
    conn: &Connection,
    home_path: &Path,
    observed_at: &str,
    source_path: &Path,
    snapshot_json: &str,
) -> Result<()> {
    conn.execute(
        r#"
        insert into limit_snapshots (
          home_path, observed_at, source_path, snapshot_json, indexed_at
        ) values (?1, ?2, ?3, ?4, ?5)
        on conflict(home_path) do update set
          observed_at = excluded.observed_at,
          source_path = excluded.source_path,
          snapshot_json = excluded.snapshot_json,
          indexed_at = excluded.indexed_at
        "#,
        params![
            home_path.to_string_lossy().to_string(),
            observed_at,
            source_path.to_string_lossy().to_string(),
            snapshot_json,
            util::now(),
        ],
    )?;
    Ok(())
}

pub fn delete_cached_limit_snapshot(conn: &Connection, home_path: &Path) -> Result<()> {
    conn.execute(
        "delete from limit_snapshots where home_path = ?1",
        params![home_path.to_string_lossy().to_string()],
    )?;
    Ok(())
}

pub fn clear_cached_limit_snapshots(conn: &Connection) -> Result<()> {
    conn.execute("delete from limit_snapshots", [])?;
    Ok(())
}

fn row_to_indexed_codex_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndexedCodexSession> {
    Ok(IndexedCodexSession {
        path: PathBuf::from(row.get::<_, String>(0)?),
        home_path: PathBuf::from(row.get::<_, String>(1)?),
        home_label: row.get(2)?,
        session_id: row.get(3)?,
        cwd: row.get::<_, Option<String>>(4)?.map(PathBuf::from),
        modified_nanos: row.get(5)?,
        size_bytes: row.get(6)?,
    })
}

fn row_to_canonical_codex_session(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CanonicalCodexSession> {
    Ok(CanonicalCodexSession {
        session_id: row.get(0)?,
        canonical_path: PathBuf::from(row.get::<_, String>(1)?),
        modified_nanos: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_account_updates_registry_references() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();

        upsert_account(&conn, "old", PathBuf::from("/tmp/cx-old")).unwrap();
        create_pool(&conn, "coding", &["old".to_string()], "limit-aware").unwrap();
        insert_session(
            &conn,
            NewSession {
                name: "old".to_string(),
                pool: Some("coding".to_string()),
                current_account: "old".to_string(),
                cwd: PathBuf::from("/tmp/project"),
                tmux_session: Some("tmux".to_string()),
                tmux_window: Some("@1".to_string()),
                tmux_pane: Some("%1".to_string()),
                codex_args: vec!["exec".to_string()],
                resume_prompt: None,
                status: "running".to_string(),
            },
        )
        .unwrap();
        log_event(&conn, "account.disabled", Some("old"), "disabled").unwrap();
        log_event(&conn, "session.start", Some("old"), "started").unwrap();

        rename_account(&conn, "old", "new", PathBuf::from("/tmp/cx-new")).unwrap();

        assert!(get_account(&conn, "old").unwrap().is_none());
        assert_eq!(
            get_account(&conn, "new")
                .unwrap()
                .unwrap()
                .codex_home
                .to_string_lossy(),
            "/tmp/cx-new"
        );
        assert_eq!(get_pool_accounts(&conn, "coding").unwrap(), vec!["new"]);
        assert_eq!(
            get_session(&conn, "old").unwrap().unwrap().current_account,
            "new"
        );

        let account_target: String = conn
            .query_row(
                "select target from events where kind = 'account.disabled'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let session_target: String = conn
            .query_row(
                "select target from events where kind = 'session.start'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(account_target, "new");
        assert_eq!(session_target, "old");
    }
}
