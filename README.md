# cx

`cx` is a local Codex CLI router for people who use multiple Codex profiles.

It gives you:

- isolated Codex accounts with separate `CODEX_HOME` directories
- smart account selection from latest 5h and weekly limit snapshots
- resume across Codex homes without remembering where a session lives
- repo-aware resume with `cx resume-here`
- install/config diagnostics with `cx doctor`
- optional tmux supervision for long-running sessions

The core idea is simple:

```bash
CODEX_HOME="$HOME/.cx/accounts/personal" codex
CODEX_HOME="$HOME/.cx/accounts/work" codex resume --last
```

`cx` wraps that pattern and adds account routing, session discovery, limit status, and optional tmux management.

![cx demo](assets/cx-demo.gif)

## Install

```bash
./install.sh
```

The installer uses `cargo` when available and falls back to `nix-shell -p cargo rustc` on machines with Nix.

Make sure `~/.local/bin` is on your `PATH`.

## Quick Start

Create two isolated Codex accounts:

```bash
cx account add personal
cx account login personal

cx account add work
cx account login work
```

Create a pool that chooses the best account from current limits:

```bash
cx pool create coding --accounts personal,work --strategy limit-aware
```

Make that pool the default:

```bash
cx config init
$EDITOR ~/.cx/config.toml
```

Run Codex:

```bash
cx personal
cx smart
cx smart -- exec "summarize this repo"
```

If you already use Codex under `~/.codex`, register that home instead of making a fresh one:

```bash
cx account add personal --codex-home ~/.codex
cx account check personal
```

## Daily Commands

```bash
# Pick the best account from all accounts or a pool
cx smart
cx smart --pool coding
cx smart --refresh

# Use a specific account
cx personal
cx work -- exec "fix the parser"

# Resume the latest session for the current git repo
cx resume-here
cx resume-here --smart
cx personal resume-here

# Resume by session id, even if the session started in another Codex home
cx sessions
cx resume <session-id>
cx personal resume <session-id>

# See account email, limits, and health
cx status
cx watch --once
cx explain
cx refresh --stale
cx account status
cx account status personal
cx account status personal --online

# Check local setup
cx doctor
cx doctor --fix

# Generate shell completions
cx completion zsh > ~/.zfunc/_cx
```

Pass Codex arguments after `--` whenever the command has its own `cx` options:

```bash
cx smart --pool coding -- --model gpt-5-codex exec "review this diff"
```

## How It Works

### Accounts

Each account has its own Codex home:

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

`cx account login personal` runs Codex with:

```bash
CODEX_HOME="$HOME/.cx/accounts/personal" codex login
```

For custom homes, `cx` uses the registered path:

```bash
cx account add personal --codex-home ~/.codex
```

### Codex Launcher

`cx` uses this launcher order:

1. `CX_CODEX_BIN`, when set
2. `codex_bin` from `~/.cx/config.toml`, when set
3. `$HOME/bin/codex`, when it exists
4. `codex` on `PATH`

This preserves local wrapper behavior, such as default sandbox flags:

```bash
CX_CODEX_BIN=/path/to/codex cx run --account personal -- --help
```

### Limits

Codex writes `rate_limits` snapshots into session JSONL files. `cx` reads the latest local snapshot and shows:

- 5h capacity remaining and reset
- weekly capacity remaining and reset
- observed snapshot age
- account health and active managed sessions

`cx` caches the latest limit snapshot for each Codex home in `~/.cx/cx.sqlite`. On a cache miss it samples recent session files instead of parsing the full history tree. Explicit refreshes read Codex's account usage endpoint without starting a model turn.

Local read:

```bash
cx account status
cx account status personal
cx watch --once
```

In account status tables, `CODEX` is the number of discovered Codex history sessions in that account home. `MGD` is the number of active cx-managed tmux sessions.

Run an active Codex health probe:

```bash
cx account status personal --online
```

`--online` may consume usage because it runs `codex exec`.

Refresh snapshots without spending a model turn:

```bash
cx refresh personal
cx refresh --pool coding --stale
cx refresh --all --stale
```

`cx smart --refresh` refreshes stale or missing snapshots before picking an account.

### Smart Routing

`limit-aware` routing chooses the eligible account with:

1. most 5h capacity remaining
2. then most weekly capacity remaining
3. then fewest active managed sessions

It skips accounts whose latest local snapshot shows an unexpired exhausted 5h or weekly limit. Accounts without snapshots are still usable, but score as unknown rather than best.

Explain the next smart pick without launching Codex:

```bash
cx explain
cx explain --pool coding
```

Pool strategies:

```text
first-healthy   first eligible account in pool order
least-sessions  fewest active managed sessions
limit-aware     most 5h capacity remaining, then weekly remaining, then sessions
```

### Resume

Codex sessions live under a `CODEX_HOME`, but `cx` searches all known homes:

- registered account homes
- the normal `~/.codex` home
- directories under `~/.cx/accounts`

Top-level resume uses the home that already owns the session:

```bash
cx resume <session-id>
cx resume --last
```

`cx` keeps a local SQLite index of discovered Codex session ids and repo cwd metadata. The first scan of a large `~/.codex/sessions` tree may take a moment; later resume and repo-aware lookups reuse the index and only parse new or changed session files.

Refresh or rebuild local indexes explicitly:

```bash
cx index
cx index --sessions --rebuild
cx index --limits
```

Account-scoped resume attaches the session to that account home first when needed:

```bash
cx personal resume <session-id>
```

`cx` keeps canonical Codex session files under `~/.cx/session-store/sessions` and hardlinks them into account homes. Each account still has its own `CODEX_HOME` for auth and config, but attached sessions share one backing file so future resumes do not fork history. Hardlinks require the session store and account homes to live on the same filesystem.

The attachment is conservative. `cx` refuses ambiguous selectors for different session ids, duplicate target copies, divergent target content, and path overwrites. If the same session id already exists in multiple homes, `cx` chooses the newest copy and imports it as canonical.

Existing installs migrate during normal session indexing:

```bash
cx index --sessions
cx index --sessions --dry-run
```

The migration imports discovered sessions into the central store and hardlinks identical account copies. Older divergent copies are left untouched instead of being overwritten silently.

`cx` also takes a per-session lock while running `codex resume` so two accounts do not append to the same JSONL transcript at the same time.

Repo-aware resume finds the latest known session whose recorded cwd is inside the current git repo:

```bash
cx resume-here
cx resume-here --smart
cx resume-here --pool coding
cx personal resume-here
```

Explicit pre-attachment is also available:

```bash
cx personal adopt <session-id>
```

Cross-account tmux migration also tries native history first. If `cx` can find a matching repo session, it attaches that session to the target account home and respawns with native `codex resume`. If not, it falls back to the semantic resume prompt.

### Config

`cx` reads `~/.cx/config.toml` when present:

```toml
default_pool = "coding"
default_strategy = "limit-aware"
limit_snapshot_max_age_minutes = 30

[smart]
refresh_before_pick = false

[daemon]
auto_migrate_auth_failed = true
auto_migrate_limited = false
auto_migrate_degraded = false
```

Commands:

```bash
cx config init
cx config path
cx config show

cx completion bash
cx completion zsh
cx completion fish
```

`CX_CODEX_BIN` still overrides `codex_bin` from config.

## Optional tmux Mode

Normal `cx` usage does not require tmux. The tmux commands are for managed long-running sessions.

Start a managed tmux session:

```bash
cx tmux run --pool coding --name atlas -C ~/code/atlas
cx tmux run --account personal --name atlas -C ~/code/atlas -- resume --last
```

List and restart:

```bash
cx tmux list
cx tmux restart atlas
```

Migrate a managed session:

```bash
cx tmux migrate atlas --account work
cx tmux migrate atlas --pool coding
```

## Daemon

The daemon is optional. It supervises managed tmux sessions.

```bash
cx daemon start
cx daemon status
cx daemon stop
```

Foreground debug mode:

```bash
cx daemon run --interval-secs 10
```

The daemon currently:

- checks auth locally by looking for `auth.json`
- restarts dead managed panes under the same account
- auto-migrates sessions away from disabled or auth-failed accounts when a pool has another candidate
- does not auto-migrate accounts marked `limited`

Daemon migration policy is configured under `[daemon]` in `~/.cx/config.toml`.

## Doctor

Run:

```bash
cx doctor
cx doctor --fix
```

It checks:

- config parse status
- Codex launcher and version
- account homes and auth files
- latest limit snapshot freshness
- default pool
- SQLite registry location
- tmux availability
- daemon pidfile/process status
- common git hygiene for ignored local files

`cx doctor --fix` creates a missing default config, adds common local artifacts to `.gitignore`, removes a stale daemon pidfile, and syncs local indexes.

## Command Reference

```bash
cx account add NAME [--codex-home PATH]
cx account login NAME
cx account logout NAME
cx account rename OLD NEW
cx account list
cx account status [NAME] [--online]
cx account check NAME [--online]
cx account check --all [--online]
cx account disable NAME [--reason TEXT]
cx account enable NAME

cx config init [--force]
cx config path
cx config show
cx completion SHELL

cx pool create NAME --accounts a,b,c [--strategy limit-aware]
cx pool list

cx tmux run [--account NAME | --pool NAME] --name NAME [-C PATH] -- ARGS...
cx tmux list
cx tmux restart NAME
cx tmux migrate NAME [--account NAME | --pool NAME]

cx run --account NAME -- ARGS...
cx run --pool NAME -- ARGS...
cx smart [--pool NAME] [--refresh] -- ARGS...
cx explain [--pool NAME]

cx sessions [--limit N]
cx resume <session-id>
cx resume --last
cx resume-here [--account NAME | --pool NAME | --smart]
cx NAME resume <session-id>
cx NAME resume-here
cx index [--sessions] [--limits] [--rebuild]

cx refresh [NAME] [--all | --pool NAME] [--stale]
cx watch [--once] [--interval-secs N]
cx doctor [--fix]
cx status
```

## Policy Note

`cx` is a local supervisor for Codex accounts you are authorized to use. It does not bypass limits. By default, limited accounts are not auto-migrated by the daemon.

## Development

Run checks:

```bash
cargo fmt -- --check
cargo test
cargo clippy -- -D warnings
```

Regenerate the terminal demo:

```bash
./demo/setup.sh
vhs demo/cx.tape
```

The demo fixture writes under `target/demo` and records through `/tmp/cx-demo`, so the GIF stays machine-neutral and does not touch real Codex accounts.

Obvious next steps:

1. Add a Unix-socket RPC layer so `cx` talks to `cxd` instead of both touching SQLite.
2. Add release packaging.
3. Add better process/log inspection to classify account health from Codex stderr.
