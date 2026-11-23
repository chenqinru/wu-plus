#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
build_dir=${1:-"$project/.build/wu-target/release-fast"}
upstream_tag=${2:-$(jq -er .tag "$project/scripts/wu-upstream.json")}
template=${WU_PLUS_APP_TEMPLATE:-"/Applications/Wu.app"}
artifact_dir=${WU_PLUS_DIST_DIR:-"$project/dist"}
version=$(awk -F '"' '/^version = / {print $2; exit}' "$project/crates/wu/Cargo.toml")
plist_buddy=/usr/libexec/PlistBuddy

actual=$("$plist_buddy" -c 'Print :CFBundleShortVersionString' "$template/Contents/Info.plist")
if [[ $actual != "${upstream_tag#v}" ]]; then
  printf '需要 Wu %s 的官方应用包作为资源模板，当前模板为 %s。\n' "$upstream_tag" "$actual" >&2
  exit 1
fi
test -x "$build_dir/wu"
test -x "$build_dir/cli"
case "$(lipo -archs "$build_dir/wu")" in
  arm64) arch=aarch64 ;;
  x86_64) arch=x86_64 ;;
  *) printf '不支持该 macOS 二进制架构。\n' >&2; exit 1 ;;
esac

mkdir -p "$artifact_dir"
stage=$(mktemp -d "$artifact_dir/.stage.XXXXXX")
trap 'rm -rf "$stage"' EXIT

mkdir "$stage/dmg"
app="$stage/dmg/Wu Plus.app"
ditto "$template" "$app"
install -m 755 "$build_dir/wu" "$app/Contents/MacOS/wu"
install -m 755 "$build_dir/cli" "$app/Contents/MacOS/cli"
strip -S -x "$app/Contents/MacOS/wu" "$app/Contents/MacOS/cli"
python3 - "$app/Contents/Info.plist" "$version" <<'PY'
import plistlib
import sys

filename, version = sys.argv[1:]
with open(filename, "rb") as file:
    plist = plistlib.load(file)
plist.update(
    CFBundleIdentifier="io.github.chenqinru.wu-plus",
    CFBundleName="Wu Plus",
    CFBundleDisplayName="Wu Plus",
    CFBundleShortVersionString=version,
    CFBundleVersion=version,
    CFBundleURLTypes=[{
        "CFBundleTypeRole": "Viewer",
        "CFBundleURLName": "Wu Plus",
        "CFBundleURLSchemes": ["wu-plus"],
    }],
)
with open(filename, "wb") as file:
    plistlib.dump(plist, file)
PY

# Use the same entitlement set as the ad-hoc branch of script/bundle-mac.
entitlements="$app/Contents/Resources/zed.entitlements"
cp "$project/crates/wu/resources/zed.entitlements" "$entitlements"
if "$plist_buddy" -c 'Print :com.apple.developer.associated-domains' "$entitlements" >/dev/null 2>&1; then
  "$plist_buddy" -c 'Delete :com.apple.developer.associated-domains' "$entitlements"
fi
codesign --force --deep --entitlements "$entitlements" --sign - "$app"
codesign --verify --deep --strict "$app"
ln -s /Applications "$stage/dmg/Applications"
dmg="Wu-Plus-$arch.dmg"
hdiutil create -quiet -volname 'Wu Plus' -srcfolder "$stage/dmg" -format UDZO "$stage/$dmg"

"$project/scripts/package-plugin.sh" "$artifact_dir"
if [[ -d "$artifact_dir/Wu Plus.app" ]]; then
  previous=$(mktemp -d "$artifact_dir/previous.XXXXXX")
  mv "$artifact_dir/Wu Plus.app" "$previous/"
fi
mv "$app" "$artifact_dir/Wu Plus.app"
mv -f "$stage/$dmg" "$artifact_dir/$dmg"
printf '%s\n' "$artifact_dir/$dmg" "$artifact_dir/Subversion.zip"
