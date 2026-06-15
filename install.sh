#!/usr/bin/env bash
set -euo pipefail

cargo build --release
mkdir -p "$HOME/.local/bin"
install -m 755 target/release/cx "$HOME/.local/bin/cx"

echo "installed cx to $HOME/.local/bin/cx"
echo "make sure $HOME/.local/bin is on PATH"
