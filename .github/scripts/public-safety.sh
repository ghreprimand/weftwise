#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

mode=${1:---staged}
temporary_index=
case "$mode" in
    --staged)
        candidate_paths=$(git diff --cached --name-only --diff-filter=ACMR)
        if [[ -z "$candidate_paths" ]]; then
            printf '%s\n' 'public-safety: no staged changes to inspect' >&2
            exit 2
        fi
        ;;
    --tree)
        candidate_paths=$(git ls-files)
        ;;
    --worktree)
        temporary_index=$(mktemp)
        rm -f "$temporary_index"
        trap 'rm -f "$temporary_index"' EXIT
        export GIT_INDEX_FILE=$temporary_index
        git read-tree HEAD
        git add -A -- .
        candidate_paths=$(git ls-files)
        ;;
    *)
        printf '%s\n' 'usage: public-safety.sh [--staged|--tree|--worktree]' >&2
        exit 2
        ;;
esac

failed=0

dangerous_paths=$(printf '%s\n' "$candidate_paths" | \
    grep -E '(^|/)(\.env($|\.)|id_(rsa|dsa|ecdsa|ed25519)($|\.)|[^/]+\.(pem|key|p12|pfx)|\.archon(/|$)|reports?(/|$)|audit(/|$))' || true)
if [[ -n "$dangerous_paths" ]]; then
    printf '%s\n' 'public-safety: blocked sensitive or local-only path:' >&2
    printf '%s\n' "$dangerous_paths" >&2
    failed=1
fi

secret_pattern='BEGIN ([A-Z0-9]+ )?PRIVATE'' KEY|github_''pat_[A-Za-z0-9_]+|gh[pousr]_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|xox[baprs]-[A-Za-z0-9-]{10,}'
secret_matches=$(git grep --cached -nI -E "$secret_pattern" -- . \
    ':(exclude).github/scripts/public-safety.sh' || true)
if [[ -n "$secret_matches" ]]; then
    printf '%s\n' 'public-safety: possible credential material:' >&2
    printf '%s\n' "$secret_matches" >&2
    failed=1
fi

path_pattern='(/home/[A-Za-z0-9._-]+/|/Users/[A-Za-z0-9._-]+/|[A-Za-z]:\\Users\\[A-Za-z0-9._-]+\\)'
path_matches=$(git grep --cached -nI -E "$path_pattern" -- . || true)
if [[ -n "$path_matches" ]]; then
    printf '%s\n' 'public-safety: machine-specific home path:' >&2
    printf '%s\n' "$path_matches" >&2
    failed=1
fi

email_matches=$(git grep --cached -nI -E \
    '[A-Za-z0-9.!#$%&*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}' -- . \
    ':(exclude).github/scripts/public-safety.sh' | \
    grep -Ev '(@users\.noreply\.github\.com|@example\.(com|org|net|invalid))' || true)
if [[ -n "$email_matches" ]]; then
    printf '%s\n' 'public-safety: non-allowlisted email address:' >&2
    printf '%s\n' "$email_matches" >&2
    failed=1
fi

if [[ "$mode" != --tree ]]; then
    commit_email=$(git config --get user.email || true)
    if [[ ! "$commit_email" =~ @users\.noreply\.github\.com$ ]]; then
        printf '%s\n' 'public-safety: git user.email is not a GitHub no-reply address' >&2
        failed=1
    fi
fi

if [[ "$mode" != --tree ]] && ! git diff --cached --check; then
    printf '%s\n' 'public-safety: staged diff has whitespace errors' >&2
    failed=1
fi

if (( failed != 0 )); then
    exit 1
fi

printf 'public-safety: automated %s checks passed\n' "${mode#--}"
printf '%s\n' 'public-safety: manual review is still required'
