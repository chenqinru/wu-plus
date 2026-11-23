#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
artifact_dir=${1:-"${WU_PLUS_DIST_DIR:-$project/dist}"}
mkdir -p "$artifact_dir"
stage=$(mktemp -d "$artifact_dir/.plugin.XXXXXX")
trap 'rm -rf "$stage"' EXIT

CARGO_TARGET_DIR="$project/.build/plugin-target" cargo +1.97.1 build   --locked --release --manifest-path "$project/plugin/Cargo.toml"
mkdir -p "$stage/Subversion/bin"
install -m 644 "$project/plugin/extension.toml" "$stage/Subversion/extension.toml"
install -m 755 "$project/.build/plugin-target/release/wu-plus-subversion-provider" "$stage/Subversion/bin/wu-plus-subversion-provider"
codesign --force --sign - "$stage/Subversion/bin/wu-plus-subversion-provider"
codesign --verify --strict "$stage/Subversion/bin/wu-plus-subversion-provider"
ditto -c -k --keepParent "$stage/Subversion" "$stage/Subversion.zip"

# Preserve the directory linked by Wu when replacing plug-in files.
mkdir -p "$artifact_dir/Subversion/bin"
mv -f "$stage/Subversion/extension.toml" "$artifact_dir/Subversion/extension.toml"
mv -f "$stage/Subversion/bin/wu-plus-subversion-provider" "$artifact_dir/Subversion/bin/wu-plus-subversion-provider"
mv -f "$stage/Subversion.zip" "$artifact_dir/Subversion.zip"
printf '%s\n' "$artifact_dir/Subversion.zip"
