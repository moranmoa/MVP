#!/bin/sh
# The deps service (compose.yaml): Electron and playwright-core for desktop/
# into the shared node_modules volume, safely when runs overlap.
#
# - The volume is named after the lockfile's hash (run.sh sets DEPS_KEY), so
#   runs with different lockfiles never share one.
# - Installs hold a lock (in its own small volume: `npm ci` empties
#   node_modules), so two runs never install at once.
# - A finished install leaves a stamp. A later run with the stamp skips
#   `npm ci`, which would otherwise empty node_modules under a running app.
set -eu
nm=/repo/desktop/node_modules
chown pwuser "$nm" /lock
exec runuser -u pwuser -- flock /lock/deps.lock sh -c '
  set -eu
  cd /repo/desktop
  want=$(sha256sum package-lock.json | cut -c1-64)
  if [ "$(cat node_modules/.mvs-deps.stamp 2>/dev/null)" = "$want" ] && [ -x node_modules/electron/dist/electron ]; then
    echo "desktop dependencies up to date"
    exit 0
  fi
  rm -f node_modules/.mvs-deps.stamp
  npm ci --no-audit --no-fund
  npx install-electron --no
  test -x node_modules/electron/dist/electron
  echo "$want" > node_modules/.mvs-deps.stamp
'
