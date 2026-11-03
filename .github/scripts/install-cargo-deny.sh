#!/usr/bin/env bash
# Downloads the pinned cargo-deny release and refuses to install it unless
# its SHA-256 matches CARGO_DENY_SHA256 (set in ci.yml).
set -euo pipefail
: "${CARGO_DENY_VERSION:?}" "${CARGO_DENY_SHA256:?}"
name="cargo-deny-${CARGO_DENY_VERSION}-x86_64-unknown-linux-musl"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -sSfL -o "$tmp/$name.tar.gz" \
  "https://github.com/EmbarkStudios/cargo-deny/releases/download/${CARGO_DENY_VERSION}/${name}.tar.gz"
echo "${CARGO_DENY_SHA256}  $tmp/$name.tar.gz" | sha256sum --check --strict
tar xzf "$tmp/$name.tar.gz" -C "$tmp"
sudo install -m 0755 "$tmp/$name/cargo-deny" /usr/local/bin/cargo-deny
cargo-deny --version
