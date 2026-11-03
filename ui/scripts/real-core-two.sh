#!/bin/bash
# M1 "talks to a friend": two Electron instances with the real core sync
# through a throwaway relay (its own container and network, started the way
# core/api/tests/two_peers.sh does). Never touches the live relay.
#
#   bash ui/scripts/real-core-two.sh
#
# Needs: ui/dist (cd ui && docker compose run --rm ui npm run build),
# desktop/.core/mvs-core (see desktop/README.md) and the desktop-real-core image
# (cd desktop && docker compose build real-core). IMAGE is the relay image
# (default mvs-relay:test, built from infra/relay unless it exists).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$here/../.."
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; mount() { (cd "$1" && pwd -W); };; *) mount() { (cd "$1" && pwd); };; esac

IMAGE=${IMAGE:-mvs-relay:test}
P=mvs-ui-two

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -t "$IMAGE" --build-arg CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" "$(mount "$repo/infra/relay")"
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
relay="/dns4/relay/tcp/4001/p2p/$relay_id"

# In the container, as pwuser: a wrapper that adds --relay (the shell passes
# only --data-dir), then one session bus + unlocked keyring per instance, so
# the two identities never share a keychain.
inner=$(cat <<EOF
set -e
printf '#!/bin/sh\nexec /repo/desktop/.core/mvs-core "\$@" --relay "$relay"\n' > /tmp/mvs-core-relay
chmod 755 /tmp/mvs-core-relay
for n in a b; do
  mkdir -p /tmp/mvs-\$n
  bus=\$(HOME=/tmp/mvs-\$n dbus-daemon --session --fork --print-address)
  echo -n test | HOME=/tmp/mvs-\$n DBUS_SESSION_BUS_ADDRESS=\$bus gnome-keyring-daemon --unlock --components=secrets >/dev/null
  export MVS_BUS_\$(echo \$n | tr a-z A-Z)=\$bus
done
npm ci --no-audit --no-fund >/dev/null 2>&1
MVS_TWO=1 MVS_CORE_PATH=/tmp/mvs-core-relay xvfb-run -a node /repo/ui/scripts/real-core-shots.mjs
EOF
)

docker run --rm --network $P -e MVS_INNER="$inner" \
  --security-opt seccomp=unconfined --security-opt apparmor=unconfined --shm-size 512m \
  -v "$(mount "$repo"):/repo" -v desktop_desktop_node_modules:/repo/desktop/node_modules \
  -w /repo/desktop desktop-real-core \
  sh -c 'chown pwuser /repo/desktop/node_modules && runuser -u pwuser -- sh -c "$MVS_INNER"'
