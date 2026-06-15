use crate::util;
use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct LimitSnapshot {
    pub observed_at: DateTime<Utc>,
    pub source: PathBuf,
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub plan_type: Option<String>,
    pub primary: Option<LimitWindow>,
    pub secondary: Option<LimitWindow>,
    pub credits: Option<Credits>,
}

#[derive(Debug, Clone)]
pub struct LimitWindow {
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct Credits {
    pub has_credits: Option<bool>,
    pub unlimited: Option<bool>,
    pub balance: Option<String>,
}

pub fn latest_snapshot(codex_home: &Path) -> Result<Option<LimitSnapshot>> {
    let session_files = session_files(&codex_home.join("sessions"))?;
    let mut latest: Option<LimitSnapshot> = None;

    for file in session_files {
        let fallback_time = file_modified_at(&file);
        for snapshot in snapshots_in_file(&file, fallback_time)? {
            if latest
                .as_ref()
                .map(|current| snapshot.observed_at > current.observed_at)
                .unwrap_or(true)
            {
                latest = Some(snapshot);
            }
        }
    }

    Ok(latest)
}

pub fn remaining_percent(window: &LimitWindow) -> f64 {
    (100.0 - window.used_percent).max(0.0)
}

pub fn is_exhausted(window: &LimitWindow) -> bool {
    window.used_percent >= 99.5
        && window
            .resets_at
            .map(|reset| reset > Utc::now())
            .unwrap_or(true)
}

pub fn compact_reset(window: Option<&LimitWindow>) -> String {
    window
        .and_then(|window| window.resets_at)
        .map(compact_duration_until)
        .unwrap_or_else(|| "-".to_string())
}

pub fn compact_observed_age(snapshot: Option<&LimitSnapshot>) -> String {
    snapshot
        .map(|snapshot| compact_duration_since(snapshot.observed_at))
        .unwrap_or_else(|| "-".to_string())
}

pub fn compact_remaining(window: Option<&LimitWindow>) -> String {
    window
        .map(|window| format!("{:.0}%", remaining_percent(window)))
        .unwrap_or_else(|| "-".to_string())
}

pub fn is_stale(snapshot: Option<&LimitSnapshot>, max_age_minutes: i64) -> bool {
    let Some(snapshot) = snapshot else {
        return true;
    };
    (Utc::now() - snapshot.observed_at).num_minutes() > max_age_minutes
}

pub fn print_snapshot(snapshot: &LimitSnapshot) {
    println!("Limits");
    println!(
        "{:<18} {}",
        "Observed",
        format_datetime(snapshot.observed_at)
    );
    println!("{:<18} {}", "Source", util::display_path(&snapshot.source));
    println!(
        "{:<18} {}",
        "Limit",
        snapshot
            .limit_name
            .as_deref()
            .or(snapshot.limit_id.as_deref())
            .unwrap_or("-")
    );
    println!(
        "{:<18} {}",
        "Plan",
        snapshot.plan_type.as_deref().unwrap_or("-")
    );

    if let Some(primary) = &snapshot.primary {
        print_window("5h limit", primary);
    }
    if let Some(secondary) = &snapshot.secondary {
        print_window("Weekly limit", secondary);
    }
    if let Some(credits) = &snapshot.credits {
        print_credits(credits);
    }
}

fn print_window(label: &str, window: &LimitWindow) {
    let window_label = window
        .window_minutes
        .map(window_name)
        .unwrap_or_else(|| "unknown window".to_string());
    let remaining = remaining_percent(window);
    let reset = window
        .resets_at
        .map(format_reset)
        .unwrap_or_else(|| "-".to_string());
    println!(
        "{:<18} {:.1}% remaining, resets {} ({})",
        label, remaining, reset, window_label
    );
}

fn print_credits(credits: &Credits) {
    let status = match (credits.unlimited, credits.has_credits) {
        (Some(true), _) => "unlimited".to_string(),
        (_, Some(true)) => "available".to_string(),
        (_, Some(false)) => "none".to_string(),
        _ => "unknown".to_string(),
    };
    println!("{:<18} {}", "Credits", status);
    if let Some(balance) = &credits.balance {
        println!("{:<18} {}", "Credit balance", balance);
    }
}

fn snapshots_in_file(path: &Path, fallback_time: DateTime<Utc>) -> Result<Vec<LimitSnapshot>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut snapshots = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if !line.contains("\"rate_limits\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(snapshot) = snapshot_from_value(&value, path, fallback_time) {
            snapshots.push(snapshot);
        }
    }

    Ok(snapshots)
}

fn snapshot_from_value(
    value: &Value,
    source: &Path,
    fallback_time: DateTime<Utc>,
) -> Option<LimitSnapshot> {
    let rate_limits = value.get("payload")?.get("rate_limits")?;
    let observed_at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339)
        .unwrap_or(fallback_time);

    Some(LimitSnapshot {
        observed_at,
        source: source.to_path_buf(),
        limit_id: rate_limits.get("limit_id").and_then(opt_string),
        limit_name: rate_limits.get("limit_name").and_then(opt_string),
        plan_type: rate_limits.get("plan_type").and_then(opt_string),
        primary: rate_limits.get("primary").and_then(window_from_value),
        secondary: rate_limits.get("secondary").and_then(window_from_value),
        credits: rate_limits.get("credits").and_then(credits_from_value),
    })
}

fn window_from_value(value: &Value) -> Option<LimitWindow> {
    Some(LimitWindow {
        used_percent: value.get("used_percent")?.as_f64()?,
        window_minutes: value.get("window_minutes").and_then(Value::as_u64),
        resets_at: value
            .get("resets_at")
            .and_then(Value::as_i64)
            .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0)),
    })
}

fn credits_from_value(value: &Value) -> Option<Credits> {
    Some(Credits {
        has_credits: value.get("has_credits").and_then(Value::as_bool),
        unlimited: value.get("unlimited").and_then(Value::as_bool),
        balance: value.get("balance").and_then(value_to_string),
    })
}

fn value_to_string(value: &Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| Some(value.to_string()))
}

fn opt_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.with_timezone(&Utc))
}

fn file_modified_at(path: &Path) -> DateTime<Utc> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now())
}

fn session_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_session_files(root, &mut files)?;
    Ok(files)
}

fn collect_session_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_session_files(&path, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }

    Ok(())
}

fn window_name(minutes: u64) -> String {
    match minutes {
        300 => "5h window".to_string(),
        10_080 => "weekly window".to_string(),
        value if value % 1_440 == 0 => format!("{}d window", value / 1_440),
        value if value % 60 == 0 => format!("{}h window", value / 60),
        value => format!("{value}m window"),
    }
}

fn format_datetime(datetime: DateTime<Utc>) -> String {
    datetime
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M:%S %Z")
        .to_string()
}

fn format_reset(datetime: DateTime<Utc>) -> String {
    let suffix = duration_suffix(datetime);
    format!("{} {}", format_datetime(datetime), suffix)
}

fn duration_suffix(datetime: DateTime<Utc>) -> String {
    let seconds = (datetime - Utc::now()).num_seconds();
    if seconds <= 0 {
        return "(expired)".to_string();
    }

    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;

    if days > 0 {
        format!("(in {days}d {hours}h)")
    } else if hours > 0 {
        format!("(in {hours}h {minutes}m)")
    } else {
        format!("(in {minutes}m)")
    }
}

fn compact_duration_until(datetime: DateTime<Utc>) -> String {
    compact_duration((datetime - Utc::now()).num_seconds(), "now")
}

fn compact_duration_since(datetime: DateTime<Utc>) -> String {
    compact_duration((Utc::now() - datetime).num_seconds(), "now")
}

fn compact_duration(seconds: i64, zero: &str) -> String {
    if seconds <= 0 {
        return zero.to_string();
    }

    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;

    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("cx-{name}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn latest_snapshot_reads_newest_rate_limits() {
        let root = temp_root("limits");
        let file = root
            .join("sessions")
            .join("2026")
            .join("06")
            .join("15")
            .join("rollout.jsonl");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            [
                r#"{"timestamp":"2026-06-15T12:00:00Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":10.0,"window_minutes":300,"resets_at":1781528400},"secondary":{"used_percent":20.0,"window_minutes":10080,"resets_at":1782133200},"credits":{"has_credits":true,"unlimited":false,"balance":"12.50"},"plan_type":"plus"}}}"#,
                r#"{"timestamp":"2026-06-15T12:05:00Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":40.0,"window_minutes":300,"resets_at":1781528400},"secondary":{"used_percent":50.0,"window_minutes":10080,"resets_at":1782133200},"credits":{"has_credits":true,"unlimited":false,"balance":"11.25"},"plan_type":"plus"}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let snapshot = latest_snapshot(&root).unwrap().unwrap();

        assert_eq!(snapshot.limit_id.as_deref(), Some("codex"));
        assert_eq!(snapshot.primary.unwrap().used_percent, 40.0);
        assert_eq!(snapshot.secondary.unwrap().used_percent, 50.0);
        assert_eq!(snapshot.credits.unwrap().balance.as_deref(), Some("11.25"));

        let _ = fs::remove_dir_all(root);
    }
}
