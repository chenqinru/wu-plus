#!/usr/bin/env bash
set -euo pipefail

source_config=${1:-"$HOME/.config/wu"}
source_data=${2:-"$HOME/Library/Application Support/Wu"}
destination=${3:-"$HOME/Library/Application Support/Wu Plus"}
if [[ -e $destination || -L $destination ]]; then
  printf '目标已存在，迁移未开始：%s\n' "$destination" >&2
  exit 1
fi
if [[ ! -d $source_config && ! -d "$source_data/extensions/installed" ]]; then
  printf '没有找到 Wu 的设置或已安装扩展。\n' >&2
  exit 1
fi

mkdir -p "$(dirname "$destination")"
stage=$(mktemp -d "$(dirname "$destination")/.wu-plus-profile.XXXXXX")
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/config" "$stage/extensions" "$stage/extensions/installed"
if [[ -d $source_config ]]; then
  ditto "$source_config" "$stage/config"
fi
if [[ -d "$source_data/extensions/installed" ]]; then
  ditto "$source_data/extensions/installed" "$stage/extensions/installed"
  python3 - "$source_data/extensions/installed" "$stage/extensions/installed" <<'PY'
from pathlib import Path
import sys

source, destination = map(Path, sys.argv[1:])
for original in source.iterdir():
    if original.is_symlink() and not original.readlink().is_absolute():
        copied = destination / original.name
        copied.unlink()
        copied.symlink_to(original.resolve())
PY
fi
python3 - "$stage" "$destination" <<'PY'
import os
import sys

stage, destination = sys.argv[1:]
if os.path.lexists(destination):
    sys.exit(f"目标已存在，迁移未完成：{destination}")
os.rename(stage, destination)
PY
printf '%s\n' "$destination"
