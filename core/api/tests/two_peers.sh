#!/bin/bash
# Runs tests/two_peers.rs against a throwaway relay: its own container and
# network, started the way infra/test/relay-catchup.sh does. Never touches
# the live relay or its network.
#
#   bash core/api/tests/two_peers.sh
#
# IMAGE (default mvs-relay:test) is built from infra/relay unless it exists.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
core="$here/../.."
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; mount() { (cd "$1" && pwd -W); };; *) mount() { (cd "$1" && pwd); };; esac

IMAGE=${IMAGE:-mvs-relay:test}
P=mvs-api-two

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -t "$IMAGE" --build-arg CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" "$(mount "$core/../infra/relay")"
fi

cleanup() {
  docker rm -f $P-relay >/dev/null 2>&1 || true
  docker network rm $P >/dev/null 2>&1 || true
}
cleanup
trap cleanup EXIT
docker network create $P >/dev/null
docker run -d --name $P-relay --network $P --network-alias relay \
  --read-only --tmpfs /tmp --tmpfs /data:uid=10001,mode=700 \
  -e ALLOW_PRIVATE_ADDRS=1 -e EXTERNAL_ADDRS=/dns4/relay/tcp/4001 -e RUST_LOG=info \
  "$IMAGE" >/dev/null
relay_id=""
for _ in $(seq 30); do
  relay_id=$(docker logs $P-relay 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | grep -o 'relay node started peer_id=[A-Za-z0-9]*' | cut -d= -f2 || true)
  [ -n "$relay_id" ] && break
  sleep 1
done
[ -n "$relay_id" ] || { docker logs $P-relay; echo "relay did not start"; exit 1; }

docker run --rm --network $P -e RELAY="/dns4/relay/tcp/4001/p2p/$relay_id" \
  -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" -v "$(mount "$core"):/work" \
  -v cargo-registry:/usr/local/cargo/registry -v core-target:/target -e CARGO_TARGET_DIR=/target \
  -w /work rust:1.90-bookworm \
  cargo test --release -p api --test two_peers -- --ignored --nocapture
