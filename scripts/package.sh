#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
build_dir=${1:-"$project/.build/wu-target/release-fast"}
version=${2:-v1.5.0}
template=${WU_PLUS_APP_TEMPLATE:-"/Applications/Wu.app"}
artifact_dir=${WU_PLUS_DIST_DIR:-"$project/dist"}
plist_buddy=/usr/libexec/PlistBuddy
identity=${WU_PLUS_CODESIGN_IDENTITY:-}
sparkle=${WU_PLUS_SPARKLE_DIR:-}
feed=${WU_PLUS_UPDATE_FEED:-}
build=${WU_PLUS_BUILD_VERSION:-}
test_update=${WU_PLUS_UPDATE_TEST:-0}

actual=$("$plist_buddy" -c "Print :CFBundleShortVersionString" "$template/Contents/Info.plist")
if [[ $actual != "${version#v}" ]]; then
  printf '需要 Wu %s 的官方应用包作为资源模板，当前模板为 %s。\n' "$version" "$actual" >&2
  exit 1
fi
test -x "$build_dir/wu"
test -x "$build_dir/cli"

mkdir -p "$artifact_dir"
stage=$(mktemp -d "$artifact_dir/.stage.XXXXXX")
trap 'rm -rf "$stage"' EXIT

app="$stage/Wu Plus.app"
ditto "$template" "$app"
install -m 755 "$build_dir/wu" "$app/Contents/MacOS/wu"
install -m 755 "$build_dir/cli" "$app/Contents/MacOS/cli"
strip -S -x "$app/Contents/MacOS/wu" "$app/Contents/MacOS/cli"
plist="$app/Contents/Info.plist"
"$plist_buddy" -c 'Set :CFBundleIdentifier io.github.chenqinru.wu-plus' "$plist"
"$plist_buddy" -c 'Set :CFBundleName Wu Plus' "$plist"
"$plist_buddy" -c 'Set :CFBundleDisplayName Wu Plus' "$plist"
python3 - "$plist" "$feed" "$build" "$test_update" <<'PY'
import base64
import os
import plistlib
import re
import sys
from urllib.parse import urlsplit

filename, feed, build, test_update = sys.argv[1:]
with open(filename, "rb") as fp:
    plist = plistlib.load(fp)
for key in ("SUFeedURL", "SUPublicEDKey", "SURequireSignedFeed", "WuPlusUpdateTest"):
    plist.pop(key, None)
plist["CFBundleURLTypes"] = [{
    "CFBundleTypeRole": "Viewer",
    "CFBundleURLName": "Wu Plus",
    "CFBundleURLSchemes": ["wu-plus"],
}]
if build:
    if not re.fullmatch(r"[1-9][0-9]{0,3}\.[1-9][0-9]?", build):
        sys.exit("Invalid Wu Plus build version. Expected RUN_NUMBER.RUN_ATTEMPT.")
    plist["CFBundleVersion"] = build
if feed:
    url = urlsplit(feed)
    if not url.hostname or url.username or url.password or url.fragment or not build:
        sys.exit("Provide an appcast URL and a Wu Plus build version.")
    if test_update == "1":
        if url.scheme != "http" or url.hostname != "localhost":
            sys.exit("Test updates are restricted to http://localhost.")
        plist["WuPlusUpdateTest"] = True
        plist["CFBundleIdentifier"] = "io.github.chenqinru.wu-plus.UpdateTest"
        plist["CFBundleName"] = "Wu Plus Update Test"
        plist["CFBundleDisplayName"] = "Wu Plus Update Test"
        plist.pop("CFBundleURLTypes", None)
        plist.setdefault("NSAppTransportSecurity", {})["NSAllowsLocalNetworking"] = True
    elif url.scheme != "https" or not os.environ.get("WU_PLUS_CODESIGN_IDENTITY"):
        sys.exit("A production feed requires HTTPS and Developer ID signing.")
    public_key = os.environ.get("WU_PLUS_SPARKLE_PUBLIC_KEY", "").strip()
    if len(base64.b64decode(public_key, validate=True)) != 32:
        sys.exit("Provide a valid Sparkle Ed25519 public key.")
    if tuple(map(int, plist["LSMinimumSystemVersion"].split("."))) < (12, 0):
        plist["LSMinimumSystemVersion"] = "12.0"
    plist.update(
        SUFeedURL=feed,
        SUPublicEDKey=public_key,
        SURequireSignedFeed=True,
        SUVerifyUpdateBeforeExtraction=True,
    )
elif test_update == "1":
    sys.exit("Test updates require a local appcast URL.")
with open(filename, "wb") as fp:
    plistlib.dump(plist, fp)
PY
if [[ -n $feed ]]; then
  test -d "$sparkle/Sparkle.framework"
  mkdir -p "$app/Contents/Frameworks"
  ditto "$sparkle/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"
  install -m 644 "$sparkle/LICENSE" "$app/Contents/Resources/Sparkle-LICENSE.txt"
fi

if [[ -n $identity ]]; then
  [[ $identity == 'Developer ID Application: '* ]]
  sign=(codesign --force --sign "$identity" --options runtime --timestamp)
  if [[ -n ${WU_PLUS_CODESIGN_KEYCHAIN:-} ]]; then
    sign+=(--keychain "$WU_PLUS_CODESIGN_KEYCHAIN")
  fi
  if [[ -n $feed ]]; then
    sparkle_version="$app/Contents/Frameworks/Sparkle.framework/Versions/Current"
    while IFS= read -r -d '' component; do
      "${sign[@]}" --preserve-metadata=identifier,entitlements "$component"
    done < <(find -H "$sparkle_version" -depth -type d \( -name '*.xpc' -o -name '*.app' \) -print0)
    "${sign[@]}" "$sparkle_version/Autoupdate"
    "${sign[@]}" "$app/Contents/Frameworks/Sparkle.framework"
  fi
  for component in "$app/Contents/MacOS/"*; do
    if [[ -f $component && $component != "$app/Contents/MacOS/wu" ]] &&
      file -b "$component" | grep -q 'Mach-O'; then
      "${sign[@]}" "$component"
    fi
  done
  codesign -d --entitlements - --xml "$template" > "$stage/entitlements.plist"
  plutil -lint "$stage/entitlements.plist"
  "${sign[@]}" --entitlements "$stage/entitlements.plist" "$app"
else
  codesign --force --deep --sign - "$app"
fi
codesign --verify --deep --strict "$app"

"${project}/scripts/package-plugin.sh" "$artifact_dir"

if [[ -d "$artifact_dir/Wu Plus.app" ]]; then
  previous="$artifact_dir/previous-$(date -u +%Y%m%d-%H%M%S)"
  mkdir "$previous"
  mv "$artifact_dir/Wu Plus.app" "$previous/"
fi
mv "$app" "$artifact_dir/Wu Plus.app"
printf '%s\n' "$artifact_dir/Wu Plus.app" "$artifact_dir/Subversion.zip"
