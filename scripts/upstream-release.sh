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
jq -nc --arg tag "$tag" --arg revision "$revision" '{tag:$tag, revision:$revision}'
