#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
mode=${1:-check}
toolchain=$(awk -F '"' '/^channel = / {print $2;exit}' "$project/rust-toolchain.toml")
upstream="$project/scripts/wu-upstream.json"
tag=$(jq -er .tag "$upstream")
target="$project/.build/wu-target"

case "$mode" in
  check|build|bundle|build-dir) ;;
  *) printf '%s\n' '用法: scripts/wu.sh {check|build|bundle|build-dir} [vX.Y.Z]' >&2; exit 2 ;;
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
if [[ $mode == bundle ]]; then
  "$project/scripts/package.sh" "$target/release-fast" "$tag"
fi
