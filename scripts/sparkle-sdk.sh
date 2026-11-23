#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=2.10.1
digest=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
destination="$project/.build/sparkle-$version"
if [[ ! -x "$destination/bin/sign_update" ]]; then
  mkdir -p "$project/.build"
  scratch=$(mktemp -d "$project/.build/sparkle.XXXXXX")
  trap 'rm -rf "$scratch"' EXIT
  gh release download "$version" --repo sparkle-project/Sparkle \
    --pattern "Sparkle-$version.tar.xz" --dir "$scratch"
  (cd "$scratch" && printf '%s  %s\n' "$digest" "Sparkle-$version.tar.xz" | shasum -a 256 -c >&2)
  mkdir "$scratch/sdk"
  tar -xf "$scratch/Sparkle-$version.tar.xz" -C "$scratch/sdk"
  mv "$scratch/sdk" "$destination"
fi
printf '%s\n' "$destination"
