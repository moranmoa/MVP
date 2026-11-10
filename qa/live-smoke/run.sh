#!/bin/bash
# Live release smoke, in Docker: a release's Linux mvs-core, twice (owner and
# member), on two separate Docker networks, against the LIVE relay named in
# that release's resources/mvs-config.json. 10 rounds: A creates a server,
# B joins by invite, a message each way (live.mjs).
#
#   TAG=v0.3.0 bash qa/live-smoke/run.sh     # ROUNDS=10 by default
#
# Uses only the release's own config; never prints or stores the relay
# address outside the git-ignored .build/. Reuses qa/upgrade's download.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; mount() { (cd "$1" && pwd -W); };; *) mount() { (cd "$1" && pwd); };; esac
TAG=${TAG:-v0.3.0}
v=${TAG#v}
dl="$repo/qa/upgrade/.build/$TAG"
work="$here/.build"
if [ ! -f "$dl/mvs-core" ]; then
  OLD="$TAG" NEW="$TAG" bash "$repo/qa/upgrade/run.sh" >/dev/null 2>&1 || true # downloads and checks only
fi
P="mvs-live-$(date +%s)-$RANDOM"
rv="$work/$P"
mkdir -p "$rv"
# This run's own copies, so runs of different tags can overlap.
mkdir -p "$rv/core" "$rv/cfg"
cp "$dl/mvs-core" "$rv/core/mvs-core"
cp "$(find "$dl" -name mvs-config.json -path '*resources*' | head -1)" "$rv/cfg/mvs-config.json"
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
docker logs "$P-a" > "$rv/a.log" 2>&1
docker logs "$P-b" > "$rv/b.log" 2>&1
cat "$rv/b.log" "$rv/a.log" | grep -E "core |round |FAIL"
