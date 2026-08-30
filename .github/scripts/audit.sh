#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

if ! command -v cargo-audit >/dev/null 2>&1; then
    printf '%s\n' 'rustsec: cargo-audit is unavailable' >&2
    exit 2
fi

cargo audit --deny warnings --file Cargo.lock
