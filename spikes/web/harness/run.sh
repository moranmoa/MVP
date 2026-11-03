#!/bin/bash
# Runs spike 5: relay + core/sync node + WebRTC native peer + headless
# Chromium and Firefox. Build first with build.sh. Results: dist/results.json.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1;; esac
cd "$here"

docker compose down --remove-orphans >/dev/null 2>&1 || true
docker compose up -d --build --quiet-pull relay >/dev/null 2>&1
# The relay makes a fresh key each run; the peers need its peer ID.
for _ in $(seq 60); do
  RELAY_ID=$(docker compose logs relay 2>/dev/null | grep -o 'peer_id=12D3[A-Za-z0-9]*' | head -1 | cut -d= -f2 || true)
  [ -n "$RELAY_ID" ] && break
  sleep 0.5
done
[ -n "$RELAY_ID" ] || { echo "relay did not start"; docker compose logs relay; exit 1; }
export RELAY_ID
echo "relay $RELAY_ID"

status=0
docker compose up --abort-on-container-exit --exit-code-from browsers sync native browsers || status=$?
docker compose logs relay sync native > dist/peers.log 2>&1 || true
docker compose down --remove-orphans >/dev/null 2>&1 || true
echo "Results: $here/dist/results.json (exit $status)"
exit $status
