#!/bin/bash
# Relay-only catch-up (T4): can a peer that was offline fetch N missed
# messages when the relay circuit is its only path to the writer?
#
# Two sync peers sit on separate Docker networks; only the relay is on both,
# so p2 can reach p1 only through a circuit. p1 writes N messages while p2 is
# off, then p2 starts and we time how long until it holds all N.
#
#   bash infra/test/relay-catchup.sh                    # docker-compose.yml's circuit limits
#   MAX_CIRCUIT_BYTES= bash infra/test/relay-catchup.sh # empty = libp2p's default for that one
#
# Uses its own containers, networks and ports; never touches the live relay.
# IMAGE (default mvs-relay:test) is built from infra/relay unless it exists.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
core="$here/../../core"
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; mount() { (cd "$1" && pwd -W); };; *) mount() { (cd "$1" && pwd); };; esac

N=${N:-10000}
TIMEOUT=${TIMEOUT:-120}
IMAGE=${IMAGE:-mvs-relay:test}
P=mvs-catchup
PORT_A=${PORT_A:-17101}
PORT_B=${PORT_B:-17102}
# Same defaults as docker-compose.yml; set one to empty for libp2p's value.
export MAX_CIRCUITS=${MAX_CIRCUITS-256}
export MAX_CIRCUITS_PER_PEER=${MAX_CIRCUITS_PER_PEER-8}
export MAX_CIRCUIT_SECS=${MAX_CIRCUIT_SECS-3600}
export MAX_CIRCUIT_BYTES=${MAX_CIRCUIT_BYTES-33554432}

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -t "$IMAGE" --build-arg CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" "$here/../relay"
fi
if [ ! -f "$here/dist/sync-peer" ]; then
  mkdir -p "$here/dist"
  docker run --rm -v "$(mount "$core"):/work" -v "$(mount "$here/dist"):/out" \
    -v cargo-registry:/usr/local/cargo/registry -v core-target-sync:/target -e CARGO_TARGET_DIR=/target \
    -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" -w /work rust:1.90-bookworm \
    bash -c "cargo build --release --locked -p sync --bin sync-peer && cp /target/release/sync-peer /out/"
fi

cleanup() {
  docker rm -f $P-relay $P-p1 $P-p2 >/dev/null 2>&1 || true
  docker network rm $P-a $P-b >/dev/null 2>&1 || true
}
cleanup
[ -n "${KEEP:-}" ] || trap cleanup EXIT
docker network create $P-a >/dev/null
docker network create $P-b >/dev/null

# Circuit limits pass through as set (empty = libp2p default).
docker run -d --name $P-relay --network $P-a --network-alias relay \
  --read-only --tmpfs /tmp --tmpfs /data:uid=10001,mode=700 \
  -e ALLOW_PRIVATE_ADDRS=1 -e EXTERNAL_ADDRS=/dns4/relay/tcp/4001 -e RUST_LOG="${RELAY_LOG:-info,relay_node=debug}" \
  -e MAX_CIRCUITS -e MAX_CIRCUITS_PER_PEER -e MAX_CIRCUIT_SECS -e MAX_CIRCUIT_BYTES \
  "$IMAGE" >/dev/null
docker network connect --alias relay $P-b $P-relay
relay_id=""
for _ in $(seq 30); do
  relay_id=$(docker logs $P-relay 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -o 'relay node started peer_id=[A-Za-z0-9]*' | cut -d= -f2 || true)
  [ -n "$relay_id" ] && break
  sleep 1
done
[ -n "$relay_id" ] || { docker logs $P-relay; echo "relay did not start"; exit 1; }
docker logs $P-relay 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -o 'circuit limits.*'

peer() { # name network port
  docker run -d --name "$P-$1" --network "$2" --tmpfs /data -v "$(mount "$here/dist"):/app:ro" \
    -e RELAY="/dns4/relay/tcp/4001/p2p/$relay_id" -e RUST_LOG="${PEER_LOG:-info}" \
    -p "127.0.0.1:$3:7000" debian:bookworm-slim /app/sync-peer "$1" >/dev/null
}
ctl() { # port json -> reply line
  exec 3<>"/dev/tcp/127.0.0.1/$1"
  echo "$2" >&3
  reply=""
  read -r -t 10 reply <&3 || true
  exec 3>&-
  echo "$reply"
}
field() { sed -n "s/.*\"$1\":\([0-9]*\).*/\1/p"; }
wait_ctl() { for _ in $(seq 30); do ctl "$1" '{"cmd":"state"}' >/dev/null 2>&1 && return; sleep 0.5; done; exit 1; }

peer p1 $P-a "$PORT_A"
peer p2 $P-b "$PORT_B"
wait_ctl "$PORT_A"
wait_ctl "$PORT_B"

ctl "$PORT_A" '{"cmd":"start"}' >/dev/null
ctl "$PORT_A" "{\"cmd\":\"write\",\"n\":$N,\"interval_ms\":0}" >/dev/null
until [ "$(ctl "$PORT_A" '{"cmd":"state"}' | field writes_done)" = "$N" ]; do sleep 0.5; done
echo "p1 wrote $N messages with p2 offline"

ctl "$PORT_B" '{"cmd":"start"}' >/dev/null
start=$(date +%s)
count=0
while [ $(( $(date +%s) - start )) -lt "$TIMEOUT" ]; do
  state=$(ctl "$PORT_B" '{"cmd":"state"}')
  count=$(echo "$state" | field count)
  [ "${count:-0}" -ge "$N" ] && break
  sleep 0.5
done

# The peers share no network, so any connection between them is relayed;
# the relay's own events confirm it (either side may have dialed).
circuits=$(docker logs $P-relay 2>&1 | grep -c 'CircuitReqAccepted' || true)
echo "relay accepted $circuits circuit(s), $(docker logs $P-relay 2>&1 | grep -c 'CircuitClosed' || true) closed"
[ "$circuits" -ge 1 ] || { echo "FAIL: no circuit through the relay (p2 has ${count:-0}/$N)"; exit 1; }
if [ "${count:-0}" -ge "$N" ]; then
  echo "OK: p2 caught up $count/$N over the relay in $(echo "$state" | field last_remote_ms) ms (since start)"
else
  echo "FAIL: p2 has ${count:-0}/$N after ${TIMEOUT}s"
  exit 1
fi
