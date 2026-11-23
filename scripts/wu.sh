#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
mode=${1:-check}
toolchain=$(awk -F '"' '/^channel = / {print $2;exit}' "$project/rust-toolchain.toml")
upstream="$project/scripts/wu-upstream.json"
tag=$(jq -er .tag "$upstream")
target="$project/.build/wu-target"

case "$mode" in
  check|build|bundle|build-dir|prepare-debug) ;;
  *) printf '%s\n' '用法: scripts/wu.sh {check|build|bundle|build-dir|prepare-debug} [vX.Y.Z]' >&2; exit 2 ;;
esac
if [[ ${2:-"$tag"} != "$tag" ]]; then
  printf '当前提交已固定 Wu %s，请先将所需版本合入集成分支。\n' "$tag" >&2
  exit 2
fi
if [[ $mode == build-dir ]]; then
  printf '%s\n' "$target/release-fast"
  exit 0
fi

if [[ ${WU_PLUS_INSTALL_RUST:-0} == 1 ]]; then
  rustup toolchain install "$toolchain" --profile minimal --component rust-src
fi
export CARGO_NET_GIT_FETCH_WITH_CLI=${CARGO_NET_GIT_FETCH_WITH_CLI:-true}
export CARGO_TARGET_DIR="$target"
export CARGO_INCREMENTAL=0
cd "$project"
cargo "+$toolchain" --version >/dev/null
common=(--locked --profile release-fast --jobs "${WU_PLUS_BUILD_JOBS:-6}" --package wu --package cli --features gpui_platform/runtime_shaders)
if [[ $mode == check ]]; then
  cargo "+$toolchain" check "${common[@]}"
  CARGO_TARGET_DIR="$project/.build/plugin-target" cargo "+$toolchain" test --locked --manifest-path "$project/plugin/Cargo.toml"
else
  cargo "+$toolchain" build "${common[@]}"
fi
case "$mode" in
  bundle)
    "$project/scripts/package.sh" "$target/release-fast" "$tag"
    ;;
  prepare-debug)
    extension_dir="$project/.build/dev-extension"
    extension="$extension_dir/Subversion"
    installed_dir="$project/.build/dev-profile/extensions/installed"
    installed_extension="$installed_dir/wu-plus-subversion"
    "$project/scripts/package-plugin.sh" "$extension_dir"
    mkdir -p "$installed_dir"
    if [[ ! -e $installed_extension && ! -L $installed_extension ]]; then
      ln -sn "$extension" "$installed_extension"
    elif [[ ! -L $installed_extension || ! $installed_extension -ef $extension ]]; then
      printf '调试配置中已有其他 Subversion 扩展：%s\n' "$installed_extension" >&2
      exit 1
    fi

    debug_dir="$project/.build/dev-bin"
    mkdir -p "$debug_dir"
    stage=$(mktemp -d "$debug_dir/.stage.XXXXXX")
    trap 'rm -rf "$stage"' EXIT

    codelldb_version=v1.12.3
    codelldb_sha256=2f114a990e1b368dd1dbd33c80c0e719767af2d228391ec0df0571c957f9ac91
    codelldb_dir="codelldb-$codelldb_version"
    if [[ ! -x "$project/.build/$codelldb_dir/extension/adapter/codelldb" ]]; then
      codelldb_asset=codelldb-darwin-arm64.vsix
      curl --fail --location --silent --show-error --retry 2 --connect-timeout 15 \
        --output "$stage/$codelldb_asset" \
        "https://github.com/vadimcn/codelldb/releases/download/$codelldb_version/$codelldb_asset"
      (cd "$stage" && printf '%s  %s\n' "$codelldb_sha256" "$codelldb_asset" | shasum -a 256 -c)
      ditto -x -k "$stage/$codelldb_asset" "$stage/$codelldb_dir"
      test -x "$stage/$codelldb_dir/extension/adapter/codelldb"
      mv "$stage/$codelldb_dir" "$project/.build/"
    fi
    ln -sfn "$codelldb_dir" "$project/.build/codelldb"

    # Keep the macOS debugging entitlement off the binary used for packaging.
    cp -c "$target/release-fast/wu" "$stage/wu"
    printf '%s\n' '{"com.apple.security.get-task-allow":true}' |
      plutil -convert xml1 -o "$stage/entitlements.plist" -
    codesign --force --sign - --entitlements "$stage/entitlements.plist" "$stage/wu"
    mv -f "$stage/wu" "$debug_dir/wu"
    ;;
esac
