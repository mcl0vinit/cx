use crate::util;
use std::path::Path;

pub fn shell_command(account_home: &Path, args: &[String]) -> String {
    let mut command = format!("CODEX_HOME={} codex", util::quote_path(account_home));
    for arg in args {
        command.push(' ');
        command.push_str(&util::quote(arg));
    }
    command
}

pub fn same_account_resume_args() -> Vec<String> {
    vec!["resume".to_string(), "--last".to_string()]
}

pub fn cross_account_resume_args(session_name: &str, cwd: &Path, previous_account: &str, resume_prompt: Option<&str>) -> Vec<String> {
    let prompt = resume_prompt.map(ToOwned::to_owned).unwrap_or_else(|| {
        format!(
            "You are resuming a cx-managed Codex session.\n\nSession: {session_name}\nPrevious account: {previous_account}\nWorking directory: {}\n\nThe session was restarted under a different healthy account. First inspect `git status`, recent changed files, and relevant project context, then continue from the current repository state. Ask before destructive actions.",
            cwd.display()
        )
    });
    vec![prompt]
}
