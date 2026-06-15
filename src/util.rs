use chrono::Utc;
use std::path::{Path, PathBuf};

pub fn now() -> String {
    Utc::now().to_rfc3339()
}

pub fn quote<S: AsRef<str>>(s: S) -> String {
    shell_words::quote(s.as_ref()).to_string()
}

pub fn quote_path(path: &Path) -> String {
    quote(path.to_string_lossy())
}

pub fn normalize_passthrough(mut args: Vec<String>) -> Vec<String> {
    if args.first().map(|s| s.as_str()) == Some("--") {
        args.remove(0);
    }
    args
}

pub fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub fn expand_tilde(path: PathBuf) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    let Some(home) = dirs::home_dir() else {
        return path;
    };

    if text == "~" {
        return home;
    }

    if let Some(rest) = text.strip_prefix("~/") {
        return home.join(rest);
    }

    path
}

pub fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}
