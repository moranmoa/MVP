#!/bin/bash
# Real two-peer voice call, end to end, all in Docker (never the host):
# a throwaway relay and two Electron apps (each with its own real mvs-core,
# temp profile and keyring) on one internal Docker network. Chromium's fake
# microphone feeds the call; call.mjs drives both apps and checks the result.
#
#   bash qa/voice-e2e/run.sh            # build what is missing, then run
#   REBUILD=1 bash qa/voice-e2e/run.sh  # rebuild mvs-core and the UI first
#   KEEP=1 bash qa/voice-e2e/run.sh     # leave the containers for debugging
#   SRC=<dir> REBUILD=1 bash ...         # core/ and ui/ from another tree (a PR
#                                        # head via `git archive`), the rest from here
#
# Every run uses its own compose project (mvs-voice-<time>-<random>), so its
# containers, network and rendezvous volume never clash with another run or
# with the live relay. Build caches (cargo, node_modules) are shared volumes.
# Results: qa/voice-e2e/results/<project>/ (logs, results-a.json, results-b.json).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; mount() { (cd "$1" && pwd -W); };; *) mount() { (cd "$1" && pwd); };; esac
src="$(cd "${SRC:-$repo}" && pwd)"
build="$here/.build"
mkdir -p "$build"

# 1. mvs-core (Linux, release), the same way desktop/README.md builds it.
if [ -n "${REBUILD:-}" ] || [ ! -f "$build/mvs-core" ]; then
  echo "== building mvs-core"
  docker run --rm -v "$(mount "$src/core"):/work:ro" -v "$(mount "$build"):/out" \
    -v cargo-registry:/usr/local/cargo/registry -v mvs-voice-e2e-core-target:/target -e CARGO_TARGET_DIR=/target \
    -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" -w /work rust:1.90-bookworm \
    sh -c 'cargo build --release --locked -p api --bin mvs-core && cp /target/release/mvs-core /out/'
fi

# 2. The UI, a production build (the app loads it from app://mvs).
if [ -n "${REBUILD:-}" ] || [ ! -f "$build/ui/index.html" ]; then
  echo "== building the UI"
  docker run --rm -v "$(mount "$src/ui"):/app" -v mvs-voice-e2e-ui-nm:/app/node_modules -v "$(mount "$build"):/out" \
    -w /app node:22.23.4-bookworm \
    sh -c 'npm ci --no-audit --no-fund >/dev/null && npx vite build --outDir /out/ui --emptyOutDir --logLevel warn'
fi

# 3. The relay image from infra/relay (Docker's cache keeps this quick).
echo "== building the relay image"
docker build -q -t mvs-relay:voice-e2e --build-arg CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" "$(mount "$repo/infra/relay")" >/dev/null

# The desktop node_modules volume is per lockfile (see deps.sh).
export DEPS_KEY="$(sha256sum "$repo/desktop/package-lock.json" | cut -c1-12)"

P="mvs-voice-$(date +%s)-$RANDOM"
out="$here/results/$P"
mkdir -p "$out"
dc() { docker compose -p "$P" -f "$(mount "$here")/compose.yaml" "$@"; }
cleanup() {
  dc logs --no-color --timestamps > "$out/compose.log" 2>&1 || true
  [ -n "${KEEP:-}" ] || dc down -v --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

# 4. Desktop node_modules (Electron, playwright-core) into a shared cache volume.
echo "== desktop dependencies"
docker volume create "mvs-voice-e2e-desktop-nm-$DEPS_KEY" >/dev/null
docker volume create mvs-voice-e2e-deps-lock >/dev/null
dc run --rm --no-deps deps

# 5. The relay, then its peer ID.
dc up -d relay
relay_id=""
for _ in $(seq 60); do
  relay_id=$(dc logs relay 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -o 'relay node started peer_id=[A-Za-z0-9]*' | cut -d= -f2 || true)
  [ -n "$relay_id" ] && break
  sleep 1
done
[ -n "$relay_id" ] || { dc logs relay; echo "relay did not start"; exit 1; }
export MVS_RELAY="/dns4/relay/tcp/4001/p2p/$relay_id"
echo "== relay up ($P)"

# 6. Both apps at once; each exits non-zero when one of its checks failed.
dc up -d a b
code_a=$(docker wait "$(dc ps -aq a)")
code_b=$(docker wait "$(dc ps -aq b)")
dc logs --no-color --no-log-prefix a > "$out/a.log" 2>&1 || true
dc logs --no-color --no-log-prefix b > "$out/b.log" 2>&1 || true
dc logs --no-color --no-log-prefix relay > "$out/relay.log" 2>&1 || true
docker cp "$(dc ps -aq a)":/rendezvous/. "$(mount "$out")" >/dev/null 2>&1 || true
# Invite links (and, with MODE=restore, the throwaway owner's words) stay out of the results.
rm -f "$out"/*.invite.json

echo
echo "== results ($out)"
grep -hE "^(PASS|FAIL|SKIP) " "$out/a.log" "$out/b.log" || true
echo "app a exit $code_a, app b exit $code_b"
[ "$code_a" = 0 ] && [ "$code_b" = 0 ]
