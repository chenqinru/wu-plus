#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
archive=true
if [[ ${1:-} == --directory-only ]]; then
  archive=false
  shift
fi
artifact_dir=${1:-"${WU_PLUS_DIST_DIR:-$project/dist}"}
mkdir -p "$artifact_dir"
stage=$(mktemp -d "$artifact_dir/.plugin.XXXXXX")
trap 'rm -rf "$stage"' EXIT

target_dir="$project/.build/plugin-target"
(
  cd "$project"
  CARGO_TARGET_DIR="$target_dir" cargo build --locked --release --manifest-path plugin/Cargo.toml
)
if [[ -n ${CARGO_BUILD_TARGET:-} ]]; then
  target_dir="$target_dir/$CARGO_BUILD_TARGET"
fi
mkdir -p "$stage/Subversion/bin"
install -m 644 "$project/plugin/extension.toml" "$stage/Subversion/extension.toml"
install -m 755 "$target_dir/release/wu-plus-subversion-provider" "$stage/Subversion/bin/wu-plus-subversion-provider"
codesign --force --sign - "$stage/Subversion/bin/wu-plus-subversion-provider"
codesign --verify --strict "$stage/Subversion/bin/wu-plus-subversion-provider"
if [[ $archive == true ]]; then
    ditto -c -k --keepParent "$stage/Subversion" "$stage/Subversion.zip"
fi

# Preserve the directory linked by Wu when replacing plug-in files.
mkdir -p "$artifact_dir/Subversion/bin"
mv -f "$stage/Subversion/extension.toml" "$artifact_dir/Subversion/extension.toml"
mv -f "$stage/Subversion/bin/wu-plus-subversion-provider" "$artifact_dir/Subversion/bin/wu-plus-subversion-provider"
if [[ $archive == true ]]; then
    mv -f "$stage/Subversion.zip" "$artifact_dir/Subversion.zip"
    printf '%s\n' "$artifact_dir/Subversion.zip"
else
    printf '%s\n' "$artifact_dir/Subversion"
fi
