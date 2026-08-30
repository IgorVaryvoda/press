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

go run github.com/rhysd/actionlint/cmd/actionlint@v1.7.12 -shellcheck= -pyflakes= .github/workflows/ci.yml .github/workflows/release.yml
