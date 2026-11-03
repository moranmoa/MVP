#!/bin/bash
# Builds everything the harness needs into harness/dist, in Docker:
# web-native, core/sync's sync-peer, and the wasm bundle (+ sizes.json).
# Run from anywhere (Git Bash on Windows).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$here/../../.."
case "$(uname -s)" in
  MINGW*|MSYS*) export MSYS_NO_PATHCONV=1; repo_mount="$(cd "$repo" && pwd -W)"; here="$(cd "$here" && pwd -W)";;
  *) repo_mount="$(cd "$repo" && pwd)";;
esac

docker build -q -t web-spike-builder -f "$here/builder.Dockerfile" "$here" >/dev/null

docker run --rm -v "$repo_mount:/work" -v cargo-registry:/usr/local/cargo/registry \
  -v web-spike-target:/target -e CARGO_TARGET_DIR=/target -w /work web-spike-builder bash -euo pipefail -c '
    out=spikes/web/harness/dist
    rm -rf $out && mkdir -p $out/www/pkg
    (cd core && cargo build --release -p sync --bin sync-peer)
    (cd spikes/web && cargo test --release -p web-wire -p web-native && cargo build --release -p web-native)
    (cd spikes/web && cargo build --profile wasm -p web-peer --target wasm32-unknown-unknown)
    cp /target/release/sync-peer /target/release/web-native $out/
    wasm-bindgen --target web --no-typescript --out-dir $out/www/pkg /target/wasm32-unknown-unknown/wasm/web_peer.wasm
    raw=$(stat -c %s $out/www/pkg/web_peer_bg.wasm)
    wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
      -o $out/www/pkg/web_peer_bg.wasm $out/www/pkg/web_peer_bg.wasm
    cp spikes/web/www/index.html $out/www/
    cd $out/www/pkg
    size() { stat -c %s "$1"; }
    gz() { gzip -9 -c "$1" | wc -c; }
    br() { brotli -q 11 -c "$1" | wc -c; }
    printf "{\"wasm_before_wasm_opt\":%s,\"wasm\":%s,\"wasm_gzip\":%s,\"wasm_brotli\":%s,\"js\":%s,\"js_gzip\":%s}\n" \
      $raw $(size web_peer_bg.wasm) $(gz web_peer_bg.wasm) $(br web_peer_bg.wasm) $(size web_peer.js) $(gz web_peer.js) > sizes.json
    cat sizes.json
  '
