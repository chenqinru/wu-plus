#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
archive=true
bundled=false
case "${1:-}" in
  --directory-only) archive=false; shift ;;
  --bundle) bundled=true; shift ;;
esac
if [[ $bundled == true ]]; then
  app=${1:?Provide the macOS app bundle}
  test -d "$app/Contents/MacOS"
fi

target_dir="$project/.build/plugin-target"
(
  cd "$project"
  CARGO_TARGET_DIR="$target_dir" cargo build --locked --release --manifest-path plugin/Cargo.toml
)
if [[ -n ${CARGO_BUILD_TARGET:-} ]]; then
  target_dir="$target_dir/$CARGO_BUILD_TARGET"
fi
provider=wu-plus-subversion-provider
if [[ $bundled == true ]]; then
  install -m 755 "$target_dir/release/$provider" "$app/Contents/MacOS/$provider"
  exit 0
fi

artifact_dir=${1:-"${WU_PLUS_DIST_DIR:-$project/dist}"}
mkdir -p "$artifact_dir"
stage=$(mktemp -d "$artifact_dir/.plugin.XXXXXX")
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/Subversion/bin"
install -m 644 "$project/plugin/extension.toml" "$stage/Subversion/extension.toml"
install -m 755 "$target_dir/release/$provider" "$stage/Subversion/bin/$provider"
codesign --force --sign - "$stage/Subversion/bin/$provider"
codesign --verify --strict "$stage/Subversion/bin/$provider"
if [[ $archive == true ]]; then
    ditto -c -k --keepParent "$stage/Subversion" "$stage/Subversion.zip"
fi

# Preserve the directory linked by Wu when replacing plug-in files.
mkdir -p "$artifact_dir/Subversion/bin"
mv -f "$stage/Subversion/extension.toml" "$artifact_dir/Subversion/extension.toml"
mv -f "$stage/Subversion/bin/$provider" "$artifact_dir/Subversion/bin/$provider"
if [[ $archive == true ]]; then
    mv -f "$stage/Subversion.zip" "$artifact_dir/Subversion.zip"
    printf '%s\n' "$artifact_dir/Subversion.zip"
else
    printf '%s\n' "$artifact_dir/Subversion"
fi
