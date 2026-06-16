use crate::{db, limits, resume, ui};
use anyhow::Result;
use rusqlite::Connection;

#[derive(Debug, Clone, Copy)]
pub struct IndexOptions {
    pub sessions: bool,
    pub limits: bool,
    pub rebuild: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct IndexResult {
    pub component: String,
    pub action: String,
    pub count: usize,
    pub detail: String,
}

pub fn sync_indexes(conn: &Connection, options: IndexOptions) -> Result<Vec<IndexResult>> {
    let sessions = options.sessions || !options.limits;
    let limits = options.limits || !options.sessions;
    let mut results = Vec::new();

    if sessions {
        let report = if options.dry_run {
            resume::preview_session_migration(conn)?
        } else {
            resume::sync_all_session_indexes(conn, options.rebuild)?
        };
        results.push(IndexResult {
            component: "sessions".to_string(),
            action: if options.dry_run {
                "preview".to_string()
            } else {
                action(options.rebuild)
            },
            count: report.sessions,
            detail: format!(
                "{} homes, {} imported, {} attached, {} divergent, {} failed",
                report.homes,
                report.migration.imported,
                report.migration.attached,
                report.migration.skipped_divergent,
                report.migration.failed
            ),
        });
    }

    if limits {
        if options.dry_run {
            results.push(IndexResult {
                component: "limits".to_string(),
                action: "preview".to_string(),
                count: 0,
                detail: "dry-run only previews session migration".to_string(),
            });
            return Ok(results);
        }
        let report = sync_limit_snapshots(conn, options.rebuild)?;
        results.push(IndexResult {
            component: "limits".to_string(),
            action: action(options.rebuild),
            count: report.snapshots,
            detail: format!("{} accounts, {} missing", report.accounts, report.missing),
        });
    }

    Ok(results)
}

pub fn print_index_results(results: &[IndexResult]) {
    println!("{}", ui::heading("Index"));
    let rows = results
        .iter()
        .map(|result| {
            vec![
                result.component.clone(),
                result.action.clone(),
                result.count.to_string(),
                result.detail.clone(),
            ]
        })
        .collect::<Vec<_>>();
    ui::print_table(&["COMPONENT", "ACTION", "COUNT", "DETAIL"], &rows, &[2]);
}

struct LimitIndexReport {
    accounts: usize,
    snapshots: usize,
    missing: usize,
}

fn sync_limit_snapshots(conn: &Connection, rebuild: bool) -> Result<LimitIndexReport> {
    if rebuild {
        db::clear_cached_limit_snapshots(conn)?;
    }

    let accounts = db::list_accounts(conn)?;
    let mut snapshots = 0;
    let mut missing = 0;
    for account in &accounts {
        if limits::refresh_snapshot_cache(conn, &account.codex_home)?.is_some() {
            snapshots += 1;
        } else {
            missing += 1;
        }
    }

    Ok(LimitIndexReport {
        accounts: accounts.len(),
        snapshots,
        missing,
    })
}

fn action(rebuild: bool) -> String {
    if rebuild {
        "rebuilt".to_string()
    } else {
        "synced".to_string()
    }
}
