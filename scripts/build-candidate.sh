#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
upstream="$project/scripts/wu-upstream.json"
output=$(mkdir -p "$1" && cd "$1" && pwd)
build=$2
repository_url=${3%/}
tag=$(jq -er .tag "$upstream")
revision=$(jq -er .revision "$upstream")
[[ $(uname -m) == arm64 && $(uname -s) == Darwin ]]
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ && $revision =~ ^[0-9a-f]{40}$ ]]
[[ $build =~ ^[1-9][0-9]{0,3}\.[1-9][0-9]?$ ]]
[[ $repository_url == https://* ]]
project_sha=$(git -C "$project" rev-parse HEAD)
if ! git -C "$project" merge-base --is-ancestor "$revision" "$project_sha"; then
  printf '源码提交未包含绑定的 Wu 上游提交 %s。\n' "$revision" >&2
  exit 1
fi
dirty=false
if [[ -n $(git -C "$project" status --porcelain) ]]; then
  dirty=true
fi
if [[ ${WU_PLUS_SIGN_CANDIDATES:-0} == 1 && $dirty == true ]]; then
  printf '%s\n' '正式签名候选必须来自干净的 Git 检出。' >&2
  exit 1
fi

scratch=$(mktemp -d)
mountpoint="$scratch/template"
keychain=
mounted=false
cleanup() {
  if [[ $mounted == true ]]; then
    hdiutil detach "$mountpoint" >/dev/null || true
  fi
  if [[ -n $keychain ]]; then
    security delete-keychain "$keychain" >/dev/null || true
  fi
  rm -rf "$scratch"
}
trap cleanup EXIT
mkdir "$mountpoint"

asset=$(jq -er .asset.url "$upstream")
digest=$(jq -er .asset.sha256 "$upstream")
[[ $asset == "https://github.com/dstrml/wu/releases/download/$tag/Wu-aarch64.dmg" &&
   $digest =~ ^[0-9a-f]{64}$ ]]
curl --fail --location --retry 3 "$asset" --output "$scratch/Wu.dmg"
(cd "$scratch" && printf '%s  %s\n' "$digest" Wu.dmg | shasum -a 256 -c)
hdiutil attach "$scratch/Wu.dmg" -readonly -nobrowse -mountpoint "$mountpoint"
mounted=true
template="$mountpoint/Wu.app"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$template/Contents/Info.plist")" = me.farshed.Wu
test "$(lipo -archs "$template/Contents/MacOS/wu")" = arm64

export WU_PLUS_APP_TEMPLATE="$template"
export WU_PLUS_DIST_DIR="$scratch/package"
export WU_PLUS_BUILD_VERSION="$build"
unset WU_PLUS_UPDATE_TEST
(
  unset APPLE_CERTIFICATE_P12 APPLE_CERTIFICATE_PASSWORD APPLE_NOTARY_KEY \
    APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER WU_PLUS_SPARKLE_PRIVATE_KEY GH_TOKEN
  "$project/scripts/wu.sh" check "$tag"
  "$project/scripts/wu.sh" build "$tag"
)
binary_dir=$("$project/scripts/wu.sh" build-dir "$tag")
signed=false
if [[ ${WU_PLUS_SIGN_CANDIDATES:-0} == 1 ]]; then
  : "${WU_PLUS_CODESIGN_IDENTITY:?}" "${APPLE_CERTIFICATE_P12:?}" "${APPLE_CERTIFICATE_PASSWORD:?}"
  : "${APPLE_NOTARY_KEY:?}" "${APPLE_NOTARY_KEY_ID:?}" "${APPLE_NOTARY_ISSUER:?}"
  : "${WU_PLUS_SPARKLE_PUBLIC_KEY:?}" "${WU_PLUS_SPARKLE_PRIVATE_KEY:?}"
  umask 077
  keychain="$scratch/wu-signing.keychain-db"
  password=$(openssl rand -hex 32)
  security create-keychain -p "$password" "$keychain"
  security set-keychain-settings -lut 14400 "$keychain"
  security unlock-keychain -p "$password" "$keychain"
  printf '%s' "$APPLE_CERTIFICATE_P12" | base64 -D > "$scratch/signing.p12"
  security import "$scratch/signing.p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
  security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain"
  printf '%s' "$APPLE_NOTARY_KEY" | base64 -D > "$scratch/notary.p8"
  export WU_PLUS_CODESIGN_KEYCHAIN="$keychain"
  export WU_PLUS_UPDATE_FEED="$repository_url/releases/latest/download/appcast.xml"
  export WU_PLUS_SPARKLE_DIR
  WU_PLUS_SPARKLE_DIR=$("$project/scripts/sparkle-sdk.sh")
  signed=true
else
  unset WU_PLUS_CODESIGN_IDENTITY WU_PLUS_UPDATE_FEED WU_PLUS_SPARKLE_PUBLIC_KEY
fi

(
  unset APPLE_CERTIFICATE_P12 APPLE_CERTIFICATE_PASSWORD APPLE_NOTARY_KEY \
    APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER WU_PLUS_SPARKLE_PRIVATE_KEY GH_TOKEN
  "$project/scripts/package.sh" "$binary_dir" "$tag"
)
app="$WU_PLUS_DIST_DIR/Wu Plus.app"
archive="$output/Wu-Plus-arm64.zip"
ditto -c -k --keepParent "$app" "$archive"
cp "$WU_PLUS_DIST_DIR/Subversion.zip" "$output/Subversion.zip"

if [[ $signed == true ]]; then
  notary=(--key "$scratch/notary.p8" --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER")
  if ! xcrun notarytool submit "$archive" "${notary[@]}" --wait --output-format json > "$output/notary.json" ||
     ! jq -e '.status == "Accepted"' "$output/notary.json" >/dev/null; then
    request=$(jq -r '.id // empty' "$output/notary.json" 2>/dev/null || true)
    if [[ -n $request ]]; then
      xcrun notarytool log "$request" "${notary[@]}" "$output/notary-log.json" || true
    fi
    exit 1
  fi
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
  rm "$archive"
  ditto -c -k --keepParent "$app" "$archive"
  signer="$WU_PLUS_SPARKLE_DIR/bin/sign_update"
  signature=$(printf '%s' "$WU_PLUS_SPARKLE_PRIVATE_KEY" | "$signer" -f - -p "$archive")
  python3 "$project/scripts/update-feed.py" create "$archive" "$app" \
    "$repository_url/releases/download/wu-plus-$build/$(basename "$archive")" "$signature" "$output/appcast.xml"
  printf '%s' "$WU_PLUS_SPARKLE_PRIVATE_KEY" | "$signer" -f - "$output/appcast.xml"
fi

archive_sha=$(shasum -a 256 "$archive" | awk '{print $1}')
appcast_sha=
if [[ $signed == true ]]; then
  appcast_sha=$(shasum -a 256 "$output/appcast.xml" | awk '{print $1}')
fi
jq -n --argjson upstream "$(cat "$upstream")" --arg project_sha "$project_sha" \
  --arg build "$build" --arg archive "$archive_sha" --arg appcast "$appcast_sha" \
  --argjson signed "$signed" --argjson dirty "$dirty" \
  '{upstream:$upstream, project_sha:$project_sha, dirty:$dirty, build:$build, notarized:$signed, archive_sha256:$archive, appcast_sha256:$appcast}' \
  > "$output/candidate.json"
printf 'Wu Plus %s (%s)\nSHA-256: %s\n' "$tag" "$build" "$archive_sha"
