use crate::{db, paths, ui, util};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    fs,
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const FILE_COMPARE_BUFFER_SIZE: usize = 1024 * 1024;

pub struct ResolvedInvocation {
    pub home: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub summary: Option<ResumeSummary>,
}

#[derive(Debug, Clone)]
pub struct ResumeSummary {
    pub repo: Option<PathBuf>,
    pub session_id: String,
    pub source: String,
    pub target: String,
    pub action: String,
}

#[derive(Debug, Clone)]
struct ResumeSummaryDraft {
    repo: Option<PathBuf>,
    source: String,
    target: String,
    action: String,
}

#[derive(Debug, Clone)]
pub struct SessionIndexReport {
    pub homes: usize,
    pub sessions: usize,
}

#[derive(Debug)]
pub struct AdoptionResult {
    pub session_id: String,
    pub source: PathBuf,
    pub target: PathBuf,
    pub already_adopted: bool,
}

struct SessionMeta {
    id: Option<String>,
    cwd: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct SessionHome {
    label: String,
    path: PathBuf,
}

#[derive(Debug, Clone)]
struct SessionFile {
    home: SessionHome,
    path: PathBuf,
    modified: SystemTime,
    id: Option<String>,
    cwd: Option<PathBuf>,
}

impl SessionFile {
    fn from_adopted_target(home: SessionHome, path: PathBuf) -> Self {
        let metadata = fs::metadata(&path).ok();
        let modified = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let meta = session_meta(&path).unwrap_or(SessionMeta {
            id: None,
            cwd: None,
        });
        let id = meta.id.or_else(|| uuid_suffix_from_path(&path));

        Self {
            home,
            path,
            modified,
            id,
            cwd: meta.cwd,
        }
    }
}

#[derive(Debug)]
enum ResumeRequest {
    Last,
    Selector(String),
}

pub fn resolve_invocation(
    conn: &Connection,
    preferred_home: Option<&Path>,
    args: &[String],
) -> Result<Option<ResolvedInvocation>> {
    let Some(request) = parse_resume_request(args) else {
        return Ok(None);
    };

    let homes = session_homes(conn, preferred_home)?;
    let session = match &request {
        ResumeRequest::Last => latest_session(conn, &homes)?
            .ok_or_else(|| anyhow!("no Codex sessions found in known homes"))?,
        ResumeRequest::Selector(selector) => find_session(conn, &homes, selector)?
            .ok_or_else(|| anyhow!("session `{}` was not found in known Codex homes", selector))?,
    };
    Ok(Some(resolved_invocation(
        session.home.path.clone(),
        args,
        &request,
        &session,
        Some(ResumeSummaryDraft {
            repo: None,
            source: session.home.label.clone(),
            target: session.home.label.clone(),
            action: "reused".to_string(),
        }),
    )?))
}

pub fn resolve_account_invocation(
    conn: &Connection,
    account_name: &str,
    account_home: &Path,
    args: &[String],
) -> Result<Option<ResolvedInvocation>> {
    let Some(request) = parse_resume_request(args) else {
        return Ok(None);
    };

    let selected_home = SessionHome {
        label: account_name.to_string(),
        path: account_home.to_path_buf(),
    };
    let selected_homes = vec![selected_home.clone()];
    let selected_session = match &request {
        ResumeRequest::Last => latest_session(conn, &selected_homes)?,
        ResumeRequest::Selector(selector) => find_session(conn, &selected_homes, selector)?,
    };

    if let Some(session) = selected_session {
        return Ok(Some(resolved_invocation(
            selected_home.path,
            args,
            &request,
            &session,
            Some(ResumeSummaryDraft {
                repo: session_file_cwd(&session)?,
                source: selected_home.label.clone(),
                target: selected_home.label.clone(),
                action: "reused".to_string(),
            }),
        )?));
    }

    let source = match &request {
        ResumeRequest::Last => {
            let homes = session_homes(conn, Some(account_home))?;
            latest_session(conn, &homes)?
                .ok_or_else(|| anyhow!("no Codex sessions found in known homes"))?
        }
        ResumeRequest::Selector(selector) => {
            let homes = session_homes(conn, Some(account_home))?;
            find_session(conn, &homes, selector)?.ok_or_else(|| {
                anyhow!("session `{}` was not found in known Codex homes", selector)
            })?
        }
    };

    let source_label = source.home.label.clone();
    let source_cwd = session_file_cwd(&source)?;
    let adopted = adopt_session_file(conn, account_name, &selected_home, source)?;
    let action = if adopted.already_adopted {
        "already adopted"
    } else {
        "adopted"
    }
    .to_string();
    let target_session =
        SessionFile::from_adopted_target(selected_home.clone(), adopted.target.clone());
    Ok(Some(resolved_invocation(
        selected_home.path.clone(),
        args,
        &request,
        &target_session,
        Some(ResumeSummaryDraft {
            repo: source_cwd,
            source: source_label,
            target: selected_home.label,
            action,
        }),
    )?))
}

pub fn adopt_session(
    conn: &Connection,
    target_account: &db::Account,
    selector: &str,
) -> Result<AdoptionResult> {
    let target_home = SessionHome {
        label: target_account.name.clone(),
        path: target_account.codex_home.clone(),
    };

    if let Some(existing) = find_session(conn, std::slice::from_ref(&target_home), selector)? {
        let id = session_file_id(&existing)?
            .ok_or_else(|| anyhow!("could not read session id from {}", existing.path.display()))?;
        return Ok(AdoptionResult {
            session_id: id,
            source: existing.path.clone(),
            target: existing.path,
            already_adopted: true,
        });
    }

    let homes = session_homes(conn, Some(&target_account.codex_home))?;
    let source = find_session(conn, &homes, selector)?
        .ok_or_else(|| anyhow!("session `{}` was not found in known Codex homes", selector))?;

    adopt_session_file(conn, &target_account.name, &target_home, source)
}

pub fn resolve_here_invocation(
    conn: &Connection,
    target_account: Option<(&str, &Path)>,
    cwd: &Path,
    extra_args: &[String],
) -> Result<ResolvedInvocation> {
    let repo_root = repo_root_for(cwd)?;
    resolve_here_in_root(conn, target_account, &repo_root, extra_args)
}

fn resolve_here_in_root(
    conn: &Connection,
    target_account: Option<(&str, &Path)>,
    repo_root: &Path,
    extra_args: &[String],
) -> Result<ResolvedInvocation> {
    let mut args = vec!["resume".to_string(), "--last".to_string()];
    args.extend(extra_args.iter().cloned());
    let request = ResumeRequest::Last;

    let homes = match target_account {
        Some((_, account_home)) => session_homes(conn, Some(account_home))?,
        None => session_homes(conn, None)?,
    };
    let source = latest_session_for_repo(conn, &homes, repo_root)?.ok_or_else(|| {
        anyhow!(
            "no Codex sessions found for repo {} in known homes",
            repo_root.display()
        )
    })?;

    if let Some((account_name, account_home)) = target_account {
        let selected_home = SessionHome {
            label: account_name.to_string(),
            path: account_home.to_path_buf(),
        };
        let source_label = source.home.label.clone();
        let (session, action) = if same_path(&source.home.path, account_home) {
            (source, "reused".to_string())
        } else {
            let adopted = adopt_session_file(conn, account_name, &selected_home, source)?;
            let action = if adopted.already_adopted {
                "already adopted"
            } else {
                "adopted"
            }
            .to_string();
            (
                SessionFile::from_adopted_target(selected_home.clone(), adopted.target),
                action,
            )
        };
        return resolved_invocation(
            selected_home.path,
            &args,
            &request,
            &session,
            Some(ResumeSummaryDraft {
                repo: Some(repo_root.to_path_buf()),
                source: source_label,
                target: selected_home.label,
                action,
            }),
        );
    }

    resolved_invocation(
        source.home.path.clone(),
        &args,
        &request,
        &source,
        Some(ResumeSummaryDraft {
            repo: Some(repo_root.to_path_buf()),
            source: source.home.label.clone(),
            target: source.home.label.clone(),
            action: "reused".to_string(),
        }),
    )
}

fn adopt_session_file(
    conn: &Connection,
    target_account_name: &str,
    target_home: &SessionHome,
    source: SessionFile,
) -> Result<AdoptionResult> {
    let id = session_file_id(&source)?
        .ok_or_else(|| anyhow!("could not read session id from {}", source.path.display()))?;

    let target_matches = sessions_with_id(conn, target_home, &id)?;
    match target_matches.as_slice() {
        [] => {}
        [existing] => {
            if files_equal(&source.path, &existing.path)? {
                return Ok(AdoptionResult {
                    session_id: id,
                    source: source.path,
                    target: existing.path.clone(),
                    already_adopted: true,
                });
            }
            anyhow::bail!(
                "target account `{}` already has session `{}` with different content at {}",
                target_account_name,
                id,
                existing.path.display()
            );
        }
        many => {
            let paths = many
                .iter()
                .map(|session| session.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "target account `{}` has multiple copies of session `{}`: {}",
                target_account_name,
                id,
                paths
            );
        }
    }

    let target = target_session_path(&source, &target_home.path)?;
    if target.exists() {
        anyhow::bail!(
            "target path already exists with different session content: {}",
            target.display()
        );
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    copy_session_file(&source.path, &target)?;

    Ok(AdoptionResult {
        session_id: id,
        source: source.path,
        target,
        already_adopted: false,
    })
}

pub fn default_resume_home(conn: &Connection, preferred_home: Option<&Path>) -> Result<PathBuf> {
    let homes = session_homes(conn, preferred_home)?;
    homes
        .into_iter()
        .next()
        .map(|home| home.path)
        .ok_or_else(|| anyhow!("no Codex homes found"))
}

pub fn print_sessions(conn: &Connection, limit: usize) -> Result<()> {
    let homes = session_homes(conn, None)?;
    let mut sessions = all_sessions(conn, &homes)?;
    sessions.sort_by_key(|session| Reverse(session.modified));

    println!("{}", ui::heading("Recent Codex Sessions"));
    if sessions.is_empty() {
        println!("No Codex session files found in known homes.");
        return Ok(());
    }

    let rows = sessions
        .into_iter()
        .take(limit)
        .map(|session| {
            let cwd = session_file_cwd(&session)?
                .map(|cwd| util::display_path(&cwd))
                .unwrap_or_else(|| "-".to_string());
            let id = session_file_id(&session)?.unwrap_or_else(|| session_name(&session.path));
            Ok(vec![
                session.home.label.clone(),
                format_time(session.modified),
                id,
                cwd,
            ])
        })
        .collect::<Result<Vec<_>>>()?;

    ui::print_table(&["HOME", "UPDATED", "SESSION", "CWD"], &rows, &[]);
    Ok(())
}

pub fn codex_session_count(conn: &Connection, label: &str, home: &Path) -> Result<i64> {
    let home = SessionHome {
        label: label.to_string(),
        path: home.to_path_buf(),
    };
    sync_session_index(conn, std::slice::from_ref(&home))?;
    db::count_indexed_codex_sessions_for_home(conn, &index_home_path(&home))
}

pub fn sync_all_session_indexes(conn: &Connection, rebuild: bool) -> Result<SessionIndexReport> {
    let homes = session_homes(conn, None)?;
    if rebuild {
        db::clear_indexed_codex_sessions(conn)?;
    }
    sync_session_index(conn, &homes)?;
    let home_paths = homes.iter().map(index_home_path).collect::<Vec<_>>();
    let sessions = db::list_indexed_codex_sessions_for_homes(conn, &home_paths)?.len();
    Ok(SessionIndexReport {
        homes: homes.len(),
        sessions,
    })
}

fn parse_resume_request(args: &[String]) -> Option<ResumeRequest> {
    if args.first().map(|arg| arg.as_str()) != Some("resume") {
        return None;
    }

    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return None;
    }

    if args.iter().skip(1).any(|arg| arg == "--last") {
        return Some(ResumeRequest::Last);
    }

    args.iter()
        .skip(1)
        .find(|arg| !arg.starts_with('-') && arg.as_str() != "--")
        .cloned()
        .map(ResumeRequest::Selector)
}

fn resolved_invocation(
    home: PathBuf,
    args: &[String],
    request: &ResumeRequest,
    session: &SessionFile,
    summary: Option<ResumeSummaryDraft>,
) -> Result<ResolvedInvocation> {
    let id = session_file_id(session)?
        .ok_or_else(|| anyhow!("could not read session id from {}", session.path.display()))?;

    Ok(ResolvedInvocation {
        home,
        args: rewrite_resume_args(args, request, &id),
        cwd: session_file_cwd(session)?,
        summary: summary.map(|summary| ResumeSummary {
            repo: summary.repo,
            session_id: id,
            source: summary.source,
            target: summary.target,
            action: summary.action,
        }),
    })
}

fn rewrite_resume_args(args: &[String], request: &ResumeRequest, session_id: &str) -> Vec<String> {
    match request {
        ResumeRequest::Last => rewrite_last_resume_args(args, session_id),
        ResumeRequest::Selector(selector) => {
            rewrite_selector_resume_args(args, selector, session_id)
        }
    }
}

fn rewrite_last_resume_args(args: &[String], session_id: &str) -> Vec<String> {
    let mut rewritten = vec!["resume".to_string()];
    let rest = args
        .iter()
        .skip(1)
        .filter(|arg| arg.as_str() != "--last")
        .cloned()
        .collect::<Vec<_>>();

    let split = rest
        .iter()
        .position(|arg| !arg.starts_with('-'))
        .unwrap_or(rest.len());
    rewritten.extend(rest[..split].iter().cloned());
    rewritten.push(session_id.to_string());
    rewritten.extend(rest[split..].iter().cloned());
    rewritten
}

fn rewrite_selector_resume_args(args: &[String], selector: &str, session_id: &str) -> Vec<String> {
    let mut replaced = false;
    args.iter()
        .map(|arg| {
            if !replaced && arg == selector {
                replaced = true;
                session_id.to_string()
            } else {
                arg.clone()
            }
        })
        .collect()
}

fn session_homes(conn: &Connection, preferred_home: Option<&Path>) -> Result<Vec<SessionHome>> {
    let mut homes = Vec::new();
    let mut seen = HashSet::new();

    if let Some(path) = preferred_home {
        push_home(
            &mut homes,
            &mut seen,
            "selected".to_string(),
            path.to_path_buf(),
        );
    }

    if let Some(home) = dirs::home_dir() {
        push_home(
            &mut homes,
            &mut seen,
            "default".to_string(),
            home.join(".codex"),
        );
    }

    for account in db::list_accounts(conn)? {
        push_home(&mut homes, &mut seen, account.name, account.codex_home);
    }

    if let Ok(accounts_dir) = paths::accounts_dir() {
        if let Ok(entries) = fs::read_dir(accounts_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let label = entry
                    .file_name()
                    .to_str()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| util::display_path(&path));
                push_home(&mut homes, &mut seen, label, path);
            }
        }
    }

    Ok(homes)
}

fn push_home(
    homes: &mut Vec<SessionHome>,
    seen: &mut HashSet<String>,
    label: String,
    path: PathBuf,
) {
    let path = util::expand_tilde(path);
    if !path.exists() {
        return;
    }

    let key_path = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let key = key_path.to_string_lossy().to_string();
    if seen.insert(key) {
        homes.push(SessionHome { label, path });
    }
}

fn latest_session(conn: &Connection, homes: &[SessionHome]) -> Result<Option<SessionFile>> {
    Ok(all_sessions(conn, homes)?
        .into_iter()
        .max_by_key(|s| s.modified))
}

fn latest_session_for_repo(
    conn: &Connection,
    homes: &[SessionHome],
    repo_root: &Path,
) -> Result<Option<SessionFile>> {
    let repo_root = normalize_path(repo_root);
    let mut matches = Vec::new();

    for session in all_sessions(conn, homes)? {
        let Some(cwd) = session_file_cwd(&session)? else {
            continue;
        };
        if cwd_matches_repo(&cwd, &repo_root) {
            matches.push(session);
        }
    }

    Ok(matches.into_iter().max_by_key(|session| session.modified))
}

fn find_session(
    conn: &Connection,
    homes: &[SessionHome],
    selector: &str,
) -> Result<Option<SessionFile>> {
    let mut matches = Vec::new();
    for session in all_sessions(conn, homes)? {
        if let Some(score) = match_score(&session.path, selector) {
            matches.push((score, session));
        }
    }

    matches.sort_by(|(a_score, a), (b_score, b)| {
        a_score
            .cmp(b_score)
            .then_with(|| b.modified.cmp(&a.modified))
    });

    let Some((best_score, best)) = matches.first().cloned() else {
        return Ok(None);
    };

    let same_score = matches
        .iter()
        .filter(|(score, _)| *score == best_score)
        .take(6)
        .collect::<Vec<_>>();
    if same_score.len() > 1 {
        let options = same_score
            .iter()
            .map(|(_, session)| format!("{}:{}", session.home.label, session_name(&session.path)))
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("session selector `{}` is ambiguous: {}", selector, options);
    }

    Ok(Some(best))
}

fn sessions_with_id(conn: &Connection, home: &SessionHome, id: &str) -> Result<Vec<SessionFile>> {
    let mut matches = Vec::new();
    for session in all_sessions(conn, std::slice::from_ref(home))? {
        if session_file_id(&session)?.as_deref() == Some(id) {
            matches.push(session);
        }
    }
    Ok(matches)
}

fn all_sessions(conn: &Connection, homes: &[SessionHome]) -> Result<Vec<SessionFile>> {
    sync_session_index(conn, homes)?;
    let home_paths = homes.iter().map(index_home_path).collect::<Vec<_>>();
    let sessions = db::list_indexed_codex_sessions_for_homes(conn, &home_paths)?
        .into_iter()
        .filter_map(|indexed| {
            homes
                .iter()
                .find(|home| index_home_path(home) == indexed.home_path)
                .map(|home| SessionFile {
                    home: home.clone(),
                    path: indexed.path,
                    modified: system_time_from_nanos(indexed.modified_nanos),
                    id: indexed.session_id,
                    cwd: indexed.cwd,
                })
        })
        .collect::<Vec<_>>();
    Ok(sessions)
}

fn sync_session_index(conn: &Connection, homes: &[SessionHome]) -> Result<()> {
    for home in homes {
        sync_home_session_index(conn, home)?;
    }
    Ok(())
}

fn sync_home_session_index(conn: &Connection, home: &SessionHome) -> Result<()> {
    let home_path = index_home_path(home);
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    collect_session_paths(&home.path.join("sessions"), &mut paths)?;
    let indexed_by_path = db::list_indexed_codex_sessions_for_home(conn, &home_path)?
        .into_iter()
        .map(|session| (session.path.clone(), session))
        .collect::<HashMap<_, _>>();

    conn.execute_batch("begin immediate")?;
    let result =
        sync_home_session_index_rows(conn, home, &home_path, paths, &mut seen, &indexed_by_path);
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

fn sync_home_session_index_rows(
    conn: &Connection,
    home: &SessionHome,
    home_path: &Path,
    paths: Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    indexed_by_path: &HashMap<PathBuf, db::IndexedCodexSession>,
) -> Result<()> {
    for path in paths {
        seen.insert(path.clone());
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                db::delete_indexed_codex_session(conn, &path)?;
                continue;
            }
        };
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let modified_nanos = system_time_nanos(modified);
        let size_bytes = metadata.len() as i64;

        let unchanged = indexed_by_path.get(&path).is_some_and(|indexed| {
            indexed.home_path == home_path
                && indexed.home_label == home.label
                && indexed.modified_nanos == modified_nanos
                && indexed.size_bytes == size_bytes
        });
        if unchanged {
            continue;
        }

        let meta = session_meta(&path).unwrap_or(SessionMeta {
            id: None,
            cwd: None,
        });
        db::upsert_indexed_codex_session(
            conn,
            db::IndexedCodexSessionUpsert {
                path: path.clone(),
                home_path: home_path.to_path_buf(),
                home_label: home.label.clone(),
                session_id: meta.id.or_else(|| uuid_suffix_from_path(&path)),
                cwd: meta.cwd,
                modified_nanos,
                size_bytes,
            },
        )?;
    }

    for indexed_path in indexed_by_path.keys() {
        if !seen.contains(indexed_path) {
            db::delete_indexed_codex_session(conn, indexed_path)?;
        }
    }

    Ok(())
}

fn repo_root_for(cwd: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(cwd)
        .output();

    if let Ok(output) = output {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let root = text.trim();
            if !root.is_empty() {
                let root_path = Path::new(root);
                if normalize_path(root_path) == normalize_path(cwd) {
                    return Ok(cwd.to_path_buf());
                }
                return Ok(normalize_path(root_path));
            }
        }
    }

    Ok(normalize_path(cwd))
}

fn cwd_matches_repo(cwd: &Path, repo_root: &Path) -> bool {
    let cwd = normalize_path(cwd);
    cwd == repo_root || cwd.starts_with(repo_root)
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn same_path(left: &Path, right: &Path) -> bool {
    normalize_path(left) == normalize_path(right)
}

fn index_home_path(home: &SessionHome) -> PathBuf {
    normalize_path(&home.path)
}

fn system_time_nanos(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos()
        .min(i64::MAX as u128) as i64
}

fn system_time_from_nanos(nanos: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(nanos.max(0) as u64)
}

fn collect_session_paths(dir: &Path, sessions: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_session_paths(&path, sessions)?;
            continue;
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }

        sessions.push(path);
    }

    Ok(())
}

fn target_session_path(source: &SessionFile, target_home: &Path) -> Result<PathBuf> {
    let sessions_root = source.home.path.join("sessions");
    let relative = source.path.strip_prefix(&sessions_root).with_context(|| {
        format!(
            "source session {} is not under {}",
            source.path.display(),
            sessions_root.display()
        )
    })?;

    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        anyhow::bail!("source session path contains parent directory components");
    }

    Ok(target_home.join("sessions").join(relative))
}

fn files_equal(left: &Path, right: &Path) -> Result<bool> {
    let left_metadata = fs::metadata(left)?;
    let right_metadata = fs::metadata(right)?;
    if left_metadata.len() != right_metadata.len() {
        return Ok(false);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if left_metadata.dev() == right_metadata.dev()
            && left_metadata.ino() == right_metadata.ino()
        {
            return Ok(true);
        }
    }

    let mut left_file = fs::File::open(left)?;
    let mut right_file = fs::File::open(right)?;
    let mut left_buffer = vec![0; FILE_COMPARE_BUFFER_SIZE];
    let mut right_buffer = vec![0; FILE_COMPARE_BUFFER_SIZE];

    loop {
        let left_read = left_file.read(&mut left_buffer)?;
        let right_read = right_file.read(&mut right_buffer)?;
        if left_read != right_read {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
        if left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
    }
}

fn copy_session_file(source: &Path, target: &Path) -> Result<()> {
    if let Err(error) = clone_session_file(source, target) {
        tracing::debug!(
            source = %source.display(),
            target = %target.display(),
            error = %error,
            "session clone unavailable; falling back to byte copy"
        );
        fs::copy(source, target)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn clone_session_file(source: &Path, target: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    extern "C" {
        fn clonefile(src: *const i8, dst: *const i8, flags: u32) -> i32;
    }

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    let result = unsafe { clonefile(source.as_ptr(), target.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn clone_session_file(_source: &Path, _target: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "copy-on-write clone is not implemented on this platform",
    ))
}

fn match_score(path: &Path, selector: &str) -> Option<u8> {
    let file_name = path.file_name()?.to_string_lossy();
    let stem = path.file_stem()?.to_string_lossy();
    let path_text = path.to_string_lossy();

    if stem == selector {
        Some(0)
    } else if stem.ends_with(selector) {
        Some(1)
    } else if file_name.contains(selector) {
        Some(2)
    } else if path_text.contains(selector) {
        Some(3)
    } else {
        None
    }
}

fn session_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>")
        .to_string()
}

fn session_file_id(session: &SessionFile) -> Result<Option<String>> {
    if session.id.is_some() {
        return Ok(session.id.clone());
    }
    session_id(&session.path)
}

fn session_file_cwd(session: &SessionFile) -> Result<Option<PathBuf>> {
    if session.cwd.is_some() {
        return Ok(session.cwd.clone());
    }
    session_cwd(&session.path)
}

fn session_id(path: &Path) -> Result<Option<String>> {
    Ok(session_meta(path)?
        .id
        .or_else(|| uuid_suffix_from_path(path)))
}

fn session_cwd(path: &Path) -> Result<Option<PathBuf>> {
    Ok(session_meta(path)?.cwd)
}

fn session_meta(path: &Path) -> Result<SessionMeta> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);

    for line in reader.lines().take(16) {
        let line = line?;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(|kind| kind.as_str()) != Some("session_meta") {
            continue;
        }
        let id = value
            .get("payload")
            .and_then(|payload| payload.get("id"))
            .and_then(|id| id.as_str())
            .map(ToOwned::to_owned);
        let cwd = value
            .get("payload")
            .and_then(|payload| payload.get("cwd"))
            .and_then(|cwd| cwd.as_str())
            .map(PathBuf::from);
        return Ok(SessionMeta { id, cwd });
    }

    Ok(SessionMeta {
        id: None,
        cwd: None,
    })
}

fn uuid_suffix_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    if stem.len() < 36 {
        return None;
    }

    let start = stem.len() - 36;
    let candidate = &stem[start..];
    if is_uuid(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn is_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }

    value.chars().enumerate().all(|(idx, ch)| {
        matches!(idx, 8 | 13 | 18 | 23) && ch == '-'
            || !matches!(idx, 8 | 13 | 18 | 23) && ch.is_ascii_hexdigit()
    })
}

fn format_time(time: SystemTime) -> String {
    let datetime: DateTime<Utc> = time.into();
    datetime.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("cx-{name}-{}-{nanos}", std::process::id()))
    }

    fn write_session(home: &Path, id: &str, cwd: &Path, suffix: &str) -> PathBuf {
        let path = home
            .join("sessions")
            .join("2026")
            .join("06")
            .join("15")
            .join(format!("rollout-2026-06-15T12-00-00-{id}-{suffix}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"{}"}}}}"#,
                cwd.display()
            ),
        )
        .unwrap();
        path
    }

    fn test_conn(source_home: &Path, target_home: &Path) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        db::upsert_account(&conn, "source", source_home.to_path_buf()).unwrap();
        db::upsert_account(&conn, "target", target_home.to_path_buf()).unwrap();
        conn
    }

    #[test]
    fn rewrite_last_resume_uses_resolved_id() {
        let args = vec![
            "resume".to_string(),
            "--last".to_string(),
            "--no-alt-screen".to_string(),
        ];

        assert_eq!(
            rewrite_last_resume_args(&args, "019ecc26-088b-7a53-9682-e2c3286727da"),
            vec![
                "resume",
                "--no-alt-screen",
                "019ecc26-088b-7a53-9682-e2c3286727da"
            ]
        );
    }

    #[test]
    fn account_resume_auto_adopts_foreign_selector() {
        let root = temp_root("auto-adopt-selector");
        let source_home = root.join("source");
        let target_home = root.join("target");
        let id = "11111111-2222-3333-4444-555555555555";
        let source_file = write_session(&source_home, id, Path::new("/tmp/project"), "source");
        let conn = test_conn(&source_home, &target_home);
        let args = vec![
            "resume".to_string(),
            id.to_string(),
            "--no-alt-screen".to_string(),
        ];

        let resolved = resolve_account_invocation(&conn, "target", &target_home, &args)
            .unwrap()
            .unwrap();

        assert_eq!(resolved.home, target_home);
        assert_eq!(resolved.args, vec!["resume", id, "--no-alt-screen"]);
        assert_eq!(resolved.cwd, Some(PathBuf::from("/tmp/project")));
        assert!(source_file.exists());
        assert!(resolved
            .home
            .join("sessions")
            .join(
                source_file
                    .strip_prefix(source_home.join("sessions"))
                    .unwrap()
            )
            .exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resume_here_auto_adopts_latest_repo_session_to_target() {
        let root = temp_root("resume-here");
        let source_home = root.join("source");
        let target_home = root.join("target");
        let repo_root = root.join("repo");
        let repo_child = repo_root.join("crate");
        fs::create_dir_all(&repo_child).unwrap();
        let id = "11111111-2222-3333-4444-777777777777";
        let source_file = write_session(&source_home, id, &repo_child, "source");
        let conn = test_conn(&source_home, &target_home);

        let resolved =
            resolve_here_in_root(&conn, Some(("target", &target_home)), &repo_root, &[]).unwrap();

        assert_eq!(resolved.home, target_home);
        assert_eq!(resolved.args, vec!["resume", id]);
        assert_eq!(resolved.cwd, Some(repo_child));
        assert!(source_file.exists());
        assert!(resolved
            .home
            .join("sessions")
            .join(
                source_file
                    .strip_prefix(source_home.join("sessions"))
                    .unwrap()
            )
            .exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn adopt_session_copies_once_then_reports_already_adopted() {
        let root = temp_root("adopt-once");
        let source_home = root.join("source");
        let target_home = root.join("target");
        let id = "11111111-2222-3333-4444-555555555555";
        let source_file = write_session(&source_home, id, Path::new("/tmp/project"), "source");
        let conn = test_conn(&source_home, &target_home);
        let target = db::get_account(&conn, "target").unwrap().unwrap();

        let adopted = adopt_session(&conn, &target, id).unwrap();
        assert!(!adopted.already_adopted);
        assert_eq!(adopted.source, source_file);
        assert!(adopted.target.exists());

        let second = adopt_session(&conn, &target, id).unwrap();
        assert!(second.already_adopted);
        assert_eq!(second.target, adopted.target);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn adopt_session_refuses_multiple_target_copies() {
        let root = temp_root("adopt-duplicate");
        let source_home = root.join("source");
        let target_home = root.join("target");
        let id = "11111111-2222-3333-4444-666666666666";
        write_session(&source_home, id, Path::new("/tmp/project"), "source");
        write_session(&target_home, id, Path::new("/tmp/project"), "target-a");
        write_session(&target_home, id, Path::new("/tmp/project"), "target-b");
        let conn = test_conn(&source_home, &target_home);
        let target = db::get_account(&conn, "target").unwrap().unwrap();

        let err = adopt_session(&conn, &target, id).unwrap_err().to_string();
        assert!(err.contains("ambiguous") || err.contains("multiple"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn session_file_copy_and_compare_are_independent() {
        let root = temp_root("copy-compare");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.jsonl");
        let same = root.join("same.jsonl");
        let different = root.join("different.jsonl");
        let copied = root.join("copied.jsonl");

        fs::write(&source, b"one\ntwo\nthree\n").unwrap();
        fs::write(&same, b"one\ntwo\nthree\n").unwrap();
        fs::write(&different, b"one\ntwo\n").unwrap();

        assert!(files_equal(&source, &same).unwrap());
        assert!(!files_equal(&source, &different).unwrap());

        copy_session_file(&source, &copied).unwrap();
        assert!(files_equal(&source, &copied).unwrap());

        fs::OpenOptions::new()
            .append(true)
            .open(&copied)
            .unwrap()
            .write_all(b"four\n")
            .unwrap();

        assert!(!files_equal(&source, &copied).unwrap());
        assert_eq!(fs::read(&source).unwrap(), b"one\ntwo\nthree\n");

        let _ = fs::remove_dir_all(root);
    }
}
