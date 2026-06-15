use crate::{codex, db, pool, resume, tmux, util};
use anyhow::{anyhow, Result};
use rusqlite::Connection;

pub fn restart(conn: &Connection, session_name: &str) -> Result<()> {
    let session = db::get_session(conn, session_name)?
        .ok_or_else(|| anyhow!("unknown session `{}`", session_name))?;
    let account = db::get_account(conn, &session.current_account)?
        .ok_or_else(|| anyhow!("unknown account `{}`", session.current_account))?;
    let args = codex::same_account_resume_args();
    let command = codex::shell_command(&account.codex_home, &args);

    if let Some(pane) = &session.tmux_pane {
        if tmux::pane_exists(pane)? {
            tmux::respawn_pane(pane, &session.cwd, &command)?;
            db::update_session_after_respawn(
                conn,
                &session.name,
                &account.name,
                None,
                None,
                None,
                "running",
            )?;
            db::log_event(
                conn,
                "session.restart",
                Some(&session.name),
                "respawned pane with same account",
            )?;
            println!(
                "restarted `{}` in pane {} using account `{}`",
                session.name, pane, account.name
            );
            return Ok(());
        }
    }

    let target = tmux::new_window(&session.name, &session.cwd, &command)?;
    db::update_session_after_respawn(
        conn,
        &session.name,
        &account.name,
        Some(&target.session_name),
        Some(&target.window_id),
        Some(&target.pane_id),
        "running",
    )?;
    db::log_event(
        conn,
        "session.restart",
        Some(&session.name),
        "created new tmux window for restart",
    )?;
    println!(
        "restarted `{}` in new pane {} using account `{}`",
        session.name, target.pane_id, account.name
    );
    Ok(())
}

pub fn migrate(
    conn: &Connection,
    session_name: &str,
    target_account: Option<&str>,
    target_pool: Option<&str>,
) -> Result<()> {
    let session = db::get_session(conn, session_name)?
        .ok_or_else(|| anyhow!("unknown session `{}`", session_name))?;
    let target = pool::choose(
        conn,
        target_account,
        target_pool.or(session.pool.as_deref()),
        Some(&session.current_account),
    )?;
    migrate_to_account(conn, &session, &target.name)
}

pub fn migrate_to_account(
    conn: &Connection,
    session: &db::Session,
    target_account: &str,
) -> Result<()> {
    let target = db::get_account(conn, target_account)?
        .ok_or_else(|| anyhow!("unknown account `{}`", target_account))?;
    let same_account = session.current_account == target.name;

    let (args, command_cwd, used_native_history) = if same_account {
        (codex::same_account_resume_args(), session.cwd.clone(), true)
    } else {
        match resume::resolve_here_invocation(
            conn,
            Some((target.name.as_str(), target.codex_home.as_path())),
            &session.cwd,
            &[],
        ) {
            Ok(resolved) => (
                resolved.args,
                resolved.cwd.unwrap_or_else(|| session.cwd.clone()),
                true,
            ),
            Err(_) => (
                codex::cross_account_resume_args(
                    &session.name,
                    &session.cwd,
                    &session.current_account,
                    session.resume_prompt.as_deref(),
                ),
                session.cwd.clone(),
                false,
            ),
        }
    };

    let command = codex::shell_command(&target.codex_home, &args);

    if let Some(pane) = &session.tmux_pane {
        if tmux::pane_exists(pane)? {
            tmux::respawn_pane(pane, &command_cwd, &command)?;
            db::update_session_after_respawn(
                conn,
                &session.name,
                &target.name,
                None,
                None,
                None,
                "running",
            )?;
            db::log_event(
                conn,
                "session.migrate",
                Some(&session.name),
                &format!(
                    "respawned pane under account `{}` ({})",
                    target.name,
                    if used_native_history {
                        "native history"
                    } else {
                        "semantic prompt"
                    }
                ),
            )?;
            println!(
                "migrated `{}` in-place in pane {}: {} -> {}",
                session.name, pane, session.current_account, target.name
            );
            return Ok(());
        }
    }

    let target_tmux = tmux::new_window(&session.name, &command_cwd, &command)?;
    db::update_session_after_respawn(
        conn,
        &session.name,
        &target.name,
        Some(&target_tmux.session_name),
        Some(&target_tmux.window_id),
        Some(&target_tmux.pane_id),
        "running",
    )?;
    db::log_event(
        conn,
        "session.migrate",
        Some(&session.name),
        &format!(
            "created new tmux window under account `{}` ({})",
            target.name,
            if used_native_history {
                "native history"
            } else {
                "semantic prompt"
            }
        ),
    )?;
    println!(
        "migrated `{}` to new pane {}: {} -> {}",
        session.name, target_tmux.pane_id, session.current_account, target.name
    );
    Ok(())
}

pub fn print_sessions(conn: &Connection) -> Result<()> {
    let sessions = db::list_sessions(conn)?;
    println!(
        "{:<24} {:<14} {:<12} {:<8} {:<16} CWD",
        "NAME", "ACCOUNT", "STATUS", "PANE", "POOL"
    );
    for session in sessions {
        let pane = session.tmux_pane.clone().unwrap_or_else(|| "-".to_string());
        let exists = session
            .tmux_pane
            .as_deref()
            .map(|pane| tmux::pane_exists(pane).unwrap_or(false))
            .unwrap_or(false);
        let status = if exists {
            session.status.clone()
        } else {
            format!("{}*", session.status)
        };
        println!(
            "{:<24} {:<14} {:<12} {:<8} {:<16} {}",
            session.name,
            session.current_account,
            status,
            pane,
            session.pool.unwrap_or_else(|| "-".to_string()),
            util::display_path(&session.cwd)
        );
    }
    Ok(())
}
