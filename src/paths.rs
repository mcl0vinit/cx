use anyhow::{anyhow, Result};
use std::path::PathBuf;

pub fn cx_root() -> Result<PathBuf> {
    if let Ok(value) = std::env::var("CX_HOME") {
        return Ok(PathBuf::from(value));
    }

    let home = dirs::home_dir().ok_or_else(|| anyhow!("could not determine home directory"))?;
    Ok(home.join(".cx"))
}

pub fn accounts_dir() -> Result<PathBuf> {
    Ok(cx_root()?.join("accounts"))
}

pub fn session_store_dir() -> Result<PathBuf> {
    Ok(cx_root()?.join("session-store"))
}

pub fn canonical_sessions_dir() -> Result<PathBuf> {
    Ok(session_store_dir()?.join("sessions"))
}

pub fn session_locks_dir() -> Result<PathBuf> {
    Ok(session_store_dir()?.join("locks"))
}

pub fn account_home(name: &str) -> Result<PathBuf> {
    Ok(accounts_dir()?.join(name))
}

pub fn db_path() -> Result<PathBuf> {
    Ok(cx_root()?.join("cx.sqlite"))
}

pub fn pid_path() -> Result<PathBuf> {
    Ok(cx_root()?.join("cxd.pid"))
}

pub fn log_path() -> Result<PathBuf> {
    Ok(cx_root()?.join("cxd.log"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(cx_root()?.join("config.toml"))
}

pub fn ensure_root_dirs() -> Result<()> {
    std::fs::create_dir_all(cx_root()?)?;
    std::fs::create_dir_all(accounts_dir()?)?;
    std::fs::create_dir_all(canonical_sessions_dir()?)?;
    std::fs::create_dir_all(session_locks_dir()?)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(cx_root()?, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::set_permissions(accounts_dir()?, std::fs::Permissions::from_mode(0o700));
        let _ =
            std::fs::set_permissions(session_store_dir()?, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::set_permissions(
            canonical_sessions_dir()?,
            std::fs::Permissions::from_mode(0o700),
        );
        let _ =
            std::fs::set_permissions(session_locks_dir()?, std::fs::Permissions::from_mode(0o700));
    }

    Ok(())
}
