#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

dependency_tree=$(cargo tree --locked --prefix none)

gtk4_versions=$(printf '%s\n' "$dependency_tree" | awk '$1 == "gtk4" { print $2 }' | sort -u)
gtk4_count=$(printf '%s\n' "$gtk4_versions" | grep -c . || true)
if (( gtk4_count != 1 )); then
    printf '%s\n' 'dependencies: expected exactly one gtk4-rs version' >&2
    printf '%s\n' "$gtk4_versions" >&2
    exit 1
fi

if printf '%s\n' "$dependency_tree" | awk '$1 == "gtk" { found = 1 } END { exit !found }'; then
    printf '%s\n' 'dependencies: unexpected GTK3 binding detected' >&2
    exit 1
fi

tokio_versions=$(printf '%s\n' "$dependency_tree" | awk '$1 == "tokio" { print $2 }' | sort -u)
tokio_count=$(printf '%s\n' "$tokio_versions" | grep -c . || true)
if (( tokio_count != 1 )); then
    printf '%s\n' 'dependencies: expected exactly one Tokio version' >&2
    printf '%s\n' "$tokio_versions" >&2
    exit 1
fi

if printf '%s\n' "$dependency_tree" | awk \
    '$1 == "async-io" || $1 == "async-executor" || $1 == "async-std" || $1 == "smol" { found = 1 } END { exit !found }'; then
    printf '%s\n' 'dependencies: unintended second async runtime detected' >&2
    exit 1
fi

printf 'dependencies: one gtk4-rs line (%s), one Tokio line (%s), no second async runtime\n' \
    "$gtk4_versions" "$tokio_versions"
