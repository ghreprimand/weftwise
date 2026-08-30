#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

safety_mode=${1:---worktree}
case "$safety_mode" in
    --tree|--worktree) ;;
    *)
        printf '%s\n' 'usage: check.sh [--tree|--worktree]' >&2
        exit 2
        ;;
esac

gate_log_dir=${WEFTWISE_GATE_LOG_DIR:-target/gate-logs}
mkdir -p "$gate_log_dir"
gate_log=$gate_log_dir/phase-0.log

{
    cargo fmt --check
    cargo clippy --all-targets --locked -- -D warnings
    cargo test --locked
    cargo doc --no-deps --locked
    bash .github/scripts/check-file-sizes.sh
    bash .github/scripts/check-dependencies.sh
    bash .github/scripts/public-safety.sh "$safety_mode"
    bash .github/scripts/audit.sh
} 2>&1 | tee "$gate_log"
