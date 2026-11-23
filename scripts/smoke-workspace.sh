#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
base=${1:-"$project/.build"}
mkdir -p "$base"
base=$(cd "$base" && pwd)
smoke=$(mktemp -d "$base/smoke.XXXXXX")
mkdir "$smoke/workspace"
for name in backend frontend; do
  svnadmin create "$smoke/$name"
  svn checkout -q "file://$smoke/$name" "$smoke/workspace/$name"
  printf '%s\n' "Initial $name" > "$smoke/workspace/$name/README.txt"
  svn add -q "$smoke/workspace/$name/README.txt"
  svn commit -q -m "Fixture revision" "$smoke/workspace/$name"
  printf '%s\n' "Local change in $name" >> "$smoke/workspace/$name/README.txt"
done
printf 'Unversioned file\n' > "$smoke/workspace/frontend/新文件 @.txt"
git init -q "$smoke/workspace/git-fixture"
printf 'Git panel fixture\n' > "$smoke/workspace/git-fixture/README.txt"
printf '%s\n' "$smoke/workspace"
