use crate::{config, util};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn bin_path() -> PathBuf {
    if let Ok(value) = std::env::var("CX_CODEX_BIN") {
        return PathBuf::from(value);
    }

    if let Ok(Some(path)) = config::load().map(|config| config.codex_bin()) {
        return path;
    }

    if let Some(home) = dirs::home_dir() {
        let local_launcher = home.join("bin").join("codex");
        if local_launcher.exists() {
            return local_launcher;
        }
    }

    PathBuf::from("codex")
}

pub fn command() -> Command {
    Command::new(bin_path())
}

pub fn args_with_model_defaults(
    args: &[String],
    model: Option<&str>,
    effort: Option<&str>,
) -> Vec<String> {
    let mut injected = Vec::new();
    if let Some(model) = model.filter(|_| !has_model_override(args)) {
        injected.push("--model".to_string());
        injected.push(model.to_string());
    }
    if let Some(effort) = effort
        .and_then(normalize_reasoning_effort)
        .filter(|_| !has_config_override(args, "model_reasoning_effort"))
    {
        injected.push("-c".to_string());
        injected.push(format!("model_reasoning_effort=\"{effort}\""));
    }
    injected.extend(args.iter().cloned());
    injected
}

pub fn shell_command(account_home: &Path, args: &[String]) -> String {
    let mut command = format!(
        "CODEX_HOME={} {}",
        util::quote_path(account_home),
        util::quote_path(&bin_path())
    );
    for arg in args {
        command.push(' ');
        command.push_str(&util::quote(arg));
    }
    command
}

pub fn same_account_resume_args() -> Vec<String> {
    vec!["resume".to_string(), "--last".to_string()]
}

pub fn cross_account_resume_args(
    session_name: &str,
    cwd: &Path,
    previous_account: &str,
    resume_prompt: Option<&str>,
) -> Vec<String> {
    let prompt = resume_prompt.map(ToOwned::to_owned).unwrap_or_else(|| {
        format!(
            "You are resuming a cx-managed Codex session.\n\nSession: {session_name}\nPrevious account: {previous_account}\nWorking directory: {}\n\nThe session was restarted under a different healthy account. First inspect `git status`, recent changed files, and relevant project context, then continue from the current repository state. Ask before destructive actions.",
            cwd.display()
        )
    });
    vec![prompt]
}

fn normalize_reasoning_effort(effort: &str) -> Option<&str> {
    match effort {
        "" => None,
        "default" => Some("medium"),
        value => Some(value),
    }
}

fn has_model_override(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--model" || arg == "-m")
        || args
            .iter()
            .any(|arg| arg.starts_with("--model=") || arg.starts_with("-m="))
        || has_config_override(args, "model")
}

fn has_config_override(args: &[String], key: &str) -> bool {
    let prefix = format!("{key}=");
    args.windows(2)
        .any(|pair| matches!(pair[0].as_str(), "-c" | "--config") && config_sets(&pair[1], key))
        || args.iter().any(|arg| {
            arg.strip_prefix("-c=")
                .or_else(|| arg.strip_prefix("--config="))
                .is_some_and(|value| config_sets(value, key))
        })
        || args.iter().any(|arg| arg.starts_with(&prefix))
}

fn config_sets(value: &str, key: &str) -> bool {
    value.trim_start().starts_with(&format!("{key}="))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_defaults_preserve_explicit_overrides() {
        let args = vec!["exec".to_string(), "hi".to_string()];
        assert_eq!(
            args_with_model_defaults(&args, Some("gpt-5.4"), Some("high")),
            vec![
                "--model",
                "gpt-5.4",
                "-c",
                "model_reasoning_effort=\"high\"",
                "exec",
                "hi"
            ]
        );

        let explicit = vec![
            "--model".to_string(),
            "gpt-5.5".to_string(),
            "-c".to_string(),
            "model_reasoning_effort=\"low\"".to_string(),
            "exec".to_string(),
            "hi".to_string(),
        ];
        assert_eq!(
            args_with_model_defaults(&explicit, Some("gpt-5.4"), Some("high")),
            explicit
        );
    }

    #[test]
    fn default_effort_maps_default_to_medium() {
        let args = vec!["resume".to_string(), "id".to_string()];
        assert_eq!(
            args_with_model_defaults(&args, None, Some("default")),
            vec!["-c", "model_reasoning_effort=\"medium\"", "resume", "id"]
        );
    }

    #[test]
    fn shell_command_uses_original_args() {
        let args = vec!["exec".to_string(), "hi".to_string()];
        let command = shell_command(Path::new("/tmp/codex-home"), &args);

        assert!(command.contains(" exec hi"));
        assert!(!command.contains("model_reasoning_effort"));
    }
}
