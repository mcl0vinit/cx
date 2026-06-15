use crate::{db, limits, ui, util};
use anyhow::Result;
use rusqlite::Connection;
use std::{
    io::{self, Write},
    thread,
    time::Duration,
};

pub fn watch(conn: &Connection, interval_secs: u64, once: bool) -> Result<()> {
    loop {
        if !once {
            print!("\x1b[2J\x1b[H");
        }
        print_once(conn)?;
        io::stdout().flush()?;

        if once {
            return Ok(());
        }

        thread::sleep(Duration::from_secs(interval_secs.max(1)));
    }
}

pub fn print_once(conn: &Connection) -> Result<()> {
    println!("{}", ui::heading("Dashboard"));
    println!(
        "Updated {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z")
    );

    let accounts = db::list_accounts(conn)?;
    if accounts.is_empty() {
        println!("No accounts registered.");
        return Ok(());
    }

    let rows = accounts
        .into_iter()
        .map(|account| {
            let active = db::active_session_count(conn, &account.name)?;
            let snapshot = limits::latest_snapshot(&account.codex_home)?;
            let status = if account.disabled {
                "disabled".to_string()
            } else {
                account.status.clone()
            };
            let five = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.primary.as_ref());
            let weekly = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.secondary.as_ref());

            Ok(vec![
                account.name,
                status,
                limits::compact_remaining(five),
                limits::compact_remaining(weekly),
                limits::compact_reset(five),
                limits::compact_reset(weekly),
                active.to_string(),
                limits::compact_observed_age(snapshot.as_ref()),
                util::display_path(&account.codex_home),
            ])
        })
        .collect::<Result<Vec<_>>>()?;

    ui::print_table(
        &[
            "ACCOUNT", "STATUS", "5H LEFT", "WK LEFT", "5H RESET", "WK RESET", "ACTIVE",
            "OBSERVED", "HOME",
        ],
        &rows,
        &[6],
    );

    Ok(())
}
