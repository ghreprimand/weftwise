#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

failed=0
while IFS= read -r source; do
    lines=$(wc -l < "$source")
    if (( lines >= 2000 )); then
        printf 'file-size: %s has %d lines; limit is fewer than 2000\n' "$source" "$lines" >&2
        failed=1
    fi
done < <(git ls-files --cached --others --exclude-standard 'src/*.rs' 'src/**/*.rs')

if (( failed != 0 )); then
    exit 1
fi

printf '%s\n' 'file-size: production Rust files are below 2000 lines'
