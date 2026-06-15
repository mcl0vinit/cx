#!/usr/bin/env bash
set -euo pipefail

if command -v cargo >/dev/null 2>&1; then
  cargo build --release
elif command -v nix-shell >/dev/null 2>&1; then
  nix-shell -p cargo rustc --run "cargo build --release"
else
  echo "error: cargo not found; install Rust or Nix first" >&2
  exit 1
fi

mkdir -p "$HOME/.local/bin"
install -m 755 target/release/cx "$HOME/.local/bin/cx"

echo "installed cx to $HOME/.local/bin/cx"
echo "make sure $HOME/.local/bin is on PATH"
