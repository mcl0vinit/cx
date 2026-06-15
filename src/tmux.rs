use anyhow::{Context, Result};
use std::{path::Path, process::Command};

#[derive(Debug, Clone)]
pub struct TmuxTarget {
    pub session_name: String,
    pub window_id: String,
    pub pane_id: String,
}

pub fn new_window(name: &str, cwd: &Path, shell_command: &str) -> Result<TmuxTarget> {
    let output = Command::new("tmux")
        .arg("new-window")
        .arg("-P")
        .arg("-F")
        .arg("#{session_name}|#{window_id}|#{pane_id}")
        .arg("-n")
        .arg(name)
        .arg("-c")
        .arg(cwd)
        .arg(shell_command)
        .output()
        .context("failed to run `tmux new-window`; are you inside tmux or is a tmux server running?")?;

    if !output.status.success() {
        anyhow::bail!("tmux new-window failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }

    parse_target(String::from_utf8_lossy(&output.stdout).trim())
}

pub fn respawn_pane(pane_id: &str, cwd: &Path, shell_command: &str) -> Result<()> {
    let output = Command::new("tmux")
        .arg("respawn-pane")
        .arg("-k")
        .arg("-t")
        .arg(pane_id)
        .arg("-c")
        .arg(cwd)
        .arg(shell_command)
        .output()
        .context("failed to run `tmux respawn-pane`")?;

    if !output.status.success() {
        anyhow::bail!("tmux respawn-pane failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }

    Ok(())
}

pub fn pane_exists(pane_id: &str) -> Result<bool> {
    let output = Command::new("tmux")
        .arg("list-panes")
        .arg("-a")
        .arg("-F")
        .arg("#{pane_id}")
        .output()
        .context("failed to run `tmux list-panes`")?;

    if !output.status.success() {
        return Ok(false);
    }

    let panes = String::from_utf8_lossy(&output.stdout);
    Ok(panes.lines().any(|line| line.trim() == pane_id))
}

fn parse_target(s: &str) -> Result<TmuxTarget> {
    let parts: Vec<&str> = s.split('|').collect();
    if parts.len() != 3 {
        anyhow::bail!("unexpected tmux target output: `{}`", s);
    }
    Ok(TmuxTarget {
        session_name: parts[0].to_string(),
        window_id: parts[1].to_string(),
        pane_id: parts[2].to_string(),
    })
}
