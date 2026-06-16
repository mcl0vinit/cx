use crate::{paths, util};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub default_pool: Option<String>,
    pub default_strategy: Option<String>,
    pub limit_snapshot_max_age_minutes: Option<i64>,
    pub codex_bin: Option<PathBuf>,
    #[serde(default)]
    pub smart: SmartConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SmartConfig {
    pub refresh_before_pick: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub auto_migrate_auth_failed: Option<bool>,
    pub auto_migrate_limited: Option<bool>,
    pub auto_migrate_degraded: Option<bool>,
}

impl Config {
    pub fn limit_snapshot_max_age_minutes(&self) -> i64 {
        self.limit_snapshot_max_age_minutes.unwrap_or(30)
    }

    pub fn smart_refresh_before_pick(&self) -> bool {
        self.smart.refresh_before_pick.unwrap_or(false)
    }

    pub fn daemon_auto_migrate_auth_failed(&self) -> bool {
        self.daemon.auto_migrate_auth_failed.unwrap_or(true)
    }

    pub fn daemon_auto_migrate_limited(&self) -> bool {
        self.daemon.auto_migrate_limited.unwrap_or(false)
    }

    pub fn daemon_auto_migrate_degraded(&self) -> bool {
        self.daemon.auto_migrate_degraded.unwrap_or(false)
    }

    pub fn codex_bin(&self) -> Option<PathBuf> {
        self.codex_bin.clone().map(util::expand_tilde)
    }
}

pub fn load() -> Result<Config> {
    let path = paths::config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }

    let text = fs::read_to_string(&path)
        .with_context(|| format!("failed to read cx config at {}", path.display()))?;
    toml::from_str(&text)
        .with_context(|| format!("failed to parse cx config at {}", path.display()))
}

pub fn print_path() -> Result<()> {
    println!("{}", paths::config_path()?.display());
    Ok(())
}

pub fn print_config() -> Result<()> {
    let path = paths::config_path()?;
    if path.exists() {
        println!("{}", fs::read_to_string(path)?);
    } else {
        println!("{}", DEFAULT_CONFIG);
    }
    Ok(())
}

pub fn init(force: bool) -> Result<()> {
    paths::ensure_root_dirs()?;
    let path = paths::config_path()?;
    if path.exists() && !force {
        anyhow::bail!(
            "{} already exists; rerun with --force to overwrite",
            path.display()
        );
    }
    fs::write(&path, DEFAULT_CONFIG)?;
    println!("wrote {}", path.display());
    Ok(())
}

pub const DEFAULT_CONFIG: &str = r#"# Default pool used by `cx run`, `cx smart`, and `cx resume-here --smart`
# when a pool is not supplied.
# default_pool = "coding"

# Default strategy for `cx pool create` when --strategy is omitted.
default_strategy = "limit-aware"

# Snapshot age shown by `cx doctor`; stale snapshots can still be used unless
# smart.refresh_before_pick is enabled.
limit_snapshot_max_age_minutes = 30

# Override Codex launcher. CX_CODEX_BIN still wins when set.
# codex_bin = "~/bin/codex"

[smart]
# If true, `cx smart` refreshes accounts with stale/missing limit snapshots
# before picking. Refresh reads Codex account usage without starting a model turn.
refresh_before_pick = false

[daemon]
auto_migrate_auth_failed = true
auto_migrate_limited = false
auto_migrate_degraded = false
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_toml() {
        let config: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert_eq!(config.default_strategy.as_deref(), Some("limit-aware"));
        assert!(!config.smart_refresh_before_pick());
        assert!(config.daemon_auto_migrate_auth_failed());
        assert!(!config.daemon_auto_migrate_limited());
    }
}
