// Writes the desktop app's build-time config ({"relay": ...} or {}) from the
// RELAY environment variable. Used by the release workflow; dependency-free.
//
//   RELAY=/dns4/<host>/tcp/443/wss/p2p/<peer id> node write-relay-config.mjs <out.json>
//
// On Windows runners, run it with MSYS2_ENV_CONV_EXCL=RELAY: Git Bash otherwise
// rewrites "/dns4/..." into "C:/Program Files/Git/dns4/..." for native programs.
import { writeFileSync } from "node:fs";

const out = process.argv[2];
if (!out) {
  console.error("usage: write-relay-config.mjs <out.json>");
  process.exit(2);
}
const relay = process.env.RELAY || "";
if (relay) {
  if (!relay.startsWith("/") || /\s/.test(relay) || relay.length > 512) {
    console.error("::error::the relay address is not a multiaddr (starts with /, no spaces, at most 512 chars)");
    process.exit(1);
  }
  // Pinned by peer ID: without /p2p/<id>, Noise would accept whoever answers
  // at that address (behind Cloudflare).
  if (!/\/p2p\/[1-9A-HJ-NP-Za-km-z]{46,60}$/.test(relay)) {
    console.error("::error::the relay address must end in /p2p/<relay peer id>");
    process.exit(1);
  }
}
writeFileSync(out, JSON.stringify(relay ? { relay } : {}));
console.log(relay ? "relay configured" : "::warning::no relay address in this build: the app will run offline");
