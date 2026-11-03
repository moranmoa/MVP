#!/usr/bin/env bash
# Exploratory checks of the core CLIs (updater, spike-peer) with bad and
# tampered input. Runs inside rust:1.90-bookworm; see qa/README.md.
#
# Every check states what SHOULD happen. Output: one PASS/FAIL line per
# check and a summary. Keys are generated under /tmp inside the container,
# never in the checkout.
set -u

REPO=${REPO:-/repo}
cd "$REPO/core"
CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4} cargo build --release --locked -q -p updater -p spike-peer || exit 2
BIN=${CARGO_TARGET_DIR:-target}/release
UP="$BIN/updater"
SP="$BIN/spike-peer"

W=$(mktemp -d /tmp/qa-updater.XXXX)
pass=0
fail=0
check() { # check <name> <expected: ok|fail> <command...>
  local name=$1 want=$2
  shift 2
  local out code
  out=$("$@" 2>&1)
  code=$?
  local got=fail
  [ $code -eq 0 ] && got=ok
  if [ "$got" = "$want" ]; then
    pass=$((pass + 1))
    printf 'PASS  %-62s (exit %s)\n' "$name" "$code"
  else
    fail=$((fail + 1))
    printf 'FAIL  %-62s (exit %s, wanted %s)\n' "$name" "$code" "$want"
    printf '      %s\n' "$(echo "$out" | tail -2 | tr '\n' ' ')"
  fi
}

now=$(date +%s)
day=86400

# ---- setup: two keys, a 1-of-1 root, an artifact and a valid manifest ------
"$UP" keygen --out "$W" --name rel >/dev/null
"$UP" keygen --out "$W" --name evil >/dev/null
"$UP" make-root --threshold 1 --pub "$W/rel.pub" --out "$W/root.json" >/dev/null
head -c 300000 /dev/urandom >"$W/app.bin"
sha=$(sha256sum "$W/app.bin" | cut -d' ' -f1)
size=$(stat -c %s "$W/app.bin")

body() { # body <version> <issued> <expires> [extra json]
  cat <<EOF
{"format":1,"channel":"stable","version":"$1","issued_at":$2,"expires_at":$3,
 "artifacts":{"linux-x86_64":{"sha256":"$sha","size":$size}}${4:-}}
EOF
}
body 1.1.0 $((now - 60)) $((now + 7 * day)) >"$W/body.json"
"$UP" sign-manifest --key "$W/rel.key" --body "$W/body.json" --out "$W/m.json" >/dev/null
V=(verify --root "$W/root.json")

echo "== updater verify"
check "valid manifest, artifact and upgrade 1.0.0 -> 1.1.0" ok \
  "$UP" "${V[@]}" --manifest "$W/m.json" --installed 1.0.0 --artifact "$W/app.bin" --platform linux-x86_64

sed 's/1\.1\.0/9.9.9/' "$W/m.json" >"$W/t1.json"
check "version inside signed body changed after signing" fail "$UP" "${V[@]}" --manifest "$W/t1.json"

sed 's/\\"stable\\"/\\"beta\\"/' "$W/m.json" >"$W/t2.json"
check "channel inside signed body changed after signing" fail "$UP" "${V[@]}" --manifest "$W/t2.json"

cp "$W/app.bin" "$W/app-tampered.bin"
printf '\x00' | dd of="$W/app-tampered.bin" bs=1 seek=1000 conv=notrunc status=none
check "artifact with one byte changed" fail \
  "$UP" "${V[@]}" --manifest "$W/m.json" --artifact "$W/app-tampered.bin" --platform linux-x86_64

head -c 299999 "$W/app.bin" >"$W/app-short.bin"
check "truncated artifact" fail \
  "$UP" "${V[@]}" --manifest "$W/m.json" --artifact "$W/app-short.bin" --platform linux-x86_64

check "artifact for a platform the manifest does not list" fail \
  "$UP" "${V[@]}" --manifest "$W/m.json" --artifact "$W/app.bin" --platform windows-x86_64

check "expired manifest (--now after expires_at)" fail \
  "$UP" "${V[@]}" --manifest "$W/m.json" --now $((now + 8 * day))

check "downgrade: installed 1.2.0, manifest offers 1.1.0" fail \
  "$UP" "${V[@]}" --manifest "$W/m.json" --installed 1.2.0

check "client on the beta channel gets a stable manifest" fail \
  "$UP" "${V[@]}" --manifest "$W/m.json" --channel beta --installed 1.0.0

"$UP" sign-manifest --key "$W/evil.key" --body "$W/body.json" --out "$W/evil.json" >/dev/null
check "manifest signed by a key outside the trust root" fail "$UP" "${V[@]}" --manifest "$W/evil.json"

# Same body, signature lifted from the real manifest onto another body.
body 1.2.0 $((now - 60)) $((now + 7 * day)) >"$W/body2.json"
"$UP" sign-manifest --key "$W/evil.key" --body "$W/body2.json" --out "$W/m2.json" >/dev/null
real_sig=$(grep -o '"sig": "[0-9a-f]*"' "$W/m.json")
real_key=$(grep -o '"key": "[0-9a-f]*"' "$W/m.json")
sed -e "s/\"sig\": \"[0-9a-f]*\"/$real_sig/" -e "s/\"key\": \"[0-9a-f]*\"/$real_key/" "$W/m2.json" >"$W/t3.json"
check "real signature copied onto a different body" fail "$UP" "${V[@]}" --manifest "$W/t3.json"

jq '.signatures = []' "$W/m.json" >"$W/t4.json" 2>/dev/null || sed 's/"signatures": \[.*/"signatures": []}/' "$W/m.json" >"$W/t4.json"
check "signatures removed" fail "$UP" "${V[@]}" --manifest "$W/t4.json"

{ printf '{"signed":"'; head -c 70000 /dev/zero | tr '\0' 'a'; printf '","signatures":[]}'; } >"$W/big.json"
check "manifest over 64 KiB is refused before parsing" fail "$UP" "${V[@]}" --manifest "$W/big.json"

: >"$W/empty.json"
check "empty manifest file" fail "$UP" "${V[@]}" --manifest "$W/empty.json"
check "manifest path that does not exist" fail "$UP" "${V[@]}" --manifest "$W/nope.json"

# A signed body with "version" twice: JSON parsers disagree on which wins,
# so a strict client must refuse it rather than pick one.
dup='{\"format\":1,\"channel\":\"stable\",\"version\":\"1.1.0\",\"version\":\"9.0.0\",\"issued_at\":'$((now - 60))',\"expires_at\":'$((now + day))',\"artifacts\":{\"linux-x86_64\":{\"sha256\":\"'$sha'\",\"size\":'$size'}}}'
printf '{"signed":"%s","signatures":[]}' "$dup" >"$W/dup-unsigned.json"
"$UP" sign-manifest --key "$W/rel.key" --manifest "$W/dup-unsigned.json" --out "$W/dup.json" >/dev/null 2>&1
check "signed body with a duplicate \"version\" key" fail "$UP" "${V[@]}" --manifest "$W/dup.json"

echo "== updater: freeze-attack limits (spec: signed metadata with an expiry)"
body 1.1.0 $((now - 60)) 18446744073709551615 >"$W/forever.json"
check "signing a manifest that never expires (expires_at = u64::MAX)" fail \
  "$UP" sign-manifest --key "$W/rel.key" --body "$W/forever.json" --out "$W/forever-m.json"
body 1.1.0 $((now + 365 * day)) $((now + 400 * day)) >"$W/future.json"
"$UP" sign-manifest --key "$W/rel.key" --body "$W/future.json" --out "$W/future-m.json" >/dev/null 2>&1
check "manifest issued a year in the future" fail "$UP" "${V[@]}" --manifest "$W/future-m.json"

echo "== updater: release tooling input"
check "keygen inside the git checkout is refused" fail "$UP" keygen --out "$REPO/core" --name qa-should-not-exist
rm -f "$REPO/core/qa-should-not-exist.key" "$REPO/core/qa-should-not-exist.pub"
check "keygen with a path in --name is refused" fail "$UP" keygen --out "$W" --name ../escape
check "keygen does not overwrite an existing key" fail "$UP" keygen --out "$W" --name rel
check "make-root --threshold 0" fail "$UP" make-root --threshold 0 --pub "$W/rel.pub" --out "$W/r0.json"
check "make-root threshold larger than the key count" fail \
  "$UP" make-root --threshold 2 --pub "$W/rel.pub" --out "$W/r2.json"
check "make-root with the same key twice" fail \
  "$UP" make-root --threshold 2 --pub "$W/rel.pub" --pub "$W/rel.pub" --out "$W/rdup.json"
body 1.1.0-beta $((now - 60)) $((now + day)) >"$W/semver.json"
check "body with a non-canonical version 1.1.0-beta" fail \
  "$UP" sign-manifest --key "$W/rel.key" --body "$W/semver.json" --out "$W/semver-m.json"
body 01.1.0 $((now - 60)) $((now + day)) >"$W/lead0.json"
check "body with a leading-zero version 01.1.0" fail \
  "$UP" sign-manifest --key "$W/rel.key" --body "$W/lead0.json" --out "$W/lead0-m.json"
body 1.1.0 $((now - 60)) $((now + day)) ',"surprise":1' >"$W/extra.json"
check "body with an unknown field" fail \
  "$UP" sign-manifest --key "$W/rel.key" --body "$W/extra.json" --out "$W/extra-m.json"
check "--now that is not a number" fail "$UP" "${V[@]}" --manifest "$W/m.json" --now -5
check "flag without a value" fail "$UP" verify --manifest
check "unknown command" fail "$UP" frobnicate

echo "== spike-peer bad input"
RELAY_ID=12D3KooWD3eckifWpRn9wQpMG9R9hX3sD158z7EqHWmweQAJU5SA
check "listen with a garbage relay address" fail "$SP" listen not-an-address
check "listen with a relay address missing /p2p/<id>" fail "$SP" listen /ip4/127.0.0.1/tcp/4001
check "dial with a garbage peer ID" fail "$SP" dial "/ip4/127.0.0.1/tcp/4001/p2p/$RELAY_ID" not-a-peer
check "dial with too many arguments" fail "$SP" dial a b c
check "interactive: stdin closed / no role" fail bash -c "'$SP' </dev/null"
check "interactive: role 3" fail bash -c "printf '3\n/ip4/127.0.0.1/tcp/4001\n\n' | '$SP'"

# Relay unreachable: the tester should get a RESULT (or an error) within the
# 30 s dial timeout, not a silent hang. 60 s is generous.
start=$(date +%s)
out=$(timeout 60 "$SP" dial "/ip4/127.0.0.1/tcp/9/p2p/$RELAY_ID" "$RELAY_ID" 2>&1)
code=$?
took=$(($(date +%s) - start))
if [ $code -eq 124 ]; then
  fail=$((fail + 1))
  echo "FAIL  dial via an unreachable relay ends with RESULT or an error  (still running after ${took}s, no RESULT)"
  echo "      $(echo "$out" | grep -v '^$' | tail -1 | cut -c1-160)"
else
  pass=$((pass + 1))
  echo "PASS  dial via an unreachable relay ends with RESULT or an error  (exit $code after ${took}s)"
fi

echo
echo "updater/spike-peer checks: $pass passed, $fail failed (known: QA-14, QA-15, QA-16 in qa/BUGS.md)"
rm -rf "$W"
[ "$fail" -eq 0 ]
