#!/bin/bash
# Builds the harness binaries in a Rust container, then runs the scenarios.
# Needs the relay from infra/ to be up. Run from anywhere (Git Bash on Windows).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
core="$here/../.."
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; core_mount="$(cd "$core" && pwd -W)";; *) core_mount="$(cd "$core" && pwd)";; esac

docker run --rm -v "$core_mount:/work" -v cargo-registry:/usr/local/cargo/registry \
  -v core-target-sync:/target -e CARGO_TARGET_DIR=/target -w /work rust:1.90-bookworm \
  bash -c "cargo build --release -p sync --bins && mkdir -p sync/harness/dist && cp /target/release/sync-peer /target/release/sync-harness sync/harness/dist/"

cd "$here"
docker compose down -v --remove-orphans >/dev/null 2>&1 || true
status=0
docker compose up --abort-on-container-exit --exit-code-from harness || status=$?
docker compose logs harness > dist/harness.log 2>&1 || true
docker compose logs p1 p2 p3 p4 > dist/peers.log 2>&1 || true
docker compose down -v --remove-orphans >/dev/null 2>&1 || true
echo "Results: $here/dist/results.md (exit $status)"
exit $status
