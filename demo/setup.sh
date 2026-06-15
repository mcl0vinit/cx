#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
demo_root="$repo_root/target/demo"
display_root="${CX_DEMO_DISPLAY_ROOT:-/tmp/cx-demo}"
demo_home="$display_root/home"
bin_dir="$display_root/bin"
cx_bin="$repo_root/target/release/cx"
fake_codex="$bin_dir/codex"

if command -v cargo >/dev/null 2>&1; then
  cargo build --release >/dev/null
elif [[ -x "$cx_bin" ]]; then
  :
elif command -v nix-shell >/dev/null 2>&1; then
  nix-shell -p cargo rustc --run "cargo build --release" >/dev/null
else
  echo "error: cargo not found and target/release/cx does not exist" >&2
  exit 1
fi

rm -rf "$demo_root"
mkdir -p "$demo_root"

if [[ -e "$display_root" && ! -L "$display_root" ]]; then
  echo "error: $display_root exists and is not a symlink" >&2
  exit 1
fi
ln -sfn "$demo_root" "$display_root"

mkdir -p "$bin_dir" "$demo_home"
ln -sf "$cx_bin" "$bin_dir/cx"

cat >"$fake_codex" <<'FAKE_CODEX'
#!/usr/bin/env bash
set -euo pipefail

display_path() {
  local value="$1"
  case "$value" in
    "$HOME"/*) printf '~/%s' "${value#"$HOME/"}" ;;
    "$HOME") printf '~' ;;
    *) printf '%s' "$value" ;;
  esac
}

case "${1:-}" in
  --version|-V|version)
    echo "codex-cli 0.139.0"
    ;;
  exec)
    echo "ok"
    ;;
  login)
    mkdir -p "${CODEX_HOME:?CODEX_HOME is required}"
    printf '{"demo":true}\n' >"$CODEX_HOME/auth.json"
    echo "logged in"
    ;;
  logout)
    rm -f "${CODEX_HOME:?CODEX_HOME is required}/auth.json"
    echo "logged out"
    ;;
  resume)
    session="${2:---last}"
    printf 'codex demo: CODEX_HOME=%s\n' "$(display_path "${CODEX_HOME:-}")"
    printf 'codex demo: resuming %s\n' "$session"
    printf 'codex demo: args=%s\n' "$*"
    ;;
  *)
    printf 'codex demo: %s\n' "$*"
    ;;
esac
FAKE_CODEX
chmod +x "$fake_codex"

export HOME="$demo_home"
export CX_HOME="$demo_home/.cx"
export CX_CODEX_BIN="$fake_codex"

"$cx_bin" config init --force >/dev/null
cat >"$CX_HOME/config.toml" <<CONFIG
default_pool = "coding"
default_strategy = "limit-aware"
limit_snapshot_max_age_minutes = 30

[smart]
refresh_before_pick = false

[daemon]
auto_migrate_auth_failed = true
auto_migrate_limited = false
auto_migrate_degraded = false
CONFIG

for account in personal work overflow; do
  "$cx_bin" account add "$account" >/dev/null
  printf '{"demo":true}\n' >"$CX_HOME/accounts/$account/auth.json"
done

"$cx_bin" account check --all >/dev/null
"$cx_bin" pool create coding --accounts personal,work,overflow --strategy limit-aware >/dev/null

json_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

write_session() {
  local account="$1"
  local id="$2"
  local cwd="$3"
  local primary="$4"
  local weekly="$5"
  local balance="$6"
  local observed five_reset weekly_reset path escaped_cwd

  observed="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  five_reset="$(($(date +%s) + 4 * 60 * 60 + 12 * 60))"
  weekly_reset="$(($(date +%s) + 5 * 24 * 60 * 60 + 6 * 60 * 60))"
  path="$CX_HOME/accounts/$account/sessions/2026/06/15/rollout-$id.jsonl"
  escaped_cwd="$(json_escape "$cwd")"

  mkdir -p "$(dirname "$path")"
  cat >"$path" <<JSONL
{"timestamp":"$observed","type":"session_meta","payload":{"id":"$id","cwd":"$escaped_cwd"}}
{"timestamp":"$observed","type":"event_msg","payload":{"type":"token_count","rate_limits":{"limit_id":"codex","limit_name":"Codex demo","plan_type":"team","primary":{"used_percent":$primary,"window_minutes":300,"resets_at":$five_reset},"secondary":{"used_percent":$weekly,"window_minutes":10080,"resets_at":$weekly_reset},"credits":{"has_credits":true,"unlimited":false,"balance":"$balance"}}}}
JSONL
}

write_session \
  personal \
  019ecc26-088b-7a53-9682-e2c3286727da \
  "$repo_root" \
  61.0 \
  42.0 \
  19.50

write_session \
  work \
  029ecc26-088b-7a53-9682-e2c3286727db \
  "/tmp/cx-demo-other/work" \
  18.0 \
  33.0 \
  27.25

write_session \
  overflow \
  039ecc26-088b-7a53-9682-e2c3286727dc \
  "/tmp/cx-demo-other/overflow" \
  97.0 \
  12.0 \
  8.00

cat <<EOF
Demo fixture ready.

  HOME=$demo_home
  CX_HOME=$CX_HOME
  CX_CODEX_BIN=$fake_codex
  PATH=$bin_dir:\$PATH
EOF
