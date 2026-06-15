use crate::{db, limits, util};
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
    println!(
        "Updated {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z")
    );
    println!();
    println!(
        "{:<16} {:<12} {:>7} {:>7} {:>10} {:>10} {:>7} {:>9} HOME",
        "ACCOUNT", "STATUS", "5H LEFT", "WK LEFT", "5H RESET", "WK RESET", "ACTIVE", "OBSERVED"
    );

    for account in db::list_accounts(conn)? {
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

        println!(
            "{:<16} {:<12} {:>7} {:>7} {:>10} {:>10} {:>7} {:>9} {}",
            account.name,
            status,
            limits::compact_remaining(five),
            limits::compact_remaining(weekly),
            limits::compact_reset(five),
            limits::compact_reset(weekly),
            active,
            limits::compact_observed_age(snapshot.as_ref()),
            util::display_path(&account.codex_home)
        );
    }

    Ok(())
}
