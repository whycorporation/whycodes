#!/bin/sh
# Copy repo-managed git hooks into .git/hooks. Idempotent; safe to re-run.
set -eu

root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$root"

if ! git rev-parse --git-dir >/dev/null 2>&1; then
    echo "install_git_hooks: not a git checkout" >&2
    exit 1
fi

# Worktrees have .git as a file pointing at the common dir.
git_dir="$(git rev-parse --git-dir)"
hooks_dir="$git_dir/hooks"
mkdir -p "$hooks_dir"

install_hook() {
    src="$1"
    name="$2"
    dest="$hooks_dir/$name"
    cp "$src" "$dest"
    chmod +x "$dest"
    echo "install_git_hooks: $dest"
}

install_hook "$root/scripts/pre-push" pre-push
