use crate::{db, util};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cmp::Reverse,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::SystemTime,
};

const RECENT_LIMIT_SCAN_FILES: usize = 64;
const DEFAULT_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api";
const CHATGPT_TOKEN_REFRESH_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitWindow {
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credits {
    pub has_credits: Option<bool>,
    pub unlimited: Option<bool>,
    pub balance: Option<String>,
}

#[cfg(test)]
fn latest_snapshot(codex_home: &Path) -> Result<Option<LimitSnapshot>> {
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

pub fn latest_snapshot_cached(
    conn: &Connection,
    codex_home: &Path,
) -> Result<Option<LimitSnapshot>> {
    let home_path = cache_home_path(codex_home);
    if let Some(cached) = db::get_cached_limit_snapshot(conn, &home_path)? {
        if cached.source_path.exists() {
            match serde_json::from_str::<LimitSnapshot>(&cached.snapshot_json) {
                Ok(snapshot) => return Ok(Some(snapshot)),
                Err(error) => {
                    tracing::debug!(
                        home = %cached.home_path.display(),
                        observed_at = %cached.observed_at,
                        error = %error,
                        "discarding invalid cached limit snapshot"
                    );
                }
            }
        }
        db::delete_cached_limit_snapshot(conn, &home_path)?;
    }

    refresh_snapshot_cache(conn, codex_home)
}

pub fn refresh_snapshot_cache(
    conn: &Connection,
    codex_home: &Path,
) -> Result<Option<LimitSnapshot>> {
    let snapshot = latest_snapshot_recent(codex_home, RECENT_LIMIT_SCAN_FILES)?;
    if let Some(snapshot) = &snapshot {
        write_snapshot_cache(conn, codex_home, snapshot)?;
    }
    Ok(snapshot)
}

pub fn refresh_snapshot_from_backend(
    conn: &Connection,
    codex_home: &Path,
) -> Result<LimitSnapshot> {
    let snapshot = backend_snapshot(codex_home)?;
    write_snapshot_cache(conn, codex_home, &snapshot)?;
    Ok(snapshot)
}

fn backend_snapshot(codex_home: &Path) -> Result<LimitSnapshot> {
    let auth_path = codex_home.join("auth.json");
    let usage_url = usage_url(codex_home);
    let mut auth = read_auth_json(&auth_path)?;
    let mut tokens = auth_tokens(&auth)?;

    let payload = match fetch_backend_usage(
        &usage_url,
        &tokens.access_token,
        tokens.account_id.as_deref(),
    ) {
        Ok(payload) => payload,
        Err(error) if error.is_unauthorized() && tokens.refresh_token.is_some() => {
            auth = refresh_auth_json(&auth_path, &auth, tokens.refresh_token.take().unwrap())?;
            tokens = auth_tokens(&auth)?;
            fetch_backend_usage(
                &usage_url,
                &tokens.access_token,
                tokens.account_id.as_deref(),
            )
            .map_err(BackendFetchError::into_error)?
        }
        Err(error) => return Err(error.into_error()),
    };

    Ok(snapshot_from_backend_payload(
        payload,
        &auth_path,
        Utc::now(),
    ))
}

fn usage_url(codex_home: &Path) -> String {
    let mut base_url = config_chatgpt_base_url(codex_home)
        .unwrap_or_else(|| DEFAULT_CHATGPT_BASE_URL.to_string())
        .trim_end_matches('/')
        .to_string();
    if (base_url.starts_with("https://chatgpt.com")
        || base_url.starts_with("https://chat.openai.com"))
        && !base_url.contains("/backend-api")
    {
        base_url = format!("{base_url}/backend-api");
    }

    if base_url.contains("/backend-api") {
        format!("{base_url}/wham/usage")
    } else {
        format!("{base_url}/api/codex/usage")
    }
}

fn config_chatgpt_base_url(codex_home: &Path) -> Option<String> {
    let text = fs::read_to_string(codex_home.join("config.toml")).ok()?;
    let value = text.parse::<toml::Value>().ok()?;
    value
        .get("chatgpt_base_url")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Debug)]
struct AuthTokens {
    access_token: String,
    refresh_token: Option<String>,
    account_id: Option<String>,
}

fn read_auth_json(auth_path: &Path) -> Result<Value> {
    let text = fs::read_to_string(auth_path)
        .with_context(|| format!("failed to read {}", auth_path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("failed to parse {}", auth_path.display()))
}

fn auth_tokens(auth: &Value) -> Result<AuthTokens> {
    let tokens = auth
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("auth.json does not contain ChatGPT tokens"))?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| anyhow!("auth.json does not contain a ChatGPT access token"))?
        .to_string();
    let refresh_token = tokens
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .map(ToOwned::to_owned);
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .filter(|account_id| !account_id.trim().is_empty())
        .map(ToOwned::to_owned);

    Ok(AuthTokens {
        access_token,
        refresh_token,
        account_id,
    })
}

#[derive(Debug)]
enum BackendFetchError {
    Status(u16, String),
    Other(anyhow::Error),
}

impl BackendFetchError {
    fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Status(401, _))
    }

    fn into_error(self) -> anyhow::Error {
        match self {
            Self::Status(status, body) => {
                anyhow!(
                    "Codex usage endpoint returned HTTP {status}: {}",
                    first_line(&body)
                )
            }
            Self::Other(error) => error,
        }
    }
}

fn fetch_backend_usage(
    usage_url: &str,
    access_token: &str,
    account_id: Option<&str>,
) -> std::result::Result<BackendUsagePayload, BackendFetchError> {
    let mut request = ureq::get(usage_url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("User-Agent", "codex-cli");
    if let Some(account_id) = account_id {
        request = request.set("ChatGPT-Account-ID", account_id);
    }

    let response = request.call().map_err(backend_fetch_error)?;
    let body = response
        .into_string()
        .map_err(|error| BackendFetchError::Other(error.into()))?;
    serde_json::from_str::<BackendUsagePayload>(&body)
        .map_err(|error| BackendFetchError::Other(error.into()))
}

fn backend_fetch_error(error: ureq::Error) -> BackendFetchError {
    match error {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            BackendFetchError::Status(status, body)
        }
        ureq::Error::Transport(error) => BackendFetchError::Other(error.into()),
    }
}

fn refresh_auth_json(auth_path: &Path, auth: &Value, refresh_token: String) -> Result<Value> {
    let body = json!({
        "client_id": CODEX_OAUTH_CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    });
    let response = ureq::post(CHATGPT_TOKEN_REFRESH_URL)
        .set("Content-Type", "application/json")
        .set("User-Agent", "codex-cli")
        .send_string(&serde_json::to_string(&body)?)
        .map_err(backend_fetch_error)
        .map_err(BackendFetchError::into_error)?;
    let response_body = response
        .into_string()
        .context("failed to read refreshed Codex auth tokens")?;
    let refresh = serde_json::from_str::<Value>(&response_body)
        .context("failed to parse refreshed Codex auth tokens")?;

    let mut updated = auth.clone();
    let tokens = updated
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("auth.json does not contain ChatGPT tokens"))?;
    for key in ["id_token", "access_token", "refresh_token"] {
        if let Some(value) = refresh.get(key).and_then(Value::as_str) {
            tokens.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
    updated["last_refresh"] = Value::String(Utc::now().to_rfc3339());
    fs::write(auth_path, serde_json::to_string_pretty(&updated)?)
        .with_context(|| format!("failed to update {}", auth_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(auth_path, fs::Permissions::from_mode(0o600));
    }

    Ok(updated)
}

fn first_line(value: &str) -> String {
    value
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("empty response")
        .trim()
        .to_string()
}

#[derive(Debug, Deserialize)]
struct BackendUsagePayload {
    plan_type: Option<String>,
    rate_limit: Option<BackendRateLimit>,
    credits: Option<BackendCredits>,
}

#[derive(Debug, Deserialize)]
struct BackendRateLimit {
    primary_window: Option<BackendRateLimitWindow>,
    secondary_window: Option<BackendRateLimitWindow>,
}

#[derive(Debug, Deserialize)]
struct BackendRateLimitWindow {
    used_percent: f64,
    reset_at: Option<i64>,
    limit_window_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct BackendCredits {
    has_credits: Option<bool>,
    unlimited: Option<bool>,
    balance: Option<Value>,
}

fn snapshot_from_backend_payload(
    payload: BackendUsagePayload,
    source: &Path,
    observed_at: DateTime<Utc>,
) -> LimitSnapshot {
    LimitSnapshot {
        observed_at,
        source: source.to_path_buf(),
        limit_id: Some("codex".to_string()),
        limit_name: None,
        plan_type: payload.plan_type,
        primary: payload
            .rate_limit
            .as_ref()
            .and_then(|limits| limits.primary_window.as_ref())
            .map(window_from_backend),
        secondary: payload
            .rate_limit
            .as_ref()
            .and_then(|limits| limits.secondary_window.as_ref())
            .map(window_from_backend),
        credits: payload.credits.as_ref().map(credits_from_backend),
    }
}

fn window_from_backend(value: &BackendRateLimitWindow) -> LimitWindow {
    LimitWindow {
        used_percent: value.used_percent,
        window_minutes: value.limit_window_seconds.map(|seconds| seconds / 60),
        resets_at: value
            .reset_at
            .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0)),
    }
}

fn credits_from_backend(value: &BackendCredits) -> Credits {
    Credits {
        has_credits: value.has_credits,
        unlimited: value.unlimited,
        balance: value.balance.as_ref().and_then(value_to_string),
    }
}

pub fn remaining_percent(window: &LimitWindow) -> f64 {
    (100.0 - window.used_percent).max(0.0)
}

fn write_snapshot_cache(
    conn: &Connection,
    codex_home: &Path,
    snapshot: &LimitSnapshot,
) -> Result<()> {
    let home_path = cache_home_path(codex_home);
    let snapshot_json = serde_json::to_string(snapshot).context("failed to serialize limits")?;
    db::upsert_cached_limit_snapshot(
        conn,
        &home_path,
        &snapshot.observed_at.to_rfc3339(),
        &snapshot.source,
        &snapshot_json,
    )
}

fn latest_snapshot_recent(codex_home: &Path, limit: usize) -> Result<Option<LimitSnapshot>> {
    let session_files = recent_session_files(&codex_home.join("sessions"), limit)?;
    latest_snapshot_in_files(session_files)
}

fn latest_snapshot_in_files(session_files: Vec<PathBuf>) -> Result<Option<LimitSnapshot>> {
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

fn recent_session_files(root: &Path, limit: usize) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_session_files_with_modified(root, &mut files)?;
    files.sort_by_key(|(modified, _)| Reverse(*modified));
    Ok(files
        .into_iter()
        .take(limit)
        .map(|(_, path)| path)
        .collect())
}

fn collect_session_files_with_modified(
    dir: &Path,
    files: &mut Vec<(SystemTime, PathBuf)>,
) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_session_files_with_modified(&path, files)?;
            continue;
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }

        let modified = fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((modified, path));
    }

    Ok(())
}

fn cache_home_path(codex_home: &Path) -> PathBuf {
    fs::canonicalize(codex_home).unwrap_or_else(|_| codex_home.to_path_buf())
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

#[cfg(test)]
fn session_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_session_files(root, &mut files)?;
    Ok(files)
}

#[cfg(test)]
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
    use rusqlite::Connection;
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

    #[test]
    fn cached_snapshot_avoids_rescanning_until_refresh() {
        let root = temp_root("limit-cache");
        let file = root
            .join("sessions")
            .join("2026")
            .join("06")
            .join("15")
            .join("rollout.jsonl");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            r#"{"timestamp":"2026-06-15T12:00:00Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":10.0,"window_minutes":300,"resets_at":1781528400}}}}"#,
        )
        .unwrap();

        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();

        let cached = latest_snapshot_cached(&conn, &root).unwrap().unwrap();
        assert_eq!(cached.primary.unwrap().used_percent, 10.0);

        fs::write(
            &file,
            r#"{"timestamp":"2026-06-15T12:05:00Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","primary":{"used_percent":80.0,"window_minutes":300,"resets_at":1781528400}}}}"#,
        )
        .unwrap();

        let cached = latest_snapshot_cached(&conn, &root).unwrap().unwrap();
        assert_eq!(cached.primary.unwrap().used_percent, 10.0);

        let refreshed = refresh_snapshot_cache(&conn, &root).unwrap().unwrap();
        assert_eq!(refreshed.primary.unwrap().used_percent, 80.0);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn backend_usage_maps_primary_to_five_hour_and_secondary_to_weekly() {
        let payload = serde_json::from_str::<BackendUsagePayload>(
            r#"{
                "plan_type": "pro",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 100,
                        "reset_at": 1781652339,
                        "limit_window_seconds": 18000
                    },
                    "secondary_window": {
                        "used_percent": 71,
                        "reset_at": 1781745507,
                        "limit_window_seconds": 604800
                    }
                },
                "credits": {
                    "has_credits": false,
                    "unlimited": false,
                    "balance": "0"
                }
            }"#,
        )
        .unwrap();

        let snapshot = snapshot_from_backend_payload(
            payload,
            Path::new("/tmp/auth.json"),
            DateTime::<Utc>::from_timestamp(1781640000, 0).unwrap(),
        );

        assert_eq!(snapshot.limit_id.as_deref(), Some("codex"));
        assert_eq!(snapshot.plan_type.as_deref(), Some("pro"));
        let primary = snapshot.primary.unwrap();
        assert_eq!(primary.used_percent, 100.0);
        assert_eq!(primary.window_minutes, Some(300));
        let secondary = snapshot.secondary.unwrap();
        assert_eq!(secondary.used_percent, 71.0);
        assert_eq!(secondary.window_minutes, Some(10_080));
        assert_eq!(snapshot.credits.unwrap().balance.as_deref(), Some("0"));
    }

    #[test]
    fn usage_url_matches_codex_backend_path_styles() {
        let default_home = temp_root("usage-default");
        assert_eq!(
            usage_url(&default_home),
            "https://chatgpt.com/backend-api/wham/usage"
        );

        let chatgpt_home = temp_root("usage-chatgpt");
        fs::create_dir_all(&chatgpt_home).unwrap();
        fs::write(
            chatgpt_home.join("config.toml"),
            r#"chatgpt_base_url = "https://chatgpt.com""#,
        )
        .unwrap();
        assert_eq!(
            usage_url(&chatgpt_home),
            "https://chatgpt.com/backend-api/wham/usage"
        );

        let codex_home = temp_root("usage-codex-api");
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(
            codex_home.join("config.toml"),
            r#"chatgpt_base_url = "https://example.test""#,
        )
        .unwrap();
        assert_eq!(
            usage_url(&codex_home),
            "https://example.test/api/codex/usage"
        );

        let _ = fs::remove_dir_all(default_home);
        let _ = fs::remove_dir_all(chatgpt_home);
        let _ = fs::remove_dir_all(codex_home);
    }
}
