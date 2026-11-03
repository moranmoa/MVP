#!/bin/bash
# Markdown tables from run results, in a Node container.
# Usage: summarize.sh [results.json ...] (paths inside harness/; default dist/results.json)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; here="$(cd "$here" && pwd -W)";; esac
files=("$@")
[ ${#files[@]} -gt 0 ] || files=(dist/results.json)
docker run --rm -v "$here:/h" -w /h node:22-bookworm node summarize.mjs "${files[@]}"
