#!/bin/bash
# Live release smoke, in Docker: a release's Linux mvs-core, twice (owner and
# member), on two separate Docker networks, against the LIVE relay named in
# that release's resources/mvs-config.json. 10 rounds: A creates a server,
# B joins by invite, a message each way (live.mjs).
#
#   MVS_LIVE_SMOKE_OK="<manager's OK, e.g. v0.3.0 smoke 2026-11-10>" TAG=v0.3.0 bash qa/live-smoke/run.sh
#
# Tech lead's conditions (#281):
#   - QA runs it, after a release, with the manager's OK for each run
#     (MVS_LIVE_SMOKE_OK). It never runs in CI: it refuses when CI is set.
#   - No relay address or IP in its output: everything printed, and the
#     saved logs, go through mask(). The address stays in the git-ignored
#     .build/ only. Reuses qa/upgrade's download.
set -euo pipefail
if [ -n "${CI:-}" ] || [ -n "${GITHUB_ACTIONS:-}" ]; then
  echo "live smoke: never in CI (it uses the live relay)" >&2
  exit 2
fi
if [ -z "${MVS_LIVE_SMOKE_OK:-}" ]; then
  echo "live smoke: needs the manager's OK for this run: MVS_LIVE_SMOKE_OK=\"<who/when>\"" >&2
  exit 2
fi
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; mount() { (cd "$1" && pwd -W); };; *) mount() { (cd "$1" && pwd); };; esac
TAG=${TAG:-v0.3.0}
dl="$repo/qa/upgrade/.build/$TAG"
work="$here/.build"
if [ ! -f "$dl/mvs-core" ]; then
  OLD="$TAG" NEW="$TAG" bash "$repo/qa/upgrade/run.sh" >/dev/null 2>&1 || true # downloads and checks only
fi
P="mvs-live-$(date +%s)-$RANDOM"
rv="$work/$P"
# This run's own copies, so runs of different tags can overlap.
mkdir -p "$rv/core" "$rv/cfg"
cp "$dl/mvs-core" "$rv/core/mvs-core"
cp "$(find "$dl" -name mvs-config.json -path '*resources*' | head -1)" "$rv/cfg/mvs-config.json"

# Hides the relay's host, peer IDs, multiaddr hosts and any IP address.
relay_host=$(grep -o '"relay" *: *"/[a-z0-9]*/[^/"]*' "$rv/cfg/mvs-config.json" | sed 's#.*/##' | head -1)
mask() {
  sed -E \
    -e "s#${relay_host:-<no-relay-host>}#<relay-host>#g" \
    -e 's#/(dns4|dns6|dnsaddr|ip4|ip6)/[^/ "]+#/\1/<masked>#g' \
    -e 's#/p2p/[1-9A-HJ-NP-Za-km-z]{40,}#/p2p/<peer>#g' \
    -e 's#([0-9]{1,3}\.){3}[0-9]{1,3}#<ip>#g' \
    -e 's#([0-9a-fA-F]{0,4}:){3,7}[0-9a-fA-F]{1,4}#<ip6>#g'
}
echo "live smoke $TAG, OK: $MVS_LIVE_SMOKE_OK" | mask

cleanup() { docker rm -f "$P-a" "$P-b" >/dev/null 2>&1 || true; docker network rm "$P-a" "$P-b" >/dev/null 2>&1 || true; }
trap cleanup EXIT INT TERM
docker network create "$P-a" >/dev/null
docker network create "$P-b" >/dev/null
side() { # role
  docker run -d --name "$P-$1" --network "$P-$1" -e ROLE="$1" -e ROUNDS="${ROUNDS:-10}" \
    -v "$(mount "$rv/core"):/core:ro" -v "$(mount "$rv/cfg"):/cfg:ro" -v "$(mount "$here"):/qa:ro" -v "$(mount "$rv"):/rendezvous" \
    mvs-voice-e2e-app sh -c "chmod 777 /rendezvous 2>/dev/null; runuser -u pwuser -- dbus-run-session -- sh -c 'echo -n test | gnome-keyring-daemon --unlock --components=secrets >/dev/null && node /qa/live.mjs'" >/dev/null
}
side a
side b
docker wait "$P-a" "$P-b" >/dev/null
docker logs "$P-a" 2>&1 | mask > "$rv/a.log"
docker logs "$P-b" 2>&1 | mask > "$rv/b.log"
cat "$rv/b.log" "$rv/a.log" | grep -E "core |round |FAIL"
