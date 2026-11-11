#!/bin/bash
# Sign-out check, in Docker: a release's mvs-core with a throwaway identity
# and a test keyring (signout.mjs says what is checked). Never a real profile.
#
#   TAG=v0.3.3 bash qa/signout/run.sh
#
# The core comes from qa/upgrade/run.sh's download (SHA256SUMS checked).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
case "$(uname -s)" in MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; mount() { (cd "$1" && pwd -W); };; *) mount() { (cd "$1" && pwd); };; esac
TAG=${TAG:-v0.3.3}
dl="$repo/qa/upgrade/.build/$TAG"
[ -f "$dl/mvs-core" ] || OLD="$TAG" NEW="$TAG" bash "$repo/qa/upgrade/run.sh" >/dev/null 2>&1 || true
# secret-tool lists the test keyring's items (labels and attributes only).
docker run --rm -v "$(mount "$here"):/qa:ro" -v "$(mount "$dl"):/bin-in:ro" mvs-voice-e2e-app sh -c "
  apt-get update -qq >/dev/null && apt-get install -y -qq --no-install-recommends libsecret-tools >/dev/null &&
  cp /bin-in/mvs-core /tmp/core && chmod +x /tmp/core &&
  runuser -u pwuser -- dbus-run-session -- sh -c 'echo -n test | gnome-keyring-daemon --unlock --components=secrets >/dev/null && node /qa/signout.mjs /tmp/core'"
