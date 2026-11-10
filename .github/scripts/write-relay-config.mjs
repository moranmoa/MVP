// Writes the desktop app's build-time config from the environment, for the
// release workflow; dependency-free.
//
//   RELAY=/dns4/<host>/tcp/443/wss/p2p/<peer id> \
//   RELAY_TCP=/dns4/<tcp host>/tcp/4001/p2p/<peer id> \
//   node write-relay-config.mjs <out.json>
//
// Output: {"relay": ..., "bootstrap": [...]}; each key only when its variable
// is set ({} for neither). bootstrap is a plain-TCP route to the relay, so the
// relay can observe clients' public addresses (DCUtR, #219).
//
// On Windows runners, run it with MSYS2_ENV_CONV_EXCL=RELAY (a prefix, so it
// covers RELAY_TCP too): Git Bash otherwise rewrites "/dns4/..." into
// "C:/Program Files/Git/dns4/..." for native programs.
import { writeFileSync } from "node:fs";

const out = process.argv[2];
if (!out) {
  console.error("usage: write-relay-config.mjs <out.json>");
  process.exit(2);
}

function checked(name, value) {
  if (!value.startsWith("/") || /\s/.test(value) || value.length > 512) {
    console.error(`::error::${name} is not a multiaddr (starts with /, no spaces, at most 512 chars)`);
    process.exit(1);
  }
  // Pinned by peer ID: without /p2p/<id>, Noise would accept whoever answers
  // at that address.
  if (!/\/p2p\/[1-9A-HJ-NP-Za-km-z]{46,60}$/.test(value)) {
    console.error(`::error::${name} must end in /p2p/<relay peer id>`);
    process.exit(1);
  }
  return value;
}

const config = {};
if (process.env.RELAY) config.relay = checked("the relay address", process.env.RELAY);
if (process.env.RELAY_TCP) config.bootstrap = [checked("the TCP bootstrap address", process.env.RELAY_TCP)];
writeFileSync(out, JSON.stringify(config));
console.log(config.relay ? "relay configured" : "::warning::no relay address in this build: the app will run offline");
console.log(config.bootstrap ? "TCP bootstrap configured" : "no TCP bootstrap address in this build");
