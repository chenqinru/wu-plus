#!/usr/bin/env bash
set -euo pipefail

requested=${1:-}
endpoint=repos/dstrml/wu/releases/latest
if [[ -n $requested ]]; then
  [[ $requested =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]
  endpoint="repos/dstrml/wu/releases/tags/$requested"
fi
release=$(gh api "$endpoint")
tag=$(jq -er 'select(.draft == false and .prerelease == false) | .tag_name' <<<"$release")
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]
revision=$(gh api "repos/dstrml/wu/commits/$tag" --jq .sha)
[[ $revision =~ ^[0-9a-f]{40}$ ]]
asset=$(jq -ce --arg tag "$tag" '
  .assets[] | select(.name == "Wu-aarch64.dmg")
  | select(.browser_download_url == ("https://github.com/dstrml/wu/releases/download/" + $tag + "/Wu-aarch64.dmg")
           and (.digest | test("^sha256:[0-9a-f]{64}$")))
  | {url: .browser_download_url, sha256: (.digest | sub("^sha256:"; ""))}
' <<<"$release")
jq -nc --arg tag "$tag" --arg revision "$revision" --argjson asset "$asset" \
  '{tag:$tag, revision:$revision, asset:$asset}'
