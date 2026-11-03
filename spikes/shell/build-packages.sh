#!/usr/bin/env bash
# Builds the six spike packages that the install / uninstall check (T12)
# consumes, all inside Docker:
#
#   shell-tauri-windows-x64-setup.exe      shell-electron-windows-x64-setup.exe
#   shell-tauri-linux-amd64.deb            shell-electron-linux-amd64.deb
#   shell-tauri-linux-amd64.AppImage       shell-electron-linux-amd64.AppImage
#
#   spikes/shell/build-packages.sh [output dir]   (default: spikes/shell/out)
#
# Exits non-zero if any of the six files is missing at the end.
set -euo pipefail
export MSYS_NO_PATHCONV=1
SHELL_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$SHELL_DIR/../.." && pwd)"
OUT="${1:-$SHELL_DIR/out}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
STAGE="$SHELL_DIR/out/stage" # inside the repo, so the containers can write it
rm -rf "$STAGE"
mkdir -p "$STAGE"

echo "== ui/dist"
docker run --rm -v "$REPO/ui":/app -v shell-ui-node-modules:/app/node_modules -w /app \
  node:22.23.4-bookworm sh -c "npm ci --no-audit --no-fund >/dev/null && npm run build >/dev/null"

echo "== Tauri (Linux deb + AppImage, Windows NSIS via mingw-w64)"
(cd "$SHELL_DIR/tauri" && docker build -q -t shell-tauri-builder . >/dev/null)
docker run --rm -v "$REPO":/repo -v shell-tauri-target-trixie:/repo/spikes/shell/tauri/target \
  -v voice-cargo-registry:/usr/local/cargo/registry -w /repo/spikes/shell/tauri -e CARGO_BUILD_JOBS=4 \
  shell-tauri-builder sh -c '
    set -e
    [ -f icons/icon.png ] || { convert -size 1024x1024 xc:"#5865f2" /tmp/i.png && cargo tauri icon /tmp/i.png -o icons >/dev/null; }
    APPIMAGE_EXTRACT_AND_RUN=1 cargo tauri build -b deb,appimage
    cargo tauri build --target x86_64-pc-windows-gnu -b nsis
    S=../out/stage
    cp target/release/bundle/deb/*.deb $S/shell-tauri-linux-amd64.deb
    cp target/release/bundle/appimage/*.AppImage $S/shell-tauri-linux-amd64.AppImage
    cp target/x86_64-pc-windows-gnu/release/bundle/nsis/*-setup.exe $S/shell-tauri-windows-x64-setup.exe'

echo "== Electron (Linux deb + AppImage, Windows NSIS)"
docker run --rm -v "$REPO":/repo -v shell-electron-node-modules:/repo/spikes/shell/electron/node_modules \
  -v shell-electron-cache:/root/.cache -w /repo/spikes/shell/electron -e HOST_IDS="$(id -u):$(id -g)" \
  electronuserland/builder:wine@sha256:f3a8d451994c881d0972e7b1846ee4d4fdc15eb41fc6b1f350b38cbf99ca90fe sh -c '
    set -e
    npm install --no-audit --no-fund
    rm -rf dist
    # Full output: in CI this is the only trace when a download or wine fails.
    npx electron-builder --linux AppImage deb --win nsis
    S=../out/stage
    cp dist/*.deb $S/shell-electron-linux-amd64.deb
    cp dist/*.AppImage $S/shell-electron-linux-amd64.AppImage
    cp dist/*Setup*.exe $S/shell-electron-windows-x64-setup.exe
    chown -R $HOST_IDS dist $S'

missing=0
for f in shell-{tauri,electron}-windows-x64-setup.exe shell-{tauri,electron}-linux-amd64.{deb,AppImage}; do
  if [ -s "$STAGE/$f" ]; then
    [ "$STAGE" = "$OUT" ] || cp "$STAGE/$f" "$OUT/$f"
    printf '%10d  %s\n' "$(stat -c %s "$OUT/$f")" "$f"
  else
    echo "MISSING $f" >&2
    missing=1
  fi
done
exit $missing
