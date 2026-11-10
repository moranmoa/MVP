#!/bin/bash
# Cross-version join check, in Docker: an owner and a joiner, each its own
# release's mvs-core on its own Docker network, meeting only through a
# throwaway relay (infra/relay, never the live one). Does the join finish?
#
#   OWNER=v0.2.5 JOINER=v0.3.1 MODE=new     bash qa/join-matrix/run.sh
#   OWNER=v0.2.5 JOINER=v0.3.1 MODE=restore bash qa/join-matrix/run.sh   # joiner = owner's words
#   SAME_LAN=1 ...      both on one Docker network (a home LAN: direct dials work)
#   RELAY_ONLY=a|b|both ...  setRelayOnly(true) on that side before create/join
#   PRESERVERS=60 MSGS=5 ROUNDS=5 ...  the owner first makes 60 servers with 5 messages each
#   QUIET_START=1 ...  the joiner starts only after the owner has made them
#                       (a long-used identity), then 5 joins in a row, each to a new server
#
# Release cores come from qa/upgrade/run.sh's download (SHA256SUMS checked).
# The relay uses infra/docker-compose.yml's circuit limits (32 MiB, 3600 s), as live;
# set MAX_CIRCUIT_BYTES= (empty) etc. for libp2p's defaults (128 KiB, 120 s).
# Logs, with addresses and peer IDs masked: qa/join-matrix/.build/<run>/.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; mount() { (cd "$1" && pwd -W); };; *) mount() { (cd "$1" && pwd); };; esac
OWNER=${OWNER:-v0.2.5}
JOINER=${JOINER:-v0.3.1}
MODE=${MODE:-new}
for t in "$OWNER" "$JOINER"; do
  [ -f "$repo/qa/upgrade/.build/$t/mvs-core" ] || OLD="$t" NEW="$t" bash "$repo/qa/upgrade/run.sh" >/dev/null 2>&1 || true
done
P="mvs-join-$(date +%s)-$RANDOM"
rv="$here/.build/$P"
mkdir -p "$rv/a" "$rv/b" "$rv/rv"
cp "$repo/qa/upgrade/.build/$OWNER/mvs-core" "$rv/a/mvs-core"
cp "$repo/qa/upgrade/.build/$JOINER/mvs-core" "$rv/b/mvs-core"
mask() {
  sed -E -e 's#/p2p/[1-9A-HJ-NP-Za-km-z]{40,}#/p2p/<peer>#g' -e 's#([0-9]{1,3}\.){3}[0-9]{1,3}#<ip>#g' \
    -e 's#\b12D3KooW[1-9A-HJ-NP-Za-km-z]{40,}#<peer>#g' -e 's#\x1b\[[0-9;]*m##g'
}
cleanup() {
  docker rm -f "$P-relay" "$P-a" "$P-b" >/dev/null 2>&1 || true
  docker network rm "$P-a" "$P-b" >/dev/null 2>&1 || true
  rm -f "$rv"/rv/*.ready*.json # it holds the owner's recovery words
}
trap cleanup EXIT INT TERM
docker network create "$P-a" >/dev/null
docker network create "$P-b" >/dev/null
docker run -d --name "$P-relay" --network "$P-a" --network-alias relay --read-only --tmpfs /tmp --tmpfs /data:uid=10001,mode=700 \
  -e ALLOW_PRIVATE_ADDRS=1 -e EXTERNAL_ADDRS=/dns4/relay/tcp/4001 -e RUST_LOG=info \
  -e MAX_CIRCUITS="${MAX_CIRCUITS-256}" -e MAX_CIRCUITS_PER_PEER="${MAX_CIRCUITS_PER_PEER-8}" \
  -e MAX_CIRCUIT_SECS="${MAX_CIRCUIT_SECS-3600}" -e MAX_CIRCUIT_BYTES="${MAX_CIRCUIT_BYTES-33554432}" mvs-relay:voice-e2e >/dev/null
docker network connect --alias relay "$P-b" "$P-relay"
relay_id=""
for _ in $(seq 60); do
  relay_id=$(docker logs "$P-relay" 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -o 'relay node started peer_id=[A-Za-z0-9]*' | cut -d= -f2 || true)
  [ -n "$relay_id" ] && break
  sleep 1
done
[ -n "$relay_id" ] || { echo "relay did not start"; exit 1; }
side() { # role
  net="$P-$1"; [ -n "${SAME_LAN:-}" ] && net="$P-a" # one LAN: direct dials work
  docker run -d --name "$P-$1" --network "$net" -e ROLE="$1" -e MODE="$MODE" -e RELAY_ONLY="${RELAY_ONLY:-}" -e JOIN_TIMEOUT="${JOIN_TIMEOUT:-120}" -e PRESERVERS="${PRESERVERS:-0}" -e MSGS="${MSGS:-0}" -e ROUNDS="${ROUNDS:-1}" \
    -e MVS_RELAY="/dns4/relay/tcp/4001/p2p/$relay_id" \
    -v "$(mount "$rv/$1"):/core:ro" -v "$(mount "$here"):/qa:ro" -v "$(mount "$rv/rv"):/rendezvous" \
    mvs-voice-e2e-app sh -c "chmod 777 /rendezvous; runuser -u pwuser -- dbus-run-session -- sh -c 'echo -n test | gnome-keyring-daemon --unlock --components=secrets >/dev/null && node /qa/join.mjs'" >/dev/null
}
side a
# QUIET_START=1: the joiner starts only once the owner has made its servers,
# so the two first meet with nothing being written.
if [ -n "${QUIET_START:-}" ]; then
  for _ in $(seq 1800); do [ -f "$rv/rv/a.prepared.json" ] && break; sleep 0.5; done
fi
side b
docker wait "$P-a" "$P-b" >/dev/null
for r in a b; do
  docker logs "$P-$r" 2>&1 | mask > "$rv/$r.log"
  mask < "$rv/rv/core-$r.log" > "$rv/core-$r.log" 2>/dev/null || true
done
docker logs "$P-relay" 2>&1 | mask > "$rv/relay.log"
rm -f "$rv"/rv/core-*.log
echo "== owner $OWNER, joiner $JOINER, mode $MODE, same LAN ${SAME_LAN:-no}, relay-only ${RELAY_ONLY:-none} ($rv)"
grep -hE "core |restored|pre-created|JOINED|HANG|FAIL|members now" "$rv/b.log" "$rv/a.log"
# Pull refusals, logged on the side that answers (sync's answer limits).
for r in a b; do
  f="$rv/core-$r.log"
  echo "$r: pull refusals: rate-limit $(grep -c 'pull rate limit exceeded' "$f") too-many $(grep -c 'too many pulls being answered' "$f") budget $(grep -c 'served-bytes budget exhausted' "$f"); pull failed $(grep -c 'pull failed' "$f")"
done
