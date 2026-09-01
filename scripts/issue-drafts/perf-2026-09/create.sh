#!/usr/bin/env bash
set -euo pipefail
# Bulk-create the 9 perf issues via `gh issue create`.
# Requires: gh auth login -h github.com
DIR="$(cd "$(dirname "$0")" && pwd)"
for f in "$DIR"/0*.md; do
  title=$(head -n1 "$f" | sed 's/^# //')
  echo "Creating: $title"
  gh issue create --title "$title" --body-file "$f" --label enhancement
  sleep 1
done
echo "Done — created $(ls "$DIR"/0*.md | wc -l) issues."
