# cx — tmux-first Codex account/session supervisor

`cx` is a Rust scaffold for running many Codex CLI sessions in tmux with isolated account profiles.

The core idea is simple:

```bash
CODEX_HOME="$HOME/.cx/accounts/personal" codex
CODEX_HOME="$HOME/.cx/accounts/work" codex resume --last
```

Each account gets its own Codex home:

```text
~/.cx/
  accounts/
    personal/
      config.toml
      auth.json
    work/
      config.toml
      auth.json
  cx.sqlite
  cxd.pid
  cxd.log
```

The tool records managed tmux panes in SQLite, so it can restart or migrate sessions with `tmux respawn-pane -k`, preserving your existing tmux layout whenever possible.

## Status of this scaffold

This is a practical v0.1 starter project, not a polished package. It includes:

- account homes using per-account `CODEX_HOME`
- `cx account add/login/logout/list/check/disable/enable`
- account pools
- tmux managed session start/list/restart
- manual migration between accounts
- a simple polling daemon that can restart dead panes and auto-migrate sessions away from disabled/auth-failed accounts
- conservative behavior for usage limits: `limited` accounts are not auto-migrated by default

## Install

```bash
cargo build --release
install -m 755 target/release/cx ~/.local/bin/cx
```

Make sure `~/.local/bin` is on your `PATH`.

## Basic setup

```bash
cx account add personal
cx account login personal

cx account add work
cx account login work

cx pool create coding --accounts personal,work
```

`cx account login personal` runs:

```bash
CODEX_HOME="$HOME/.cx/accounts/personal" codex login
```

and writes this to the account config if missing:

```toml
cli_auth_credentials_store = "file"
```

## Daily use

Run Codex directly with an account:

```bash
cx personal
cx work resume --last
cx run --account personal -- exec "summarize this repo"
```

Start a managed tmux session:

```bash
cx tmux run --pool coding --name atlas -C ~/code/atlas
```

Pass Codex args after `--`:

```bash
cx tmux run --account personal --name atlas -C ~/code/atlas -- resume --last
cx tmux run --pool coding --name worker-1 -C ~/code/atlas -- exec "fix the parser"
```

List managed sessions:

```bash
cx tmux list
```

Restart a session in place:

```bash
cx tmux restart atlas
# or
cx restart atlas
```

Migrate a session to a specific account:

```bash
cx migrate atlas --account work
```

Migrate using the session's pool:

```bash
cx migrate atlas --pool coding
```

## Daemon

Start the polling supervisor:

```bash
cx daemon start
cx daemon status
```

Stop it:

```bash
cx daemon stop
```

Run it in the foreground for debugging:

```bash
cx daemon run --interval-secs 10
```

The daemon currently:

- checks account auth locally by looking for `auth.json`
- restarts dead managed panes under the same account
- auto-migrates sessions whose current account is disabled or auth-failed, if the session has a pool with another eligible account
- does **not** auto-migrate `limited` accounts by default

## Account health

Local check:

```bash
cx account check --all
```

Online check, which runs `codex exec` and may consume usage:

```bash
cx account check personal --online
cx account check --all --online
```

Statuses used by the scaffold:

```text
healthy      usable
unknown      allowed as a pool candidate until checked
limited      not auto-migrated by daemon; manual intervention recommended
auth_failed  auto-migrated by daemon if a pool has another candidate
disabled     auto-migrated by daemon if a pool has another candidate
degraded     allowed as a fallback candidate in v0.1
```

## Important resume behavior

Same-account restarts use:

```bash
codex resume --last
```

Cross-account migrations may not be able to use native Codex history, because session history can live under the previous `CODEX_HOME`. For cross-account migration, this scaffold starts Codex with a generated semantic resume prompt that tells it to inspect the current repo state and continue.

You can provide your own cross-account migration prompt:

```bash
cx tmux run \
  --pool coding \
  --name atlas \
  -C ~/code/atlas \
  --resume-prompt "Resume the atlas router refactor. Inspect git status and continue from current files."
```

## Conservative policy note

This scaffold is designed as a local supervisor for accounts you are authorized to use. By default it does not automatically migrate sessions from accounts marked `limited`; it pauses/manualizes that case. Keep your usage within the terms for the services and accounts involved.

## Useful commands

```bash
cx account list
cx pool list
cx status
cx tmux list
cx daemon status
```

## Development notes

The project is intentionally synchronous and small. Obvious next steps:

1. Add a Unix-socket RPC layer so `cx` talks to `cxd` instead of both touching SQLite.
2. Add config-driven daemon policy.
3. Add richer tmux adoption/import.
4. Add better process/log inspection to classify account health from Codex stderr.
5. Add shell completions.
6. Add tests around DB and command construction.

