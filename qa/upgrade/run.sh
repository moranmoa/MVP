#!/bin/bash
# Profile upgrade check, in Docker: a profile written by the OLD release's
# mvs-core opens in the NEW one (upgrade.mjs says what is compared).
#
#   OLD=v0.2.5 NEW=v0.3.0 bash qa/upgrade/run.sh
#
# Both cores come from the releases' linux-x86_64.tar.gz (a draft works too,
# with gh auth), checked against each release's SHA256SUMS. NEW_CORE=<path>
# uses a locally built mvs-core instead of the NEW release's.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; mount() { (cd "$1" && pwd -W); };; *) mount() { (cd "$1" && pwd); };; esac
OLD=${OLD:-v0.2.5}
NEW=${NEW:-v0.3.0}
work="$here/.build"
mkdir -p "$work"

fetch() { # tag -> $work/<tag>/mvs-core
  local tag=$1 dir="$work/$1" v=${1#v}
  [ -f "$dir/mvs-core" ] && return
  rm -rf "$dir" && mkdir -p "$dir"
  gh release download "$tag" -R moranmoa/MVS -D "$(mount "$dir")" -p "mvs-$v-linux-x86_64.tar.gz" -p SHA256SUMS
  (cd "$dir" && grep " mvs-$v-linux-x86_64.tar.gz\$" SHA256SUMS | sha256sum -c -)
  tar -xzf "$dir/mvs-$v-linux-x86_64.tar.gz" -C "$dir"
  cp "$(find "$dir" -type f -name mvs-core -path '*resources*' | head -1)" "$dir/mvs-core"
}
fetch "$OLD"
if [ -n "${NEW_CORE:-}" ]; then
  mkdir -p "$work/$NEW" && cp "$NEW_CORE" "$work/$NEW/mvs-core"
else
  fetch "$NEW"
fi

# The desktop e2e image: Playwright's (Node) plus a session D-Bus and
# gnome-keyring, so both cores share one test keychain.
docker build -q -t mvs-voice-e2e-app -f "$(mount "$repo/desktop")/Dockerfile.e2e" "$(mount "$repo/desktop")" >/dev/null
docker run --rm -v "$(mount "$here"):/qa:ro" -v "$(mount "$work"):/bin-in:ro" mvs-voice-e2e-app sh -c "
  cp /bin-in/$OLD/mvs-core /tmp/old && cp /bin-in/$NEW/mvs-core /tmp/new && chmod +x /tmp/old /tmp/new &&
  runuser -u pwuser -- dbus-run-session -- sh -c 'echo -n test | gnome-keyring-daemon --unlock --components=secrets >/dev/null && node /qa/upgrade.mjs /tmp/old /tmp/new'"
