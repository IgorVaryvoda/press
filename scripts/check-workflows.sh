#!/usr/bin/env bash

set -euo pipefail

go_version="$(go env GOVERSION)"
if [[ ! "$go_version" =~ ^go([0-9]+)\.([0-9]+)\.([0-9]+)([^0-9].*)?$ ]]; then
  echo "Go 1.25.0 or newer is required; found $go_version" >&2
  exit 1
fi

if (( BASH_REMATCH[1] < 1 || (BASH_REMATCH[1] == 1 && BASH_REMATCH[2] < 25) )); then
  echo "Go 1.25.0 or newer is required; found $go_version" >&2
  exit 1
fi

# A tag can be moved onto other code, and these workflows hand the updater's
# signing key and the Apple identity to whatever the tag resolves to. Actions in
# this repository (./) and container actions carry no such ref.
unpinned=0
while read -r ref; do
  case "$ref" in
  './'* | 'docker://'*) continue ;;
  esac
  if [[ ! "$ref" =~ @[0-9a-f]{40}$ ]]; then
    echo "unpinned action: $ref — use owner/action@<40-hex commit> # <tag>" >&2
    unpinned=1
  fi
done < <(sed -nE 's/^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*"?([^"[:space:]]+)"?.*$/\2/p' .github/workflows/*.yml)
(( unpinned == 0 )) || exit 1

go run github.com/rhysd/actionlint/cmd/actionlint@v1.7.12 -shellcheck= -pyflakes= .github/workflows/ci.yml .github/workflows/release.yml
